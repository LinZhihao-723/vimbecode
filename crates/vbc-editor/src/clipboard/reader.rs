//! Reading the clipboard without the render loop waiting on the answer.
//!
//! The failure path is what sets this shape, not the success path. A warm helper answers in a
//! fraction of a millisecond, but that number was measured against a clipboard nobody owned; the
//! case that matters follows a Ctrl+C in a live Windows app which still owns it, and answering
//! then can mean a synchronous round trip to that app. A locked workstation is slower again and
//! answers every call with a refusal it takes over a second to give. Any of those on the render
//! loop is a frozen editor, so no clipboard call is made there: they are made on a worker thread
//! and the render loop only ever asks what has come back so far.
//!
//! Two deadlines are what the render loop holds a read to. At [`SOFT_DEADLINE`] the read is slow
//! enough to be worth saying so, and the status line says [`READING_NOTICE`] while the editor
//! carries on taking keys. At [`HARD_DEADLINE`] the read is over as far as the editor is
//! concerned: it is abandoned, the paste is refused, and nothing is inserted. Nothing means
//! nothing -- not a cached answer from an earlier read, and not the abandoned read's own answer
//! turning up afterwards. A read that finishes at three seconds is delivered into a buffer the
//! user has been editing for a second and a half, so it is dropped where it lands.
//!
//! Nothing here retries. A refusal costs over a second to obtain and .NET's clipboard API hides a
//! second of retrying of its own inside that, so a loop around it multiplies rather than rescues:
//! twelve tries eight milliseconds apart once turned a fifth of a millisecond into a thirteen
//! second freeze. One request is one call, and a request made while a call is already out joins
//! that call rather than starting a second one.
//!
//! Nothing here reads the clipboard unasked either. Starting the helper is what costs, and that is
//! paid on the worker thread when the session starts; what the clipboard holds is asked for when a
//! paste asks for it and at no other time.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
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
/// It is cheap to clone, and every clone speaks to the same worker and about the same read.
#[derive(Clone, Debug)]
pub struct Reader {
    shared: Arc<Shared>,
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
        let (sender, receiver) = mpsc::channel();
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                sender: Some(sender),
                worker: None,
                current: None,
                next_id: 0,
                issued: 0,
            }),
        });

        let weak = Arc::downgrade(&shared);
        let worker = thread::spawn(move || serve(&weak, &receiver, factory));
        shared.locked().worker = Some(worker);

        Self { shared }
    }

    /// Asks for what the clipboard holds, joining the read that is already out if there is one.
    ///
    /// # Returns
    ///
    /// The request, which is polled rather than waited on.
    pub fn request(&self) -> Request {
        let mut state = self.shared.locked();
        if let Some(current) = state.current.as_ref().filter(|current| current.joinable()) {
            return Request {
                shared: Arc::clone(&self.shared),
                id: current.id,
            };
        }

        let id = state.next_id;
        state.next_id += 1;
        state.issued += 1;
        state.current = Some(Reading {
            id,
            started: Instant::now(),
            outcome: None,
            abandoned: false,
        });

        let sent = state
            .sender
            .as_ref()
            .is_some_and(|sender| sender.send(Command::Read(id)).is_ok());
        if !sent {
            state.settle(
                id,
                Outcome::Unavailable(Reason::Refused(STOPPED.to_owned())),
            );
        }
        drop(state);

        Request {
            shared: Arc::clone(&self.shared),
            id,
        }
    }

    /// # Returns
    ///
    /// The number of reads this reader has put to the source, which is one per request that did
    /// not join a read already out.
    pub fn reads_issued(&self) -> u64 {
        self.shared.locked().issued
    }

    /// Stops the worker and waits for it, taking the source down with it.
    ///
    /// A read already under way is waited for, since the call making it cannot be taken back, so
    /// this belongs to a session that is ending rather than to a frame.
    pub fn shutdown(&self) {
        let worker = {
            let mut state = self.shared.locked();
            state.sender = None;

            state.worker.take()
        };
        if let Some(worker) = worker {
            let _ = worker.join();
        }
    }
}

/// One asking of what the clipboard holds.
///
/// Requests made while a read is out all name that read, so one call to the source answers all of
/// them and answers them alike.
#[derive(Clone, Debug)]
pub struct Request {
    shared: Arc<Shared>,
    id: u64,
}

impl Request {
    /// # Returns
    ///
    /// How far along this read is, which is what the frame asking is drawn from. Polling never
    /// waits, and what a read past [`HARD_DEADLINE`] comes to is that it was abandoned.
    pub fn poll(&self) -> Progress {
        let mut state = self.shared.locked();
        let Some(current) = state
            .current
            .as_mut()
            .filter(|current| current.id == self.id)
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
}

/// What a request comes to once the reader has stopped, which is not an answer about the clipboard
/// at all.
const STOPPED: &str = "the clipboard reader has stopped";

/// What the render loop and the worker thread say to each other about.
#[derive(Debug)]
struct Shared {
    state: Mutex<State>,
}

impl Shared {
    /// # Returns
    ///
    /// The state, locked. A panic elsewhere leaves nothing here worth refusing to paste over, so
    /// the poison is taken rather than propagated.
    fn locked(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The one read that may be out, and the account of what has been asked for.
#[derive(Debug)]
struct State {
    sender: Option<Sender<Command>>,
    worker: Option<JoinHandle<()>>,
    current: Option<Reading>,
    next_id: u64,
    issued: u64,
}

impl State {
    /// Gives a read its answer, unless it is not the read the editor is waiting on any more.
    fn settle(&mut self, id: u64, outcome: Outcome) {
        let Some(current) = self.current.as_mut().filter(|current| current.id == id) else {
            return;
        };
        if current.abandoned || current.outcome.is_some() {
            return;
        }

        current.outcome = Some(outcome);
    }

    /// # Returns
    ///
    /// Whether a read is still worth making, which one the editor has stopped waiting on is not.
    fn wanted(&self, id: u64) -> bool {
        self.current
            .as_ref()
            .is_some_and(|current| current.id == id && current.joinable())
    }
}

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
    /// Whether another request may be answered by this read rather than starting one of its own.
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

/// Builds the source and then serves reads from it, one at a time, until there is nobody left to
/// serve.
///
/// # Type Parameters
///
/// * `SourceType` - What the clipboard is read from.
/// * `FactoryType` - What builds it.
fn serve<SourceType, FactoryType>(
    shared: &Weak<Shared>,
    receiver: &Receiver<Command>,
    factory: FactoryType,
) where
    SourceType: Source,
    FactoryType: FnOnce() -> Result<SourceType, Error>,
{
    let mut source = factory();

    while let Ok(Command::Read(id)) = receiver.recv() {
        match shared.upgrade() {
            None => return,
            Some(owner) if !owner.locked().wanted(id) => continue,
            Some(_) => {}
        }

        let outcome = match source.as_mut() {
            Ok(source) => answer(source.read_clipboard()),
            Err(error) => Outcome::Unavailable(Reason::Refused(error.to_string())),
        };

        let Some(owner) = shared.upgrade() else {
            return;
        };
        owner.locked().settle(id, outcome);
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
