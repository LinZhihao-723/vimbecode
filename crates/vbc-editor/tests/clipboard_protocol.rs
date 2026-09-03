//! Checks what the clipboard protocol is for: that an answer says which of the things it can be,
//! and that a megabyte of it arrives whole and quickly.
//!
//! The status is the point of the protocol rather than decoration on it. Windows hands back
//! nothing for an image and nothing for a list of files, exactly as it does for a clipboard that
//! is empty, so a wire carrying text-or-nothing collapses three different situations into one
//! silent paste. The four answers are required here to be four outcomes, told apart both at the
//! API and on the wire, and a malformed frame is required to be a fifth thing again: an error,
//! never a short string. Pasting the truncated prefix of a yank as though it were the yank is the
//! one failure this protocol exists to make impossible, so every way of cutting a megabyte-long
//! frame short is fed to the reader and required to come back as an error.
//!
//! The cost is checked as well as the correctness, because the reason there is a helper at all is
//! that the naive way of reading a clipboard is slow. A pipe read costs about the same whatever it
//! returns, and a reader that takes one byte at a time turned a thirty-millisecond megabyte into
//! forty-one seconds here once already. The reader below charges for reads the way a pipe does, so
//! a byte-at-a-time reader would pay a fixed cost a million times over; the bound the read is held
//! to is one the arithmetic in the test says such a reader could not meet, and it is asserted
//! alongside the count of the reads themselves so the guard does not rest on a machine's mood.
//!
//! The wire bytes are written out here rather than taken from the protocol's own constants. A test
//! that asks the code what tag it uses agrees with the code by construction; these are the numbers
//! the two ends of a pipe have to agree on, so changing one of them should be a red test.

use std::io::{self, Read};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use vbc_editor::clipboard::protocol::{read_frame, Error, Request, Response, MAX_FRAME_SIZE};

#[cfg(unix)]
use std::os::unix::net::UnixStream;

/// The bytes of the payload the size checks carry, which is the megabyte-and-change that yanking
/// a build log out of a terminal is.
const PAYLOAD_SIZE: usize = 1_100_000;

/// The bytes a pipe hands over in one read, which is what makes the reads below plural.
const PIPE_CHUNK: usize = 64 * 1024;

/// What one read costs the timed reader, standing in for the syscall a real pipe read is.
const READ_COST: Duration = Duration::from_micros(20);

/// How long the timed read of a megabyte is allowed. A reader taking a byte at a time would spend
/// [`READ_COST`] on every one of them, which the test checks is more than this.
const BOUND: Duration = Duration::from_secs(2);

/// The reads a megabyte-long frame is allowed to take, which is generous for a whole-buffer reader
/// over a pipe and unreachable for one that takes a byte at a time.
const READ_BUDGET: usize = 64;

/// The wire byte naming a request for the clipboard's contents.
const READ_TAG: u8 = 0x01;

/// The wire byte naming a request to put text on the clipboard.
const WRITE_TAG: u8 = 0x02;

/// The wire bytes naming each of the answers, in the order the protocol numbers them.
const TEXT_STATUS: u8 = 0x01;
const EMPTY_STATUS: u8 = 0x02;
const NON_TEXT_STATUS: u8 = 0x03;
const FAILED_STATUS: u8 = 0x04;
const STORED_STATUS: u8 = 0x05;

/// A byte no tag and no status is spoken for by.
const UNSPOKEN: u8 = 0x7f;

/// A reader that hands bytes over the way a pipe does: never more than one buffer's worth at a
/// time, at a fixed cost per read whatever that read returns, and counting them.
struct PipeLike {
    bytes: Vec<u8>,
    offset: usize,
    cost: Duration,
    reads: usize,
}

impl PipeLike {
    /// # Returns
    ///
    /// A reader over some bytes, charging nothing per read.
    fn over(bytes: Vec<u8>) -> Self {
        Self {
            bytes,
            offset: 0,
            cost: Duration::ZERO,
            reads: 0,
        }
    }

    /// # Returns
    ///
    /// A reader over some bytes, charging a cost per read.
    fn charging(bytes: Vec<u8>, cost: Duration) -> Self {
        Self {
            cost,
            ..Self::over(bytes)
        }
    }
}

impl Read for PipeLike {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if !self.cost.is_zero() {
            thread::sleep(self.cost);
        }
        self.reads += 1;

        let taken = buffer
            .len()
            .min(PIPE_CHUNK)
            .min(self.bytes.len() - self.offset);
        buffer[..taken].copy_from_slice(&self.bytes[self.offset..self.offset + taken]);
        self.offset += taken;

        Ok(taken)
    }
}

#[test]
fn a_megabyte_of_text_round_trips_through_the_protocol() -> Result<()> {
    let text = payload();
    let mut wire = Vec::new();
    Request::Write(text.clone()).write_to(&mut wire)?;
    Response::Text(text.clone()).write_to(&mut wire)?;

    let mut reader = PipeLike::over(wire);

    assert_eq!(
        Request::Write(text.clone()),
        Request::read_from(&mut reader)?
    );
    assert_eq!(Response::Text(text), Response::read_from(&mut reader)?);

    Ok(())
}

#[cfg(unix)]
#[test]
fn a_megabyte_round_trips_over_the_kind_of_socket_the_helper_is_spoken_to_on() -> Result<()> {
    let (mut editor, mut helper) = UnixStream::pair()?;
    let text = payload();
    let echoed = text.clone();
    let serving = thread::spawn(move || -> Result<()> {
        let Request::Write(text) = Request::read_from(&mut helper)? else {
            return Err(anyhow!(
                "the helper was asked for something it was not sent"
            ));
        };
        Response::Text(text).write_to(&mut helper)?;

        Ok(())
    });

    Request::Write(echoed).write_to(&mut editor)?;
    let answer = Response::read_from(&mut editor)?;
    serving
        .join()
        .map_err(|_| anyhow!("the serving thread panicked"))??;

    assert_eq!(Response::Text(text), answer);

    Ok(())
}

#[test]
fn a_megabyte_is_read_in_a_handful_of_reads_rather_than_one_per_byte() -> Result<()> {
    let mut wire = Vec::new();
    Response::Text(payload()).write_to(&mut wire)?;
    let bytes = wire.len();
    let mut reader = PipeLike::over(wire);

    read_frame(&mut reader)?;

    assert!(
        READ_BUDGET * 1000 < bytes,
        "a budget of {READ_BUDGET} reads for {bytes} bytes is not far enough below one per byte \
         to catch a reader that takes them one at a time"
    );
    assert!(
        reader.reads <= READ_BUDGET,
        "{bytes} bytes took {} reads, over the budget of {READ_BUDGET}",
        reader.reads
    );

    Ok(())
}

#[test]
fn a_megabyte_is_read_inside_a_bound_a_byte_at_a_time_reader_could_not_meet() -> Result<()> {
    let text = payload();
    let mut wire = Vec::new();
    Response::Text(text.clone()).write_to(&mut wire)?;
    let bytes = wire.len();

    let byte_at_a_time = READ_COST * u32::try_from(bytes)?;
    assert!(
        BOUND < byte_at_a_time,
        "a reader taking {bytes} bytes one at a time would spend {byte_at_a_time:?}, which \
         already meets the bound of {BOUND:?}, so the bound proves nothing"
    );

    let mut reader = PipeLike::charging(wire, READ_COST);
    let started = Instant::now();
    let answer = Response::read_from(&mut reader)?;
    let spent = started.elapsed();

    assert_eq!(Response::Text(text), answer);
    assert!(
        spent < BOUND,
        "reading {bytes} bytes took {spent:?}, over the bound of {BOUND:?}"
    );

    Ok(())
}

#[test]
fn the_four_answers_to_a_read_are_four_outcomes_rather_than_three_and_a_guess() -> Result<()> {
    let answers = [
        Response::Text("what was copied".to_owned()),
        Response::Empty,
        Response::NonText,
        Response::Failed("the clipboard is held by another process".to_owned()),
    ];

    for answer in &answers {
        let mut wire = Vec::new();
        answer.write_to(&mut wire)?;

        assert_eq!(*answer, Response::read_from(&mut wire.as_slice())?);
    }

    for (index, answer) in answers.iter().enumerate() {
        for other in &answers[index + 1..] {
            assert_ne!(answer, other);
            assert_ne!(answered(answer)?, answered(other)?);
        }
    }

    assert_ne!(Response::Empty, Response::Text(String::new()));
    assert_ne!(
        answered(&Response::Empty)?,
        answered(&Response::Text(String::new()))?
    );

    Ok(())
}

#[test]
fn the_wire_is_a_length_a_tag_and_a_body() -> Result<()> {
    assert_eq!(vec![0, 0, 0, 1, READ_TAG], asked(&Request::Read)?);
    assert_eq!(
        vec![0, 0, 0, 3, WRITE_TAG, b'h', b'i'],
        asked(&Request::Write("hi".to_owned()))?
    );
    assert_eq!(
        vec![0, 0, 0, 3, TEXT_STATUS, b'h', b'i'],
        answered(&Response::Text("hi".to_owned()))?
    );
    assert_eq!(vec![0, 0, 0, 1, EMPTY_STATUS], answered(&Response::Empty)?);
    assert_eq!(
        vec![0, 0, 0, 1, NON_TEXT_STATUS],
        answered(&Response::NonText)?
    );
    assert_eq!(
        vec![0, 0, 0, 2, FAILED_STATUS, b'!'],
        answered(&Response::Failed("!".to_owned()))?
    );
    assert_eq!(
        vec![0, 0, 0, 1, STORED_STATUS],
        answered(&Response::Stored)?
    );

    Ok(())
}

#[test]
fn a_truncated_frame_is_an_error_rather_than_a_short_string() -> Result<()> {
    let mut wire = Vec::new();
    Response::Text(payload()).write_to(&mut wire)?;

    for kept in [0, 1, 3, 4, 5, wire.len() / 2, wire.len() - 1] {
        let answer = Response::read_from(&mut &wire[..kept]);

        assert!(
            matches!(answer, Err(Error::Truncated { .. })),
            "{kept} of {} bytes of a frame came back as {answer:?}",
            wire.len()
        );
    }

    Ok(())
}

#[test]
fn a_malformed_frame_is_an_error_rather_than_a_guess() {
    assert_eq!(Err(Error::NoTag), Request::decode(&[]));
    assert_eq!(Err(Error::NoTag), Response::decode(&[]));
    assert_eq!(
        Err(Error::UnknownRequest { tag: UNSPOKEN }),
        Request::decode(&[UNSPOKEN])
    );
    assert_eq!(
        Err(Error::UnknownStatus { status: UNSPOKEN }),
        Response::decode(&[UNSPOKEN])
    );
    assert_eq!(
        Err(Error::UnexpectedBody {
            tag: EMPTY_STATUS,
            length: 3
        }),
        Response::decode(&[EMPTY_STATUS, b'a', b'b', b'c'])
    );
    assert_eq!(
        Err(Error::UnexpectedBody {
            tag: READ_TAG,
            length: 1
        }),
        Request::decode(&[READ_TAG, b'a'])
    );
    assert_eq!(Err(Error::NotUtf8), Response::decode(&[TEXT_STATUS, 0xff]));
    assert_eq!(Err(Error::NotUtf8), Request::decode(&[WRITE_TAG, 0xff]));
}

#[test]
fn a_length_no_message_could_have_is_an_error_rather_than_an_allocation() {
    let claimed = u32::MAX;
    let mut wire = claimed.to_be_bytes().to_vec();
    wire.push(TEXT_STATUS);

    assert!(MAX_FRAME_SIZE < u64::from(claimed));
    assert_eq!(
        Err(Error::TooLarge {
            length: u64::from(claimed)
        }),
        read_frame(&mut wire.as_slice())
    );
}

#[test]
fn a_payload_that_holds_the_protocols_own_framing_bytes_survives() -> Result<()> {
    let text = String::from_utf8(framing_bytes()?)?;
    let mut wire = Vec::new();
    Request::Write(text.clone()).write_to(&mut wire)?;
    Response::Text(text.clone()).write_to(&mut wire)?;
    Response::Empty.write_to(&mut wire)?;

    let mut reader = PipeLike::over(wire);

    assert_eq!(
        Request::Write(text.clone()),
        Request::read_from(&mut reader)?
    );
    assert_eq!(Response::Text(text), Response::read_from(&mut reader)?);
    assert_eq!(Response::Empty, Response::read_from(&mut reader)?);

    Ok(())
}

/// # Returns
///
/// The bytes of a complete frame, followed by a length prefix claiming megabytes and one of every
/// tag the protocol speaks, on success. Every byte is under `0x80`, so the whole of it is text a
/// clipboard could be holding.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`Response::write_to`]'s return values on failure.
fn framing_bytes() -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    Response::Failed("a frame inside a payload".to_owned()).write_to(&mut bytes)?;
    bytes.extend_from_slice(&[0x00, 0x7f, 0x7f, 0x7f]);
    bytes.extend_from_slice(&[
        READ_TAG,
        WRITE_TAG,
        NON_TEXT_STATUS,
        FAILED_STATUS,
        STORED_STATUS,
    ]);

    Ok(bytes)
}

/// # Returns
///
/// The text the size checks carry, which is [`PAYLOAD_SIZE`] bytes of the mixture a yanked
/// terminal holds: ASCII, wide characters, and the newlines between them.
fn payload() -> String {
    let line = "the quick brown fox jumps over the lazy dog -- 敏捷的棕色狐狸跳过懒狗\n";
    let mut text = String::with_capacity(PAYLOAD_SIZE + line.len());
    while text.len() < PAYLOAD_SIZE {
        text.push_str(line);
    }

    text
}

/// # Returns
///
/// The bytes a request goes onto the wire as, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`Request::write_to`]'s return values on failure.
fn asked(request: &Request) -> Result<Vec<u8>> {
    let mut wire = Vec::new();
    request.write_to(&mut wire)?;

    Ok(wire)
}

/// # Returns
///
/// The bytes a response goes onto the wire as, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`Response::write_to`]'s return values on failure.
fn answered(response: &Response) -> Result<Vec<u8>> {
    let mut wire = Vec::new();
    response.write_to(&mut wire)?;

    Ok(wire)
}
