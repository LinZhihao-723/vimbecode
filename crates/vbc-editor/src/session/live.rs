//! A session an application holds open while it draws: started, read, asked and answered.
//!
//! Everything below this module is a round trip somebody has to be waiting on. [`Client::turn`]
//! sends a message and reads until the turn ends, which is the right shape for a test and the
//! wrong one for a program that must go on drawing frames and reading keys while a model thinks.
//! So the child is read on a thread of its own and what it says arrives on a channel, and the
//! whole of what an application loop does with a session is ask it what has arrived since the last
//! frame.
//!
//! What has arrived is not events, though. The panel reads blocks, and the queue of what a session
//! is waiting on is not in the transcript at all -- a `can_use_tool` frame is a control round trip
//! rather than something that was said. Both are folded together here: the frames become the
//! blocks the panel already knows how to draw, and every question still outstanding is drawn under
//! them as the call it is, so a session that has stopped and is waiting says so where a reader is
//! already looking rather than only in a status line they may not be reading.
//!
//! Rebuilding the panel is what an arrival costs, and it costs the conversation rather than the
//! block that arrived, because the panel is built over a transcript rather than appended to. It is
//! paid when something arrives and never on a keystroke, so reading a session that has stopped
//! talking costs what reading a compiled-in exchange costs. What it also costs is the reader's
//! place: a rebuilt panel is drawn from its first row, so a block arriving while somebody is
//! reading carries them back to the top of what was said.
//!
//! The gate is here as well, and it is here rather than in the binary because it is not a dialog
//! -- it is the step between deciding what a directory may run and starting a child in it, and a
//! program that puts the question somewhere else can put it after the spawn without noticing.
//! [`Session::opened`] takes the asking as a function and does the ordering itself, so the terminal
//! prompt and a test's own answer reach the child through the same three steps in the same order.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::chat::block::{Block, Kind as BlockKind};
use crate::chat::fold::Tag;
use crate::chat::transcript::Transcript;

use super::blocks::Conversation;
use super::client::Client;
use super::control::{Answer, Ask, Decision, Question, Request, Subject};
use super::error::Error;
use super::event::{Event, Kind};
use super::identity::{Identity, SessionId};
use super::queue::Queue;
use super::spawn::Spawn;
use super::trust::{Admission, Answer as Trusted, Gate, Standing};

/// What the block an outstanding question is drawn as says first, which is what tells a reader
/// that the session has stopped rather than that it called something and carried on.
pub const WAITING: &str = "waiting to be answered";

/// What a denial says where the reader gave no reason of their own, which the model is told and a
/// silent refusal would leave it guessing at.
pub const REFUSED: &str = "the reader refused it";

/// How long a read of the session's stream waits before the loop looks again for something to
/// send. It bounds how long an answer sits in hand while the session waits for it, so it is a
/// frame rather than a turn.
const TICK: Duration = Duration::from_millis(20);

/// How long the child is given to exit once the session has been let go.
const ENDING: Duration = Duration::from_secs(5);

/// What a session is to be started as: which conversation, in which directory, through which
/// binary and on which model, and the record the directory's standing is read out of.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Plan {
    identity: Identity,
    directory: PathBuf,
    binary: OsString,
    model: Option<String>,
    gate: Gate,
}

impl Plan {
    /// Factory function.
    ///
    /// # Returns
    ///
    /// A newly created plan to run `identity` in `directory`, on the `claude` the path finds and
    /// the model it would choose for itself.
    #[must_use]
    pub fn new(identity: Identity, directory: impl AsRef<Path>, gate: Gate) -> Self {
        Self {
            identity,
            directory: directory.as_ref().to_owned(),
            binary: OsString::from(super::spawn::BINARY),
            model: None,
            gate,
        }
    }

    /// # Returns
    ///
    /// The plan, of a binary other than the one on the path.
    #[must_use]
    pub fn with_binary(mut self, binary: impl AsRef<OsStr>) -> Self {
        self.binary = binary.as_ref().to_owned();
        self
    }

    /// # Returns
    ///
    /// The plan, on a named model.
    #[must_use]
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// # Returns
    ///
    /// The directory the session is to be started in, which is the directory whose standing the
    /// gate is read for.
    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// Decides what the directory may be run as, which is decided before anything is run in it.
    ///
    /// # Returns
    ///
    /// The admission, on success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * Forwards [`Gate::admit`]'s return values on failure.
    pub fn admit(&self) -> Result<Admission, Error> {
        self.gate.admit(&self.directory)
    }

    /// # Returns
    ///
    /// How the child is to be started, in the directory `admission` was taken for and standing as
    /// that admission admitted it.
    #[must_use]
    pub fn spawn(&self, admission: &Admission) -> Spawn {
        let spawn = Spawn::new(self.identity.clone())
            .with_binary(&self.binary)
            .with_admission(admission);

        match &self.model {
            Some(model) => spawn.with_model(model.clone()),
            None => spawn,
        }
    }
}

/// A session an application holds: the child it is spoken to over, what has been said in it so
/// far, and what it is waiting to be answered.
#[derive(Debug)]
pub struct Session {
    live: Live,
    conversation: Conversation,
    queue: Queue,
    standing: Standing,
    failure: Option<String>,
    revision: u64,
    id: SessionId,
    model: Option<String>,
    responding: bool,
}

impl Session {
    /// Starts a session, putting the question of what the directory may run to the reader first
    /// where they are owed it.
    ///
    /// The three steps happen in this order and nowhere else: the standing is read, the reader is
    /// asked where the directory is one they have not trusted, and only then is a child started.
    /// A gate written the other way round -- start the child, notice, kill it -- has already run
    /// the project's session hook and started its MCP servers by the time it notices.
    ///
    /// # Type Parameters
    ///
    /// * `AskingReader` - What puts the question to the reader and says what they answered. It is
    ///   called only where the reader is owed the question.
    ///
    /// # Returns
    ///
    /// The session on success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * Forwards [`Plan::admit`]'s return values on failure.
    /// * Forwards [`Gate::answered`]'s return values on failure.
    /// * Forwards [`Session::started`]'s return values on failure.
    pub fn opened<AskingReader: FnOnce(&Admission) -> Trusted>(
        plan: &Plan,
        asking: AskingReader,
    ) -> Result<Self, Error> {
        let admission = plan.admit()?;
        let admission = if admission.asks() {
            let answer = asking(&admission);
            plan.gate.answered(&admission, answer)?
        } else {
            admission
        };

        Self::started(&plan.spawn(&admission))
    }

    /// Starts a session over a spawn that has already been through whatever gate it is going
    /// through.
    ///
    /// # Returns
    ///
    /// The session on success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * Forwards [`Live::start`]'s return values on failure.
    pub fn started(spawn: &Spawn) -> Result<Self, Error> {
        Ok(Self {
            live: Live::start(spawn)?,
            conversation: Conversation::new(),
            queue: Queue::new(),
            standing: spawn.standing(),
            failure: None,
            revision: 0,
            id: spawn.identity().session_id().clone(),
            model: spawn.model().map(str::to_owned),
            responding: false,
        })
    }

    /// Sends one message from the reader, which is a turn, and puts it into the transcript.
    ///
    /// The child echoes no prompt, so a question is in the transcript because this put it there or
    /// it is not in the transcript at all.
    pub fn ask(&mut self, text: &str) {
        self.conversation.asked(text);
        self.live.ask(text);
        self.responding = true;
        self.revision += 1;
    }

    /// Stops the turn in flight and every message queued behind it.
    ///
    /// # Returns
    ///
    /// Whether a turn was running to be stopped.
    pub fn interrupt(&mut self) -> bool {
        if !self.responding {
            return false;
        }
        self.live.interrupt();

        true
    }

    /// Takes everything the session has said since it was last read, which is nothing at all where
    /// it has said nothing. Whether that changed anything is [`Session::revision`]'s to say, so
    /// that a caller has one account of it rather than two that can disagree.
    pub fn read(&mut self) {
        let mut arrived = false;
        for arrival in self.live.read() {
            arrived = true;
            match arrival {
                Ok(event) => {
                    match event.kind() {
                        Kind::Turn(_) => {
                            self.responding = false;
                            self.queue = Queue::new();
                        }
                        Kind::Init(_) | Kind::Assistant | Kind::User => self.responding = true,
                        Kind::Control | Kind::System(_) | Kind::Other(_) => {}
                    }
                    self.conversation.read(&event);
                    self.queue.read(&event);
                }
                Err(error) => self.failure = Some(error.to_string()),
            }
        }
        if arrived {
            self.revision += 1;
        }
    }

    /// Answers the question the session has been waiting on longest.
    ///
    /// # Returns
    ///
    /// What was answered, or `None` where the session is waiting on nothing.
    pub fn answer(&mut self, decision: &Decision) -> Option<Ask> {
        let answer = self.queue.oldest()?.answer(decision);
        let asked = self.queue.take(answer.request_id());
        self.live.answer(&answer);
        self.revision += 1;

        asked
    }

    /// # Returns
    ///
    /// Every question the session is waiting on, oldest first.
    #[must_use]
    pub fn outstanding(&self) -> &[Ask] {
        self.queue.outstanding()
    }

    /// # Returns
    ///
    /// The question the session has been waiting longest to be answered in words, or `None` where
    /// it is waiting on nothing or on an approval, which words are not an answer to.
    #[must_use]
    pub fn question(&self) -> Option<String> {
        let Subject::Questions(questions) = self.queue.oldest().map(Ask::subject)? else {
            return None;
        };

        Some(questions.first()?.question.clone())
    }

    /// # Returns
    ///
    /// What the directory the session runs in was admitted as, which says whether the project's
    /// own memory, hooks and MCP servers are the session's as well.
    #[must_use]
    pub fn standing(&self) -> Standing {
        self.standing
    }

    /// # Returns
    ///
    /// What went wrong with the session, or `None` where nothing has. A session that failed is
    /// still readable: what it said before it failed is what a reader has left.
    #[must_use]
    pub fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }

    /// # Returns
    ///
    /// The identifier the session runs under, which is the one it last announced and the one it
    /// was started under until it has announced one.
    #[must_use]
    pub fn id(&self) -> &str {
        self.conversation
            .session_id()
            .unwrap_or_else(|| self.id.as_str())
    }

    /// # Returns
    ///
    /// The model the session last announced it runs on, the one it was started on until it has
    /// announced one, and [`None`] where it was started on none and has announced none.
    #[must_use]
    pub fn model(&self) -> Option<&str> {
        self.conversation.model().or(self.model.as_deref())
    }

    /// # Returns
    ///
    /// Whether a turn is running: one was asked for or has begun, and has not yet ended.
    #[must_use]
    pub fn responding(&self) -> bool {
        self.responding
    }

    /// # Returns
    ///
    /// A number that differs whenever what the session would be drawn as differs, which is what
    /// says a panel built from it is out of date.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// # Returns
    ///
    /// The transcript and the tags of it a panel is built over: what has been said, and every
    /// question the session has stopped for drawn under it as the call it is.
    #[must_use]
    pub fn panel(&self) -> (Transcript, Vec<Tag>) {
        let mut transcript = self.conversation.transcript().clone();
        let mut tags = self.conversation.tags().to_vec();
        for ask in self.queue.outstanding() {
            transcript.push(asked(ask));
            tags.push(Tag::untagged());
        }

        (transcript, tags)
    }
}

/// The child a session is, read on a thread of its own so that an application can go on drawing
/// while a model thinks.
#[derive(Debug)]
struct Live {
    errands: Option<Sender<Errand>>,
    events: Receiver<Result<Event, Error>>,
    reader: Option<JoinHandle<()>>,
}

impl Live {
    /// Starts the child and the thread that reads it.
    ///
    /// # Returns
    ///
    /// The child on success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * Forwards [`Client::start`]'s return values on failure.
    fn start(spawn: &Spawn) -> Result<Self, Error> {
        let client = Client::start(spawn)?;
        let (errands, asked) = mpsc::channel();
        let (arrivals, events) = mpsc::channel();
        let reader = thread::spawn(move || pump(client, &asked, &arrivals));

        Ok(Self {
            errands: Some(errands),
            events,
            reader: Some(reader),
        })
    }

    /// Asks the session to take a turn over `text`.
    fn ask(&self, text: &str) {
        self.send(Errand::Ask(text.to_owned()));
    }

    /// Answers one question the session is waiting on.
    fn answer(&self, answer: &Answer) {
        self.send(Errand::Answer(answer.clone()));
    }

    /// Stops the turn in flight and the queue behind it.
    fn interrupt(&self) {
        self.send(Errand::Interrupt);
    }

    /// # Returns
    ///
    /// Everything the session has said since it was last read, which is nothing at all where it
    /// has said nothing. It never waits: a loop that drew a frame per arrival would draw no frame
    /// at all through a turn that is still thinking.
    fn read(&self) -> Vec<Result<Event, Error>> {
        let mut arrivals = Vec::new();
        while let Ok(arrival) = self.events.try_recv() {
            arrivals.push(arrival);
        }

        arrivals
    }

    /// Hands one errand to the thread holding the child, dropping it where that thread is gone:
    /// what became of the session is on the channel the arrivals come over rather than here.
    fn send(&self, errand: Errand) {
        if let Some(errands) = &self.errands {
            let _ignored = errands.send(errand);
        }
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        self.errands = None;
        if let Some(reader) = self.reader.take() {
            let _ignored = reader.join();
        }
    }
}

/// Something the application asks of the child, which only the thread holding it can do.
#[derive(Clone, Debug, PartialEq)]
enum Errand {
    /// Take a turn over this text.
    Ask(String),

    /// Answer a question the session is waiting on.
    Answer(Answer),

    /// Stop the turn in flight and the queue behind it.
    Interrupt,
}

/// Drives the child until it ends or nobody is holding it any more: everything asked of it is sent
/// as it is asked, and everything it says goes onto the channel.
///
/// The read is bounded rather than blocking, because a session with a question outstanding says
/// nothing whatever until it is answered -- so a loop that waited on the stream would wait holding
/// the answer the wait is for.
fn pump(mut client: Client, errands: &Receiver<Errand>, arrivals: &Sender<Result<Event, Error>>) {
    loop {
        match errands.try_recv() {
            Ok(errand) => {
                let sent = match errand {
                    Errand::Ask(text) => client.ask(&text),
                    Errand::Answer(answer) => client.answer(&answer),
                    Errand::Interrupt => client.send(&Request::interrupt().frame()),
                };
                if let Err(error) = sent {
                    let _ignored = arrivals.send(Err(error));

                    return;
                }

                continue;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                let _ignored = client.finish(ENDING);

                return;
            }
        }

        match client.next(TICK) {
            Ok(event) => {
                if arrivals.send(Ok(event)).is_err() {
                    return;
                }
            }
            Err(Error::Silent { .. }) => {}
            Err(error) => {
                let _ignored = arrivals.send(Err(error));

                return;
            }
        }
    }
}

/// # Returns
///
/// The block a question the session is waiting on is drawn as, which is the call it is asking to
/// make and, under it, whatever the call is actually asking for.
fn asked(ask: &Ask) -> Block {
    let body = match ask.subject() {
        Subject::Tool => detailed(ask),
        Subject::Questions(questions) => questions
            .iter()
            .map(spoken)
            .collect::<Vec<String>>()
            .join("\n\n"),
        Subject::Plan(plan) => plan,
    };

    Block::new(
        BlockKind::ToolCall {
            name: ask.tool().to_owned(),
        },
        format!("{WAITING}\n{body}"),
    )
}

/// # Returns
///
/// What a tool call the session is waiting on is asking to do: what it says about itself, and the
/// input it would run with where it says nothing.
fn detailed(ask: &Ask) -> String {
    let description = ask.description().trim();
    if !description.is_empty() {
        return description.to_owned();
    }

    serde_json::to_string_pretty(ask.input()).unwrap_or_else(|_| ask.input().to_string())
}

/// # Returns
///
/// One question the session asked, written out with the answers it offered, which a reader
/// answering it is not held to.
fn spoken(question: &Question) -> String {
    let mut said = question.question.clone();
    for option in &question.options {
        said.push_str("\n- ");
        said.push_str(&option.label);
        if !option.description.trim().is_empty() {
            said.push_str(": ");
            said.push_str(&option.description);
        }
    }

    said
}
