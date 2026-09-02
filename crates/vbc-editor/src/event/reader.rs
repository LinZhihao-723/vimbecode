//! The raw terminal events an event source reads, and the terminal that produces them.
//!
//! A reader is the narrowest thing an event source needs: something that waits for an event and
//! then hands it over. Keeping it behind a trait is what lets the source be tested against a
//! terminal that misbehaves on purpose -- one whose read never returns, which is what a real
//! terminal does to crossterm while a partial escape sequence sits in its parser.

use std::io;
use std::time::Duration;

use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste, Event as TerminalEvent};
use crossterm::execute;

/// A source of raw terminal events.
pub trait Reader {
    /// The reason an event could not be waited for or read.
    type Error: std::error::Error;

    /// Waits for an event to become available.
    ///
    /// # Parameters
    ///
    /// * `timeout` - How long to wait before reporting that nothing is available.
    ///
    /// # Returns
    ///
    /// Whether an event is available on success.
    ///
    /// # Errors
    ///
    /// Returns an error if the terminal could not be waited on.
    fn poll(&mut self, timeout: Duration) -> Result<bool, Self::Error>;

    /// Reads the event [`Reader::poll`] reported, blocking until one arrives if it reported none.
    ///
    /// # Returns
    ///
    /// The event read on success.
    ///
    /// # Errors
    ///
    /// Returns an error if the terminal could not be read.
    fn read(&mut self) -> Result<TerminalEvent, Self::Error>;
}

/// The terminal the process is attached to, read through crossterm.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TerminalReader;

impl TerminalReader {
    /// # Returns
    ///
    /// A newly created reader of the process's terminal.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl Reader for TerminalReader {
    type Error = io::Error;

    fn poll(&mut self, timeout: Duration) -> Result<bool, Self::Error> {
        crossterm::event::poll(timeout)
    }

    fn read(&mut self) -> Result<TerminalEvent, Self::Error> {
        crossterm::event::read()
    }
}

/// Asks the terminal to bracket pasted text, without which a paste arrives as the keys it spells
/// and cannot be told apart from typing.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`std::io::Error`] if the request could not be written to the standard output.
pub fn enable_bracketed_paste() -> io::Result<()> {
    execute!(io::stdout(), EnableBracketedPaste)
}

/// Asks the terminal to stop bracketing pasted text, leaving it as a terminal that was never
/// asked to bracket it behaves.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`std::io::Error`] if the request could not be written to the standard output.
pub fn disable_bracketed_paste() -> io::Result<()> {
    execute!(io::stdout(), DisableBracketedPaste)
}
