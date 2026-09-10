//! Whether the directory a session runs in may run its own code, decided before anything runs.
//!
//! Headless Claude Code has no workspace-trust dialog. In a directory it has never seen it neither
//! asks nor refuses: it reads the project's `CLAUDE.md`, runs the project's `SessionStart` hook and
//! starts whatever the project's `.mcp.json` declares, records nothing about having done so, and
//! says nothing about it on the protocol. Pointing vimbecode at a repository somebody else wrote
//! would therefore run that repository's code on the first keystroke, and there would be no frame,
//! no file and no prompt to say that it had.
//!
//! So the gate is here rather than in the binary, and it is a gate rather than a check. What a
//! directory is admitted as is settled before [`std::process::Command::spawn`], because a spawn
//! that was aborted still fired the hook and still started the MCP server: there is nothing left to
//! undo by the time a failure is in hand. That is why an [`Admission`] is the only thing a spawn
//! takes trust from, why nothing but a [`Gate`] can mint one, and why a spawn nobody gated runs
//! restricted.
//!
//! Restricted is one flag. `--setting-sources user` suppresses the project's memory, the project's
//! settings hooks and the project's MCP servers together, and leaves the reader's own settings --
//! their model, their permissions, their own hooks -- exactly where they were. A session in a
//! directory the reader has not trusted is a working session; it is only not the project's.
//!
//! Trust is keyed on the root of the working copy holding the directory, and on the resolved
//! directory itself where no working copy holds it. That is deliberately not the key a transcript
//! is stored under, which is the path as given: two directories of one repository are two
//! histories and one decision, and a reader who trusted a repository has trusted the whole of it.
//!
//! The record is the reader's own `~/.claude.json`, which a live Claude Code session writes too.
//! It is read, changed and written whole onto a file of its own that is renamed over the record in
//! one step, so a reader of it sees the whole of one version or the whole of another and never
//! half of either -- and the version being replaced is kept beside it first. Nothing locks it, so
//! two writers that read the same version do write it back one after the other and the second
//! carries the first away; what that costs is a grant that has to be made again, and it cannot
//! cost more, because a version written back from an older read holds what its own reader had
//! already said.
//!
//! One thing about the record is not its content. It holds the reader's account, and on this
//! machine it is theirs to read and nobody else's; a replacement is a file this process created,
//! so it is created under this process's umask and would hand a `0600` record back at `0644`. The
//! permissions therefore travel with the content, and a record written where there was none is
//! narrowed to its owner rather than left to the umask.

use std::env;
use std::fs::{self, File};
use std::io::{self, Write};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Map, Value};

use super::error::Error;

/// The flag that runs a session on the reader's configuration and none of the project's. It takes
/// the project's memory, the project's settings hooks and the project's MCP servers out together,
/// so there is no second flag to forget.
pub const SETTING_SOURCES_FLAG: &str = "--setting-sources";
pub const SETTING_SOURCES_USER: &str = "user";

/// What the root of a working copy holds, which is a directory where the working copy is the
/// repository and a file where it is linked to one kept elsewhere.
pub const REPOSITORY_MARKER: &str = ".git";

/// The record trust is kept in, under the reader's home, and the names a directory's trust is
/// written under inside it.
pub const RECORD: &str = ".claude.json";
pub const PROJECTS: &str = "projects";
pub const ACCEPTED: &str = "hasTrustDialogAccepted";

/// What the version a grant replaces is kept as, and what a version being written is called until
/// it is whole. Neither is the name Claude Code's own backup goes under: a backup that wrote over
/// the one a live session took would be a copy of the accident rather than of what came before it.
const KEPT: &str = "vimbecode.backup";
const PARTIAL: &str = "vimbecode.partial";

/// How many records this process has begun writing, which is what tells one half-written file from
/// another. Two grants at once -- two vimbecodes, or two threads of one -- must not be writing the
/// same one.
static WRITING: AtomicU64 = AtomicU64::new(0);

/// The path a directory's trust is recorded under.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Key(String);

impl Key {
    /// # Returns
    ///
    /// The key `directory` is trusted under -- the root of the working copy holding it, and the
    /// directory itself where none does -- with every link on the way there resolved, on success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * [`Error::Trust`] if the directory does not resolve, or resolves to a path a JSON record
    ///   cannot name it by.
    pub fn of(directory: &Path) -> Result<Self, Error> {
        let here = resolved(directory)?;
        let root = here
            .ancestors()
            .find(|ancestor| ancestor.join(REPOSITORY_MARKER).exists())
            .unwrap_or(here.as_path());

        named(root)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn as_path(&self) -> &Path {
        Path::new(&self.0)
    }
}

/// What a directory is admitted as, which is what the reader has said about it and not what the
/// directory holds.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Standing {
    /// The reader has not trusted the directory, so the session runs on their configuration and
    /// none of the project's. It is the default because a directory nobody was asked about is a
    /// directory nobody trusted.
    #[default]
    Restricted,

    /// The reader has trusted the directory, so the project's memory, hooks and MCP servers are
    /// the session's as well.
    Trusted,
}

impl Standing {
    /// # Returns
    ///
    /// The arguments that hold a spawn to this standing, which are none where the directory is
    /// trusted.
    #[must_use]
    pub fn arguments(self) -> Vec<String> {
        match self {
            Self::Trusted => Vec::new(),
            Self::Restricted => vec![
                SETTING_SOURCES_FLAG.to_owned(),
                SETTING_SOURCES_USER.to_owned(),
            ],
        }
    }
}

/// What the reader said when they were asked whether a directory may run its own code.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Answer {
    /// Nothing, or no. It is the default because it is the answer a question that was shown and
    /// not answered has to count as.
    #[default]
    Withheld,

    /// Yes, and record it.
    Granted,
}

/// A directory, the key it is trusted under and what it is admitted as. It is the only thing a
/// spawn takes trust from, and a [`Gate`] is the only thing that mints one.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Admission {
    directory: PathBuf,
    key: Key,
    standing: Standing,
}

impl Admission {
    /// # Returns
    ///
    /// The directory the admission was taken for, as it was given rather than as it resolves.
    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    #[must_use]
    pub fn key(&self) -> &Key {
        &self.key
    }

    #[must_use]
    pub fn standing(&self) -> Standing {
        self.standing
    }

    /// # Returns
    ///
    /// Whether the reader is still owed the question, which they are for every directory they have
    /// not already trusted.
    #[must_use]
    pub fn asks(&self) -> bool {
        Standing::Restricted == self.standing
    }
}

/// The record a reader's trust is kept in, and the decisions read out of it.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Gate {
    record: PathBuf,
}

impl Gate {
    /// # Returns
    ///
    /// The gate over the reader's own record, on success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * [`Error::Trust`] if this process has no home directory to find the record under.
    pub fn of_reader() -> Result<Self, Error> {
        let home = env::home_dir().ok_or(Error::Trust {
            path: RECORD.to_owned(),
            reason: "this process has no home directory to keep it under".to_owned(),
        })?;

        Ok(Self::of_record(home.join(RECORD)))
    }

    /// # Returns
    ///
    /// The gate over a record kept somewhere other than the reader's home.
    #[must_use]
    pub fn of_record(record: impl AsRef<Path>) -> Self {
        Self {
            record: record.as_ref().to_owned(),
        }
    }

    #[must_use]
    pub fn record(&self) -> &Path {
        &self.record
    }

    /// Decides what a directory may be run as, which is decided before anything is run in it.
    ///
    /// # Returns
    ///
    /// The admission, on success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * [`Error::Trust`] if the record could not be read or holds anything but an object.
    /// * Forwards [`Key::of`]'s return values on failure.
    pub fn admit(&self, directory: &Path) -> Result<Admission, Error> {
        let key = Key::of(directory)?;
        let standing = if self.accepted(&key)? {
            Standing::Trusted
        } else {
            Standing::Restricted
        };

        Ok(Admission {
            directory: directory.to_owned(),
            key,
            standing,
        })
    }

    /// Takes what the reader answered about a directory they were asked about.
    ///
    /// # Returns
    ///
    /// What the answer leaves: an admission that is trusted and recorded where the reader granted
    /// it, and the admission it was given where they did not, which is what a question nobody
    /// answered leaves as well.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * Forwards [`Gate::grant`]'s return values on failure.
    pub fn answered(&self, admission: &Admission, answer: Answer) -> Result<Admission, Error> {
        match answer {
            Answer::Granted => self.grant(admission),
            Answer::Withheld => Ok(admission.clone()),
        }
    }

    /// Records that the reader has trusted the directory an admission was taken for.
    ///
    /// # Returns
    ///
    /// The admission, trusted, on success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * [`Error::Trust`] if the record could not be read, holds anything but an object or an
    ///   object where it keeps projects, or could not be written.
    pub fn grant(&self, admission: &Admission) -> Result<Admission, Error> {
        let held = self.held()?;
        let mut record = match &held {
            Some(text) => self.parsed(text)?,
            None => Map::new(),
        };

        let Some(projects) = record
            .entry(PROJECTS)
            .or_insert_with(|| Value::Object(Map::new()))
            .as_object_mut()
        else {
            return Err(self.unusable("what it keeps projects under is not an object"));
        };
        let Some(project) = projects
            .entry(admission.key.as_str())
            .or_insert_with(|| Value::Object(Map::new()))
            .as_object_mut()
        else {
            return Err(self.unusable("what it keeps this project under is not an object"));
        };
        project.insert(ACCEPTED.to_owned(), Value::Bool(true));

        let written = serde_json::to_string_pretty(&Value::Object(record))
            .map_err(|error| self.unusable(&error.to_string()))?;
        if let Some(text) = &held {
            self.write(&self.beside(KEPT), text)?;
        }
        self.write(&self.record, &format!("{written}\n"))?;

        Ok(Admission {
            directory: admission.directory.clone(),
            key: admission.key.clone(),
            standing: Standing::Trusted,
        })
    }

    /// # Returns
    ///
    /// Whether the record says the reader has trusted `key`, on success. A record that does not
    /// exist yet has trusted nothing.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * [`Error::Trust`] if the record could not be read or holds anything but an object.
    fn accepted(&self, key: &Key) -> Result<bool, Error> {
        let Some(text) = self.held()? else {
            return Ok(false);
        };

        Ok(self
            .parsed(&text)?
            .get(PROJECTS)
            .and_then(Value::as_object)
            .and_then(|projects| projects.get(key.as_str()))
            .and_then(Value::as_object)
            .and_then(|project| project.get(ACCEPTED))
            .and_then(Value::as_bool)
            .unwrap_or(false))
    }

    /// # Returns
    ///
    /// What the record holds, or nothing where there is no record yet or nothing in it, on
    /// success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * [`Error::Trust`] if the record exists and could not be read.
    fn held(&self) -> Result<Option<String>, Error> {
        let text = match fs::read_to_string(&self.record) {
            Ok(text) => text,
            Err(error) if io::ErrorKind::NotFound == error.kind() => return Ok(None),
            Err(error) => return Err(self.unusable(&error.to_string())),
        };

        if text.trim().is_empty() {
            return Ok(None);
        }

        Ok(Some(text))
    }

    /// # Returns
    ///
    /// What a record's text holds, on success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * [`Error::Trust`] if the text is not a JSON object.
    fn parsed(&self, text: &str) -> Result<Map<String, Value>, Error> {
        match serde_json::from_str(text) {
            Ok(Value::Object(record)) => Ok(record),
            Ok(_other) => Err(self.unusable("it holds something other than an object")),
            Err(error) => Err(self.unusable(&error.to_string())),
        }
    }

    /// Writes a file whole: onto a file of its own beside the record, flushed to the disk, given
    /// the record's own permissions, and renamed over its destination in one step. A record
    /// written over in place would be a record that is briefly neither version, and a live Claude
    /// Code session reads this one.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * [`Error::Trust`] if the file could not be written, flushed or renamed into place.
    fn write(&self, path: &Path, text: &str) -> Result<(), Error> {
        let writing = WRITING.fetch_add(1, Ordering::Relaxed);
        let partial = self.beside(&format!("{PARTIAL}.{}.{writing}", process::id()));
        let written = File::create(&partial).and_then(|mut file| {
            file.write_all(text.as_bytes())?;
            file.sync_all()
        });
        if let Err(error) = written {
            let _ignored = fs::remove_file(&partial);
            return Err(self.unusable(&error.to_string()));
        }

        match fs::metadata(&self.record) {
            Ok(held) => {
                let _ignored = fs::set_permissions(&partial, held.permissions());
            }
            Err(_missing) => {
                let _ignored = narrowed(&partial);
            }
        }

        fs::rename(&partial, path).map_err(|error| {
            let _ignored = fs::remove_file(&partial);
            self.unusable(&error.to_string())
        })
    }

    /// # Returns
    ///
    /// A path beside the record, named after it by what it holds.
    fn beside(&self, suffix: &str) -> PathBuf {
        let mut name = self.record.file_name().unwrap_or_default().to_owned();
        name.push(".");
        name.push(suffix);

        self.record.with_file_name(name)
    }

    /// # Returns
    ///
    /// The error a record that cannot be used is reported as.
    fn unusable(&self, reason: &str) -> Error {
        Error::Trust {
            path: self.record.to_string_lossy().into_owned(),
            reason: reason.to_owned(),
        }
    }
}

/// Narrows a file to its owner, which is what a record holding the reader's account is created as
/// where there was no record to take permissions from.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`std::fs::set_permissions`]'s return values on failure.
#[cfg(unix)]
fn narrowed(path: &Path) -> io::Result<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

/// Leaves a file as the platform made it, which is where its permissions are not a mode.
///
/// # Errors
///
/// Returns an error if:
///
/// * Never.
#[cfg(not(unix))]
fn narrowed(_path: &Path) -> io::Result<()> {
    Ok(())
}

/// # Returns
///
/// The directory with every link on the way to it resolved, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`Error::Trust`] if the directory does not resolve.
fn resolved(directory: &Path) -> Result<PathBuf, Error> {
    fs::canonicalize(directory).map_err(|error| Error::Trust {
        path: directory.to_string_lossy().into_owned(),
        reason: error.to_string(),
    })
}

/// # Returns
///
/// The key a path is recorded under, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`Error::Trust`] if the path is not text a JSON record can name it by, which is the one thing
///   a record cannot hold and therefore cannot be trusted through.
fn named(path: &Path) -> Result<Key, Error> {
    let Some(text) = path.to_str() else {
        return Err(Error::Trust {
            path: path.to_string_lossy().into_owned(),
            reason: "the path is not text a record can name it by".to_owned(),
        });
    };

    Ok(Key(text.to_owned()))
}
