//! What can go wrong between vimbecode and the session it drives.
//!
//! The variants divide by who is owed the report. [`Error::Untyped`] and [`Error::Undecodable`]
//! are framing: a message one side wrote that the other cannot read, and the first of them is
//! raised before the line is written because the protocol will not raise it afterwards.
//! [`Error::FlagRejected`] and [`Error::FlagIgnored`] are the two ways the permission flag can
//! stop working, which are worth telling apart because one is a binary that refused to start and
//! the other is a binary that started and will deny everything. [`Error::Trust`] is the only one
//! raised before a child exists at all: a directory whose standing could not be settled is a
//! directory nothing is started in, because by the time a spawn has failed the project's code has
//! already run. The rest are the child's life: spawning it, writing to it, and it ending or
//! falling silent.

use std::error::Error as StdError;
use std::fmt::{Display, Formatter, Result as FmtResult};
use std::io;
use std::time::Duration;

/// What went wrong driving a session.
#[derive(Debug)]
pub enum Error {
    /// A frame was asked to be sent that names no type, which the child would drop in silence.
    Untyped {
        /// The frame, as it would have gone out.
        frame: String,
    },

    /// A line the child wrote could not be read as a JSON object.
    Undecodable {
        /// The line, truncated to what is worth printing.
        line: String,

        /// What the JSON reader said about it.
        reason: String,
    },

    /// The `claude` binary refused the permission flag, so it never started.
    FlagRejected {
        /// The flag it refused.
        flag: String,

        /// What it wrote to its standard error before exiting.
        detail: String,
    },

    /// The `claude` binary took the permission flag and did not honour it, so every tool call
    /// needing approval would be auto-denied.
    FlagIgnored {
        /// The flag it accepted and ignored.
        flag: String,

        /// The tools the flag admits, which its own catalog did not name.
        missing: Vec<String>,
    },

    /// Whether a directory may run its own code could not be settled, either because the
    /// directory does not resolve or because the record the reader's trust is kept in could not be
    /// read, understood or written.
    Trust {
        /// The directory, or the record, that could not be used.
        path: String,

        /// What went wrong with it.
        reason: String,
    },

    /// The child could not be started.
    Spawn {
        /// The binary that could not be started.
        binary: String,

        /// What the operating system said about it.
        reason: String,
    },

    /// The child ended, or its stream did, before it had said what was being waited for.
    Ended {
        /// What it wrote to its standard error, which is where a binary that quits says why.
        detail: String,
    },

    /// The child stayed alive and said nothing for longer than was allowed.
    Silent {
        /// How long it was given.
        waited: Duration,
    },

    /// A read or a write on one of the child's pipes failed.
    Pipe {
        /// What the operating system said about it.
        reason: String,
    },
}

impl Error {
    /// # Returns
    ///
    /// A newly created [`Error::Pipe`] over an I/O failure.
    #[must_use]
    pub fn pipe(error: &io::Error) -> Self {
        Self::Pipe {
            reason: error.to_string(),
        }
    }
}

impl Display for Error {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        match self {
            Self::Untyped { frame } => {
                write!(
                    formatter,
                    "refusing to send a frame that names no type, which the session would drop \
                     without reporting it: {frame}"
                )
            }
            Self::Undecodable { line, reason } => {
                write!(formatter, "the session wrote a line ({reason}): {line}")
            }
            Self::FlagRejected { flag, detail } => {
                write!(
                    formatter,
                    "the claude binary refused `{flag}`, which is the flag every tool approval \
                     goes through: {detail}"
                )
            }
            Self::FlagIgnored { flag, missing } => {
                write!(
                    formatter,
                    "the claude binary accepted `{flag}` and did not honour it, so every tool \
                     call needing approval will be auto-denied; its catalog names none of {}",
                    missing.join(", ")
                )
            }
            Self::Trust { path, reason } => {
                write!(
                    formatter,
                    "whether `{path}` may run its own code could not be settled, so no session \
                     was started in it: {reason}"
                )
            }
            Self::Spawn { binary, reason } => {
                write!(formatter, "`{binary}` could not be started: {reason}")
            }
            Self::Ended { detail } => write!(formatter, "the session ended: {detail}"),
            Self::Silent { waited } => {
                write!(formatter, "the session said nothing for {waited:?}")
            }
            Self::Pipe { reason } => write!(formatter, "the session's pipe failed: {reason}"),
        }
    }
}

impl StdError for Error {}
