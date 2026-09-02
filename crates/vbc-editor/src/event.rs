//! The events an application loop consumes, and the source that delivers them.
//!
//! crossterm's `poll` does not return while a partial escape sequence sits in its parser: the
//! bytes are read, the parser has no event to yield, and the read is entered again without a
//! timeout between it and the caller. An `ESC[` typed on its own is enough, and so is a UTF-8
//! sequence split across two reads. A watchdog cannot rescue such a read, because the watchdog
//! would have to run on the thread that is already inside `poll`. The source therefore reads on a
//! thread of its own and hands events to the application over a channel, which costs a wedged
//! read the input that follows it and nothing else. Redraw ticks come from a timer on a second
//! thread rather than from a poll timeout, which is what keeps the screen alive while input is
//! stuck -- and it is why the application loop must never poll the terminal itself.
//!
//! Pasted text arrives as one event and is delivered as one event, with `\r\n` and `\r` turned
//! into `\n` and with a limit on how much of it is kept. A terminator embedded in the pasted text
//! ends the paste early, and the terminal sends the remainder as ordinary keys, which a modal
//! editor would run as commands. Keys that arrive on the heels of a paste are therefore folded
//! back into it as text; the ones that have no textual form are dropped and counted rather than
//! delivered.

pub mod reader;

use std::fmt::{Display, Formatter, Result as FmtResult};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvError, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crossterm::event::{Event as TerminalEvent, KeyCode, KeyEventKind, KeyModifiers};

pub use crossterm::event::KeyEvent;

use crate::event::reader::Reader;

/// How long a paste's terminator may be missing before the keys that follow are taken for typing
/// rather than for the remainder of the paste.
pub const DEFAULT_PASTE_GUARD: Duration = Duration::from_millis(25);

/// How much pasted text is kept, in bytes.
pub const DEFAULT_PASTE_LIMIT: u64 = 4 * 1024 * 1024;

/// How long a read waits before it is entered again, which is how long a source takes to notice
/// that it has been dropped.
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// How long the timer waits between redraw ticks.
pub const DEFAULT_TICK_INTERVAL: Duration = Duration::from_millis(16);

/// Something the application loop acts on.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Event {
    /// A key the terminal reported, which no paste was in the middle of arriving.
    Key(KeyEvent),

    /// Text pasted into the terminal, which is text and never commands.
    Paste(Paste),

    /// The terminal was resized.
    Resize {
        /// The width of the terminal, in cells.
        columns: u16,

        /// The height of the terminal, in cells.
        rows: u16,
    },

    /// The timer asking for the screen to be painted again.
    Redraw,

    /// Something the reader could not do, to be shown rather than acted on.
    Notice(Notice),
}

/// Something worth telling the reader of the screen about, which no keystroke asked for.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Notice {
    /// A paste was discarded for being larger than the limit.
    PasteTooLarge {
        /// The limit the paste was measured against, in bytes.
        limit: u64,
    },

    /// The terminal could no longer be read, which ends the delivery of input.
    ReaderFailed {
        /// What the reader reported.
        message: String,
    },
}

impl Display for Notice {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        match self {
            Self::PasteTooLarge { limit } => {
                write!(formatter, "paste discarded: larger than {limit} bytes")
            }
            Self::ReaderFailed { message } => {
                write!(formatter, "terminal input stopped: {message}")
            }
        }
    }
}

/// Text pasted into the terminal.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Paste {
    /// The pasted text, with every `\r\n` and every lone `\r` turned into a `\n`.
    pub text: String,

    /// How many keys the terminal split out of the paste were dropped for having no textual form.
    /// They are dropped rather than delivered because a modal editor would run them as commands.
    pub dropped_keys: usize,
}

/// What a source is asked to do beyond reading: how often to ask for a redraw, how long to keep
/// treating keys as the remainder of a paste, and how much pasted text to keep.
///
/// An interval of zero is raised to a millisecond, since a timer that never sleeps only starves
/// the threads it was meant to serve.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Config {
    tick_interval: Duration,
    paste_guard: Duration,
    paste_limit: u64,
    poll_interval: Duration,
}

impl Config {
    /// # Returns
    ///
    /// This configuration with the timer waiting the given time between redraw ticks.
    #[must_use]
    pub fn with_tick_interval(mut self, interval: Duration) -> Self {
        self.tick_interval = interval;
        self
    }

    /// # Returns
    ///
    /// This configuration treating keys that arrive within the given time of a paste as its
    /// remainder.
    #[must_use]
    pub fn with_paste_guard(mut self, guard: Duration) -> Self {
        self.paste_guard = guard;
        self
    }

    /// # Returns
    ///
    /// This configuration keeping at most the given number of bytes of pasted text.
    #[must_use]
    pub fn with_paste_limit(mut self, limit: u64) -> Self {
        self.paste_limit = limit;
        self
    }

    /// # Returns
    ///
    /// This configuration waiting the given time in each read before entering the next one.
    #[must_use]
    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            tick_interval: DEFAULT_TICK_INTERVAL,
            paste_guard: DEFAULT_PASTE_GUARD,
            paste_limit: DEFAULT_PASTE_LIMIT,
            poll_interval: DEFAULT_POLL_INTERVAL,
        }
    }
}

/// The events of a terminal, delivered from threads of their own.
///
/// One thread reads the terminal and one keeps time, so a read that never returns costs the
/// application the input behind it and leaves everything else running. Neither thread is joined
/// when the source is dropped: a read wedged in crossterm's parser cannot be woken, so the reading
/// thread is asked to stop and left to notice on its next return, and the source stops delivering
/// its events either way.
#[derive(Debug)]
pub struct Source {
    events: Receiver<Event>,
    redraw_pending: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
}

impl Source {
    /// Factory function.
    ///
    /// Starts reading the given terminal and ticking the timer.
    ///
    /// # Type Parameters
    ///
    /// * `ReaderType` - The terminal to read, which is moved onto the reading thread.
    ///
    /// # Returns
    ///
    /// A newly created source, already delivering.
    pub fn start<ReaderType: Reader + Send + 'static>(reader: ReaderType, config: Config) -> Self {
        let (events, receiver) = mpsc::channel();
        let redraw_pending = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));

        let reader_events = events.clone();
        let reader_stop = Arc::clone(&stop);
        thread::spawn(move || read_events(reader, &reader_events, &reader_stop, &config));

        let timer_pending = Arc::clone(&redraw_pending);
        let timer_stop = Arc::clone(&stop);
        let interval = config.tick_interval.max(MINIMUM_INTERVAL);
        thread::spawn(move || tick(&events, &timer_pending, &timer_stop, interval));

        Self {
            events: receiver,
            redraw_pending,
            stop,
        }
    }

    /// Waits for the next event.
    ///
    /// # Returns
    ///
    /// The next event on success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * [`RecvError`] if neither the terminal nor the timer will deliver again.
    pub fn recv(&self) -> Result<Event, RecvError> {
        self.events.recv().map(|event| self.dispatch(event))
    }

    /// Waits for the next event, giving up after the given time.
    ///
    /// # Returns
    ///
    /// The next event on success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * [`RecvTimeoutError::Timeout`] if no event arrived in time.
    /// * [`RecvTimeoutError::Disconnected`] if neither the terminal nor the timer will deliver
    ///   again.
    pub fn recv_timeout(&self, timeout: Duration) -> Result<Event, RecvTimeoutError> {
        self.events
            .recv_timeout(timeout)
            .map(|event| self.dispatch(event))
    }

    /// Hands an event to the caller, letting the timer queue another tick once the one it queued
    /// has been taken, so that an application that stops reading does not come back to a backlog
    /// of stale ticks.
    ///
    /// # Returns
    ///
    /// The event handed over.
    fn dispatch(&self, event: Event) -> Event {
        if Event::Redraw == event {
            self.redraw_pending.store(false, Ordering::Relaxed);
        }
        event
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// The shortest a source waits anywhere it would otherwise be asked to wait no time at all.
const MINIMUM_INTERVAL: Duration = Duration::from_millis(1);

/// Appends a character of a paste, refusing it if the paste would outgrow the limit.
///
/// # Returns
///
/// Whether the character was appended.
fn append_pasted_char(text: &mut String, character: char, limit: u64) -> bool {
    if text.len() as u64 + character.len_utf8() as u64 > limit {
        return false;
    }
    text.push(character);
    true
}

/// Appends a chunk of a paste with its line endings normalized, refusing it if the paste would
/// outgrow the limit.
///
/// The chunk is measured before it is normalized, which only ever shortens it, so a chunk past the
/// limit is refused without being copied.
///
/// # Returns
///
/// Whether the chunk was appended.
fn append_pasted_chunk(text: &mut String, chunk: &str, limit: u64) -> bool {
    if text.len() as u64 + chunk.len() as u64 > limit {
        return false;
    }

    let mut characters = chunk.chars().peekable();
    while let Some(character) = characters.next() {
        if '\r' == character {
            if Some(&'\n') == characters.peek() {
                characters.next();
            }
            text.push('\n');
        } else {
            text.push(character);
        }
    }
    true
}

/// Collects a paste, together with whatever the terminal split out of it.
///
/// A terminator embedded in the pasted text ends the paste early, and the remainder arrives as
/// keys. Keys that keep arriving within the guard are therefore taken for the rest of the paste
/// and folded back into its text, so that none of them reaches the application as a keystroke.
///
/// # Type Parameters
///
/// * `ReaderType` - The terminal the rest of the paste is read from.
///
/// # Returns
///
/// The events to deliver, the paste or the notice that replaces it first, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`Reader::poll`]'s return values on failure.
/// * Forwards [`Reader::read`]'s return values on failure.
fn collect_paste<ReaderType: Reader>(
    reader: &mut ReaderType,
    pasted: &str,
    config: &Config,
) -> Result<Vec<Event>, ReaderType::Error> {
    let mut text = String::new();
    let mut dropped_keys = 0;
    let mut rejected = !append_pasted_chunk(&mut text, pasted, config.paste_limit);
    let mut deferred = Vec::new();

    while reader.poll(config.paste_guard)? {
        match reader.read()? {
            TerminalEvent::Paste(more) => {
                rejected = rejected || !append_pasted_chunk(&mut text, &more, config.paste_limit);
            }
            TerminalEvent::Key(key) => {
                if KeyEventKind::Release == key.kind {
                    continue;
                }
                match pasted_char(&key) {
                    Some(character) => {
                        rejected = rejected
                            || !append_pasted_char(&mut text, character, config.paste_limit);
                    }
                    None => dropped_keys += 1,
                }
            }
            TerminalEvent::Resize(columns, rows) => {
                deferred.push(Event::Resize { columns, rows });
            }
            _ => {}
        }

        if rejected {
            text = String::new();
        }
    }

    let mut delivered = Vec::with_capacity(deferred.len() + 1);
    delivered.push(if rejected {
        Event::Notice(Notice::PasteTooLarge {
            limit: config.paste_limit,
        })
    } else {
        Event::Paste(Paste { text, dropped_keys })
    });
    delivered.append(&mut deferred);
    Ok(delivered)
}

/// # Returns
///
/// The character a key stands for in pasted text, or `None` if it stands for none, which every
/// chord and every key that is not a character does.
fn pasted_char(key: &KeyEvent) -> Option<char> {
    if !key.modifiers.difference(KeyModifiers::SHIFT).is_empty() {
        return None;
    }

    match key.code {
        KeyCode::Char(character) => Some(character),
        KeyCode::Enter => Some('\n'),
        KeyCode::Tab => Some('\t'),
        _ => None,
    }
}

/// Waits for the terminal to report something, and turns what it reports into what the
/// application is given.
///
/// # Type Parameters
///
/// * `ReaderType` - The terminal to read.
///
/// # Returns
///
/// The events to deliver, none of them if the wait ran out, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`Reader::poll`]'s return values on failure.
/// * Forwards [`Reader::read`]'s return values on failure.
/// * Forwards [`collect_paste`]'s return values on failure.
fn next_events<ReaderType: Reader>(
    reader: &mut ReaderType,
    poll_interval: Duration,
    config: &Config,
) -> Result<Vec<Event>, ReaderType::Error> {
    if !reader.poll(poll_interval)? {
        return Ok(Vec::new());
    }

    Ok(match reader.read()? {
        TerminalEvent::Paste(pasted) => collect_paste(reader, &pasted, config)?,
        TerminalEvent::Key(key) => vec![Event::Key(key)],
        TerminalEvent::Resize(columns, rows) => vec![Event::Resize { columns, rows }],
        _ => Vec::new(),
    })
}

/// Reads the terminal until it fails, until the source is dropped, or until nothing is listening,
/// delivering what it reads.
///
/// # Type Parameters
///
/// * `ReaderType` - The terminal to read.
fn read_events<ReaderType: Reader>(
    mut reader: ReaderType,
    events: &Sender<Event>,
    stop: &AtomicBool,
    config: &Config,
) {
    let poll_interval = config.poll_interval.max(MINIMUM_INTERVAL);

    while !stop.load(Ordering::Relaxed) {
        let (delivered, failed) = match next_events(&mut reader, poll_interval, config) {
            Ok(delivered) => (delivered, false),
            Err(error) => (
                vec![Event::Notice(Notice::ReaderFailed {
                    message: error.to_string(),
                })],
                true,
            ),
        };

        for event in delivered {
            if events.send(event).is_err() {
                return;
            }
        }
        if failed {
            return;
        }
    }
}

/// Asks for a redraw every interval until the source is dropped or nothing is listening, holding
/// off while a tick nobody has taken is still queued.
fn tick(
    events: &Sender<Event>,
    redraw_pending: &AtomicBool,
    stop: &AtomicBool,
    interval: Duration,
) {
    while !stop.load(Ordering::Relaxed) {
        thread::sleep(interval);
        if stop.load(Ordering::Relaxed) {
            return;
        }
        if redraw_pending.swap(true, Ordering::Relaxed) {
            continue;
        }
        if events.send(Event::Redraw).is_err() {
            return;
        }
    }
}
