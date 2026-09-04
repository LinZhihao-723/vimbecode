//! The machine's one clipboard, as the tests that reach it share it.
//!
//! Two things here are shared rather than written twice. The oracle is `Get-Clipboard` and nothing
//! else, and it is what says whether a yank reached Windows; a second copy of it would be a second
//! definition of what "reached Windows" means. And the turn is the clipboard itself, which is one
//! thing on the station: every test that puts something on it has to have it to itself, or a
//! passing suite is a suite whose tests overwrote each other's fixtures and read back somebody
//! else's.
//!
//! The oracle skips where there is no Windows and fails where there is one that will not answer,
//! because those two are not the same result and only one of them is nobody's fault.

#![allow(dead_code)]

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::process::{self, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};
use std::{env, thread};

use anyhow::{anyhow, bail, Result};

/// The script that asks Windows what is on the clipboard.
const ORACLE_SCRIPT: &str = include_str!("oracle.ps1");

/// The shell the stand-in programs are run by, the stand-in writer, and the stand-in helper. Both
/// stand-ins are real processes speaking to the real code over real pipes, with only the desktop at
/// the far end of them replaced.
pub const SHELL: &str = "/bin/sh";
pub const CLIP_STUB: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/clipboard/clip_stub.sh");
pub const HELPER_STUB: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/clipboard/stub.sh");

/// The programs the real write path and the real oracle are.
pub const CLIP: &str = "clip.exe";
pub const POWERSHELL: &str = "powershell.exe";
pub const PATH_TRANSLATOR: &str = "wslpath";

/// The text the oracle is probed with, which is ASCII and holds no line ending, so that what the
/// probe asks is whether this machine has a clipboard at all rather than whether it is faithful.
/// A fidelity this machine does not have makes the tests red; only the absence of a Windows makes
/// them skip.
const SENTINEL: &str = "vimbecode clipboard probe";

/// How long a Windows that is here but will not open its clipboard is waited out for, and how long
/// is left between asking again.
///
/// A clipboard some other process is holding is a thing that clears on its own, and one held open
/// with a null window denies every process on the station for as long as it lasts. Waiting is what
/// can be done about the first. What must not be done about either is passing: a machine with a
/// Windows that will not answer has not checked the round trip, and says so.
const PROBE_BUDGET: Duration = Duration::from_secs(30);
const PROBE_GAP: Duration = Duration::from_secs(1);

/// The file one test binary holds the machine's clipboard by, how long another waits for it, and
/// how often it looks.
const TURN_FILE: &str = "vimbecode-clipboard-turn.lock";
const TURN_BUDGET: Duration = Duration::from_secs(60);
const TURN_GAP: Duration = Duration::from_millis(25);

/// One test's turn at the machine's clipboard, given back when it is dropped.
pub struct Turn {
    _held: MutexGuard<'static, ()>,
    path: PathBuf,
}

impl Drop for Turn {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Tells one test's temporary directory from another's.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// The clipboard is one thing on the machine, so the tests that put something on it take turns.
static CLIPBOARD: Mutex<()> = Mutex::new(());

/// A directory a test lays its captures and its scripts down in, taken away with the test.
pub struct Directory {
    path: PathBuf,
}

impl Directory {
    /// # Returns
    ///
    /// A directory of this test's own, on success.
    ///
    /// # Errors
    ///
    /// Returns an error if the directory could not be created.
    pub fn create() -> Result<Self> {
        let path = env::temp_dir().join(format!(
            "vbc-clipboard-{}-{}",
            process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path)?;

        Ok(Self { path })
    }

    /// # Returns
    ///
    /// The path a name inside this directory has.
    pub fn join(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// What handing bytes straight to the real writer did, with the reason kept apart from the answer
/// so that a writer which is not here is told from a writer which would not take them.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Written {
    /// The writer took the bytes.
    Taken,

    /// There is no writer on this machine, which is what a machine with no Windows looks like.
    NoWriter,

    /// There is one, and this is what it did instead of taking them.
    Refused(String),
}

/// What asking this machine for a clipboard came back with.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Probed {
    /// The sentinel went onto the clipboard and came back off it.
    Answered,

    /// There is no Windows here to ask, and this is how that was found out.
    NoWindows(String),

    /// There is a Windows here, and this is what stopped the sentinel making the round trip.
    Trouble(String),
}

/// What Windows says is on the clipboard.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Held {
    /// The clipboard holds this text.
    Text(String),

    /// The clipboard holds no text.
    Nothing,

    /// The clipboard could not be read, and this is what Windows said about it.
    Unreadable(String),
}

/// `Get-Clipboard`, as a thing a test can ask.
pub struct Oracle {
    _directory: Directory,
    script: OsString,
    answer: PathBuf,
    answer_for_windows: OsString,
}

impl Oracle {
    /// Opens the oracle, on a machine that has a Windows to open one against.
    ///
    /// There are two things this can find, and only one of them is a skip. A machine with no
    /// Windows cannot be asked what a Windows clipboard does, and skips. A machine that has one
    /// which will not answer has not asked either, and that is a failure rather than a skip: the
    /// clipboard on this station can be held open by another process for as long as an hour, and
    /// a suite that goes quiet for that hour reports a round trip it never made.
    ///
    /// # Returns
    ///
    /// The oracle, on success, or `None` where there is no Windows on this machine.
    ///
    /// # Errors
    ///
    /// Returns an error if the oracle's script could not be laid down, or if this machine has a
    /// Windows whose clipboard would not answer within [`PROBE_BUDGET`].
    pub fn open() -> Result<Option<Self>> {
        let directory = Directory::create()?;
        let script = directory.join("oracle.ps1");
        fs::write(&script, ORACLE_SCRIPT)?;

        let Some(script) = windows_path(&script)? else {
            eprintln!(
                "skipped: {PATH_TRANSLATOR} is not on this machine, so Windows is not either"
            );
            return Ok(None);
        };
        let answer = directory.join("answer.txt");
        let Some(answer_for_windows) = windows_path(&answer)? else {
            eprintln!("skipped: {PATH_TRANSLATOR} named no Windows path, so Windows is not here");
            return Ok(None);
        };

        let oracle = Self {
            _directory: directory,
            script,
            answer,
            answer_for_windows,
        };

        let deadline = Instant::now() + PROBE_BUDGET;
        let trouble = loop {
            match oracle.probe() {
                Probed::Answered => return Ok(Some(oracle)),
                Probed::NoWindows(said) => {
                    eprintln!("skipped: {said}");
                    return Ok(None);
                }
                Probed::Trouble(said) if deadline <= Instant::now() => break said,
                Probed::Trouble(_) => thread::sleep(PROBE_GAP),
            }
        };

        bail!(
            "this machine has a {CLIP} and a clipboard that would not answer it for \
             {PROBE_BUDGET:?}: {trouble}. These tests skip where there is no Windows to ask and \
             fail where there is one that will not answer, because a suite that passes without \
             having asked is worth less than one that says it could not"
        )
    }

    /// Asks whether this machine has a clipboard that takes text and hands it back.
    ///
    /// The sentinel is encoded here rather than by the write path, because a gate that runs
    /// through the code it guards is a gate that code can switch off: an encoder that emits
    /// big-endian units fails this probe, and every test the probe stands in front of then passes
    /// by never running.
    ///
    /// # Returns
    ///
    /// What the machine came back with.
    pub fn probe(&self) -> Probed {
        match hand_to_writer(&utf16le(SENTINEL)) {
            Written::Taken => {}
            Written::NoWriter => {
                return Probed::NoWindows(format!("{CLIP} is not on this machine"));
            }
            Written::Refused(said) => {
                return Probed::Trouble(format!("{CLIP} would not take the probe: {said}"));
            }
        }

        match self.read() {
            Ok(Held::Text(text)) if SENTINEL == text => Probed::Answered,
            Ok(held) => Probed::Trouble(format!("the clipboard answered with {held:?}")),
            Err(error) => Probed::Trouble(format!("the clipboard could not be asked: {error}")),
        }
    }

    /// Asks Windows what is on the clipboard.
    ///
    /// # Returns
    ///
    /// What it said, on success.
    ///
    /// # Errors
    ///
    /// Returns an error if the oracle could not be run, or answered in a way it does not have an
    /// answer for.
    pub fn read(&self) -> Result<Held> {
        let _ = fs::remove_file(&self.answer);
        let run = Command::new(POWERSHELL)
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
            ])
            .arg("-File")
            .arg(&self.script)
            .arg("-Out")
            .arg(&self.answer_for_windows)
            .stdin(Stdio::null())
            .output()?;

        match run.status.code() {
            Some(0) => Ok(Held::Text(String::from_utf8(fs::read(&self.answer)?)?)),
            Some(2) => {
                let said = fs::read(&self.answer)?;
                Ok(Held::Unreadable(
                    String::from_utf8_lossy(&said).into_owned(),
                ))
            }
            Some(3) => Ok(Held::Nothing),
            code => Err(anyhow!("the oracle exited with {code:?}")),
        }
    }

    /// # Returns
    ///
    /// The text the clipboard holds, on success.
    ///
    /// # Errors
    ///
    /// Returns an error if the clipboard holds anything else.
    ///
    /// Forwards [`Oracle::read`]'s return values on failure.
    pub fn text(&self) -> Result<String> {
        match self.read()? {
            Held::Text(text) => Ok(text),
            held => Err(anyhow!(
                "the clipboard was expected to hold text, and held {held:?}"
            )),
        }
    }
}

/// # Returns
///
/// The turn a test takes at the machine's one clipboard, held until it is dropped.
///
/// There are two ways to lose that turn and this takes both back. Two threads of one test binary
/// are held apart by a lock of the process, taken back off a thread that panicked holding it
/// because a poisoned lock would turn one failure into every failure. Two test binaries are
/// separate processes and cargo runs them at once, so they are held apart by a file only one of
/// them can create; a turn nobody gave back inside [`TURN_BUDGET`] is taken anyway, since a suite
/// wedged behind a crashed process is worse than two tests overlapping.
pub fn turn() -> Turn {
    let held = CLIPBOARD
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let path = env::temp_dir().join(TURN_FILE);

    let deadline = Instant::now() + TURN_BUDGET;
    while let Err(error) = File::options().write(true).create_new(true).open(&path) {
        if ErrorKind::AlreadyExists != error.kind() || deadline <= Instant::now() {
            let _ = fs::remove_file(&path);
            break;
        }
        thread::sleep(TURN_GAP);
    }

    Turn { _held: held, path }
}

/// # Returns
///
/// The path Windows knows a path of this filesystem by, on success, or `None` if this machine has
/// no Windows to know it.
///
/// # Errors
///
/// Returns an error if the translation ran and said nothing.
pub fn windows_path(path: &Path) -> Result<Option<OsString>> {
    let translated = match Command::new(PATH_TRANSLATOR).arg("-w").arg(path).output() {
        Ok(translated) => translated,
        Err(_) => return Ok(None),
    };
    if !translated.status.success() {
        return Ok(None);
    }

    let windows = String::from_utf8_lossy(&translated.stdout)
        .trim_end()
        .to_owned();
    if windows.is_empty() {
        bail!("{PATH_TRANSLATOR} named no Windows path for {path:?}");
    }

    Ok(Some(windows.into()))
}

/// # Returns
///
/// The UTF-16LE bytes a text is spelled by, written out here rather than taken from the write
/// path, so that the probe standing in front of the real-clipboard tests shares no code with what
/// those tests are checking.
pub fn utf16le(text: &str) -> Vec<u8> {
    text.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

/// Hands bytes to the real writer without encoding them, which is how an encoding the write path
/// does not use is put to Windows, and how the probe reaches the clipboard without it.
///
/// # Returns
///
/// What the writer did with them.
pub fn hand_to_writer(bytes: &[u8]) -> Written {
    let started = Command::new(CLIP)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let mut child = match started {
        Ok(child) => child,
        Err(error) if ErrorKind::NotFound == error.kind() => return Written::NoWriter,
        Err(error) => return Written::Refused(error.to_string()),
    };

    let Some(mut input) = child.stdin.take() else {
        let _ = child.kill();
        let _ = child.wait();

        return Written::Refused(format!("{CLIP} was started without its input pipe"));
    };
    let written = input.write_all(bytes);
    drop(input);

    match child.wait() {
        Ok(status) if !status.success() => Written::Refused(format!("{CLIP} exited {status}")),
        Ok(_) => match written {
            Ok(()) => Written::Taken,
            Err(error) => Written::Refused(error.to_string()),
        },
        Err(error) => Written::Refused(error.to_string()),
    }
}

/// Hands bytes to the real writer, on a machine the probe has already found a writer on.
///
/// # Errors
///
/// Returns an error if the writer has gone, or did not take the bytes.
pub fn put_raw(bytes: &[u8]) -> Result<()> {
    match hand_to_writer(bytes) {
        Written::Taken => Ok(()),
        Written::NoWriter => bail!("{CLIP} went missing between the probe and the test"),
        Written::Refused(said) => bail!("{CLIP} did not take the bytes: {said}"),
    }
}

/// # Returns
///
/// The text a sequence of UTF-16LE bytes spells, on success, decoded without going anywhere near
/// the encoder under test.
///
/// # Errors
///
/// Returns an error if the bytes are an odd number of them, or do not spell text.
pub fn decoded(bytes: &[u8]) -> Result<String> {
    if !bytes.len().is_multiple_of(2) {
        bail!(
            "{} bytes is not a whole number of UTF-16 units",
            bytes.len()
        );
    }

    let units = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair));

    char::decode_utf16(units)
        .collect::<Result<String, _>>()
        .map_err(|error| anyhow!("the bytes are not UTF-16: {error}"))
}
