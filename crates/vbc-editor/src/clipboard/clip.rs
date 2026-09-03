//! The write path: `clip.exe`, fed UTF-16LE with no byte order mark.
//!
//! A yank is written by a process of its own rather than through the long-lived helper, and the
//! two costs are why. `clip.exe` takes about thirty milliseconds end to end, while the clipboard
//! API the helper's `Set-Clipboard` sits on carries a hidden retry of over a second, which a yank
//! onto a clipboard some other window is holding pays in full. Reading is the operation worth a
//! resident process, because it happens with the cursor waiting on it; writing is the one worth a
//! program that cannot stall.
//!
//! What `clip.exe` is fed decides whether the yank survives. Its standard input is decoded in the
//! console's code page, which on this machine is 936, so UTF-8 bytes arrive as whatever GBK makes
//! of them and everything outside ASCII is destroyed. UTF-16LE is read as UTF-16LE whatever the
//! code page says, and a byte order mark is text rather than a mark once it is on the clipboard,
//! so the encoding here is UTF-16LE with nothing in front of it.
//!
//! The round trip is not byte-exact and this module does not pretend otherwise. `clip.exe` rewrites
//! every line ending to CRLF, so a buffer's own LF comes back as CRLF and a lone CR comes back as
//! CRLF as well. Rather than claim a fidelity it does not have, the text read back is put through
//! [`normalize`], which is why what the editor yanks and what the editor pastes agree while the
//! bytes on the clipboard do not.

use std::error::Error as StdError;
use std::ffi::{OsStr, OsString};
use std::fmt::{Display, Formatter, Result as FmtResult};
use std::io::{ErrorKind, Write};
use std::process::{Command, ExitStatus, Stdio};

/// The program the Windows clipboard is written by.
pub const CLIP: &str = "clip.exe";

/// What stopped a yank from reaching the clipboard.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Error {
    /// The writer could not be started.
    Spawn {
        /// The program that was to be started.
        program: String,

        /// What kind of failure starting it reported.
        kind: ErrorKind,

        /// What the system said about it.
        message: String,
    },

    /// The text could not be handed to a writer that had started.
    Pipe {
        /// What kind of failure the pipe reported.
        kind: ErrorKind,

        /// What the system said about it.
        message: String,
    },

    /// The writer ran and reported that it had not taken the text.
    Refused {
        /// How the writer exited.
        status: String,
    },
}

impl Display for Error {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        match self {
            Self::Spawn {
                program,
                kind,
                message,
            } => write!(
                formatter,
                "{program} could not be started: {kind}: {message}"
            ),
            Self::Pipe { kind, message } => {
                write!(
                    formatter,
                    "the yank could not be written: {kind}: {message}"
                )
            }
            Self::Refused { status } => {
                write!(formatter, "the clipboard writer refused the yank: {status}")
            }
        }
    }
}

impl StdError for Error {}

/// The program a yank is written by, and the arguments it is started with.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Clip {
    program: OsString,
    arguments: Vec<OsString>,
}

impl Clip {
    /// # Returns
    ///
    /// The writer the Windows clipboard is served by.
    pub fn windows() -> Self {
        Self::of(CLIP.into(), Vec::new())
    }

    /// # Returns
    ///
    /// A writer that is some other program taking the text on its standard input.
    pub fn of(program: OsString, arguments: Vec<OsString>) -> Self {
        Self { program, arguments }
    }

    /// # Returns
    ///
    /// The program a yank is written by.
    pub fn program(&self) -> &OsStr {
        &self.program
    }

    /// # Returns
    ///
    /// The arguments it is started with.
    pub fn arguments(&self) -> &[OsString] {
        &self.arguments
    }

    /// Puts text on the clipboard.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * [`Error::Spawn`] if the writer could not be started, or was started without an input pipe.
    /// * [`Error::Pipe`] if the text could not be written to it, or it could not be waited for.
    /// * [`Error::Refused`] if the writer exited saying it had not taken the text.
    pub fn put(&self, text: &str) -> Result<(), Error> {
        let mut child = Command::new(&self.program)
            .args(&self.arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| Error::Spawn {
                program: self.program.to_string_lossy().into_owned(),
                kind: error.kind(),
                message: error.to_string(),
            })?;

        let Some(mut input) = child.stdin.take() else {
            let _ = child.kill();
            let _ = child.wait();

            return Err(Error::Spawn {
                program: self.program.to_string_lossy().into_owned(),
                kind: ErrorKind::BrokenPipe,
                message: "the writer was started without its input pipe".to_owned(),
            });
        };

        let written = input.write_all(&encode(text)).and_then(|()| input.flush());
        drop(input);

        let status = child.wait().map_err(pipe_error);
        written.map_err(pipe_error)?;
        exited(status?)
    }
}

/// # Returns
///
/// The bytes `clip.exe` is fed for this text, which are UTF-16LE with no byte order mark.
pub fn encode(text: &str) -> Vec<u8> {
    let mut encoded = Vec::with_capacity(2 * text.len());
    let mut buffer = [0_u16; 2];
    for character in text.chars() {
        for unit in character.encode_utf16(&mut buffer) {
            encoded.extend_from_slice(&unit.to_le_bytes());
        }
    }

    encoded
}

/// # Returns
///
/// The text with every line ending as a single LF, which is what a buffer holds and what the
/// clipboard's CRLF has to be read back as.
pub fn normalize(text: &str) -> String {
    if !text.contains('\r') {
        return text.to_owned();
    }

    let mut normalized = String::with_capacity(text.len());
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        if '\r' == character {
            if Some(&'\n') == characters.peek() {
                characters.next();
            }
            normalized.push('\n');
        } else {
            normalized.push(character);
        }
    }

    normalized
}

/// # Returns
///
/// The failure a pipe reported, as this module's error.
fn pipe_error(error: std::io::Error) -> Error {
    Error::Pipe {
        kind: error.kind(),
        message: error.to_string(),
    }
}

/// Checks that the writer said it had taken the text.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`Error::Refused`] if the writer did not exit successfully.
fn exited(status: ExitStatus) -> Result<(), Error> {
    if status.success() {
        return Ok(());
    }

    Err(Error::Refused {
        status: status.to_string(),
    })
}
