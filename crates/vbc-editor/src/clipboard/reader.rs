//! Reading the clipboard without the render loop waiting on the answer.
//!
//! The failure path is what sets this shape, not the success path. A warm helper answers in a
//! fraction of a millisecond, but that number was measured against a clipboard nobody owned; the
//! case that matters follows a Ctrl+C in a live Windows app which still owns it, and answering
//! then can mean a synchronous round trip to that app. A locked workstation is slower again and
//! answers every call with a refusal it takes over a second to give. Any of those on the render
//! loop is a frozen editor, so no clipboard call is made there: they are made on a worker thread,
//! and what the render loop does is ask what has come back so far.
//!
//! Two deadlines are what a read is held to. At [`SOFT_DEADLINE`] it is slow enough to be worth
//! saying so, and the status line says [`READING_NOTICE`] while the editor carries on taking keys.
//! At [`HARD_DEADLINE`] it is over as far as the editor is concerned: it is abandoned, the paste is
//! refused, and nothing is inserted. Nothing means nothing -- not an answer an earlier read found,
//! and not the abandoned read's own answer turning up afterwards. A read that comes back at three
//! seconds comes back to a buffer the user has been editing for a second and a half, so it is
//! dropped where it lands, and it is the moment it came back that says so rather than the moment a
//! frame got round to looking: a loop that draws nothing while it waits on a key is held to the
//! same deadline as one drawing every eight milliseconds.
//!
//! Nothing here retries. A refusal costs over a second to obtain and .NET's clipboard API hides a
//! second of retrying of its own inside that, so a loop around it multiplies rather than rescues:
//! twelve tries eight milliseconds apart once turned a fifth of a millisecond into a thirteen
//! second freeze. One request is one call, and a paste asked for while a call is already out joins
//! that call rather than starting a second one.
//!
//! Nothing here reads the clipboard unasked either. Starting the helper is what costs, and that is
//! paid on the worker thread as the session starts; what the clipboard holds is asked for when a
//! paste asks for it and at no other time.
//!
//! The deadlines are read off the render loop's own clock rather than kept by anyone else, and the
//! whole of the reader's state belongs to the thread that draws: the worker is spoken to over a
//! pair of channels and shares nothing else with it.

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use super::helper::{Error, Helper};
use super::protocol::Response;

/// How long a read may take before the editor says out loud that it is waiting on one.
pub const SOFT_DEADLINE: Duration = Duration::from_millis(150);

/// How long a read is waited for at the very most, after which it is abandoned and the paste is
/// refused rather than delayed any further.
pub const HARD_DEADLINE: Duration = Duration::from_millis(1500);

/// What the status line says while a read is past [`SOFT_DEADLINE`].
pub const READING_NOTICE: &str = "reading clipboard…";

/// What a read came back with, which is what a paste has to work from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Outcome {
    /// The clipboard holds text, which is what a paste inserts.
    Text(String),

    /// The clipboard holds nothing.
    Empty,

    /// The clipboard holds something a paste cannot insert, such as an image.
    NonText,

    /// Nothing is being pasted, and this is why.
    Unavailable(Reason),
}

/// Why a read has nothing for a paste to insert.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Reason {
    /// Nothing came back inside [`HARD_DEADLINE`], so the read was given up on. An answer that
    /// arrives after that is thrown away rather than pasted.
    Abandoned,

    /// The clipboard was reached, could not be read, and said this about it.
    Refused(String),
}

/// How far along a read is, as the render loop sees it on the frame it asks.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Progress {
    /// The read is out, and is not slow enough to be worth mentioning.
    Waiting,

    /// The read is past [`SOFT_DEADLINE`], and the status line says so.
    Slow,

    /// The read is over, and this is what it came to.
    Ready(Outcome),
}

impl Progress {
    /// # Returns
    ///
    /// What the status line says about this read, if it says anything.
    pub fn notice(&self) -> Option<&'static str> {
        matches!(self, Self::Slow).then_some(READING_NOTICE)
    }
}

/// One asking of what the clipboard holds.
///
/// Pastes asked for while a read is out all name that read, so one call to the clipboard answers
/// all of them and answers them alike.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Request {
    id: u64,
}

/// Somewhere the clipboard is read from, one call at a time, by a call that blocks for as long as
/// the system takes to answer it.
pub trait Source: Send + 'static {
    /// # Returns
    ///
    /// What the clipboard holds, on success.
    ///
    /// # Errors
    ///
    /// Returns an error if the exchange said nothing about the clipboard at all.
    fn read_clipboard(&mut self) -> Result<Response, Error>;
}

impl Source for Helper {
    fn read_clipboard(&mut self) -> Result<Response, Error> {
        self.read()
    }
}

/// The clipboard as the render loop reaches it: a worker thread that makes the calls, and requests
/// that are asked how they are getting on rather than waited for.
///
/// It belongs to the thread that draws, which is the thread the deadlines are about. Dropping it
/// ends the worker and the source with it, without waiting for either.
#[derive(Debug)]
pub struct Reader {
    commands: Option<Sender<Command>>,
    answers: Receiver<Answer>,
    worker: Option<JoinHandle<()>>,
    current: Option<Reading>,
    next_id: u64,
    issued: u64,
}

impl Reader {
    /// Starts the worker thread, which builds the source and then waits to be asked for something.
    ///
    /// Building the source is what the startup cost is, so it happens on the worker rather than
    /// here: this returns without waiting for it, and a request made before the source is up is
    /// held to the same deadlines as any other.
    ///
    /// # Type Parameters
    ///
    /// * `SourceType` - What the clipboard is read from.
    /// * `FactoryType` - What builds it, on the worker thread.
    ///
    /// # Returns
    ///
    /// The reader.
    pub fn start<SourceType, FactoryType>(factory: FactoryType) -> Self
    where
        SourceType: Source,
        FactoryType: FnOnce() -> Result<SourceType, Error> + Send + 'static,
    {
        let (commands, orders) = mpsc::channel();
        let (replies, answers) = mpsc::channel();
        let worker = thread::spawn(move || serve(&orders, &replies, factory));

        Self {
            commands: Some(commands),
            answers,
            worker: Some(worker),
            current: None,
            next_id: 0,
            issued: 0,
        }
    }

    /// Asks for what the clipboard holds, joining the read that is already out if there is one.
    ///
    /// # Returns
    ///
    /// The request, which is polled rather than waited on.
    pub fn request(&mut self) -> Request {
        self.collect();
        if let Some(current) = self.current.as_ref().filter(|current| current.joinable()) {
            return Request { id: current.id };
        }

        let id = self.next_id;
        self.next_id += 1;

        let sent = self
            .commands
            .as_ref()
            .is_some_and(|commands| commands.send(Command::Read(id)).is_ok());
        self.issued += u64::from(sent);
        let outcome = (!sent).then(|| Outcome::Unavailable(Reason::Refused(STOPPED.to_owned())));
        self.current = Some(Reading {
            id,
            started: Instant::now(),
            outcome,
            abandoned: false,
        });

        Request { id }
    }

    /// # Returns
    ///
    /// How far along a read is, which is what the frame asking is drawn from. Polling never waits,
    /// and what a read past [`HARD_DEADLINE`] comes to is that it was abandoned.
    pub fn poll(&mut self, request: Request) -> Progress {
        self.collect();
        let Some(current) = self
            .current
            .as_mut()
            .filter(|current| current.id == request.id)
        else {
            return Progress::Ready(Outcome::Unavailable(Reason::Abandoned));
        };

        if let Some(outcome) = current.outcome.as_ref() {
            return Progress::Ready(outcome.clone());
        }
        if current.abandoned {
            return Progress::Ready(Outcome::Unavailable(Reason::Abandoned));
        }

        let waited = current.started.elapsed();
        if HARD_DEADLINE <= waited {
            current.abandoned = true;

            return Progress::Ready(Outcome::Unavailable(Reason::Abandoned));
        }
        if SOFT_DEADLINE <= waited {
            return Progress::Slow;
        }

        Progress::Waiting
    }

    /// # Returns
    ///
    /// The number of reads this reader has put to the source, which is one per request that did
    /// not join a read already out.
    pub fn reads_issued(&self) -> u64 {
        self.issued
    }

    /// Stops the worker and waits for it, taking the source down with it.
    ///
    /// A read already under way is waited for, since the call making it cannot be taken back, so
    /// this belongs to a session that is ending rather than to a frame.
    pub fn shutdown(&mut self) {
        self.commands = None;
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }

    /// Takes in whatever the worker has answered, which is nothing at all while a read is still
    /// out. An answer to a read the editor has stopped waiting on is thrown away here rather than
    /// pasted, and so is one that took longer than [`HARD_DEADLINE`] to arrive, whichever frame
    /// happens to be the one that finds it.
    fn collect(&mut self) {
        while let Ok(answer) = self.answers.try_recv() {
            let Some(current) = self
                .current
                .as_mut()
                .filter(|current| current.id == answer.id && current.joinable())
            else {
                continue;
            };
            if HARD_DEADLINE <= answer.landed.saturating_duration_since(current.started) {
                current.abandoned = true;
                continue;
            }
            current.outcome = Some(answer.outcome);
        }
    }
}

/// What a request comes to once the reader has stopped, which is not an answer about the clipboard
/// at all.
const STOPPED: &str = "the clipboard reader has stopped";

/// A read that has been asked for, and what has become of it.
#[derive(Debug)]
struct Reading {
    id: u64,
    started: Instant,
    outcome: Option<Outcome>,
    abandoned: bool,
}

impl Reading {
    /// # Returns
    ///
    /// Whether this read is still one an answer is wanted for, which a read that has been answered
    /// or given up on is not.
    fn joinable(&self) -> bool {
        self.outcome.is_none() && !self.abandoned
    }
}

/// What the render loop asks the worker thread for.
#[derive(Debug)]
enum Command {
    /// Read the clipboard, and answer the read this names.
    Read(u64),
}

/// What the worker thread hands back, named after the read it answers so that an answer nobody is
/// waiting for any more can be told from an answer to the read they are, and stamped with the
/// moment it came so that how late it is does not depend on when a frame next looks.
#[derive(Debug)]
struct Answer {
    id: u64,
    outcome: Outcome,
    landed: Instant,
}

/// Builds the source and then serves reads from it, one at a time, until there is nobody left to
/// serve.
///
/// # Type Parameters
///
/// * `SourceType` - What the clipboard is read from.
/// * `FactoryType` - What builds it.
fn serve<SourceType, FactoryType>(
    orders: &Receiver<Command>,
    replies: &Sender<Answer>,
    factory: FactoryType,
) where
    SourceType: Source,
    FactoryType: FnOnce() -> Result<SourceType, Error>,
{
    let mut source = factory();

    while let Ok(Command::Read(id)) = orders.recv() {
        let outcome = match source.as_mut() {
            Ok(source) => answer(source.read_clipboard()),
            Err(error) => Outcome::Unavailable(Reason::Refused(error.to_string())),
        };
        if replies
            .send(Answer {
                id,
                outcome,
                landed: Instant::now(),
            })
            .is_err()
        {
            return;
        }
    }
}

/// # Returns
///
/// What a paste makes of what the source came back with.
fn answer(read: Result<Response, Error>) -> Outcome {
    match read {
        Ok(Response::Text(text)) => Outcome::Text(text),
        Ok(Response::Empty) => Outcome::Empty,
        Ok(Response::NonText) => Outcome::NonText,
        Ok(Response::Failed(reason)) => Outcome::Unavailable(Reason::Refused(reason)),
        Ok(Response::Stored) => Outcome::Unavailable(Reason::Refused(
            "the helper answered a read with a write's answer".to_owned(),
        )),
        Err(error) => Outcome::Unavailable(Reason::Refused(error.to_string())),
    }
}
