//! The request and response frames the editor and the clipboard helper exchange.
//!
//! A frame is a four-byte big-endian length followed by that many bytes, and the first byte of
//! those names what the frame is. Length prefixing is what lets a payload hold anything at all: a
//! clipboard carries whatever was copied into it, up to and including the bytes this protocol
//! frames its own messages with, so nothing may be found by scanning for a delimiter.
//!
//! An answer is a typed status rather than a string that may or may not be there. `Get-Clipboard`
//! hands back nothing at all for an image and for a list of files, exactly as it does for a
//! clipboard that is empty and for one holding an empty string, so a helper that returned text or
//! no text would make `"+p` after a screenshot indistinguishable from `"+p` after copying nothing:
//! both would paste silently and neither would say why. The four answers are therefore four
//! things, and a fifth says a yank landed.
//!
//! A malformed frame is a different kind of answer again, and it is a [`Result`] error rather than
//! a status: a stream that ended early has told us nothing about the clipboard, and the one thing
//! that must never happen is pasting the truncated prefix of a megabyte as though it were all that
//! was copied. Every length is checked against what actually arrived, and no frame larger than
//! [`MAX_FRAME_SIZE`] is allocated for on the strength of its own claim about itself.
//!
//! Frames are read in bulk. A pipe read costs about the same whether it returns one byte or a
//! megabyte, so a reader that takes a byte at a time pays that cost a million times over: it is
//! the difference between reading 1.1 MB in thirty milliseconds and taking forty-one seconds over
//! it, which is why the reading here is whole-buffer and why the tests time it.

use std::error::Error as StdError;
use std::fmt::{Display, Formatter, Result as FmtResult};
use std::io::{self, ErrorKind, Read, Write};

/// The largest frame either side sends or accepts, in bytes. A length prefix claiming more than
/// this is a malformed frame rather than a large one, so no peer can talk this side into
/// allocating for a message nobody is going to send.
pub const MAX_FRAME_SIZE: u64 = 64 * 1024 * 1024;

/// The bytes a frame's length prefix occupies.
const LENGTH_PREFIX_SIZE: usize = 4;

/// The tag naming a request for what the clipboard holds.
const READ_TAG: u8 = 0x01;

/// The tag naming a request to put text on the clipboard.
const WRITE_TAG: u8 = 0x02;

/// The status naming a clipboard that holds text, whose body is that text.
const TEXT_STATUS: u8 = 0x01;

/// The status naming a clipboard that holds nothing.
const EMPTY_STATUS: u8 = 0x02;

/// The status naming a clipboard that holds something other than text.
const NON_TEXT_STATUS: u8 = 0x03;

/// The status naming a request the helper could not carry out, whose body says why.
const FAILED_STATUS: u8 = 0x04;

/// The status naming a completed write.
const STORED_STATUS: u8 = 0x05;

/// What is wrong with a frame, or with the stream it was being read from.
///
/// Every variant means the exchange told us nothing about the clipboard, which is what separates
/// these from [`Response::Failed`]: that one is the helper reporting, and these are the report
/// never arriving intact.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Error {
    /// The stream ended before the frame it had begun was complete.
    Truncated {
        /// The bytes the read needed, which the stream had fewer of.
        expected: u64,
    },

    /// A length prefix claimed more than [`MAX_FRAME_SIZE`] bytes.
    TooLarge {
        /// The length the prefix claimed.
        length: u64,
    },

    /// A frame carried no bytes at all, so it names nothing.
    NoTag,

    /// A request named something this protocol does not speak.
    UnknownRequest {
        /// The tag the request carried.
        tag: u8,
    },

    /// A response named a status this protocol does not speak.
    UnknownStatus {
        /// The status the response carried.
        status: u8,
    },

    /// A frame carried a body its tag accounts for no bytes of.
    UnexpectedBody {
        /// The tag the frame carried.
        tag: u8,

        /// The bytes that followed it.
        length: u64,
    },

    /// A body that is meant to be text was not valid UTF-8.
    NotUtf8,

    /// The stream itself failed.
    Io {
        /// What kind of failure the stream reported.
        kind: ErrorKind,

        /// What the stream said about it.
        message: String,
    },
}

impl Display for Error {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        match self {
            Self::Truncated { expected } => write!(
                formatter,
                "the stream ended before the {expected} bytes a frame needed"
            ),
            Self::TooLarge { length } => write!(
                formatter,
                "a frame claimed {length} bytes, over the {MAX_FRAME_SIZE} byte limit"
            ),
            Self::NoTag => write!(formatter, "a frame carried no tag"),
            Self::UnknownRequest { tag } => write!(formatter, "unknown request tag {tag:#04x}"),
            Self::UnknownStatus { status } => {
                write!(formatter, "unknown response status {status:#04x}")
            }
            Self::UnexpectedBody { tag, length } => write!(
                formatter,
                "tag {tag:#04x} carries no body, and {length} bytes followed it"
            ),
            Self::NotUtf8 => write!(formatter, "a body that is meant to be text is not UTF-8"),
            Self::Io { kind, message } => write!(formatter, "the stream failed: {kind}: {message}"),
        }
    }
}

impl StdError for Error {}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::Io {
            kind: error.kind(),
            message: error.to_string(),
        }
    }
}

/// What the editor asks the helper for.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Request {
    /// Hand back what the clipboard holds.
    Read,

    /// Put this text on the clipboard.
    Write(String),
}

impl Request {
    /// # Returns
    ///
    /// The request a frame's payload holds, on success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * [`Error::NoTag`] if the payload is empty.
    /// * [`Error::UnknownRequest`] if the payload names a request this protocol does not speak.
    /// * Forwards [`body_free`]'s return values on failure.
    /// * Forwards [`text`]'s return values on failure.
    pub fn decode(payload: &[u8]) -> Result<Self, Error> {
        let (tag, body) = payload.split_first().ok_or(Error::NoTag)?;
        match *tag {
            READ_TAG => {
                body_free(*tag, body)?;
                Ok(Self::Read)
            }
            WRITE_TAG => Ok(Self::Write(text(body)?)),
            tag => Err(Error::UnknownRequest { tag }),
        }
    }

    /// Reads one request from a stream.
    ///
    /// # Type Parameters
    ///
    /// * `ReaderType` - The stream the request is read from.
    ///
    /// # Returns
    ///
    /// The request that was read, on success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * Forwards [`read_frame`]'s return values on failure.
    /// * Forwards [`Request::decode`]'s return values on failure.
    pub fn read_from<ReaderType: Read>(reader: &mut ReaderType) -> Result<Self, Error> {
        Self::decode(&read_frame(reader)?)
    }

    /// Writes this request to a stream and flushes it.
    ///
    /// # Type Parameters
    ///
    /// * `WriterType` - The stream the request is written to.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * Forwards [`write_frame`]'s return values on failure.
    pub fn write_to<WriterType: Write>(&self, writer: &mut WriterType) -> Result<(), Error> {
        write_frame(writer, &self.payload())
    }

    /// # Returns
    ///
    /// The tag and body this request is framed around.
    fn payload(&self) -> Vec<u8> {
        match self {
            Self::Read => tagged(READ_TAG, &[]),
            Self::Write(text) => tagged(WRITE_TAG, text.as_bytes()),
        }
    }
}

/// What the helper answers with.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Response {
    /// The clipboard holds text, which is what a paste inserts.
    Text(String),

    /// The clipboard holds nothing.
    Empty,

    /// The clipboard holds something a paste cannot insert, such as an image or a list of files.
    NonText,

    /// The helper could not carry the request out, and says why.
    Failed(String),

    /// The text a write carried is on the clipboard.
    Stored,
}

impl Response {
    /// # Returns
    ///
    /// The response a frame's payload holds, on success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * [`Error::NoTag`] if the payload is empty.
    /// * [`Error::UnknownStatus`] if the payload names a status this protocol does not speak.
    /// * Forwards [`body_free`]'s return values on failure.
    /// * Forwards [`text`]'s return values on failure.
    pub fn decode(payload: &[u8]) -> Result<Self, Error> {
        let (status, body) = payload.split_first().ok_or(Error::NoTag)?;
        match *status {
            TEXT_STATUS => Ok(Self::Text(text(body)?)),
            EMPTY_STATUS => {
                body_free(*status, body)?;
                Ok(Self::Empty)
            }
            NON_TEXT_STATUS => {
                body_free(*status, body)?;
                Ok(Self::NonText)
            }
            FAILED_STATUS => Ok(Self::Failed(text(body)?)),
            STORED_STATUS => {
                body_free(*status, body)?;
                Ok(Self::Stored)
            }
            status => Err(Error::UnknownStatus { status }),
        }
    }

    /// Reads one response from a stream.
    ///
    /// # Type Parameters
    ///
    /// * `ReaderType` - The stream the response is read from.
    ///
    /// # Returns
    ///
    /// The response that was read, on success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * Forwards [`read_frame`]'s return values on failure.
    /// * Forwards [`Response::decode`]'s return values on failure.
    pub fn read_from<ReaderType: Read>(reader: &mut ReaderType) -> Result<Self, Error> {
        Self::decode(&read_frame(reader)?)
    }

    /// Writes this response to a stream and flushes it.
    ///
    /// # Type Parameters
    ///
    /// * `WriterType` - The stream the response is written to.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * Forwards [`write_frame`]'s return values on failure.
    pub fn write_to<WriterType: Write>(&self, writer: &mut WriterType) -> Result<(), Error> {
        write_frame(writer, &self.payload())
    }

    /// # Returns
    ///
    /// The status and body this response is framed around.
    fn payload(&self) -> Vec<u8> {
        match self {
            Self::Text(text) => tagged(TEXT_STATUS, text.as_bytes()),
            Self::Empty => tagged(EMPTY_STATUS, &[]),
            Self::NonText => tagged(NON_TEXT_STATUS, &[]),
            Self::Failed(reason) => tagged(FAILED_STATUS, reason.as_bytes()),
            Self::Stored => tagged(STORED_STATUS, &[]),
        }
    }
}

/// Reads one frame from a stream, in as few reads as the stream will give it up in.
///
/// # Type Parameters
///
/// * `ReaderType` - The stream the frame is read from.
///
/// # Returns
///
/// The payload the frame carried, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`Error::TooLarge`] if the frame's length prefix claims more than [`MAX_FRAME_SIZE`] bytes.
/// * Forwards [`read_exactly`]'s return values on failure.
pub fn read_frame<ReaderType: Read>(reader: &mut ReaderType) -> Result<Vec<u8>, Error> {
    let mut prefix = [0_u8; LENGTH_PREFIX_SIZE];
    read_exactly(reader, &mut prefix)?;

    let length = u64::from(u32::from_be_bytes(prefix));
    if MAX_FRAME_SIZE < length {
        return Err(Error::TooLarge { length });
    }
    let length = usize::try_from(length).map_err(|_| Error::TooLarge { length })?;

    let mut payload = vec![0_u8; length];
    read_exactly(reader, &mut payload)?;

    Ok(payload)
}

/// Writes one frame to a stream and flushes it.
///
/// # Type Parameters
///
/// * `WriterType` - The stream the frame is written to.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`Error::TooLarge`] if the payload is longer than [`MAX_FRAME_SIZE`].
/// * Forwards [`std::io::Write::write_all`]'s return values on failure.
/// * Forwards [`std::io::Write::flush`]'s return values on failure.
pub fn write_frame<WriterType: Write>(
    writer: &mut WriterType,
    payload: &[u8],
) -> Result<(), Error> {
    let length = payload.len() as u64;
    if MAX_FRAME_SIZE < length {
        return Err(Error::TooLarge { length });
    }
    let prefix = (length as u32).to_be_bytes();

    writer.write_all(&prefix)?;
    writer.write_all(payload)?;
    writer.flush()?;

    Ok(())
}

/// # Returns
///
/// The payload a tag and a body make up, which is what a frame's length prefix counts.
fn tagged(tag: u8, body: &[u8]) -> Vec<u8> {
    let mut tagged = Vec::with_capacity(1 + body.len());
    tagged.push(tag);
    tagged.extend_from_slice(body);

    tagged
}

/// Fills a buffer from a stream.
///
/// # Type Parameters
///
/// * `ReaderType` - The stream the buffer is filled from.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`Error::Truncated`] if the stream ended before the buffer was full.
/// * Forwards [`std::io::Read::read_exact`]'s return values on failure.
fn read_exactly<ReaderType: Read>(reader: &mut ReaderType, buffer: &mut [u8]) -> Result<(), Error> {
    reader.read_exact(buffer).map_err(|error| {
        if ErrorKind::UnexpectedEof == error.kind() {
            Error::Truncated {
                expected: buffer.len() as u64,
            }
        } else {
            error.into()
        }
    })
}

/// # Returns
///
/// The text a body holds, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`Error::NotUtf8`] if the body is not valid UTF-8.
fn text(body: &[u8]) -> Result<String, Error> {
    String::from_utf8(body.to_vec()).map_err(|_| Error::NotUtf8)
}

/// Checks that a tag which accounts for no body was sent with none.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`Error::UnexpectedBody`] if any bytes followed the tag.
fn body_free(tag: u8, body: &[u8]) -> Result<(), Error> {
    if body.is_empty() {
        return Ok(());
    }

    Err(Error::UnexpectedBody {
        tag,
        length: body.len() as u64,
    })
}
