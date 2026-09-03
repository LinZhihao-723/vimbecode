//! Checks what a yank puts on the clipboard and what a paste gets back off it.
//!
//! The write path is one encoding and one program, and both halves of it are checked here because
//! neither half is safe on its own. The bytes that leave this side are checked against a stand-in
//! writer that keeps everything it is handed, which is a question about this process and has an
//! answer on any machine; the text those bytes come back as is checked against `Get-Clipboard`,
//! which is a question about Windows and can only be asked where there is a Windows to ask. The
//! second half is skipped, loudly, where there is not.
//!
//! The corpus is what earlier fidelity work did not cover. That work stopped at seventy-eight
//! bytes and spelled every accented character precomposed, so a code path that truncates, that
//! deadlocks on a full pipe, or that mangles a combining mark would have passed it. What is asked
//! here is ASCII, CJK, emoji, ZWJ sequences, decomposed NFD forms and stacked combining marks, each
//! at a kilobyte, a hundred kilobytes and a megabyte.
//!
//! Nothing here claims the round trip is byte-exact, because it is not. `clip.exe` rewrites line
//! endings on the way in and there is no asking it not to, so what is asserted is what actually
//! happens in each direction: LF goes out and CRLF comes back, and the text a paste inserts is the
//! text a yank sent only because the read normalizes. The one thing that cannot be made lossless is
//! a carriage return, and it has a test of its own rather than a note in a comment.

#![cfg(target_os = "linux")]

use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{self, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};

use anyhow::{anyhow, bail, Result};
use vbc_editor::clipboard::clip::{self, Clip, Error};
use vbc_editor::clipboard::helper::{Helper, Launch};
use vbc_editor::clipboard::protocol::Response;

/// The shell the stand-in writer is run by, and the stand-in itself.
const SHELL: &str = "/bin/sh";
const CLIP_STUB: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/clipboard/clip_stub.sh");

/// The script that asks Windows what is on the clipboard.
const ORACLE_SCRIPT: &str = include_str!("clipboard/oracle.ps1");

/// The stand-in clipboard helper, which is what a read is served by where there is no Windows.
const HELPER_STUB: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/clipboard/stub.sh");

/// The programs the real write path and the real oracle are.
const CLIP: &str = "clip.exe";
const POWERSHELL: &str = "powershell.exe";
const PATH_TRANSLATOR: &str = "wslpath";

/// The text the oracle is probed with, to tell a machine with no Windows from a Windows whose
/// clipboard cannot be opened. The probe asks whether there is a clipboard at all and not whether
/// it is faithful, so that a fidelity this machine does not have makes the tests red rather than
/// making them skip.
const SENTINEL: &str = "vimbecode clipboard probe";

/// Plain ASCII, which is the case that survives every encoding and therefore proves nothing on its
/// own.
const ASCII: &str = "The quick brown fox jumps over the lazy dog 0123456789 !\"#$%&'()*+,-./:;<=>?";

/// Han, kana and hangul, which the console code page mangles differently from each other.
const CJK: &str = "中文字符测试 繁體與简体 日本語のテキスト 한국어 텍스트 漢字仮名交じり文";

/// Emoji outside the basic plane, which are surrogate pairs once they are UTF-16.
const EMOJI: &str = "😀🎉🚀🐧🌍🍜🧊🎿🛰🪐";

/// Sequences joined by U+200D, which are several code points the terminal draws as one thing.
const ZWJ: &str = "👩\u{200d}💻 👨\u{200d}👩\u{200d}👧\u{200d}👦 🏳\u{fe0f}\u{200d}🌈 🧑\u{200d}🚀";

/// Decomposed forms, spelled out here rather than typed, because a precomposed source file is
/// exactly the gap this corpus exists to close.
const NFD: &str = "cafe\u{301} nai\u{308}ve re\u{301}sume\u{301} A\u{30a}ngstro\u{308}m \
                   ou\u{302} c\u{327}a";

/// Marks stacked on one base, which is where a per-character encoder that is really a per-grapheme
/// encoder falls over.
const COMBINING: &str = "q\u{323}\u{307} e\u{304}\u{301} o\u{31b}\u{323} \
                         a\u{300}\u{301}\u{302}\u{303} \u{5d0}\u{5b8}\u{5bc}";

/// The corpus, named so a failure says which sample it was.
const SAMPLES: [(&str, &str); 6] = [
    ("ascii", ASCII),
    ("cjk", CJK),
    ("emoji", EMOJI),
    ("zwj", ZWJ),
    ("nfd", NFD),
    ("combining", COMBINING),
];

/// The sizes each sample is grown to, which are the ones earlier fidelity work never reached.
const SIZES: [(&str, usize); 3] = [("1KB", 1024), ("100KB", 100 * 1024), ("1MB", 1024 * 1024)];

/// The combining marks the decomposed samples are required to still be spelled with.
const MARKS: [char; 5] = ['\u{301}', '\u{308}', '\u{30a}', '\u{323}', '\u{5b8}'];

/// Tells one test's temporary directory from another's.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// The clipboard is one thing on the machine, so the tests that put something on it take turns.
static CLIPBOARD: Mutex<()> = Mutex::new(());

/// A directory a test lays its captures and its scripts down in, taken away with the test.
struct Directory {
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
    fn create() -> Result<Self> {
        let path = env::temp_dir().join(format!(
            "vbc-clipboard-write-{}-{}",
            process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path)?;

        Ok(Self { path })
    }

    /// # Returns
    ///
    /// The path a name inside this directory has.
    fn join(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// What Windows says is on the clipboard.
#[derive(Clone, Debug, Eq, PartialEq)]
enum Held {
    /// The clipboard holds this text.
    Text(String),

    /// The clipboard holds no text.
    Nothing,

    /// The clipboard could not be read, and this is what Windows said about it.
    Unreadable(String),
}

/// `Get-Clipboard`, as a thing a test can ask.
struct Oracle {
    _directory: Directory,
    script: OsString,
    answer: PathBuf,
    answer_for_windows: OsString,
}

impl Oracle {
    /// Opens the oracle, if this machine has a Windows clipboard that answers at all.
    ///
    /// A locked workstation is a Windows whose clipboard cannot be opened for as long as it stays
    /// locked, which is not something a test can do anything about, so it is told apart from a
    /// machine with no Windows and skipped the same way.
    ///
    /// # Returns
    ///
    /// The oracle, on success, or `None` if there is no clipboard to ask.
    ///
    /// # Errors
    ///
    /// Returns an error if the oracle's script could not be laid down.
    fn open() -> Result<Option<Self>> {
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
            return Ok(None);
        };

        let oracle = Self {
            _directory: directory,
            script,
            answer,
            answer_for_windows,
        };

        match Clip::windows().put(SENTINEL) {
            Ok(()) => {}
            Err(Error::Spawn { .. }) => {
                eprintln!("skipped: {CLIP} is not on this machine");
                return Ok(None);
            }
            Err(error) => {
                eprintln!("skipped: {CLIP} is here but would not take a yank: {error}");
                return Ok(None);
            }
        }

        match oracle.read() {
            Ok(Held::Text(text)) if text.contains(SENTINEL) => Ok(Some(oracle)),
            Ok(held) => {
                eprintln!("skipped: this machine's clipboard does not answer: {held:?}");
                Ok(None)
            }
            Err(error) => {
                eprintln!("skipped: this machine's clipboard could not be asked: {error}");
                Ok(None)
            }
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
    fn read(&self) -> Result<Held> {
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
    fn text(&self) -> Result<String> {
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
/// The turn a test takes at the machine's one clipboard, taken back off a thread that panicked
/// holding it because a poisoned lock would turn one failure into every failure.
fn turn() -> MutexGuard<'static, ()> {
    CLIPBOARD
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// # Returns
///
/// The path Windows knows a path of this filesystem by, on success, or `None` if this machine has
/// no Windows to know it.
///
/// # Errors
///
/// Returns an error if the translation ran and said nothing.
fn windows_path(path: &Path) -> Result<Option<OsString>> {
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
/// The sample repeated until it is at least this many bytes of UTF-8, with a space between the
/// repeats so the joins are not silently a different character.
fn grown(sample: &str, size: usize) -> String {
    let mut grown = String::with_capacity(size + sample.len());
    while grown.len() < size {
        grown.push_str(sample);
        grown.push(' ');
    }

    grown
}

/// # Returns
///
/// The text a sequence of UTF-16LE bytes spells, on success, decoded without going anywhere near
/// the encoder under test.
///
/// # Errors
///
/// Returns an error if the bytes are an odd number of them, or do not spell text.
fn decoded(bytes: &[u8]) -> Result<String> {
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

/// # Returns
///
/// A write path that is the stand-in writer, keeping what it is handed in a file.
fn stand_in(capture: &Path) -> Clip {
    Clip::of(SHELL.into(), vec![CLIP_STUB.into(), capture.into()])
}

/// Hands bytes to the real writer without encoding them, which is how the encoding the write path
/// does not use is put to Windows.
///
/// # Errors
///
/// Returns an error if the writer could not be run, or refused the bytes.
fn put_raw(bytes: &[u8]) -> Result<()> {
    let mut child = Command::new(CLIP)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let mut input = child
        .stdin
        .take()
        .ok_or_else(|| anyhow!("{CLIP} was started without its input pipe"))?;
    input.write_all(bytes)?;
    drop(input);

    let status = child.wait()?;
    if !status.success() {
        bail!("{CLIP} refused the bytes: {status}");
    }

    Ok(())
}

/// # Returns
///
/// What the writer's line ending rewrite does to a text, which is every LF and every lone CR
/// becoming a CRLF.
fn rewritten(text: &str) -> String {
    let mut rewritten = String::with_capacity(text.len());
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '\r' => {
                if Some(&'\n') == characters.peek() {
                    characters.next();
                }
                rewritten.push_str("\r\n");
            }
            '\n' => rewritten.push_str("\r\n"),
            character => rewritten.push(character),
        }
    }

    rewritten
}

/// The encoding is UTF-16, little end first, and nothing sits in front of it.
///
/// A byte order mark would be text once it is on the clipboard rather than a mark on it, so a yank
/// of "hello" that arrives as "\u{feff}hello" is a yank that pastes a stray character into the
/// buffer.
#[test]
fn the_encoding_is_utf16le_and_carries_no_byte_order_mark() -> Result<()> {
    assert_eq!(clip::encode("A"), vec![0x41, 0x00]);
    assert_eq!(clip::encode("中"), vec![0x2d, 0x4e]);
    assert_eq!(clip::encode("😀"), vec![0x3d, 0xd8, 0x00, 0xde]);
    assert_eq!(clip::encode("\u{301}"), vec![0x01, 0x03]);
    assert_eq!(clip::encode(""), Vec::<u8>::new());

    for (name, sample) in SAMPLES {
        let encoded = clip::encode(sample);
        assert!(
            !encoded.starts_with(&[0xff, 0xfe]) && !encoded.starts_with(&[0xfe, 0xff]),
            "{name} was encoded behind a byte order mark"
        );
        assert_eq!(
            encoded.len(),
            2 * sample.encode_utf16().count(),
            "{name} was encoded as something other than whole UTF-16 units"
        );
    }

    Ok(())
}

/// The corpus is the one this milestone said it would be: decomposed, not precomposed.
///
/// A source file whose accents are typed rather than spelled out drifts back to precomposed the
/// first time someone reformats it, and the fidelity tests then pass without ever putting a
/// combining mark on the clipboard. This is what stops that being silent.
#[test]
fn the_decomposed_samples_are_still_decomposed() -> Result<()> {
    for (name, sample) in [("nfd", NFD), ("combining", COMBINING)] {
        let marks = sample.chars().filter(|character| MARKS.contains(character));
        assert!(
            2 <= marks.count(),
            "{name} has lost the combining marks it exists to carry"
        );
    }

    assert!(
        !NFD.contains('\u{e9}') && !NFD.contains('\u{fc}') && !NFD.contains('\u{e5}'),
        "the nfd sample has been respelled with precomposed characters"
    );

    Ok(())
}

/// Every sample at every size comes back out of the encoding as itself.
///
/// This is the half of fidelity that does not need Windows, and it is the half earlier work never
/// reached: seventy-eight bytes is under one pipe buffer and inside every plausible off-by-one, so
/// a truncation at sixty-four kilobytes would have passed it.
#[test]
fn every_sample_survives_the_encoding_at_every_size() -> Result<()> {
    for (sample_name, sample) in SAMPLES {
        for (size_name, size) in SIZES {
            let text = grown(sample, size);
            assert!(
                size <= text.len(),
                "{sample_name} at {size_name} was not grown"
            );
            assert_eq!(
                decoded(&clip::encode(&text))?,
                text,
                "{sample_name} at {size_name} did not survive the encoding"
            );
        }
    }

    Ok(())
}

/// What the writer is handed is the encoding and nothing else, at every size.
///
/// The stand-in keeps every byte, so this is the real spawn, the real pipe and the real megabyte,
/// with only the clipboard at the far end replaced. A write that stopped at the pipe buffer, or
/// that deadlocked on it, fails here on any machine.
#[test]
fn the_writer_is_handed_utf16le_and_nothing_else() -> Result<()> {
    let directory = Directory::create()?;
    let capture = directory.join("capture");
    let clip = stand_in(&capture);

    for (sample_name, sample) in SAMPLES {
        for (size_name, size) in SIZES {
            let text = grown(sample, size);
            clip.put(&text)?;

            let handed = fs::read(&capture)?;
            assert_eq!(
                handed.len(),
                2 * text.encode_utf16().count(),
                "{sample_name} at {size_name} reached the writer as the wrong number of bytes"
            );
            assert_eq!(
                decoded(&handed)?,
                text,
                "{sample_name} at {size_name} did not reach the writer intact"
            );
        }
    }

    Ok(())
}

/// A writer that is not there, and a writer that says no, are both errors rather than a silent
/// yank into nothing.
#[test]
fn a_writer_that_does_not_take_the_text_says_so() -> Result<()> {
    let missing = Clip::of("vbc-there-is-no-such-clipboard-writer".into(), Vec::new());
    assert!(
        matches!(missing.put("anything"), Err(Error::Spawn { .. })),
        "a missing writer was not reported"
    );

    let directory = Directory::create()?;
    let capture = directory.join("capture");
    let refusing = Clip::of(
        SHELL.into(),
        vec![CLIP_STUB.into(), capture.into(), "refuse".into()],
    );
    assert!(
        matches!(refusing.put("anything"), Err(Error::Refused { .. })),
        "a refusing writer was not reported"
    );

    Ok(())
}

/// Normalization makes every line ending a single LF, whichever of the three it arrived as.
#[test]
fn normalization_makes_every_line_ending_an_lf() -> Result<()> {
    assert_eq!(clip::normalize("first\r\nsecond"), "first\nsecond");
    assert_eq!(clip::normalize("first\rsecond"), "first\nsecond");
    assert_eq!(clip::normalize("first\nsecond"), "first\nsecond");
    assert_eq!(clip::normalize("\r\n\r\n"), "\n\n");
    assert_eq!(clip::normalize("first\r\r\nsecond"), "first\n\nsecond");
    assert_eq!(clip::normalize("first\n\rsecond"), "first\n\nsecond");
    assert_eq!(clip::normalize("trailing\r\n"), "trailing\n");
    assert_eq!(clip::normalize(""), "");
    assert_eq!(clip::normalize("中\r\n文"), "中\n文");

    let once = clip::normalize("a\r\nb\rc\nd");
    assert_eq!(
        clip::normalize(&once),
        once,
        "normalization is not idempotent"
    );

    Ok(())
}

/// The rewrite the writer does is exactly what normalization undoes, for text a buffer holds.
///
/// A buffer's line endings are LF, so this is the round trip the editor actually makes, modelled
/// on this side of the pipe so that it is checked on machines with no clipboard as well.
#[test]
fn what_the_writer_rewrites_normalization_undoes() -> Result<()> {
    for (name, sample) in SAMPLES {
        let text = format!("{sample}\n{sample}\n\n{sample}\n");
        let rewritten = rewritten(&text);
        assert!(
            rewritten.contains("\r\n"),
            "{name} was modelled without a rewrite to undo"
        );
        assert_eq!(
            clip::normalize(&rewritten),
            text,
            "{name} did not come back through the rewrite"
        );
    }

    Ok(())
}

/// A carriage return does not survive the round trip, and this is what it becomes instead.
///
/// This one cannot be fixed. `clip.exe` rewrites a lone CR to CRLF before anything of ours sees it,
/// so a CR that was a character in a line is a line ending by the time it is read back, and a CR
/// that preceded an LF is gone. Normalization is what makes the common case whole, not what makes
/// this one whole, and the limitation is pinned here rather than described in a comment.
#[test]
fn a_carriage_return_is_the_known_limitation() -> Result<()> {
    assert_eq!(clip::normalize(&rewritten("a\rb")), "a\nb");
    assert_eq!(clip::normalize(&rewritten("a\r\nb")), "a\nb");
    assert_eq!(clip::normalize(&rewritten("a\r\r\nb")), "a\n\nb");

    let carriage_return = "one\rtwo";
    assert_ne!(
        clip::normalize(&rewritten(carriage_return)),
        carriage_return,
        "a carriage return has stopped being lossy, which wants a better test than this one"
    );

    Ok(())
}

/// A read hands back a buffer's line endings, not the clipboard's.
///
/// The helper is the stand-in one, because what is being checked is that the read normalizes at
/// all, and a clipboard that cannot be opened would leave that unchecked.
#[test]
fn a_read_hands_back_line_endings_a_buffer_can_hold() -> Result<()> {
    let launch = Launch::of(SHELL.into(), vec![HELPER_STUB.into()]).with_environment(
        "VBC_STUB_TEXT".into(),
        "first\r\nsecond\rthird\nfourth".into(),
    );
    let mut helper = Helper::launch(launch)?;

    assert_eq!(
        helper.read()?,
        Response::Text("first\nsecond\nthird\nfourth".to_owned()),
        "the read handed back the clipboard's line endings"
    );

    Ok(())
}

/// Every sample at every size goes onto the real clipboard and comes back off it as itself.
///
/// This is the validation the whole module is for, and it is the one that needs a Windows: the
/// encoding could be right and the round trip still lose characters, because what happens between
/// `clip.exe` and `Get-Clipboard` is Windows' business and not ours.
#[test]
fn the_round_trip_is_faithful_for_every_sample_at_every_size() -> Result<()> {
    let _turn = turn();
    let Some(oracle) = Oracle::open()? else {
        return Ok(());
    };

    for (sample_name, sample) in SAMPLES {
        for (size_name, size) in SIZES {
            let text = grown(sample, size);
            Clip::windows().put(&text)?;
            assert_eq!(
                clip::normalize(&oracle.text()?),
                text,
                "{sample_name} at {size_name} did not survive the round trip"
            );
        }
    }

    Ok(())
}

/// UTF-8 is not an encoding this write path may be simplified into.
///
/// The trap is that it looks like it works. `clip.exe` decodes its input in the console's code
/// page, and every code page agrees with UTF-8 about ASCII, so an ASCII yank round trips perfectly
/// and nothing at all is wrong until the first character that is not ASCII. That is what this
/// pins: ASCII survives, and everything else does not.
#[test]
fn raw_utf8_is_destroyed_by_the_console_code_page() -> Result<()> {
    let _turn = turn();
    let Some(oracle) = Oracle::open()? else {
        return Ok(());
    };

    put_raw(ASCII.as_bytes())?;
    assert_eq!(
        clip::normalize(&oracle.text()?),
        ASCII,
        "ASCII stopped surviving raw UTF-8, so this test no longer says what it is for"
    );

    for (name, sample) in [("cjk", CJK), ("emoji", EMOJI), ("nfd", NFD)] {
        put_raw(sample.as_bytes())?;
        let held = clip::normalize(&oracle.text()?);
        assert_ne!(
            held, sample,
            "{name} survived raw UTF-8, which the console code page should have made impossible"
        );

        Clip::windows().put(sample)?;
        assert_eq!(
            clip::normalize(&oracle.text()?),
            sample,
            "{name} did not survive UTF-16LE, which is what raw UTF-8 is rejected in favour of"
        );
    }

    Ok(())
}

/// What goes out and what comes back, in the writer's own words.
///
/// An LF leaves and a CRLF arrives; a lone CR leaves and a CRLF arrives as well. Neither is
/// something this side chose, and both are asserted against the real clipboard rather than assumed
/// from the model the other tests use.
#[test]
fn line_endings_go_out_as_lf_and_come_back_as_crlf() -> Result<()> {
    let _turn = turn();
    let Some(oracle) = Oracle::open()? else {
        return Ok(());
    };

    for (name, sent, expected) in [
        ("lf", "first\nsecond", "first\r\nsecond"),
        ("lone cr", "first\rsecond", "first\r\nsecond"),
        ("crlf", "first\r\nsecond", "first\r\nsecond"),
        ("blank line", "first\n\nsecond", "first\r\n\r\nsecond"),
        ("trailing lf", "first\n", "first\r\n"),
    ] {
        Clip::windows().put(sent)?;
        let held = oracle.text()?;
        assert_eq!(
            held, expected,
            "{name} did not reach the clipboard the way this test says it does"
        );
        assert_eq!(
            clip::normalize(&held),
            clip::normalize(sent),
            "{name} did not come back as line endings a buffer can hold"
        );
    }

    Ok(())
}
