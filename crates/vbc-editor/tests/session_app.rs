//! A session started, read, answered and ended by the application, driven the way a reader drives
//! it.
//!
//! This is the milestone the whole client was for, and the thing it has to prove is not that the
//! pieces work -- `session_stub.rs`, `session_control.rs` and `session_panel.rs` each prove one of
//! them already -- but that a keystroke reaches them. So nothing here calls the client. Every case
//! builds an [`App`] over a session and then does what a reader does: types `:ask`, waits for the
//! frames to arrive on the timer's own tick, walks the panel, and answers what the session stopped
//! for from inside the panel it stopped in.
//!
//! What stands in for the model is the same pair of stand-ins the client's own tests are driven
//! against, because the failures that matter here are the ones a real session will not produce on
//! demand: a directory whose project code must not run, a turn that stops for an approval and does
//! not go on until it gets one, and a conversation long enough for a frame's cost to be read off
//! two places in it. Everything between the keystroke and the child is real -- a process of its
//! own, three pipes, NDJSON both ways, and a thread reading it while the application draws.
//!
//! The trust cases are paired, as P.4's own are, and for the same reason: an absence is the
//! easiest thing in the world to assert by accident. The same fixture, the same stand-in and the
//! same spawn are run twice, once withheld and once granted, and the pair is what says the gate is
//! what stopped the project's code rather than a spawn that was never going to run it.
//!
//! The cost case is the panel's own property spelled against a session the application started. A
//! conversation grows without bound, so a frame drawn deep in a long one has to cost the screenful
//! it draws and not the conversation above it; the long message arrives over the pipes like every
//! other frame here, and what is measured is the frame [`App::draw`] writes.

#![cfg(target_os = "linux")]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::fs;
use std::hint::black_box;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer as Cells;
use ratatui::layout::Rect;
use serde_json::json;
use tempfile::TempDir;
use vbc_editor::app::{App, Focus};
use vbc_editor::event::Event;
use vbc_editor::session::identity::{Identity, SessionId};
use vbc_editor::session::live::{Plan, Session, WAITING};
use vbc_editor::session::spawn::Spawn;
use vbc_editor::session::trust::{Answer, Gate, Standing, RECORD};
use vbc_layout::buffer::Buffer;

/// The stand-ins the application is driven against: the one that answers turns, the one that stops
/// for approvals, and the one that runs a project's own code on the way up.
const STUB: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/session/stub.sh");
const CONTROL: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/session/control.sh");
const TRUSTING: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/session/trusting.sh");

/// A turn a real session was recorded writing, which the stand-in writes back out over the same
/// pipes, and the code the answer in it fenced -- byte for byte as it was sent.
const RECORDED: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/session/answered.ndjson");
const CODE: &str = "fn main() {\n    todo!();\n}";

/// The file the stand-in answers a turn out of, and the one it writes down what it was answered
/// in. Both live in the directory the child was started in.
const SAID: &str = "said.1";
const ANSWERED: &str = "answered";
const REJECT: &str = "reject";

/// The project's own code, which runs unless the session was restricted, and what it leaves behind
/// when it does.
const HOOK: &str = ".claude/session-start";
const PROJECT_CODE: &str = "#!/bin/sh\nprintf 'the project ran its own code\\n' > sentinel\n";
const SENTINEL: &str = "sentinel";

/// What the stand-in asks to write, and what it writes there once it is allowed to.
const WRITTEN_FILE: &str = "note.txt";
const WRITTEN: &str = "hello";
const WRITE_TOOL: &str = "Write";

/// What the reader types at the session, and what the stand-in answers the first turn with.
const ASKED: &str = "hello";
const TURN: &str = "turn 1";

/// The flag the stand-in is told to refuse, which is the failure a reader has to be told about
/// rather than left to read a session that denies everything.
const PERMISSION_FLAG: &str = "--permission-prompt-tool";

/// The file the reader has open behind the transcript, which is where a put lands.
const FILE: &str = "a file the reader left open";

/// The window the application is driven in, wide enough that nothing a session says wraps.
const COLUMNS: u16 = 120;
const ROWS: u16 = 24;

/// How long the application is driven waiting for a session to say something, and how long it is
/// left between two of those frames. Nothing here waits on a model, so the patience is for a
/// loaded machine rather than for a network.
const PATIENCE: Duration = Duration::from_secs(20);
const TICK: Duration = Duration::from_millis(5);

/// The lines of the message the cost is measured over, and the two rows of the panel a frame of it
/// is drawn from. The second is ten thousand tab-indented lines below the first, which a panel
/// that walked to the row it draws would lay out on the way there.
const LONG: usize = 20_000;
const SHALLOW: usize = 64;
const DEEP: usize = 10_064;

/// The number of runs a timing takes the fastest of, which is what keeps a machine's own noise out
/// of a ratio.
const RUNS: usize = 9;

/// The factor by which a frame drawn deep in a long conversation may cost more in time than the
/// same frame at its top.
const MARGIN: u32 = 8;

/// The allocator every measurement here is read through.
#[global_allocator]
static ALLOCATOR: Counting = Counting;

thread_local! {
    /// The bytes this thread has asked for since the last [`counted`] began, given back or not.
    static ASKED_FOR: Cell<usize> = const { Cell::new(0) };

    /// The number of times it asked for them.
    static CALLS: Cell<usize> = const { Cell::new(0) };
}

/// An allocator that counts what it was asked for and hands the asking on.
struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = System.alloc(layout);
        if !pointer.is_null() {
            let _ = ASKED_FOR.try_with(|asked| asked.set(asked.get() + layout.size()));
            let _ = CALLS.try_with(|calls| calls.set(calls.get() + 1));
        }

        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        System.dealloc(pointer, layout);
    }
}

/// Validation 1: a session the application started answers a question the application asked, and
/// what it answered is in the panel as blocks.
#[test]
fn a_session_the_application_started_answers_in_its_own_panel() -> Result<()> {
    let directory = TempDir::new()?;
    let mut app = reading(Session::started(&spawn(directory.path(), STUB))?);

    say(&mut app, ASKED);
    let said = settled(&mut app, |app| app.panel().text().contains(TURN));

    assert!(
        said,
        "the session said nothing the panel drew in {PATIENCE:?}; it drew {:?}",
        app.panel().text()
    );
    assert!(
        app.panel().text().contains(ASKED),
        "the panel drew the answer without the question it answered: {:?}",
        app.panel().text()
    );
    assert_eq!(
        Some(None),
        app.session()
            .map(vbc_editor::session::live::Session::failure),
        "the session failed while it was being read"
    );

    Ok(())
}

/// Validation 2: `yac` over a code block a live session sent takes its source, and `p` on the
/// other side of `<C-T>` puts it in the file. The headline gesture, against a session the
/// application is holding open rather than against a transcript it was handed.
#[test]
fn yac_takes_the_code_a_live_session_sent_and_p_puts_it_in_the_file() -> Result<()> {
    let directory = TempDir::new()?;
    fs::copy(RECORDED, directory.path().join(SAID))?;
    let mut app = reading(Session::started(&spawn(directory.path(), STUB))?);

    say(&mut app, ASKED);
    let drawn = settled(&mut app, |app| line_of(app, CODE).is_some());
    assert!(
        drawn,
        "the session sent no code the panel drew in {PATIENCE:?}; it drew {:?}",
        app.panel().text()
    );

    cross(&mut app);
    assert_eq!(Focus::Transcript, app.focus(), "`<C-T>` reached no panel");
    let down = line_of(&mut app, CODE).ok_or(anyhow!("the code left the panel"))?;
    for _ in 0..down {
        press(&mut app, typed('j'));
    }
    for key in "yac".chars() {
        press(&mut app, typed(key));
    }
    cross(&mut app);
    press(&mut app, typed('p'));

    assert_eq!(format!("{FILE}\n{CODE}"), app.text().text());

    Ok(())
}

/// Validation 3: a session that stops for an approval draws the question in the panel, is answered
/// from inside that panel, and goes on to do the thing it asked about.
#[test]
fn a_session_that_stops_for_an_approval_is_answered_from_the_panel() -> Result<()> {
    let directory = TempDir::new()?;
    let mut app = reading(Session::started(&spawn(directory.path(), CONTROL))?);

    say(&mut app, ASKED);
    let asked = settled(&mut app, |app| app.panel().text().contains(WAITING));
    assert!(
        asked,
        "the session asked nothing the panel drew in {PATIENCE:?}; it drew {:?}",
        app.panel().text()
    );
    assert!(
        app.status().contains(WRITE_TOOL),
        "the status line says nothing about the call the session is waiting on: {:?}",
        app.status()
    );
    assert!(
        !directory.path().join(WRITTEN_FILE).exists(),
        "the session wrote the file before anybody answered it"
    );

    cross(&mut app);
    assert_eq!(Focus::Transcript, app.focus(), "`<C-T>` reached no panel");
    say(&mut app, "");
    let wrote = settled(&mut app, |_| directory.path().join(WRITTEN_FILE).exists());

    assert!(
        wrote,
        "the session was allowed from the panel and wrote nothing in {PATIENCE:?}; the panel \
         draws {:?}",
        app.panel().text()
    );
    assert_eq!(
        WRITTEN,
        fs::read_to_string(directory.path().join(WRITTEN_FILE))?.trim()
    );
    assert!(
        settled(&mut app, |app| !app.panel().text().contains(WAITING)),
        "the question stayed in the panel after it had been answered: {:?}",
        app.panel().text()
    );

    Ok(())
}

/// The other half of validation 3: a refusal typed at the same panel is what the session is told,
/// so the allowance above is the answer that was sent rather than the only answer there is.
#[test]
fn a_refusal_typed_at_the_panel_is_what_the_session_is_told() -> Result<()> {
    let directory = TempDir::new()?;
    let mut app = reading(Session::started(&spawn(directory.path(), CONTROL))?);

    say(&mut app, ASKED);
    assert!(
        settled(&mut app, |app| app.panel().text().contains(WAITING)),
        "the session asked nothing the panel drew in {PATIENCE:?}"
    );

    cross(&mut app);
    typing(&mut app, ":deny not this time");
    press(&mut app, entered());
    let told = settled(&mut app, |_| answered(directory.path()).contains("deny"));

    assert!(
        told,
        "the session was refused from the panel and was told nothing in {PATIENCE:?}: {:?}",
        answered(directory.path())
    );
    assert!(
        !directory.path().join(WRITTEN_FILE).exists(),
        "the session wrote the file it had been refused"
    );

    Ok(())
}

/// Validation 5: a directory the reader has not trusted is asked about before anything is started
/// in it, and the project's own code does not run.
#[test]
fn a_directory_the_reader_has_not_trusted_runs_none_of_the_project_code_in_it() -> Result<()> {
    let home = TempDir::new()?;
    let directory = TempDir::new()?;
    project(directory.path())?;
    let asked = AtomicBool::new(false);

    let session = Session::opened(&plan(home.path(), directory.path()), |_| {
        asked.store(true, Ordering::SeqCst);

        Answer::Withheld
    })?;
    assert_eq!(Standing::Restricted, session.standing());

    let mut app = reading(session);
    say(&mut app, ASKED);
    assert!(
        settled(&mut app, |app| app.panel().text().contains(TURN)),
        "the restricted session answered nothing in {PATIENCE:?}, so this case read a spawn that \
         never ran rather than a gate that held"
    );

    assert!(asked.load(Ordering::SeqCst), "the reader was never asked");
    assert!(
        !directory.path().join(SENTINEL).exists(),
        "the project's own code ran in a directory the reader was asked about and did not trust"
    );

    Ok(())
}

/// The pair that says the case above asserts a gate rather than a fixture that was never going to
/// run: the same directory, the same stand-in and the same spawn, trusted.
#[test]
fn the_same_directory_trusted_runs_the_project_code_in_it() -> Result<()> {
    let home = TempDir::new()?;
    let directory = TempDir::new()?;
    project(directory.path())?;

    let session = Session::opened(&plan(home.path(), directory.path()), |_| Answer::Granted)?;
    assert_eq!(Standing::Trusted, session.standing());

    let mut app = reading(session);
    say(&mut app, ASKED);
    assert!(
        settled(&mut app, |app| app.panel().text().contains(TURN)),
        "the trusted session answered nothing in {PATIENCE:?}"
    );

    assert!(
        settled(&mut app, |_| directory.path().join(SENTINEL).exists()),
        "the project's own code did not run in a directory the reader trusted, so the absence the \
         case above asserts is an absence of a fixture rather than of a gate"
    );

    Ok(())
}

/// A reader is told when the session they are reading is not one that can answer, rather than left
/// watching a panel that will never fill.
#[test]
fn a_session_that_refused_the_permission_flag_says_so_in_the_status_line() -> Result<()> {
    let directory = TempDir::new()?;
    fs::write(directory.path().join(REJECT), "")?;
    let mut app = reading(Session::started(&spawn(directory.path(), STUB))?);

    say(&mut app, ASKED);
    let told = settled(&mut app, |app| app.status().contains(PERMISSION_FLAG));

    assert!(
        told,
        "a session that refused `{PERMISSION_FLAG}` was read for {PATIENCE:?} without the reader \
         being told; the status line says {:?}",
        app.status()
    );

    Ok(())
}

/// Validation 6: the panel's cost property, over a conversation the application read off a
/// session's own pipes.
#[test]
fn a_frame_deep_in_a_long_live_session_asks_for_what_one_at_its_top_asks_for() -> Result<()> {
    let directory = TempDir::new()?;
    fs::write(directory.path().join(SAID), spoken(LONG))?;
    let mut app = reading(Session::started(&spawn(directory.path(), STUB))?);

    say(&mut app, ASKED);
    assert!(
        settled(&mut app, |app| app.panel().text().lines().count() > LONG),
        "the session sent no long message in {PATIENCE:?}"
    );
    cross(&mut app);
    assert_eq!(Focus::Transcript, app.focus(), "`<C-T>` reached no panel");

    let mut cells = Cells::empty(area());
    scrolled(&mut app, SHALLOW);
    app.draw(&mut cells, area());
    let above = frame(&cells);
    let (_, at_the_top) = counted(|| app.draw(&mut cells, area()));
    let quickest = fastest(|| app.draw(&mut cells, area()));

    scrolled(&mut app, DEEP - SHALLOW);
    app.draw(&mut cells, area());
    let below = frame(&cells);
    let (_, deep) = counted(|| app.draw(&mut cells, area()));
    let taken = fastest(|| app.draw(&mut cells, area()));

    assert_ne!(
        above, below,
        "the panel drew the same rows at row {SHALLOW} and at row {DEEP}, so the two costs below \
         are the cost of one place rather than of two"
    );
    assert_eq!(
        at_the_top, deep,
        "a frame at row {DEEP} of a {LONG}-line message a session sent asked for {deep:?}, and \
         the same frame at its row {SHALLOW} asked for {at_the_top:?}"
    );
    assert!(
        taken < quickest * MARGIN,
        "a frame at row {DEEP} of a {LONG}-line message a session sent took {taken:?}, and the \
         same frame at its row {SHALLOW} took {quickest:?}"
    );

    Ok(())
}

/// # Returns
///
/// The application a reader types at, over `session`, drawn in a window with a status line.
fn reading(session: Session) -> App {
    App::new(Buffer::from_text(FILE))
        .with_status(true)
        .with_session(session)
}

/// # Returns
///
/// How a stand-in is started in `directory`, under a session identifier of its own.
fn spawn(directory: &Path, binary: &str) -> Spawn {
    Spawn::new(Identity::Fresh(SessionId::generated()))
        .with_binary(binary)
        .with_directory(directory)
}

/// # Returns
///
/// The plan a session in `directory` is opened from, whose trust is kept in a record under `home`
/// rather than in the reader's own.
fn plan(home: &Path, directory: &Path) -> Plan {
    Plan::new(
        Identity::Fresh(SessionId::generated()),
        directory,
        Gate::of_record(home.join(RECORD)),
    )
    .with_binary(TRUSTING)
}

/// Writes the project's own code into `directory`, which is code the project owns and the reader
/// did not write.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`std::fs::create_dir_all`]'s return values on failure.
/// * Forwards [`std::fs::write`]'s return values on failure.
/// * Forwards [`std::fs::set_permissions`]'s return values on failure.
fn project(directory: &Path) -> Result<()> {
    let hook = directory.join(HOOK);
    fs::create_dir_all(hook.parent().ok_or(anyhow!("the hook has no directory"))?)?;
    fs::write(&hook, PROJECT_CODE)?;
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755))?;

    Ok(())
}

/// # Returns
///
/// One assistant frame saying `lines` tab-indented lines, written the way a session writes one, so
/// that two frames of it differ by where they are drawn from and by nothing else.
fn spoken(lines: usize) -> String {
    let mut said = String::new();
    for line in 0..lines {
        said.push_str(&format!("\tline {line:05} of a very long answer\n"));
    }

    format!(
        "{}\n",
        json!({
            "type": "assistant",
            "parent_tool_use_id": Option::<String>::None,
            "session_id": "0f9c1c8a-0000-4000-8000-000000000001",
            "message": {
                "id": "msg_long",
                "type": "message",
                "role": "assistant",
                "model": "stub",
                "content": [{"type": "text", "text": said}],
            },
        })
    )
}

/// Types an ex line at the application and enters it: `:ask <text>` where there is something to
/// say, and `:allow` where there is not.
fn say(app: &mut App, text: &str) {
    let line = if text.is_empty() {
        ":allow".to_owned()
    } else {
        format!(":ask {text}")
    };
    typing(app, &line);
    press(app, entered());
}

/// Types `line` at the application, one key at a time.
fn typing(app: &mut App, line: &str) {
    for key in line.chars() {
        press(app, typed(key));
    }
}

/// Hands the application one key, through the loop's own door rather than through [`App::press`],
/// so that whatever the session said while the key was being typed is read as well.
fn press(app: &mut App, key: KeyEvent) {
    app.handle(area(), &Event::Key(key));
}

/// Gives the keys to the other half of the application, as `<C-T>` does.
fn cross(app: &mut App) {
    press(app, control('t'));
}

/// Scrolls the panel `rows` rows further down, a `CTRL-E` at a time, which is the only way a
/// reader moves it.
fn scrolled(app: &mut App, rows: usize) {
    for _ in 0..rows {
        app.press(area(), control('e'));
    }
}

/// Drives the application on the timer's own tick until `done`, which is what a reader watching a
/// turn arrive does.
///
/// # Returns
///
/// Whether it came about within [`PATIENCE`].
fn settled(app: &mut App, done: impl Fn(&mut App) -> bool) -> bool {
    let deadline = Instant::now() + PATIENCE;
    loop {
        app.handle(area(), &Event::Redraw);
        if done(app) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(TICK);
    }
}

/// # Returns
///
/// What the stand-in wrote down about the answers it was given, which is empty where it was given
/// none.
fn answered(directory: &Path) -> String {
    fs::read_to_string(directory.join(ANSWERED)).unwrap_or_default()
}

/// # Returns
///
/// The last line of the folded panel that is written from nothing but the first line of `said`, or
/// `None` where the panel draws no such line.
fn line_of(app: &mut App, said: &str) -> Option<usize> {
    let first = said.lines().next()?;

    app.panel()
        .text()
        .lines()
        .enumerate()
        .filter(|(_, line)| first == *line)
        .map(|(at, _)| at)
        .last()
}

/// Runs `measure` with the allocator counting.
///
/// # Returns
///
/// What `measure` returned, together with the bytes it asked the allocator for and the number of
/// times it asked.
fn counted<ValueType>(measure: impl FnOnce() -> ValueType) -> (ValueType, (usize, usize)) {
    ASKED_FOR.with(|asked| asked.set(0));
    CALLS.with(|calls| calls.set(0));
    let value = measure();

    (value, (ASKED_FOR.with(Cell::get), CALLS.with(Cell::get)))
}

/// Runs `measure` [`RUNS`] times, which is what keeps a machine's own noise out of a ratio.
///
/// # Returns
///
/// The fastest of those runs.
fn fastest<ValueType>(mut measure: impl FnMut() -> ValueType) -> Duration {
    let mut quickest = Duration::MAX;
    for _ in 0..RUNS {
        let started = Instant::now();
        let value = measure();
        quickest = quickest.min(started.elapsed());
        black_box(&value);
    }

    quickest
}

/// # Returns
///
/// The text of every row of the frame in `cells`, top to bottom, each trimmed of the blanks the
/// frame padded it to the width with.
fn frame(cells: &Cells) -> Vec<String> {
    let area = area();

    (area.y..area.bottom())
        .map(|y| {
            let drawn: String = (area.x..area.right())
                .map(|x| cells[(x, y)].symbol().to_owned())
                .collect();

            drawn.trim_end().to_owned()
        })
        .collect()
}

/// # Returns
///
/// The area every case is driven in.
fn area() -> Rect {
    Rect::new(0, 0, COLUMNS, ROWS)
}

/// # Returns
///
/// The key `character` typed with no modifier.
fn typed(character: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE)
}

/// # Returns
///
/// The key `character` typed with `CTRL` held.
fn control(character: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(character), KeyModifiers::CONTROL)
}

/// # Returns
///
/// The key that enters the line typed at the status line.
fn entered() -> KeyEvent {
    KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
}
