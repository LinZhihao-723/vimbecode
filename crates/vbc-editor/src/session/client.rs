//! The one long-lived `claude` child a session is, and the two pipes it is spoken to over.
//!
//! One process serves a whole conversation. That is what the print lane's stream format buys and
//! it is the only thing that does: a process per turn would re-read the project's memory, re-run
//! its session hooks and re-start its MCP servers every time somebody pressed enter, and would
//! carry no context from the turn before. So the child is started once, kept, and written to
//! again; the proof that it really is one process is that a second turn can answer a question only
//! the first turn was told the answer to.
//!
//! Reading it takes a thread. The child writes NDJSON to its stdout whenever it has something to
//! say, which is not when we ask, and it writes to its stderr independently -- a pipe nobody drains
//! is a pipe that eventually fills and stops the child mid-turn. Both are drained by threads of
//! their own onto channels of their own, one carrying decoded events and one carrying what the
//! child said on its way out, because that is the only account there is of a child that refused to
//! start.
//!
//! Not all of the silence is the model's. A session that wants to write a file asks first, and
//! from the moment it asks it writes nothing whatever -- no prose, no result, no error -- until
//! the question is answered. A turn that waits for its own end therefore waits forever on the one
//! kind of turn a reader most wants to watch, which is why the turn that answers is here rather
//! than in whatever draws it: the question and the answer are the same round trip as the turn.
//!
//! Silence is not an event. Nothing at all arrives until the first frame is sent, a turn may think
//! for minutes before its first word, and a child whose stream has ended looks exactly like a
//! child that has not spoken yet. Every read here is therefore bounded and every ending is
//! reported: a stream that closed is [`probe::ending`]'s to explain, and a stream that stayed open
//! and said nothing is [`Error::Silent`].

use std::io::{self, BufRead, BufReader, Write};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use super::control::{Answer, Receipt, Request};
use super::error::Error;
use super::event::{Event, Kind};
use super::frame::Frame;
use super::probe;
use super::queue::Queue;
use super::spawn::Spawn;

/// How often a child that is being waited on is asked whether it has exited yet.
const REAPING_INTERVAL: Duration = Duration::from_millis(10);

/// A session: one `claude` child, the frames written to it and the events read back.
#[derive(Debug)]
pub struct Client {
    child: Child,
    input: Option<ChildStdin>,
    events: Receiver<Result<Event, Error>>,
    errors: Receiver<String>,
    said: String,
    readers: Vec<JoinHandle<()>>,
    probed: bool,
}

impl Client {
    /// Starts the child a session is spoken to.
    ///
    /// The child is up when this returns and has said nothing, because nothing is said until a
    /// frame is sent. Whether the permission flag survived is therefore answered by the first init
    /// frame rather than here.
    ///
    /// # Returns
    ///
    /// The session on success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * [`Error::Spawn`] if the binary could not be started.
    /// * [`Error::Pipe`] if the child was started without the pipes it was asked for.
    pub fn start(spawn: &Spawn) -> Result<Self, Error> {
        let mut child = spawn.command().spawn().map_err(|error| Error::Spawn {
            binary: spawn.binary().to_string_lossy().into_owned(),
            reason: error.to_string(),
        })?;

        let taken = (child.stdin.take(), child.stdout.take(), child.stderr.take());
        let (Some(input), Some(output), Some(errors)) = taken else {
            return Err(Error::Pipe {
                reason: "the child was started without one of its pipes".to_owned(),
            });
        };

        let (frames, events) = mpsc::channel();
        let (lines, said) = mpsc::channel();
        let readers = vec![
            thread::spawn(move || decode(output, &frames)),
            thread::spawn(move || collect(errors, &lines)),
        ];

        Ok(Self {
            child,
            input: Some(input),
            events,
            errors: said,
            said: String::new(),
            readers,
            probed: false,
        })
    }

    /// Sends one message from the user, which is a turn.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * Forwards [`Client::send`]'s return values on failure.
    pub fn ask(&mut self, text: &str) -> Result<(), Error> {
        self.send(&Frame::user(text))
    }

    /// Sends one frame.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * [`Error::FlagRejected`] or [`Error::Ended`] if the child is gone, which is what a write to
    ///   a closed pipe means and which the child's standard error is the account of.
    /// * [`Error::Pipe`] if the write failed for any other reason.
    pub fn send(&mut self, frame: &Frame) -> Result<(), Error> {
        let line = frame.line();
        let written = match self.input.as_mut() {
            Some(input) => input
                .write_all(line.as_bytes())
                .and_then(|()| input.flush()),
            None => Err(io::Error::from(io::ErrorKind::BrokenPipe)),
        };

        match written {
            Ok(()) => Ok(()),
            Err(error) if io::ErrorKind::BrokenPipe == error.kind() => {
                let said = self.reaped();
                Err(probe::ending(&said))
            }
            Err(error) => Err(Error::pipe(&error)),
        }
    }

    /// Reads the next event the session wrote, waiting for it.
    ///
    /// The first init frame to arrive is where the permission flag is checked, because it is the
    /// first thing the session says about itself and the last moment before a tool call could be
    /// denied without anybody being told. It is not necessarily the first frame: a session with a
    /// `SessionStart` hook writes that hook's frames ahead of it, so what is waited for here is
    /// the init frame rather than whatever arrives first.
    ///
    /// # Returns
    ///
    /// The event on success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * [`Error::Silent`] if the session stayed open and said nothing in time.
    /// * [`Error::Undecodable`] if it wrote a line that is not a frame.
    /// * [`Error::Pipe`] if its stream could not be read.
    /// * Forwards [`probe::honoured`]'s return values on failure.
    /// * Forwards [`probe::ending`]'s error where the session's stream has ended.
    pub fn next(&mut self, waiting: Duration) -> Result<Event, Error> {
        let received = self.events.recv_timeout(waiting);
        let event = match received {
            Ok(event) => event?,
            Err(RecvTimeoutError::Timeout) => return Err(Error::Silent { waited: waiting }),
            Err(RecvTimeoutError::Disconnected) => {
                let said = self.reaped();
                return Err(probe::ending(&said));
            }
        };

        if let (false, Some(init)) = (self.probed, event.init()) {
            probe::honoured(init)?;
            self.probed = true;
        }

        Ok(event)
    }

    /// Takes one turn: sends a message and reads until the turn ends.
    ///
    /// # Returns
    ///
    /// Every event the turn wrote, the result frame that ended it last, on success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * [`Error::Silent`] if the turn had not ended within the time allowed for the whole of it.
    /// * Forwards [`Client::ask`]'s return values on failure.
    /// * Forwards [`Client::next`]'s return values on failure.
    pub fn turn(&mut self, text: &str, waiting: Duration) -> Result<Vec<Event>, Error> {
        self.ask(text)?;

        let deadline = Instant::now() + waiting;
        let mut events = Vec::new();
        loop {
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                return Err(Error::Silent { waited: waiting });
            };
            let event = self.next(left)?;
            let ended = matches!(event.kind(), Kind::Turn(_));
            events.push(event);
            if ended {
                return Ok(events);
            }
        }
    }

    /// Takes one turn, answering what the session asks along the way.
    ///
    /// A turn that is not answered does not end. The session writes its question and then nothing
    /// at all, so [`Client::turn`] on a turn that needs approval waits out its whole deadline
    /// while the session waits out the reader; this is that turn with somebody at the other end.
    /// `answering` is handed the whole of what is outstanding and answers as much or as little of
    /// it as it likes, which is the same freedom an interactive reader has and the same three
    /// calls -- read, queue, answer -- a drawing loop makes in its own order. It is asked again
    /// for as long as it goes on striking questions off, and only then is the session waited on: a
    /// session with a question outstanding writes nothing, so a loop that answered one question
    /// per event would wait out its deadline holding the answer to the question the wait is for.
    ///
    /// What it hands back that names nothing outstanding is dropped rather than sent, and a round
    /// that strikes nothing off ends the asking. A reader who has decided is not obliged to forget
    /// it, so a policy that goes on offering the answer it already gave is an ordinary one; a loop
    /// that took each offer as work to do would send the session an answer per round to a question
    /// it stopped waiting on at the first, and would never reach the read that the deadline is
    /// enforced in.
    ///
    /// # Type Parameters
    ///
    /// * `AnsweringPolicy` - What decides, given everything the session is waiting on, which of it
    ///   to answer and how.
    ///
    /// # Returns
    ///
    /// Every event the turn wrote, the questions included and the result frame that ended it last,
    /// on success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * [`Error::Silent`] if the turn had not ended within the time allowed for the whole of it.
    /// * Forwards [`Client::ask`]'s return values on failure.
    /// * Forwards [`Client::next`]'s return values on failure.
    /// * Forwards [`Client::answer`]'s return values on failure.
    pub fn turn_answering<AnsweringPolicy: FnMut(&Queue) -> Vec<Answer>>(
        &mut self,
        text: &str,
        waiting: Duration,
        queue: &mut Queue,
        mut answering: AnsweringPolicy,
    ) -> Result<Vec<Event>, Error> {
        self.ask(text)?;

        let deadline = Instant::now() + waiting;
        let mut events = Vec::new();
        loop {
            loop {
                let answers = answering(queue);
                let awaited: Vec<&Answer> = answers
                    .iter()
                    .filter(|answer| queue.get(answer.request_id()).is_some())
                    .collect();
                if awaited.is_empty() {
                    break;
                }
                for answer in awaited {
                    self.answer(answer)?;
                    queue.answered(answer);
                }
            }

            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                return Err(Error::Silent { waited: waiting });
            };
            let event = self.next(left)?;
            queue.read(&event);
            let ended = matches!(event.kind(), Kind::Turn(_));
            events.push(event);
            if ended {
                return Ok(events);
            }
        }
    }

    /// Answers one question the session is waiting on.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * Forwards [`Client::send`]'s return values on failure.
    pub fn answer(&mut self, answer: &Answer) -> Result<(), Error> {
        self.send(&answer.frame())
    }

    /// Stops the turn in flight, and the queue behind it.
    ///
    /// The receipt is written before the aborted turn's own result frame, so the events between
    /// the two are the turn's last and are handed back rather than dropped: they are what the
    /// session had said by the time it was stopped, which is the part of a stopped turn a reader
    /// still has.
    ///
    /// # Returns
    ///
    /// What the session says it stopped, on success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * [`Error::Silent`] if no receipt had arrived within the time allowed.
    /// * [`Error::Refused`] if the session answered the interrupt with an error.
    /// * Forwards [`Client::send`]'s return values on failure.
    /// * Forwards [`Client::next`]'s return values on failure.
    pub fn interrupt(
        &mut self,
        waiting: Duration,
        passed: &mut Vec<Event>,
    ) -> Result<Receipt, Error> {
        let request = Request::interrupt();
        self.send(&request.frame())?;

        let deadline = Instant::now() + waiting;
        loop {
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                return Err(Error::Silent { waited: waiting });
            };
            let event = self.next(left)?;
            if let Some(receipt) = Receipt::read(&event, request.request_id()) {
                return receipt;
            }
            passed.push(event);
        }
    }

    /// # Returns
    ///
    /// Everything the child has written to its standard error so far, which is where a child that
    /// refused to start says why.
    pub fn stderr(&mut self) -> String {
        while let Ok(line) = self.errors.try_recv() {
            self.said.push_str(&line);
        }

        self.said.clone()
    }

    /// Ends the session the way it ends itself: the child's input is closed, and a session with
    /// nothing left to read exits.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * [`Error::Silent`] if the child was still running when the time allowed ran out. It is
    ///   killed rather than left behind.
    /// * [`Error::Pipe`] if the child could not be waited on.
    pub fn finish(&mut self, waiting: Duration) -> Result<(), Error> {
        self.input = None;

        let deadline = Instant::now() + waiting;
        loop {
            match self.child.try_wait() {
                Ok(Some(_status)) => return Ok(()),
                Ok(None) => {}
                Err(error) => return Err(Error::pipe(&error)),
            }
            if Instant::now() >= deadline {
                let _ignored = self.child.kill();
                return Err(Error::Silent { waited: waiting });
            }
            thread::sleep(REAPING_INTERVAL);
        }
    }

    /// # Returns
    ///
    /// What the child wrote to its standard error, once it has been waited for and the threads
    /// draining its pipes have finished. A child that has just died has usually not been read to
    /// the end of yet, and its last line is the one saying why it died.
    fn reaped(&mut self) -> String {
        self.input = None;
        let _ignored = self.child.wait();
        for reader in self.readers.drain(..) {
            let _ignored = reader.join();
        }

        self.stderr()
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.input = None;
        let _ignored = self.child.kill();
        let _ignored = self.child.wait();
        for reader in self.readers.drain(..) {
            let _ignored = reader.join();
        }
    }
}

/// Reads the child's stream, decoding every line onto a channel until the stream ends or nobody is
/// listening. A blank line is not a frame and is passed over: the child ends its output with a
/// newline, and a reader that decoded what follows it would report the end of a healthy stream as
/// a malformed frame.
fn decode(output: ChildStdout, sender: &Sender<Result<Event, Error>>) {
    let mut lines = BufReader::new(output);
    let mut line = String::new();
    loop {
        line.clear();
        match lines.read_line(&mut line) {
            Ok(0) => return,
            Ok(_read) => {}
            Err(error) => {
                let _ignored = sender.send(Err(Error::pipe(&error)));
                return;
            }
        }

        let frame = line.trim_end_matches(['\r', '\n']);
        if frame.is_empty() {
            continue;
        }
        if sender.send(Event::decoded(frame)).is_err() {
            return;
        }
    }
}

/// Keeps everything the child writes to its standard error, so that a child which has ended can
/// still say why.
fn collect(errors: ChildStderr, said: &Sender<String>) {
    let mut lines = BufReader::new(errors);
    let mut line = String::new();
    while let Ok(read) = lines.read_line(&mut line) {
        if 0 == read {
            return;
        }
        if said.send(line.clone()).is_err() {
            return;
        }
        line.clear();
    }
}
