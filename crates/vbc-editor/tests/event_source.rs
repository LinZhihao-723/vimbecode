//! Checks what the event source is for: that nothing the terminal does to a read can stop the
//! application, and that a paste stays text.
//!
//! The terminal here is a scripted one, which is what makes the checks deterministic. Its script
//! may end in a wedge: a read that never returns, which is what crossterm does while a partial
//! escape sequence sits in its parser. A real terminal doing the same to a real crossterm is
//! checked in `pty_wedge.rs`, which needs a terminal to be checked against; what is checked here
//! is that the design survives it -- if the reading ever moved onto the application's thread,
//! every wedge test below would hang instead of passing.

use std::collections::VecDeque;
use std::io;
use std::sync::mpsc::RecvTimeoutError;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use crossterm::event::{Event as TerminalEvent, KeyCode, KeyEvent, KeyModifiers};
use vbc_editor::event::reader::Reader;
use vbc_editor::event::{Config, Event, Notice, Source};

/// A bare CSI introducer, which is what `ESC[` is: the beginning of an escape sequence that never
/// arrives.
const BARE_CSI: &[u8] = b"\x1b[";

/// A CSI sequence cut off in its parameters, which is what pressing a function key into a
/// disconnected terminal leaves behind.
const NUMERIC_CSI: &[u8] = b"\x1b[20";

/// The first two bytes of the three that spell `U+2500`, which is a UTF-8 sequence split across
/// two reads.
const SPLIT_UTF8: &[u8] = &[0xe2, 0x94];

/// How long a test waits for an event it expects, which is long enough that only an event that is
/// never coming runs it out.
const PATIENCE: Duration = Duration::from_secs(5);

/// The tick interval the tests run the timer at, short enough to watch several ticks go by and
/// long enough that a loaded machine still keeps to it.
const TICK: Duration = Duration::from_millis(20);

/// How long a paste's remainder is waited for in the tests.
const GUARD: Duration = Duration::from_millis(50);

/// A step of a scripted terminal.
enum Step {
    /// An event the terminal reports.
    Event(TerminalEvent),

    /// A failure the terminal reports instead of an event.
    Fail(&'static str),

    /// Nothing to report for a while.
    Idle(Duration),

    /// Bytes that leave crossterm's parser holding a partial sequence, after which its read never
    /// returns.
    Wedge(&'static [u8]),
}

/// A terminal that reports what its script says, and stops reporting anything at all once the
/// script wedges.
struct ScriptedReader {
    steps: VecDeque<Step>,
    wedged: bool,
}

impl ScriptedReader {
    /// # Returns
    ///
    /// A newly created terminal reporting the given steps.
    fn new(steps: Vec<Step>) -> Self {
        Self {
            steps: steps.into(),
            wedged: false,
        }
    }
}

impl Reader for ScriptedReader {
    type Error = io::Error;

    fn poll(&mut self, timeout: Duration) -> io::Result<bool> {
        if self.wedged {
            wedge(&[]);
        }

        match self.steps.front_mut() {
            Some(Step::Event(_)) => Ok(true),
            Some(Step::Fail(reported)) => Err(io::Error::other(*reported)),
            Some(Step::Idle(remaining)) => {
                let slept = timeout.min(*remaining);
                thread::sleep(slept);
                *remaining -= slept;
                if remaining.is_zero() {
                    self.steps.pop_front();
                }
                Ok(false)
            }
            Some(Step::Wedge(bytes)) => {
                self.wedged = true;
                wedge(bytes);
            }
            None => {
                thread::sleep(timeout);
                Ok(false)
            }
        }
    }

    fn read(&mut self) -> io::Result<TerminalEvent> {
        match self.steps.pop_front() {
            Some(Step::Event(event)) => Ok(event),
            _ => Err(io::Error::other("the script has no event to read")),
        }
    }
}

#[test]
fn a_bare_csi_prefix_does_not_wedge_the_application() -> Result<()> {
    assert_ticks_outlive_a_wedge(BARE_CSI)
}

#[test]
fn a_csi_prefix_cut_off_in_its_parameters_does_not_wedge_the_application() -> Result<()> {
    assert_ticks_outlive_a_wedge(NUMERIC_CSI)
}

#[test]
fn a_split_utf8_sequence_does_not_wedge_the_application() -> Result<()> {
    assert_ticks_outlive_a_wedge(SPLIT_UTF8)
}

#[test]
fn redraw_ticks_keep_their_cadence_while_a_read_is_wedged() -> Result<()> {
    const TICKS: u32 = 10;

    let source = start(vec![Step::Wedge(BARE_CSI)], config());
    let started = Instant::now();
    for _ in 0..TICKS {
        expect_redraw(&source)?;
    }
    let elapsed = started.elapsed();

    let expected = TICK * TICKS;
    assert!(
        elapsed >= expected / 2,
        "{TICKS} ticks of {TICK:?} arrived in {elapsed:?}, faster than the timer asks for"
    );
    assert!(
        elapsed <= expected * 5,
        "{TICKS} ticks of {TICK:?} took {elapsed:?}, slower than the timer asks for"
    );
    Ok(())
}

#[test]
fn a_terminal_that_stops_being_readable_is_reported_rather_than_waited_on() -> Result<()> {
    let source = start(
        vec![Step::Event(key('a')), Step::Fail("the terminal went away")],
        config(),
    );

    assert_eq!(
        expect_input(&source)?,
        Event::Key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE))
    );
    assert_eq!(
        expect_input(&source)?,
        Event::Notice(Notice::ReaderFailed {
            message: "the terminal went away".to_owned()
        })
    );
    expect_redraw(&source)?;
    Ok(())
}

#[test]
fn a_paste_terminator_in_pasted_text_does_not_run_the_remainder_as_commands() -> Result<()> {
    let source = start(
        vec![
            Step::Event(TerminalEvent::Paste("print(1)".to_owned())),
            Step::Event(key('d')),
            Step::Event(key('d')),
            Step::Event(chord('c', KeyModifiers::CONTROL)),
            Step::Event(key(':')),
            Step::Event(key('q')),
            Step::Event(TerminalEvent::Key(KeyEvent::new(
                KeyCode::Enter,
                KeyModifiers::NONE,
            ))),
            Step::Idle(GUARD * 4),
            Step::Event(key('x')),
        ],
        config(),
    );

    let Event::Paste(paste) = expect_input(&source)? else {
        return Err(anyhow!("the paste was not delivered as a paste"));
    };
    assert_eq!(paste.text, "print(1)dd:q\n");
    assert_eq!(paste.dropped_keys, 1);

    assert_eq!(
        expect_input(&source)?,
        Event::Key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE)),
        "typing after the paste should be delivered as keys again"
    );
    Ok(())
}

#[test]
fn a_paste_over_the_limit_is_rejected_with_a_notice() -> Result<()> {
    const LIMIT: u64 = 4 * 1024;

    let source = start(
        vec![Step::Event(TerminalEvent::Paste(
            "x".repeat(usize::try_from(LIMIT)? + 1),
        ))],
        config().with_paste_limit(LIMIT),
    );

    assert_eq!(
        expect_input(&source)?,
        Event::Notice(Notice::PasteTooLarge { limit: LIMIT })
    );
    Ok(())
}

#[test]
fn a_paste_of_a_megabyte_is_delivered_whole() -> Result<()> {
    const MEGABYTE: usize = 1024 * 1024;

    let source = start(
        vec![Step::Event(TerminalEvent::Paste("y".repeat(MEGABYTE)))],
        config(),
    );

    let Event::Paste(paste) = expect_input(&source)? else {
        return Err(anyhow!("the paste was not delivered as a paste"));
    };
    assert_eq!(paste.text.len(), MEGABYTE);
    Ok(())
}

#[test]
fn carriage_returns_in_a_paste_are_normalized() -> Result<()> {
    let source = start(
        vec![Step::Event(TerminalEvent::Paste(
            "first\r\nsecond\rthird\nfourth\r\n".to_owned(),
        ))],
        config(),
    );

    let Event::Paste(paste) = expect_input(&source)? else {
        return Err(anyhow!("the paste was not delivered as a paste"));
    };
    assert_eq!(paste.text, "first\nsecond\nthird\nfourth\n");
    Ok(())
}

#[test]
fn a_resize_during_a_paste_is_delivered_after_it() -> Result<()> {
    let source = start(
        vec![
            Step::Event(TerminalEvent::Paste("text".to_owned())),
            Step::Event(TerminalEvent::Resize(80, 24)),
        ],
        config(),
    );

    let Event::Paste(paste) = expect_input(&source)? else {
        return Err(anyhow!("the paste was not delivered as a paste"));
    };
    assert_eq!(paste.text, "text");
    assert_eq!(
        expect_input(&source)?,
        Event::Resize {
            columns: 80,
            rows: 24
        }
    );
    Ok(())
}

#[test]
fn a_burst_of_events_is_neither_lost_nor_reordered() -> Result<()> {
    const BURST: usize = 4096;

    let alphabet: Vec<char> = ('a'..='z').collect();
    let sent: Vec<char> = (0..BURST).map(|index| alphabet[index % 26]).collect();
    let source = start(
        sent.iter().map(|typed| Step::Event(key(*typed))).collect(),
        config(),
    );

    let mut received = Vec::with_capacity(BURST);
    for _ in 0..BURST {
        let Event::Key(event) = expect_input(&source)? else {
            return Err(anyhow!("the burst delivered something other than a key"));
        };
        let KeyCode::Char(typed) = event.code else {
            return Err(anyhow!(
                "the burst delivered a key that was not a character"
            ));
        };
        received.push(typed);
    }

    assert_eq!(received, sent);
    Ok(())
}

/// Checks that the timer outlives a read that never returns, which is what makes the source's
/// thread worth its keep.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`expect_input`]'s return values on failure.
/// * Forwards [`expect_redraw`]'s return values on failure.
fn assert_ticks_outlive_a_wedge(partial: &'static [u8]) -> Result<()> {
    const TICKS: usize = 5;

    let source = start(vec![Step::Event(key('a')), Step::Wedge(partial)], config());

    assert_eq!(
        expect_input(&source)?,
        Event::Key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE)),
        "the key typed before the partial sequence should still arrive"
    );
    for _ in 0..TICKS {
        expect_redraw(&source)?;
    }
    Ok(())
}

/// # Returns
///
/// A key event for the given character with a modifier held.
fn chord(typed: char, modifiers: KeyModifiers) -> TerminalEvent {
    TerminalEvent::Key(KeyEvent::new(KeyCode::Char(typed), modifiers))
}

/// # Returns
///
/// The configuration the tests run a source under.
fn config() -> Config {
    Config::default()
        .with_tick_interval(TICK)
        .with_paste_guard(GUARD)
        .with_poll_interval(Duration::from_millis(10))
}

/// Waits for the next event that is not a redraw tick.
///
/// # Returns
///
/// The event delivered on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`RecvTimeoutError`] if no such event arrived before the source ran out of patience.
fn expect_input(source: &Source) -> Result<Event, RecvTimeoutError> {
    let deadline = Instant::now() + PATIENCE;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match source.recv_timeout(remaining)? {
            Event::Redraw => continue,
            event => return Ok(event),
        }
    }
}

/// Waits for the next redraw tick.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`RecvTimeoutError`] if no tick arrived before the source ran out of patience.
fn expect_redraw(source: &Source) -> Result<(), RecvTimeoutError> {
    let deadline = Instant::now() + PATIENCE;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if Event::Redraw == source.recv_timeout(remaining)? {
            return Ok(());
        }
    }
}

/// # Returns
///
/// A key event for the given character, typed with nothing held.
fn key(typed: char) -> TerminalEvent {
    TerminalEvent::Key(KeyEvent::new(KeyCode::Char(typed), KeyModifiers::NONE))
}

/// # Returns
///
/// A source reading the given script.
fn start(steps: Vec<Step>, config: Config) -> Source {
    Source::start(ScriptedReader::new(steps), config)
}

/// Stops the calling thread for good, the way crossterm's read stops on a partial sequence.
fn wedge(bytes: &[u8]) -> ! {
    if !bytes.is_empty() {
        eprintln!("the scripted terminal wedged on {bytes:x?}");
    }
    loop {
        thread::park();
    }
}
