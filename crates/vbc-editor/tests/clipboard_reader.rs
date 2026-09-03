//! Checks the five things the asynchronous clipboard policy is for: that a clipboard which takes
//! five seconds costs the render loop nothing and pastes nothing, that a clipboard which refuses is
//! asked once rather than in a loop, that two pastes overlapping each other cost one read between
//! them, that a read which lands after it was given up on stays out of the buffer whether the
//! frames kept coming or stopped, and that a failure never falls back on what an earlier read
//! found.
//!
//! Every one of these is about time, so time is what they assert on. The stand-in clipboard is told
//! how long to take and what to answer, and it counts the calls made to it, so the retry assertions
//! are made against its own account rather than against the reader's. Responsiveness is asserted
//! the way the user meets it: a stand-in render loop draws frames throughout, and the test is red
//! if the frames stop coming or if any one of them waits.
//!
//! What the render loop would insert is modelled too. Each loop keeps a buffer that starts as what
//! the user had typed and grows by whatever a read hands it, so "pastes nothing" is asserted as the
//! buffer being untouched rather than as an enum having the shape we hoped for.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use vbc_editor::clipboard::helper::Error;
use vbc_editor::clipboard::protocol::Response;
use vbc_editor::clipboard::reader::{
    Outcome, Progress, Reader, Reason, Request, Source, HARD_DEADLINE, READING_NOTICE,
    SOFT_DEADLINE,
};

/// What the stand-in clipboard holds when it holds text.
const CLIPBOARD_TEXT: &str = "what was copied in the other window";

/// What it says when it will not be read, which is what a locked workstation says.
const REFUSAL: &str = "the clipboard is in use by another process";

/// What the buffer already held before any of this, which a refused paste leaves alone.
const TYPED: &str = "the line the user was working on";

/// What the user types after a paste was refused, which a late answer must not land in the middle
/// of.
const EDIT: &str = ", carried on with";

/// How long a clipboard call takes when the test is about a clipboard that does not come back.
const SLOW_READ: Duration = Duration::from_secs(5);

/// How long one takes when the test is about an answer arriving after it stopped being wanted,
/// which is well past [`HARD_DEADLINE`] and well inside the test.
const LATE_READ: Duration = Duration::from_secs(3);

/// How long the helper takes to come up, which is what starting the reader must not wait for.
const STARTUP: Duration = Duration::from_secs(1);

/// How long a read that answers promptly is given.
const PATIENCE: Duration = Duration::from_millis(800);

/// How long a failing clipboard is watched for further attempts. A retry loop of the kind that
/// once cost thirteen seconds would show several of them inside this.
const WATCH: Duration = Duration::from_secs(2);

/// How long the reader is left alone to see whether it reads a clipboard nobody asked it about.
const IDLE: Duration = Duration::from_millis(300);

/// How long a frame takes in the stand-in render loop.
const FRAME: Duration = Duration::from_millis(8);

/// The longest any one frame may spend asking how a read is getting on. The clipboard calls these
/// frames run against take seconds, so anything on this side of this bound is a frame that did not
/// make one.
const FRAME_BOUND: Duration = Duration::from_millis(50);

/// The longest starting the reader may take, which is on the near side of [`STARTUP`] because
/// starting the helper is the worker's business rather than the caller's.
const START_BOUND: Duration = Duration::from_millis(50);

/// The fewest frames a loop running against a five second clipboard must have drawn. An editor
/// that waited for the read would have drawn one.
const MIN_FRAMES: u64 = 100;

/// A clipboard that is told how long to take and what to answer, and that counts what is asked of
/// it.
#[derive(Clone, Debug)]
struct Stub {
    calls: Arc<AtomicU64>,
    answer: Arc<Mutex<(Duration, Result<Response, Error>)>>,
}

impl Stub {
    /// # Returns
    ///
    /// A clipboard that takes this long over every call and answers every one of them alike.
    fn new(delay: Duration, answer: Result<Response, Error>) -> Self {
        Self {
            calls: Arc::new(AtomicU64::new(0)),
            answer: Arc::new(Mutex::new((delay, answer))),
        }
    }

    /// Changes what it takes and what it answers from the next call on.
    fn answers_with(&self, delay: Duration, answer: Result<Response, Error>) {
        *self.answer.lock().unwrap_or_else(PoisonError::into_inner) = (delay, answer);
    }

    /// # Returns
    ///
    /// The number of calls made to it.
    fn calls(&self) -> u64 {
        self.calls.load(Ordering::SeqCst)
    }
}

impl Source for Stub {
    fn read_clipboard(&mut self) -> Result<Response, Error> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let (delay, answer) = self
            .answer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        thread::sleep(delay);

        answer
    }
}

/// A stand-in render loop: it draws frames, pastes whatever a read hands it, and keeps the account
/// the assertions are made against.
#[derive(Clone, Debug)]
struct Frames {
    buffer: String,
    drawn: u64,
    slowest: Duration,
    notices: u64,
    answers: Vec<Outcome>,
}

impl Frames {
    /// # Returns
    ///
    /// A loop whose buffer holds what the user had already typed.
    fn over(buffer: &str) -> Self {
        Self {
            buffer: buffer.to_owned(),
            drawn: 0,
            slowest: Duration::ZERO,
            notices: 0,
            answers: Vec::new(),
        }
    }

    /// Draws one frame, pasting what the read has come back with and timing the look at it.
    fn draw(&mut self, reader: &mut Reader, request: Request) {
        let asked = Instant::now();
        let progress = reader.poll(request);
        self.slowest = self.slowest.max(asked.elapsed());
        self.drawn += 1;

        if Some(READING_NOTICE) == progress.notice() {
            self.notices += 1;
        }

        let Progress::Ready(outcome) = progress else {
            return;
        };
        if Some(&outcome) == self.answers.last() {
            return;
        }
        if let Outcome::Text(text) = &outcome {
            self.buffer.push_str(text);
        }
        self.answers.push(outcome);
    }

    /// Draws frames until the given moment.
    fn draw_until(&mut self, reader: &mut Reader, request: Request, until: Instant) {
        while Instant::now() < until {
            self.draw(reader, request);
            thread::sleep(FRAME);
        }
    }
}

/// # Returns
///
/// A reader serving this clipboard, whose source is up as soon as the worker is.
fn reader_of(stub: &Stub) -> Reader {
    let stub = stub.clone();

    Reader::start(move || Ok(stub))
}

/// # Returns
///
/// A clipboard that takes this long over a call and answers it with what it holds.
fn holding_text(delay: Duration) -> Stub {
    Stub::new(delay, Ok(Response::Text(CLIPBOARD_TEXT.to_owned())))
}

#[test]
fn a_five_second_clipboard_keeps_the_frames_coming_and_pastes_nothing() -> Result<()> {
    let stub = holding_text(SLOW_READ);
    let mut reader = reader_of(&stub);
    let request = reader.request();

    let mut frames = Frames::over(TYPED);
    frames.draw_until(&mut reader, request, Instant::now() + SLOW_READ + PATIENCE);

    assert_eq!(
        frames.answers,
        vec![Outcome::Unavailable(Reason::Abandoned)]
    );
    assert_eq!(frames.buffer, TYPED);
    assert!(
        FRAME_BOUND >= frames.slowest,
        "a frame spent {:?} on the clipboard",
        frames.slowest
    );
    assert!(
        MIN_FRAMES <= frames.drawn,
        "only {} frames were drawn",
        frames.drawn
    );
    assert!(0 < frames.notices, "the slow read was never mentioned");
    assert_eq!(stub.calls(), 1);

    Ok(())
}

#[test]
fn a_refusing_clipboard_is_asked_once_and_not_again() -> Result<()> {
    let stub = Stub::new(Duration::ZERO, Ok(Response::Failed(REFUSAL.to_owned())));
    let mut reader = reader_of(&stub);
    let request = reader.request();

    let mut frames = Frames::over(TYPED);
    frames.draw_until(&mut reader, request, Instant::now() + WATCH);

    assert_eq!(
        frames.answers,
        vec![Outcome::Unavailable(Reason::Refused(REFUSAL.to_owned()))]
    );
    assert_eq!(frames.buffer, TYPED);
    assert_eq!(stub.calls(), 1);
    assert_eq!(reader.reads_issued(), 1);

    Ok(())
}

#[test]
fn a_paste_asked_for_while_a_read_is_out_costs_no_second_read() -> Result<()> {
    let stub = holding_text(SOFT_DEADLINE * 2);
    let mut reader = reader_of(&stub);

    let first = reader.request();
    let mut early = Frames::over("");
    early.draw_until(&mut reader, first, Instant::now() + SOFT_DEADLINE / 2);

    let second = reader.request();
    let mut late = Frames::over("");
    let until = Instant::now() + PATIENCE;
    while Instant::now() < until {
        early.draw(&mut reader, first);
        late.draw(&mut reader, second);
        thread::sleep(FRAME);
    }

    assert_eq!(
        early.answers,
        vec![Outcome::Text(CLIPBOARD_TEXT.to_owned())]
    );
    assert_eq!(late.answers, early.answers);
    assert_eq!(early.buffer, CLIPBOARD_TEXT);
    assert_eq!(late.buffer, CLIPBOARD_TEXT);
    assert_eq!(stub.calls(), 1);
    assert_eq!(reader.reads_issued(), 1);

    Ok(())
}

#[test]
fn a_read_landing_after_the_hard_deadline_stays_out_of_the_buffer() -> Result<()> {
    let stub = holding_text(LATE_READ);
    let mut reader = reader_of(&stub);
    let request = reader.request();

    let mut frames = Frames::over(TYPED);
    frames.draw_until(
        &mut reader,
        request,
        Instant::now() + HARD_DEADLINE + FRAME_BOUND,
    );
    assert_eq!(
        frames.answers,
        vec![Outcome::Unavailable(Reason::Abandoned)]
    );

    frames.buffer.push_str(EDIT);
    frames.draw_until(&mut reader, request, Instant::now() + LATE_READ);

    assert_eq!(
        frames.answers,
        vec![Outcome::Unavailable(Reason::Abandoned)]
    );
    assert_eq!(frames.buffer, format!("{TYPED}{EDIT}"));
    assert_eq!(stub.calls(), 1);

    Ok(())
}

#[test]
fn a_read_landing_while_no_frame_is_drawn_stays_out_of_the_buffer() -> Result<()> {
    let stub = holding_text(LATE_READ);
    let mut reader = reader_of(&stub);
    let request = reader.request();

    let mut frames = Frames::over(TYPED);
    frames.draw(&mut reader, request);
    thread::sleep(LATE_READ + FRAME_BOUND);

    frames.buffer.push_str(EDIT);
    frames.draw_until(&mut reader, request, Instant::now() + PATIENCE);

    assert_eq!(
        frames.answers,
        vec![Outcome::Unavailable(Reason::Abandoned)]
    );
    assert_eq!(frames.buffer, format!("{TYPED}{EDIT}"));
    assert_eq!(stub.calls(), 1);

    Ok(())
}

#[test]
fn a_failure_pastes_nothing_rather_than_what_the_last_read_found() -> Result<()> {
    let stub = holding_text(Duration::ZERO);
    let mut reader = reader_of(&stub);

    let first = reader.request();
    let mut frames = Frames::over("");
    frames.draw_until(&mut reader, first, Instant::now() + PATIENCE);
    assert_eq!(frames.buffer, CLIPBOARD_TEXT);

    stub.answers_with(Duration::ZERO, Ok(Response::Failed(REFUSAL.to_owned())));
    let second = reader.request();
    let mut after = Frames::over(&frames.buffer);
    after.draw_until(&mut reader, second, Instant::now() + PATIENCE);

    assert_eq!(
        after.answers,
        vec![Outcome::Unavailable(Reason::Refused(REFUSAL.to_owned()))]
    );
    assert_eq!(after.buffer, CLIPBOARD_TEXT);
    assert_eq!(
        reader.poll(first),
        Progress::Ready(Outcome::Unavailable(Reason::Abandoned))
    );
    assert_eq!(stub.calls(), 2);

    Ok(())
}

#[test]
fn starting_the_reader_waits_for_nothing_and_reads_nothing() -> Result<()> {
    let stub = holding_text(Duration::ZERO);
    let started = Instant::now();
    let mut reader = Reader::start({
        let stub = stub.clone();
        move || {
            thread::sleep(STARTUP);

            Ok(stub)
        }
    });
    let cost = started.elapsed();

    assert!(START_BOUND >= cost, "starting the reader cost {cost:?}");

    thread::sleep(STARTUP + IDLE);
    assert_eq!(stub.calls(), 0);
    assert_eq!(reader.reads_issued(), 0);

    let request = reader.request();
    let mut frames = Frames::over("");
    frames.draw_until(&mut reader, request, Instant::now() + PATIENCE);

    assert_eq!(frames.buffer, CLIPBOARD_TEXT);
    assert_eq!(stub.calls(), 1);

    Ok(())
}

#[test]
fn the_status_line_mentions_a_read_that_passes_the_soft_deadline() -> Result<()> {
    let stub = holding_text(SOFT_DEADLINE * 4);
    let mut reader = reader_of(&stub);
    let request = reader.request();

    let fresh = reader.poll(request);
    assert_eq!(fresh, Progress::Waiting);
    assert_eq!(fresh.notice(), None);

    thread::sleep(SOFT_DEADLINE + FRAME_BOUND);
    let slow = reader.poll(request);
    assert_eq!(slow, Progress::Slow);
    assert_eq!(slow.notice(), Some(READING_NOTICE));

    let mut frames = Frames::over("");
    frames.draw_until(&mut reader, request, Instant::now() + PATIENCE);
    assert_eq!(frames.buffer, CLIPBOARD_TEXT);

    Ok(())
}

#[test]
fn a_reader_that_has_stopped_refuses_rather_than_waits() -> Result<()> {
    let stub = holding_text(Duration::ZERO);
    let mut reader = reader_of(&stub);
    reader.shutdown();

    let request = reader.request();
    let asked = Instant::now();
    let progress = reader.poll(request);
    let cost = asked.elapsed();

    assert!(matches!(
        progress,
        Progress::Ready(Outcome::Unavailable(Reason::Refused(_)))
    ));
    assert!(FRAME_BOUND >= cost, "a refusal cost {cost:?}");
    assert_eq!(stub.calls(), 0);
    assert_eq!(reader.reads_issued(), 0);

    Ok(())
}
