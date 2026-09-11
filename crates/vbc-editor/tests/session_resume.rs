//! A resumed session's history: found where Claude Code stored it, read into the blocks its live
//! frames became, and put in the panel ahead of anything the session says after it.
//!
//! The claim everything else rests on is that replayed equals live. One real session was recorded
//! twice over at once -- the frames it wrote on its stream, and the transcripts Claude Code kept of
//! it -- through a prompt, a fenced reply, a tool call, `/compact` and `/clear`, and the two are
//! required to become the same blocks, tags and breaks. The live half is read the way the program
//! reads it, off a child writing the recorded frames over real pipes. `/clear` starts a transcript
//! of its own, so what was said before it is compared with the first transcript and what was said
//! from it on with the second.
//!
//! The recordings are claude 2.1.269's, as they were written, with these exceptions: the frames and
//! entries that carry the recording machine's own environment -- hook output, attachments, the
//! MCP and skill catalogs of the init frame -- are left out; thinking signatures are emptied; the
//! compaction summary and the hook's stdout are kept to their first 240 characters; and a sentence
//! of one reply that echoed the recording machine's memory file is replaced, in both halves alike.
//! None of those is anything the replay reads.
//!
//! The last case makes the same claim against a real session and is ignored for the reason
//! `session_live.rs` gives: it costs a model call and needs a login.

#![cfg(target_os = "linux")]

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{anyhow, ensure, Result};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::{Terminal, TerminalOptions, Viewport};
use serde_json::Value;
use tempfile::TempDir;
use vbc_editor::app::App;
use vbc_editor::event::Event as Delivered;
use vbc_editor::session::blocks::{Conversation, Reason};
use vbc_editor::session::client::Client;
use vbc_editor::session::error::Error;
use vbc_editor::session::identity::{Identity, SessionId};
use vbc_editor::session::live::Session;
use vbc_editor::session::spawn::Spawn;
use vbc_editor::session::stored::{key, Store, Stored, EXTENSION};

/// The stand-in the recorded frames are written back out through.
const STUB: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/session/stub.sh");

/// The recorded session's frames, turn by turn, each turn headed by a line naming what the reader
/// asked in it, and the field that line names it in.
const RECORDED: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/session/resumed/live.ndjson"
);
const ASKED_FIELD: &str = "asked";

/// The two transcripts Claude Code kept of the recorded session, one either side of its `/clear`,
/// and the identifier the first is kept under.
const BEFORE_CLEAR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/session/resumed/db8d3c4f-a11c-447d-abda-eec9fa76c9e4.jsonl"
);
const AFTER_CLEAR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/session/resumed/955957eb-0506-4ac7-a303-3ff563281b90.jsonl"
);
const RECORDED_ID: &str = "db8d3c4f-a11c-447d-abda-eec9fa76c9e4";

/// The directory the recorded session was started in, and the name Claude Code kept its
/// transcripts under, as it was read off the machine it was recorded on.
const RECORDED_IN: &str = "/tmp/s3-resume/a.b_c d-é";
const RECORDED_KEY: &str = "-tmp-s3-resume-a-b-c-d--";

/// What the recorded session was asked first, and what it answered.
const FIRST_ASKED: &str = "Reply with the single word PONG and nothing else.";
const FIRST_ANSWER: &str = "PONG";

/// What the recorded session fenced, and what its tool answered.
const FENCED: &str = "fn main() {}";
const PRINTED: &str = "hi";

/// The two commands that break a history.
const COMPACT: &str = "/compact";
const CLEAR: &str = "/clear";

/// How long a turn against a process on this machine is given.
const TURN: Duration = Duration::from_secs(10);

/// How long the application is driven waiting for something to arrive, and how long it is left
/// between two frames while it is.
const PATIENCE: Duration = Duration::from_secs(20);
const TICK: Duration = Duration::from_millis(20);

/// How long the application is driven while its history is withheld, which is long enough for
/// the stand-in to have answered the draft sent through it.
const WITHHELD: Duration = Duration::from_millis(500);

/// How long the history is withheld at most, after which it is written anyway so that a reading
/// that held everything up waiting for it is red rather than stuck.
const GIVEN_UP: Duration = Duration::from_secs(5);

/// The window every case is drawn in.
const COLUMNS: u16 = 120;
const ROWS: u16 = 40;

/// What the reader sends a resumed session, and what the stand-in answers a first turn with.
const DRAFT: &str = "hello again";
const ANSWERED: &str = "turn 1";

/// The model the real case runs on, how long one of its turns is given, and how long its child is
/// given to exit.
const MODEL: &str = "haiku";
const REAL_TURN: Duration = Duration::from_secs(180);
const ENDING: Duration = Duration::from_secs(30);

/// What the real case asks, which is what the recording asked.
const REAL_ASKED: [&str; 7] = [
    FIRST_ASKED,
    "Reply with one fenced rust code block holding exactly: fn main() {} and then one short \
     sentence.",
    "Run exactly this shell command with the Bash tool and then say DONE: printf 'hi\\n'",
    COMPACT,
    "Say the word after and nothing else.",
    CLEAR,
    "Say the word cleared and nothing else.",
];

/// One turn of the recording: what the reader asked, and the frames the session answered with.
struct Turn {
    asked: String,
    said: String,
}

/// The name Claude Code gave three real directories is the name the key gives them: one with a
/// dot, an underscore, a space and a letter outside ASCII; one with a character outside the basic
/// plane, which is two UTF-16 units and so two replacements; and one longer than a key may be.
#[test]
fn the_key_is_the_name_claude_code_gave_real_directories() -> Result<()> {
    let long = format!("/tmp/s3-resume/{}/{}", "k".repeat(120), "l.m_n".repeat(20));

    assert_eq!(RECORDED_KEY, key(Path::new(RECORDED_IN)));
    assert_eq!(
        "-tmp-s3-resume-e----x",
        key(Path::new("/tmp/s3-resume/e \u{1f600} x"))
    );
    assert_eq!(
        format!(
            "-tmp-s3-resume-{}-{}l-m--ejesh9",
            "k".repeat(120),
            "l-m-n".repeat(12)
        ),
        key(Path::new(&long))
    );

    Ok(())
}

/// Validation 3, against the recording: the blocks, tags and breaks the stored transcripts become
/// are the ones the live frames became, `/compact`'s break after the command that asked for it and
/// `/clear`'s at the head of the transcript it began.
#[test]
fn a_stored_transcript_replays_into_the_blocks_its_live_frames_built() -> Result<()> {
    let directory = TempDir::new()?;
    let live = lived(directory.path())?;
    let before = Stored::at(BEFORE_CLEAR).read()?;
    let after = Stored::at(AFTER_CLEAR).read()?;

    replays(&live, &before, &after)?;

    for said in [FIRST_ASKED, FIRST_ANSWER, FENCED, PRINTED] {
        position(&before, said)?;
    }
    let compacted = position(&before, COMPACT)? + 1;
    assert_eq!(vec![(compacted, Reason::Compacted)], breaks(&before));
    assert_eq!(vec![(1, Reason::Cleared)], breaks(&after));

    Ok(())
}

/// A session is found by its identifier from a directory other than its own, and from its own
/// directory the transcript kept there is found ahead of a newer one kept anywhere else.
#[test]
fn a_session_is_found_by_its_identifier_from_any_directory() -> Result<()> {
    let root = TempDir::new()?;
    let started = TempDir::new()?;
    let elsewhere = TempDir::new()?;
    let unrelated = TempDir::new()?;
    let id = SessionId::known(RECORDED_ID);
    let started_in = fs::canonicalize(started.path())?;

    let own = plant(root.path(), &started_in, &id)?;
    fs::File::options()
        .write(true)
        .open(&own)?
        .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1))?;
    let newer = plant(root.path(), &fs::canonicalize(elsewhere.path())?, &id)?;
    let store = Store::at(root.path());

    assert_eq!(own, store.find(&id, started.path())?.path());
    assert_eq!(newer, store.find(&id, unrelated.path())?.path());
    assert_eq!(
        Some(started_in),
        store.find(&id, started.path())?.directory()?
    );

    Ok(())
}

/// An identifier no project holds a transcript of is [`Error::Unknown`], and so is one written to
/// reach a transcript outside the projects, even where there is one there to reach.
#[test]
fn an_identifier_nothing_is_stored_under_is_an_error_naming_it() -> Result<()> {
    let root = TempDir::new()?;
    let started = TempDir::new()?;
    plant(
        root.path(),
        &fs::canonicalize(started.path())?,
        &SessionId::known(RECORDED_ID),
    )?;
    fs::copy(
        BEFORE_CLEAR,
        root.path().join(format!("escaped.{EXTENSION}")),
    )?;
    let store = Store::at(root.path());

    for unknown in ["00000000-0000-4000-8000-000000000000", "../escaped", ""] {
        match store.find(&SessionId::known(unknown), started.path()) {
            Err(Error::Unknown { id, .. }) => assert_eq!(unknown, id),
            other => return Err(anyhow!("`{unknown}` was answered with {other:?}")),
        }
    }

    Ok(())
}

/// Validation 1, through the application: what a resumed session said is in the panel before
/// anything is sent, and what is sent and answered afterwards is drawn after it.
#[test]
fn a_resumed_session_shows_what_it_said_before_anything_it_says_now() -> Result<()> {
    let directory = TempDir::new()?;
    let session = Session::started(&spawn(directory.path()))?.recalled(Stored::at(BEFORE_CLEAR));
    let mut app = App::chat().with_session(session);

    assert!(
        settled(&mut app, |app| line_of(app, FIRST_ANSWER).is_some()),
        "the history never reached the panel: {:?}",
        app.panel().text()
    );
    typing(&mut app, &format!("{DRAFT}\u{1b}:wq\r"));
    assert!(
        settled(&mut app, |app| line_of(app, ANSWERED).is_some()),
        "the answer never reached the panel: {:?}",
        app.panel().text()
    );

    let order = [FIRST_ASKED, FIRST_ANSWER, DRAFT, ANSWERED]
        .map(|said| line_of(&mut app, said).unwrap_or(usize::MAX));
    assert!(
        order.is_sorted() && usize::MAX != order[3],
        "the panel drew the history and what followed it at lines {order:?}: {:?}",
        app.panel().text()
    );

    Ok(())
}

/// Validation 5's property, made deterministic: a history nobody has written yet holds up neither
/// the first frame nor a draft sent while it is being read, and when it does arrive it is drawn
/// ahead of both the draft and the answer the session gave it in the meantime.
///
/// The transcript is a named pipe, so reading it waits until the test writes it. A reading that
/// held the first frame would wait for [`GIVEN_UP`] and then draw the history in it.
#[test]
fn a_history_still_being_read_holds_up_neither_the_first_frame_nor_the_reader() -> Result<()> {
    let directory = TempDir::new()?;
    let pipe = directory.path().join(format!("withheld.{EXTENSION}"));
    ensure!(
        Command::new("mkfifo").arg(&pipe).status()?.success(),
        "mkfifo could not make {pipe:?}"
    );
    let (release, released) = mpsc::channel::<()>();
    let writer = {
        let pipe = pipe.clone();
        thread::spawn(move || -> Result<bool> {
            let gave_up = matches!(
                released.recv_timeout(GIVEN_UP),
                Err(RecvTimeoutError::Timeout)
            );
            let history = fs::read(BEFORE_CLEAR)?;
            fs::File::options()
                .write(true)
                .open(&pipe)?
                .write_all(&history)?;

            Ok(gave_up)
        })
    };

    let started = Instant::now();
    let session = Session::started(&spawn(directory.path()))?.recalled(Stored::at(&pipe));
    let mut app = App::chat().with_session(session);
    drawn(&mut app)?;
    let first = started.elapsed();
    let recalling = app.session().is_some_and(Session::recalling);
    typing(&mut app, &format!("{DRAFT}\u{1b}:wq\r"));
    driven(&mut app, WITHHELD);
    let answered_early = line_of(&mut app, ANSWERED);
    let _ignored = release.send(());
    let arrived = settled(&mut app, |app| line_of(app, ANSWERED).is_some());
    let gave_up = writer
        .join()
        .map_err(|_| anyhow!("the writer panicked"))??;

    assert!(
        recalling && !gave_up,
        "the first frame waited {first:?} for the history"
    );
    assert_eq!(
        None, answered_early,
        "the session's answer was drawn before the history it follows"
    );
    assert!(
        arrived,
        "the history and the answer never reached the panel"
    );
    let order = [FIRST_ANSWER, DRAFT, ANSWERED].map(|said| line_of(&mut app, said));
    assert!(
        order.iter().all(Option::is_some) && order.is_sorted(),
        "the panel drew the history, the draft and the answer at lines {order:?}"
    );

    Ok(())
}

/// Validation 3, against a real session: everything [`REAL_ASKED`] asks, read live as it is said,
/// and the two transcripts Claude Code kept of it, read afterwards, are the same blocks, tags and
/// breaks.
#[test]
#[ignore = "starts a real Claude Code session, so it needs the binary, a login and a network"]
fn a_real_sessions_transcripts_replay_into_the_blocks_it_built_live() -> Result<()> {
    let directory = TempDir::new()?;
    let id = SessionId::generated();
    let spawn = Spawn::new(Identity::Fresh(id.clone()))
        .with_model(MODEL)
        .with_directory(directory.path());
    let mut client = Client::start(&spawn)?;
    let mut live = Conversation::new();
    for asked in REAL_ASKED {
        live.asked(asked);
        live.read_all(&client.turn(asked, REAL_TURN)?);
    }
    client.finish(ENDING)?;
    let cleared = SessionId::known(
        live.session_id()
            .ok_or(anyhow!("the session never announced itself"))?,
    );

    let store = Store::of_reader()?;
    let before = store.find(&id, directory.path())?;
    let after = store.find(&cleared, directory.path())?;
    assert_eq!(
        Some(fs::canonicalize(directory.path())?),
        before.directory()?
    );
    replays(&live, &before.read()?, &after.read()?)?;
    assert_eq!(
        vec![Reason::Compacted, Reason::Cleared],
        breaks(&live)
            .into_iter()
            .map(|(_, reason)| reason)
            .collect::<Vec<Reason>>()
    );

    Ok(())
}

/// Asserts that `before` and `after` replay the conversation `live` was read into, split at its
/// `/clear`.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`position`]'s return values on failure.
fn replays(live: &Conversation, before: &Conversation, after: &Conversation) -> Result<()> {
    let cleared = position(live, CLEAR)?;
    let blocks = live.transcript().blocks();
    let tags = live.tags();
    let spoken = breaks(live);

    assert_eq!(
        &blocks[..cleared],
        before.transcript().blocks(),
        "the history before `/clear` was replayed into other blocks than it was read into live"
    );
    assert_eq!(&tags[..cleared], before.tags());
    assert_eq!(
        spoken
            .iter()
            .filter(|(at, _)| *at <= cleared)
            .copied()
            .collect::<Vec<(usize, Reason)>>(),
        breaks(before)
    );
    assert_eq!(
        &blocks[cleared..],
        after.transcript().blocks(),
        "the history from `/clear` on was replayed into other blocks than it was read into live"
    );
    assert_eq!(&tags[cleared..], after.tags());
    assert_eq!(
        spoken
            .iter()
            .filter(|(at, _)| *at > cleared)
            .map(|(at, reason)| (at - cleared, *reason))
            .collect::<Vec<(usize, Reason)>>(),
        breaks(after)
    );

    Ok(())
}

/// # Returns
///
/// The conversation the recorded frames become, read off a child writing them back out turn by
/// turn with each turn's question put in ahead of it, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`anyhow::Error`] if the recording says anything before its first question.
/// * Forwards [`std::fs::read_to_string`]'s return values on failure.
/// * Forwards [`serde_json::from_str`]'s return values on failure.
/// * Forwards [`std::fs::write`]'s return values on failure.
/// * Forwards [`Client::start`]'s return values on failure.
/// * Forwards [`Client::turn`]'s return values on failure.
/// * Forwards [`Client::finish`]'s return values on failure.
fn lived(directory: &Path) -> Result<Conversation> {
    let mut turns: Vec<Turn> = Vec::new();
    for line in fs::read_to_string(RECORDED)?.lines() {
        let frame: Value = serde_json::from_str(line)?;
        if let Some(asked) = frame.get(ASKED_FIELD).and_then(Value::as_str) {
            turns.push(Turn {
                asked: asked.to_owned(),
                said: String::new(),
            });
            continue;
        }
        let turn = turns.last_mut().ok_or(anyhow!(
            "the recording says something before it asks anything"
        ))?;
        turn.said.push_str(line);
        turn.said.push('\n');
    }
    for (index, turn) in turns.iter().enumerate() {
        fs::write(directory.join(format!("said.{}", index + 1)), &turn.said)?;
    }

    let mut client = Client::start(
        &Spawn::new(Identity::Fresh(SessionId::known(RECORDED_ID)))
            .with_binary(STUB)
            .with_directory(directory),
    )?;
    let mut conversation = Conversation::new();
    for turn in &turns {
        conversation.asked(&turn.asked);
        conversation.read_all(&client.turn(&turn.asked, TURN)?);
    }
    client.finish(TURN)?;

    Ok(conversation)
}

/// Keeps the first recorded transcript in `root` under the project `started_in` names, as the
/// transcript of `id` started in `started_in`.
///
/// # Returns
///
/// Where it was kept, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`std::fs::read_to_string`]'s return values on failure.
/// * Forwards [`std::fs::create_dir_all`]'s return values on failure.
/// * Forwards [`std::fs::write`]'s return values on failure.
fn plant(root: &Path, started_in: &Path, id: &SessionId) -> Result<PathBuf> {
    let transcript =
        fs::read_to_string(BEFORE_CLEAR)?.replace(RECORDED_IN, &started_in.display().to_string());
    let project = root.join(key(started_in));
    fs::create_dir_all(&project)?;
    let kept = project.join(format!("{id}.{EXTENSION}"));
    fs::write(&kept, transcript)?;

    Ok(kept)
}

/// # Returns
///
/// The index of the first block of `conversation` whose source is `said`, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`anyhow::Error`] if no block's source is `said`.
fn position(conversation: &Conversation, said: &str) -> Result<usize> {
    conversation
        .transcript()
        .blocks()
        .iter()
        .position(|block| said == block.source())
        .ok_or(anyhow!("no block says {said:?}"))
}

/// # Returns
///
/// Every break of `conversation`, as where it is and why.
fn breaks(conversation: &Conversation) -> Vec<(usize, Reason)> {
    conversation
        .breaks()
        .iter()
        .map(|broken| (broken.after(), broken.reason()))
        .collect()
}

/// # Returns
///
/// How the stand-in is started in `directory`, resuming the recorded session.
fn spawn(directory: &Path) -> Spawn {
    Spawn::new(Identity::Resumed(SessionId::known(RECORDED_ID)))
        .with_binary(STUB)
        .with_directory(directory)
}

/// Draws one frame of the application the way the program does: the timer's tick, and then the
/// frame written through a terminal backend.
///
/// # Returns
///
/// The bytes the frame was written as, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`ratatui::Terminal::with_options`]'s return values on failure.
/// * Forwards [`ratatui::Terminal::draw`]'s return values on failure.
fn drawn(app: &mut App) -> Result<Vec<u8>> {
    app.handle(area(), &Delivered::Redraw);
    let mut bytes: Vec<u8> = Vec::new();
    let mut terminal = Terminal::with_options(
        CrosstermBackend::new(&mut bytes),
        TerminalOptions {
            viewport: Viewport::Fixed(area()),
        },
    )?;
    terminal.draw(|frame| app.render(frame))?;
    drop(terminal);

    Ok(bytes)
}

/// Drives the application on the timer's own tick until `done`.
///
/// # Returns
///
/// Whether it came about within [`PATIENCE`].
fn settled(app: &mut App, done: impl Fn(&mut App) -> bool) -> bool {
    let deadline = Instant::now() + PATIENCE;
    loop {
        app.handle(area(), &Delivered::Redraw);
        if done(app) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(TICK);
    }
}

/// Drives the application on the timer's own tick for `lasting`.
fn driven(app: &mut App, lasting: Duration) {
    let deadline = Instant::now() + lasting;
    while Instant::now() < deadline {
        app.handle(area(), &Delivered::Redraw);
        thread::sleep(TICK);
    }
}

/// Types `keys` at the application one at a time, an escape and a carriage return as the keys
/// they are.
fn typing(app: &mut App, keys: &str) {
    for key in keys.chars() {
        let code = match key {
            '\u{1b}' => KeyCode::Esc,
            '\r' => KeyCode::Enter,
            key => KeyCode::Char(key),
        };
        app.handle(
            area(),
            &Delivered::Key(KeyEvent::new(code, KeyModifiers::NONE)),
        );
    }
}

/// # Returns
///
/// The first line of the history panel that ends with `said`, or [`None`] where none does.
fn line_of(app: &mut App, said: &str) -> Option<usize> {
    app.panel()
        .text()
        .lines()
        .position(|line| line.trim_end().ends_with(said))
}

/// # Returns
///
/// The area every case is driven in.
fn area() -> Rect {
    Rect::new(0, 0, COLUMNS, ROWS)
}
