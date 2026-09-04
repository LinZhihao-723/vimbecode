//! The seam between the register file and the desktop: `"+` as the system clipboard rather than as
//! a drawer of the editor's own.
//!
//! Everything either side of this module already existed and neither end reached the other. The
//! helper, the protocol, the deadlines and the write path were built and tested, and no keystroke
//! could arrive at any of them; `"+` was a name the keybinding table knew and modalkit threw the
//! writes to it away. What is here is the wire between the two, and it is one register wide:
//! [`super::REGISTER`] is what the desktop's clipboard is, [`super::ALIAS`] is another name for the
//! same one, and every other register is the editor's own business.
//!
//! The two directions are not symmetrical, because vim's are not. A yank into `"+` is mirrored out
//! to the desktop as it happens: it is invisible, it is idempotent, and a reader who yanked
//! expects the yank to have gone somewhere. A put out of `"+` is the other way round -- what
//! another window copied has to be fetched, and fetching it is what can take a second and a half
//! -- so it is asked for when a put asks for it and at no other time. Plain `p` never asks, which
//! is what keeps `p` meaning "the thing I just yanked" rather than "whatever the desktop last
//! held".
//!
//! Neither direction happens on the drawing thread. The read is [`Reader`]'s, held to its soft and
//! hard deadlines, and a read that misses the hard one pastes nothing at all rather than something
//! stale; the write is [`Writer`]'s, handed over and not waited for. What the render loop does is
//! ask how far along things are.
//!
//! The mirror is by content rather than by event. What was last sent to the desktop is kept here,
//! and a yank that leaves `"+` holding what the desktop already holds writes nothing, so a paste
//! fetched from Windows is not sent straight back to it and a register the reader keeps re-yanking
//! the same text into costs one process rather than one per keystroke.

use std::path::PathBuf;

use super::clip::Clip;
use super::helper::{Error, Helper, Launch};
use super::reader::{Outcome, Progress, Reader, Reason, Request, Source, READING_NOTICE};
use super::writer::{Sink, Writer};
use super::{ALIAS, REGISTER};
use crate::engine::{Held, Registers, Shape};

/// What the status line says about a put the desktop had nothing to give.
pub const EMPTY_NOTICE: &str = "the system clipboard is empty";

/// What it says about a put the desktop had something other than text for.
pub const NON_TEXT_NOTICE: &str = "the system clipboard holds no text";

/// What it says about a put the desktop did not answer inside the hard deadline.
pub const ABANDONED_NOTICE: &str = "the system clipboard did not answer, so nothing was pasted";

/// How far along the read a put is waiting on has got, as the render loop sees it on the frame it
/// asks.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Settled {
    /// Nothing has come back, and it is not slow enough to be worth mentioning.
    Waiting,

    /// Nothing has come back and the read is past its soft deadline, so the status line says so.
    Slow(&'static str),

    /// The register holds what the put is to read, and this is what the status line has to say
    /// about it, which is nothing at all where the desktop handed text over.
    Ready(Option<String>),
}

/// The system clipboard as a register: one register file, one worker thread each way, and the read
/// a put is waiting on.
///
/// It belongs to the thread that draws, which is the thread the deadlines are about, and it is
/// handed the register file the engines share rather than keeping one of its own.
#[derive(Debug)]
pub struct Bridge {
    registers: Registers,
    reader: Reader,
    writer: Writer,
    reading: Option<Request>,
    mirrored: Option<String>,
    filled: u64,
}

impl Bridge {
    /// Factory function.
    ///
    /// # Returns
    ///
    /// A newly created bridge over the Windows clipboard: read through the helper that lives as
    /// long as the session, and written by `clip.exe`.
    ///
    /// Neither end is started here. The helper is built on the reader's own worker, because
    /// starting it is what costs, and the writer's program is started once per yank.
    #[must_use]
    pub fn windows() -> Self {
        let directory: PathBuf = std::env::temp_dir();

        Self::served_by(
            move || Helper::launch(Launch::windows_clipboard(&directory)?),
            Clip::windows(),
        )
    }

    /// Factory function.
    ///
    /// # Returns
    ///
    /// A newly created bridge over a clipboard read from `factory`'s source and written to `sink`,
    /// which is how a desktop that is not this machine's is stood in for.
    ///
    /// # Type Parameters
    ///
    /// * `SourceType` - What the clipboard is read from.
    /// * `FactoryType` - What builds it, on the reader's worker thread.
    /// * `SinkType` - Where a yank is put.
    #[must_use]
    pub fn served_by<SourceType, FactoryType, SinkType>(
        factory: FactoryType,
        sink: SinkType,
    ) -> Self
    where
        SourceType: Source,
        FactoryType: FnOnce() -> Result<SourceType, Error> + Send + 'static,
        SinkType: Sink,
    {
        Self {
            registers: Registers::new(),
            reader: Reader::start(factory),
            writer: Writer::start(sink),
            reading: None,
            mirrored: None,
            filled: 0,
        }
    }

    /// # Returns
    ///
    /// This bridge writing into and reading out of `registers` rather than a file of its own, so
    /// that the register a keystroke names is the register the desktop is reached through.
    #[must_use]
    pub fn sharing(mut self, registers: Registers) -> Self {
        self.filled = registers.fills();
        self.registers = registers;

        self
    }

    /// # Returns
    ///
    /// Whether the register named `name` is the one this bridge serves from the desktop.
    #[must_use]
    pub fn serves(name: char) -> bool {
        REGISTER == name || ALIAS == name
    }

    /// Asks the desktop what it holds, joining a read that is already out rather than starting a
    /// second one.
    pub fn read(&mut self) {
        if self.reading.is_none() {
            self.reading = Some(self.reader.request());
        }
    }

    /// # Returns
    ///
    /// How far along the read a put is waiting on has got, and [`Settled::Ready`] where none was
    /// ever asked for. A read that is over leaves the register holding what the put is to read,
    /// which is what the desktop handed over, or nothing at all where it handed nothing over or
    /// took too long about it.
    pub fn settled(&mut self) -> Settled {
        let Some(request) = self.reading else {
            return Settled::Ready(None);
        };

        let outcome = match self.reader.poll(request) {
            Progress::Waiting => return Settled::Waiting,
            Progress::Slow => return Settled::Slow(READING_NOTICE),
            Progress::Ready(outcome) => outcome,
        };
        self.reading = None;

        let (text, notice) = match outcome {
            Outcome::Text(text) => (text, None),
            Outcome::Empty => (String::new(), Some(EMPTY_NOTICE.to_owned())),
            Outcome::NonText => (String::new(), Some(NON_TEXT_NOTICE.to_owned())),
            Outcome::Unavailable(Reason::Abandoned) => {
                (String::new(), Some(ABANDONED_NOTICE.to_owned()))
            }
            Outcome::Unavailable(Reason::Refused(said)) => (String::new(), Some(said)),
        };
        self.fill(text);

        Settled::Ready(notice)
    }

    /// Writes what `"+` holds out to the desktop, where a keystroke has left it holding something
    /// the desktop does not already hold.
    ///
    /// `addressed` is the register the keystroke named, which is what says whether a yank could
    /// have reached the clipboard's own through modalkit; a fill the editor made itself is found
    /// by the register file's own count instead. Neither is read out of the register file unless
    /// one of them says something may have changed, because what a register holds is as large as
    /// what was yanked into it and a keystroke may not cost that.
    pub fn mirror(&mut self, addressed: Option<char>) {
        let filled = self.registers.fills();
        let touched = filled != self.filled || addressed.is_some_and(Self::serves);
        self.filled = filled;
        if !touched {
            return;
        }

        let held = self.registers.get(REGISTER).map(|held| held.text);
        let Some(text) = held else {
            self.mirrored = Some(String::new());

            return;
        };
        if self.mirrored.as_ref() == Some(&text) {
            return;
        }
        self.mirrored = Some(text.clone());
        self.writer.write(text);
    }

    /// # Returns
    ///
    /// What a yank the desktop refused said about it, and [`None`] where every yank so far was
    /// taken or is still on its way.
    pub fn refusal(&mut self) -> Option<String> {
        self.writer.refusal()
    }

    /// # Returns
    ///
    /// The number of times the desktop has been asked what it holds, which is what says a put that
    /// never asked never asked.
    #[must_use]
    pub fn reads_issued(&self) -> u64 {
        self.reader.reads_issued()
    }

    /// # Returns
    ///
    /// The number of yanks that have been handed to the desktop.
    #[must_use]
    pub fn writes_issued(&self) -> u64 {
        self.writer.writes_issued()
    }

    /// Stops the worker threads and waits for them, taking the helper and any yank still on its way
    /// down with them.
    ///
    /// A read already under way cannot be taken back, so this belongs to a session that is ending
    /// rather than to a frame. What it buys is that the helper is asked to exit by the process that
    /// started it: a session that let its worker be torn down by the exit instead leaves a
    /// PowerShell behind.
    pub fn shutdown(&mut self) {
        self.reader.shutdown();
        self.writer.shutdown();
    }

    /// Writes `text` into `"+` as what the desktop holds, so that the put waiting on it reads the
    /// desktop's text rather than whatever the register held before.
    ///
    /// Text ending in a line break is whole lines, which is what vim puts one line above another
    /// rather than into the middle of one. The text is recorded as mirrored, because it came from
    /// the desktop and sending it back would be a process spent putting the clipboard where it
    /// already is.
    fn fill(&mut self, text: String) {
        let shape = if text.ends_with('\n') {
            Shape::Linewise
        } else {
            Shape::Charwise
        };
        self.registers.fill(
            REGISTER,
            &Held {
                text: text.clone(),
                shape,
            },
        );
        self.filled = self.registers.fills();
        self.mirrored = Some(text);
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        self.shutdown();
    }
}
