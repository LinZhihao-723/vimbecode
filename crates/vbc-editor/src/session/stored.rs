//! What a resumed session already said, read back from the transcript Claude Code stored it in.
//!
//! The stream protocol replays none of a resumed session's history, so what was said before it
//! was resumed is read from disk instead: Claude Code writes every session to
//! `projects/<key>/<session id>.jsonl` under its configuration directory, one JSON entry per line,
//! and this module finds that file and reads it into the same [`Conversation`] live frames are
//! read into. A stored reply and a live one become the same blocks by going through the same code.
//!
//! The transcript is disclaimed as internal and changes between releases, so it is read the way a
//! stranger's file is: a line that does not decode, an entry of a type this does not know and a
//! field that is missing are all passed over rather than reported.

use std::env;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader};
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

use super::blocks::{Conversation, Reason};
use super::error::Error;
use super::event::{Event, Kind};
use super::identity::SessionId;

/// The variable naming Claude Code's configuration directory, and the directory under the reader's
/// home it uses where the variable is not set.
pub const CONFIG_DIR: &str = "CLAUDE_CONFIG_DIR";
pub const CONFIG_HOME: &str = ".claude";

/// The directory under the configuration directory that holds one directory per project, and the
/// extension every transcript in those is written with.
pub const PROJECTS: &str = "projects";
pub const EXTENSION: &str = "jsonl";

/// The longest a project key is written before it is cut short and suffixed with a hash of the
/// whole path, which is what keeps it a name a filesystem accepts.
pub const KEY_LENGTH: usize = 200;

/// What a project key holds in place of every UTF-16 unit that is not an ASCII letter or digit.
const REPLACEMENT: char = '-';

/// The multiplier of the string hash a long key is suffixed with, and the radix it is written in.
const HASH_MULTIPLIER: i32 = 31;
const RADIX: u32 = 36;

/// The characters a session identifier is written with. Anything else could name a file outside a
/// project's directory.
const IDENTIFIER_SEPARATORS: [char; 2] = ['-', '_'];

/// The fields of an entry this reads: the directory the session was working in, the marks of an
/// entry that is the client writing to its own history or a subagent working apart from the
/// conversation, and the source only a prompt carries.
const DIRECTORY_FIELD: &str = "cwd";
const META_FIELD: &str = "isMeta";
const SUMMARY_FIELD: &str = "isCompactSummary";
const SIDECHAIN_FIELD: &str = "isSidechain";
const SOURCE_FIELD: &str = "promptSource";

/// The source of a prompt nobody typed, which is what a background task reporting back is.
const SYSTEM_SOURCE: &str = "system";

/// The fields a message is read from, and the content item holding text.
const MESSAGE_FIELD: &str = "message";
const CONTENT_FIELD: &str = "content";
const ITEM_FIELD: &str = "type";
const TEXT_ITEM: &str = "text";
const TEXT_SEPARATOR: &str = "\n";

/// The subtype of the entry a compaction leaves where it replaced the history.
const BOUNDARY_SUBTYPE: &str = "compact_boundary";

/// The tags a slash command is stored inside, the character it is typed after, and the two
/// commands that break a history.
const COMMAND_OPEN: &str = "<command-name>";
const COMMAND_CLOSE: &str = "</command-name>";
const ARGUMENTS_OPEN: &str = "<command-args>";
const ARGUMENTS_CLOSE: &str = "</command-args>";
const COMMAND_PREFIX: char = '/';
const CLEAR_COMMAND: &str = "clear";
const COMPACT_COMMAND: &str = "compact";

/// Where Claude Code keeps the transcript of every session on this machine, one directory per
/// project.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    /// # Returns
    ///
    /// The store the reader's own Claude Code writes to, under the directory [`CONFIG_DIR`] names
    /// or under [`CONFIG_HOME`] in their home where it names none, on success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * [`Error::Stored`] if [`CONFIG_DIR`] is not set and this process has no home directory.
    pub fn of_reader() -> Result<Self, Error> {
        if let Some(configured) = env::var_os(CONFIG_DIR).filter(|named| !named.is_empty()) {
            return Ok(Self::at(PathBuf::from(configured).join(PROJECTS)));
        }
        let home = env::home_dir().ok_or(Error::Stored {
            path: CONFIG_HOME.to_owned(),
            reason: "this process has no home directory to find it under".to_owned(),
        })?;

        Ok(Self::at(home.join(CONFIG_HOME).join(PROJECTS)))
    }

    /// # Returns
    ///
    /// The store whose project directories are the directories in `root`.
    #[must_use]
    pub fn at(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_owned(),
        }
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Finds the transcript of the session `id`, looking first in the project `near` is kept
    /// under and then in every other one.
    ///
    /// # Returns
    ///
    /// The transcript on success: the one in `near`'s project where that holds one, and otherwise
    /// the one written to last.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * [`Error::Unknown`] if `id` is not written the way a session identifier is, or no project
    ///   holds a transcript of it.
    /// * [`Error::Stored`] if the directory the projects are kept in exists and could not be read.
    pub fn find(&self, id: &SessionId, near: &Path) -> Result<Stored, Error> {
        let unknown = || Error::Unknown {
            id: id.to_string(),
            searched: self.root.display().to_string(),
        };
        if !identifies(id.as_str()) {
            return Err(unknown());
        }

        let name = format!("{id}.{EXTENSION}");
        let near = fs::canonicalize(near).unwrap_or_else(|_| near.to_owned());
        let nearest = self.root.join(key(&near)).join(&name);
        if written(&nearest).is_some() {
            return Ok(Stored::at(nearest));
        }

        let projects = match fs::read_dir(&self.root) {
            Ok(projects) => projects,
            Err(error) if io::ErrorKind::NotFound == error.kind() => return Err(unknown()),
            Err(error) => {
                return Err(Error::Stored {
                    path: self.root.display().to_string(),
                    reason: error.to_string(),
                })
            }
        };
        let mut latest: Option<(SystemTime, PathBuf)> = None;
        for project in projects.flatten() {
            let candidate = project.path().join(&name);
            let Some(modified) = written(&candidate) else {
                continue;
            };
            if latest.as_ref().is_none_or(|(newest, _)| modified > *newest) {
                latest = Some((modified, candidate));
            }
        }

        latest.map(|(_, path)| Stored::at(path)).ok_or_else(unknown)
    }
}

/// One session's transcript, as Claude Code wrote it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Stored {
    path: PathBuf,
}

impl Stored {
    /// # Returns
    ///
    /// The transcript kept at `path`.
    #[must_use]
    pub fn at(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_owned(),
        }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Reads the directory the session was started in, which is the directory Claude Code finds
    /// it from again.
    ///
    /// # Returns
    ///
    /// The directory the first entry naming one names, or [`None`] where no entry names one, on
    /// success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * Forwards [`Stored::each`]'s return values on failure.
    pub fn directory(&self) -> Result<Option<PathBuf>, Error> {
        let mut directory = None;
        self.each(|line| {
            let named = serde_json::from_str::<Value>(line)
                .ok()
                .and_then(|entry| entry.get(DIRECTORY_FIELD)?.as_str().map(PathBuf::from));
            match named {
                Some(named) => {
                    directory = Some(named);
                    ControlFlow::Break(())
                }
                None => ControlFlow::Continue(()),
            }
        })?;

        Ok(directory)
    }

    /// Reads everything the session said into the blocks it was drawn as while it was said.
    ///
    /// # Returns
    ///
    /// The conversation on success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * Forwards [`Stored::each`]'s return values on failure.
    pub fn read(&self) -> Result<Conversation, Error> {
        let mut replay = Replay::default();
        self.each(|line| {
            if let Ok(event) = Event::decoded(line) {
                replay.entry(&event);
            }
            ControlFlow::Continue(())
        })?;

        Ok(replay.finished())
    }

    /// Hands `visit` every line of the transcript that is text, in order, until it says to stop.
    ///
    /// # Type Parameters
    ///
    /// * `VisitorType` - What each line is handed to, and what says whether to read on.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * [`Error::Stored`] if the transcript could not be opened or read.
    fn each<VisitorType: FnMut(&str) -> ControlFlow<()>>(
        &self,
        mut visit: VisitorType,
    ) -> Result<(), Error> {
        let file = File::open(&self.path).map_err(|error| self.unreadable(&error))?;
        let mut reader = BufReader::new(file);
        let mut line = Vec::new();
        loop {
            line.clear();
            let read = reader
                .read_until(b'\n', &mut line)
                .map_err(|error| self.unreadable(&error))?;
            if 0 == read {
                return Ok(());
            }
            let Ok(text) = std::str::from_utf8(&line) else {
                continue;
            };
            if visit(text.trim_end()).is_break() {
                return Ok(());
            }
        }
    }

    /// # Returns
    ///
    /// A newly created [`Error::Stored`] over a failure to read the transcript.
    fn unreadable(&self, error: &io::Error) -> Error {
        Error::Stored {
            path: self.path.display().to_string(),
            reason: error.to_string(),
        }
    }
}

/// # Returns
///
/// The name Claude Code gives the directory it keeps the transcripts of sessions started in
/// `directory` under: every UTF-16 unit of the path that is not an ASCII letter or digit written
/// as [`REPLACEMENT`], and a path longer than [`KEY_LENGTH`] cut there and suffixed with a hash of
/// the whole of it.
#[must_use]
pub fn key(directory: &Path) -> String {
    let path = directory.to_string_lossy();
    let key: String = path
        .encode_utf16()
        .map(|unit| match u8::try_from(unit) {
            Ok(byte) if byte.is_ascii_alphanumeric() => char::from(byte),
            _ => REPLACEMENT,
        })
        .collect();
    if KEY_LENGTH >= key.len() {
        return key;
    }

    format!(
        "{}{REPLACEMENT}{}",
        &key[..KEY_LENGTH],
        radix(hashed(&path).unsigned_abs())
    )
}

/// A transcript part way through being read: what it has said so far, and whether a compaction
/// has left a break that has not been put anywhere yet.
#[derive(Debug, Default)]
struct Replay {
    conversation: Conversation,
    compacted: bool,
}

impl Replay {
    /// Reads one entry of the transcript into the conversation.
    fn entry(&mut self, event: &Event) {
        if flagged(event.raw(), SIDECHAIN_FIELD) {
            return;
        }

        match event.kind() {
            Kind::User => self.user(event),
            Kind::Assistant => {
                self.settle();
                self.conversation.read(event);
            }
            Kind::System(subtype) if BOUNDARY_SUBTYPE == subtype.as_str() => {
                self.settle();
                self.compacted = true;
            }
            Kind::Init(_) | Kind::Turn(_) | Kind::Control | Kind::System(_) | Kind::Other(_) => {}
        }
    }

    /// Reads one user entry: a prompt the reader typed, a slash command they ran, or what the
    /// tools answered.
    ///
    /// A compaction is stored ahead of the `/compact` that asked for it, and its break is put
    /// after that command, which is where the live stream puts it.
    fn user(&mut self, event: &Event) {
        let raw = event.raw();
        if flagged(raw, META_FIELD) || flagged(raw, SUMMARY_FIELD) {
            return;
        }
        if let Some(prompt) = prompted(raw) {
            self.settle();
            self.conversation.asked(&prompt);
            return;
        }
        let Some(command) = Command::read(raw) else {
            self.settle();
            self.conversation.read(event);
            return;
        };

        if COMPACT_COMMAND != command.name {
            self.settle();
        }
        self.conversation.asked(&command.typed());
        self.settle();
        if CLEAR_COMMAND == command.name {
            self.conversation.broke(Reason::Cleared);
        }
    }

    /// Puts the break a compaction left where the transcript now ends, if one is waiting.
    fn settle(&mut self) {
        if std::mem::take(&mut self.compacted) {
            self.conversation.broke(Reason::Compacted);
        }
    }

    /// # Returns
    ///
    /// The conversation the whole transcript was read into.
    fn finished(mut self) -> Conversation {
        self.settle();
        self.conversation
    }
}

/// A slash command as the transcript stores it: its name, without the slash, and what it was given.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Command {
    name: String,
    arguments: String,
}

impl Command {
    /// # Returns
    ///
    /// The command a user entry stores, or [`None`] where the entry stores none.
    fn read(raw: &Value) -> Option<Self> {
        let content = raw.get(MESSAGE_FIELD)?.get(CONTENT_FIELD)?.as_str()?;
        if !content.trim_start().starts_with(COMMAND_OPEN) {
            return None;
        }
        let name = between(content, COMMAND_OPEN, COMMAND_CLOSE)?
            .trim()
            .trim_start_matches(COMMAND_PREFIX);

        Some(Self {
            name: name.to_owned(),
            arguments: between(content, ARGUMENTS_OPEN, ARGUMENTS_CLOSE)
                .unwrap_or_default()
                .trim()
                .to_owned(),
        })
    }

    /// # Returns
    ///
    /// The command as it was typed.
    fn typed(&self) -> String {
        if self.arguments.is_empty() {
            return format!("{COMMAND_PREFIX}{}", self.name);
        }

        format!("{COMMAND_PREFIX}{} {}", self.name, self.arguments)
    }
}

/// # Returns
///
/// Whether `id` is written only with the characters a session identifier is.
fn identifies(id: &str) -> bool {
    !id.is_empty()
        && id.chars().all(|written| {
            written.is_ascii_alphanumeric() || IDENTIFIER_SEPARATORS.contains(&written)
        })
}

/// # Returns
///
/// When the file at `path` was last written, or [`None`] where there is no such file.
fn written(path: &Path) -> Option<SystemTime> {
    let metadata = fs::metadata(path).ok().filter(|found| !found.is_dir())?;

    Some(metadata.modified().unwrap_or(UNIX_EPOCH))
}

/// # Returns
///
/// Whether an entry carries `field` as `true`.
fn flagged(raw: &Value, field: &str) -> bool {
    raw.get(field).and_then(Value::as_bool).unwrap_or_default()
}

/// # Returns
///
/// What the reader typed, where the entry is a prompt they typed, and [`None`] where it is
/// anything else: a tool's answer, a command, or something the client wrote to its own history.
fn prompted(raw: &Value) -> Option<String> {
    let source = raw.get(SOURCE_FIELD)?.as_str()?;
    if SYSTEM_SOURCE == source {
        return None;
    }

    let content = raw.get(MESSAGE_FIELD)?.get(CONTENT_FIELD)?;
    if let Some(typed) = content.as_str() {
        return Some(typed.to_owned());
    }
    let typed: Vec<&str> = content
        .as_array()?
        .iter()
        .filter(|item| Some(TEXT_ITEM) == item.get(ITEM_FIELD).and_then(Value::as_str))
        .filter_map(|item| item.get(TEXT_ITEM)?.as_str())
        .collect();
    if typed.is_empty() {
        return None;
    }

    Some(typed.join(TEXT_SEPARATOR))
}

/// # Returns
///
/// What `text` holds between the first `open` and the `close` after it, or [`None`] where it holds
/// no such pair.
fn between<'text>(text: &'text str, open: &str, close: &str) -> Option<&'text str> {
    let start = text.find(open)? + open.len();
    let length = text[start..].find(close)?;

    Some(&text[start..start + length])
}

/// # Returns
///
/// The 32-bit string hash of `path` over its UTF-16 units, wrapping as it overflows.
fn hashed(path: &str) -> i32 {
    path.encode_utf16().fold(0, |hash: i32, unit| {
        hash.wrapping_mul(HASH_MULTIPLIER)
            .wrapping_add(i32::from(unit))
    })
}

/// # Returns
///
/// `number` written in base [`RADIX`], in lowercase.
///
/// # Panics
///
/// Panics if a remainder of [`RADIX`] is not a digit of it, which it always is.
fn radix(mut number: u32) -> String {
    let mut digits = Vec::new();
    loop {
        digits.push(char::from_digit(number % RADIX, RADIX).expect("a remainder is a digit"));
        number /= RADIX;
        if 0 == number {
            break;
        }
    }

    digits.iter().rev().collect()
}
