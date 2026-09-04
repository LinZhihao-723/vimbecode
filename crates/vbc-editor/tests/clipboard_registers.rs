//! `"+` and `"*` as the desktop's clipboard, driven the way a reader drives them.
//!
//! Everything under `clipboard/` was built and tested before any of it was reachable. The frames,
//! the helper's life, the deadlines and the write path each had a file of their own and each
//! passed, and no keystroke could arrive at one of them: `"+` was a name the keybinding table knew
//! and modalkit threw the writes to it away. A suite can go on being green over that for as long
//! as nobody types the keys, which is why nothing here constructs a `Bridge`, a `Reader`, a
//! `Writer` or a `Helper` for the product cases. Every one of them builds an [`App`] the way the
//! binary builds one, types what a reader types, and asks Windows itself what happened.
//!
//! Windows itself is the point. The write path's central claim -- that `clip.exe` fed UTF-16LE
//! puts a yank where another application can paste it -- had never once been executed against a
//! real clipboard when it was written, because the station's session was locked. It is executed
//! here: `Get-Clipboard` is the oracle for what a yank left behind, and `clip.exe` is what puts
//! something there for a put to find. Where there is no Windows the cases that need one skip
//! loudly; where there is one whose clipboard will not answer they fail, because those two results
//! are not the same and only one of them is nobody's fault.
//!
//! Three of the cases need no Windows at all and are the ones that would still be worth running on
//! a machine that has none. A plain `p` reading the desktop is the regression that would make `p`
//! mean "whatever another window last copied", so it is checked against a source that counts what
//! it is asked -- an assertion no clipboard can make, since a read that happened and answered with
//! the same text as the register is a read that leaves no trace in the buffer. A clipboard that
//! takes five seconds to answer is not a clipboard any station can be asked to have, so it is
//! stood in for, and what is asserted is both halves of what the deadlines promise: the frames go
//! on being drawn while it is out, and what a put inserts when it never answers is nothing.
//!
//! The helper is started with the session and takes the better part of a second of PowerShell to
//! start, so the cases that read Windows wait that out before they type. That wait is the product
//! working rather than the test working around it -- a reader who has had the editor open for a
//! second has already paid it -- and it is spelled out here because a test that types faster than
//! any person could would be timing the startup rather than the read.

#![cfg(target_os = "linux")]

mod clipboard;

use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{ensure, Result};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer as Cells;
use ratatui::layout::Rect;
use vbc_editor::app::{App, Focus, Outcome};
use vbc_editor::chat::block::{Block, Kind, Role};
use vbc_editor::chat::transcript::Transcript;
use vbc_editor::clipboard::clip::{Clip, Error as WriteError};
use vbc_editor::clipboard::helper::{Error, Helper, Launch};
use vbc_editor::clipboard::protocol::Response;
use vbc_editor::clipboard::reader::{Source, HARD_DEADLINE, READING_NOTICE, SOFT_DEADLINE};
use vbc_editor::clipboard::register::{Bridge, ABANDONED_NOTICE};
use vbc_editor::clipboard::writer::Sink;
use vbc_editor::engine::typed;
use vbc_editor::event::Event;
use vbc_layout::buffer::Buffer;

use crate::clipboard::{decoded, put_raw, turn, Directory, Oracle, CLIP_STUB, HELPER_STUB, SHELL};

/// The window every case is driven in, wide enough that the fixture's lines do not wrap.
const COLUMNS: u16 = 80;
const ROWS: u16 = 24;

/// The file the reader has open, whose first line is what a `"+yy` sends to the desktop and whose
/// second is what a put lands between.
const FIRST: &str = "the line a reader yanks to the desktop";
const SECOND: &str = "the line under it";

/// What another Windows application left on the clipboard for a put to find. It is not ASCII,
/// because ASCII survives the console code page and would pass against a write path that had been
/// destroyed by it.
const COPIED: &str = "从另一个窗口复制的一行 with an é\u{301} and 🎉";

/// What was asked of Claude and the answer holding the code block the reader came for.
const ASKED: &str = "show me the main function";
const ANSWERED: &str = concat!(
    "Here it is:\n",
    "\n",
    "```rust\n",
    "fn main() {\n",
    "    todo!();\n",
    "}\n",
    "```\n",
    "\n",
    "That should do it.",
);

/// The code that answer fenced, which is what `yac` takes out of it, and the line of the flattened
/// transcript the cursor has to be on to be inside it.
const CODE: &str = "fn main() {\n    todo!();\n}";
const INSIDE_THE_CODE: usize = 5;

/// How long the session-lived helper is given to come up before a case types a put at it.
const WARM_UP: Duration = Duration::from_secs(4);

/// How long a held put is driven for, and how long a frame is left between two ticks. The budget
/// is well past the reader's own hard deadline, because a put that is still held after that has
/// stopped being held to it.
const SETTLE_BUDGET: Duration = Duration::from_secs(8);
const TICK: Duration = Duration::from_millis(8);

/// How long the stand-in clipboard takes to answer, which is far past every deadline the reader
/// holds a read to.
const STALL: Duration = Duration::from_secs(5);

/// How long it takes to answer where the case is about a put that is held and then runs, which is
/// long enough that a key typed straight after the put arrives while the put is still held and
/// short enough to be well inside the hard deadline.
const WAIT: Duration = Duration::from_millis(200);

/// The longest a frame may take to draw while a put is waiting on that stand-in. A render loop that
/// waited on the clipboard would spend the whole stall inside one of these.
const FRAME_BUDGET: Duration = Duration::from_millis(50);

/// How long the stalled case keeps drawing for before it starts asking whether the put is over,
/// which is past the soft deadline and short of the hard one.
const WHILE_SLOW: Duration = Duration::from_millis(600);

/// Validation 1: `"+yy` in the editor puts the line on the Windows clipboard.
#[test]
fn a_yank_to_the_clipboard_register_reaches_windows() -> Result<()> {
    let _turn = turn();
    let Some(oracle) = Oracle::open()? else {
        return Ok(());
    };

    put_raw(&[])?;
    let mut app = editing();
    press(&mut app, "\"+yy");
    drop(app);

    assert_eq!(
        format!("{FIRST}\r\n"),
        oracle.text()?,
        "`\"+yy` left the line somewhere other than the Windows clipboard"
    );

    Ok(())
}

/// Validation 1, again through the other name: `"*` is the same clipboard as `"+`.
///
/// This asks nothing of Windows, because the two names being one register is a fact about the
/// editor rather than about a desktop, and a station whose clipboard is unavailable is no reason
/// to stop checking it.
#[test]
fn a_yank_to_the_alias_reaches_the_same_clipboard() -> Result<()> {
    let directory = Directory::create()?;
    let capture = directory.join("capture.bin");
    let mut app = editing_through(&capture, "");

    press(&mut app, "j\"*yy");
    drop(app);

    assert_eq!(
        format!("{SECOND}\n"),
        decoded(&fs::read(&capture)?)?,
        "`\"*yy` reached a register of the editor's own rather than the desktop"
    );

    Ok(())
}

/// Validation 2: `"+p` puts what Windows holds into the buffer.
#[test]
fn a_put_from_the_clipboard_register_inserts_what_windows_holds() -> Result<()> {
    let _turn = turn();
    let Some(_oracle) = Oracle::open()? else {
        return Ok(());
    };

    let mut app = editing();
    thread::sleep(WARM_UP);
    put_raw(&utf16le(COPIED))?;

    press(&mut app, "\"+p");
    settle(&mut app)?;

    let (head, rest) = FIRST.split_at(1);

    assert_eq!(
        format!("{head}{COPIED}{rest}\n{SECOND}"),
        written(&app),
        "`\"+p` inserted something other than what Windows held: {:?}",
        app.notice()
    );

    Ok(())
}

/// Validation 3: a `yac` in the transcript panel reaches the Windows clipboard, so a code block
/// Claude wrote can be pasted into another Windows application.
#[test]
fn a_code_block_yanked_in_the_panel_reaches_windows() -> Result<()> {
    let _turn = turn();
    let Some(oracle) = Oracle::open()? else {
        return Ok(());
    };

    put_raw(&[])?;
    let mut app = reading();
    press(&mut app, &"j".repeat(INSIDE_THE_CODE));
    press(&mut app, "yac");
    drop(app);

    assert_eq!(
        rewritten(CODE),
        oracle.text()?,
        "`yac` in the panel left the code block nowhere Windows could see it"
    );

    Ok(())
}

/// Validation 4: a plain `p` never asks the desktop anything.
///
/// What is asserted is that the desktop was not asked, rather than that what it holds was not
/// pasted. Those are different claims and only the first one holds whatever the desktop happens to
/// hold: a read that ran and came back with the same bytes the reader had just yanked would be
/// invisible in the buffer.
#[test]
fn a_plain_put_never_reads_the_desktop() -> Result<()> {
    let asked = Arc::new(AtomicU64::new(0));
    let mut app = stood_in(&asked, Duration::ZERO);

    press(&mut app, "yyp");
    settle(&mut app)?;

    assert_eq!(
        0,
        asked.load(Ordering::Relaxed),
        "a plain `p` asked the desktop what it held"
    );
    assert_eq!(
        Some(0),
        app.clipboard().map(Bridge::reads_issued),
        "a plain `p` put a read to the clipboard reader"
    );
    assert_eq!(
        format!("{FIRST}\n{FIRST}\n{SECOND}"),
        written(&app),
        "a plain `p` put back something other than what was just yanked"
    );

    Ok(())
}

/// Validation 4, the other half: the read a `"+p` does make is the only one made.
#[test]
fn only_a_put_from_the_clipboard_register_reads_the_desktop() -> Result<()> {
    let asked = Arc::new(AtomicU64::new(0));
    let mut app = stood_in(&asked, Duration::ZERO);

    press(&mut app, "yyjdd\"+p");
    settle(&mut app)?;

    assert_eq!(
        1,
        asked.load(Ordering::Relaxed),
        "the desktop was asked once per put rather than once per `\"+p`"
    );

    Ok(())
}

/// Validation 5: a clipboard that takes five seconds to answer leaves the editor drawing, and
/// leaves the put inserting nothing at all.
///
/// Both halves are asserted because either on its own is passable by a broken editor. One that
/// blocked the render loop would insert the right text after five seconds; one that pasted the
/// register's stale contents would keep drawing perfectly.
///
/// The register is loaded before the put is asked for, because an empty one cannot tell those two
/// apart. A put abandoned over a register holding nothing inserts nothing whether the abandonment
/// emptied it or left it exactly as it stood, so the second half asserts nothing until there is
/// something for a stale answer to be. The `"+yy` is what puts it there, and what it leaves is a
/// whole line, which a put would lay down where no reader could miss it.
#[test]
fn a_clipboard_that_stalls_neither_stops_the_frames_nor_pastes_anything() -> Result<()> {
    let asked = Arc::new(AtomicU64::new(0));
    let mut app = stood_in(&asked, STALL);

    press(&mut app, "\"+yy");

    let before = written(&app);

    press(&mut app, "\"+p");

    let mut cells = Cells::empty(area());
    let mut frames = 0_u64;
    let started = Instant::now();
    let mut slowest = Duration::ZERO;
    while started.elapsed() < WHILE_SLOW {
        let drawn = Instant::now();
        let _cursor = app.draw(&mut cells, area());
        slowest = slowest.max(drawn.elapsed());
        frames += 1;
        app.handle(area(), &Event::Redraw);
        thread::sleep(TICK);
    }

    assert!(
        slowest < FRAME_BUDGET,
        "a frame took {slowest:?} while the clipboard was out, so the render loop waited on it"
    );
    assert!(
        WHILE_SLOW.as_millis() / (2 * TICK.as_millis()) < u128::from(frames),
        "only {frames} frames were drawn in {WHILE_SLOW:?}, so the loop was not running"
    );
    assert_eq!(
        Some(READING_NOTICE),
        app.notice(),
        "nothing was said about a read that is past {SOFT_DEADLINE:?}"
    );

    settle(&mut app)?;

    assert_eq!(
        before,
        written(&app),
        "a put the clipboard never answered inserted something"
    );
    assert_eq!(
        Some(ABANDONED_NOTICE),
        app.notice(),
        "nothing was said about a put that was abandoned"
    );
    assert!(
        started.elapsed() < STALL,
        "the put was not abandoned until the clipboard answered, {HARD_DEADLINE:?} being the \
         deadline it was held to"
    );

    Ok(())
}

/// Validation 1, on a machine with no Windows: what leaves the editor for the desktop's writer is
/// the line, encoded the way the desktop's writer has to be fed.
///
/// The desktop is the only thing stood in for. The keystrokes, the register, the mirror, the worker
/// thread, the spawn and the pipe are all the real ones, and what is read back is the bytes a real
/// program was handed on its real standard input.
#[test]
fn a_yank_to_the_clipboard_register_reaches_the_writer_as_utf16le() -> Result<()> {
    let directory = Directory::create()?;
    let capture = directory.join("capture.bin");
    let mut app = editing_through(&capture, "");

    press(&mut app, "\"+yy");
    drop(app);

    assert_eq!(
        format!("{FIRST}\n"),
        decoded(&fs::read(&capture)?)?,
        "`\"+yy` handed the writer something other than the line it yanked"
    );

    Ok(())
}

/// Validation 3, on a machine with no Windows: a `yac` in the transcript panel reaches that same
/// writer, so the code block leaves the editor without anyone naming a register.
#[test]
fn a_code_block_yanked_in_the_panel_reaches_the_writer() -> Result<()> {
    let directory = Directory::create()?;
    let capture = directory.join("capture.bin");
    let mut app = editing_through(&capture, "").with_transcript(said());
    app.press(area(), control('t'));

    press(&mut app, &"j".repeat(INSIDE_THE_CODE));
    press(&mut app, "yac");
    drop(app);

    assert_eq!(
        format!("{CODE}\n"),
        decoded(&fs::read(&capture)?)?,
        "`yac` in the panel handed the writer something other than the code block"
    );

    Ok(())
}

/// Validation 2, on a machine with no Windows: what a helper process answers a read with is what
/// `"+p` inserts.
///
/// The helper here is a shell script rather than PowerShell, and everything between it and the
/// keystroke is the real thing: the framed protocol over its real pipes, the worker thread, the
/// deadlines, and the register the put reads.
#[test]
fn a_put_from_the_clipboard_register_inserts_what_the_helper_answered() -> Result<()> {
    let directory = Directory::create()?;
    let capture = directory.join("capture.bin");
    let mut app = editing_through(&capture, COPIED);

    press(&mut app, "\"+p");
    settle(&mut app)?;

    let (head, rest) = FIRST.split_at(1);

    assert_eq!(
        format!("{head}{COPIED}{rest}\n{SECOND}"),
        written(&app),
        "`\"+p` inserted something other than what the helper answered with: {:?}",
        app.notice()
    );

    Ok(())
}

/// A key typed while a put is held runs after the put rather than ahead of it.
///
/// The `j` here is what tells the two orders apart. Run after the put, it leaves the cursor on the
/// second line of a file whose first line was pasted into; run ahead of it, it would have carried
/// the put down a line and the paste would have landed in the second line instead. Both the text
/// and the cursor are read back, because either one alone is passable by the wrong order.
#[test]
fn a_key_typed_while_a_put_waits_runs_after_it() -> Result<()> {
    let asked = Arc::new(AtomicU64::new(0));
    let mut app = stood_in(&asked, WAIT);

    press(&mut app, "\"+p");

    assert!(app.awaits_clipboard(), "the put was not held at all");

    press(&mut app, "j");
    settle(&mut app)?;

    let (head, rest) = FIRST.split_at(1);

    assert_eq!(
        format!("{head}{COPIED}{rest}\n{SECOND}"),
        written(&app),
        "the `j` ran before the put it was typed after"
    );
    assert_eq!(1, app.cursor().line, "the `j` never ran at all");

    Ok(())
}

/// A `q` typed while a put is held ends the session, on the frame the put is over rather than
/// ahead of it.
///
/// The clipboard here is the one that never answers, so that the put lays nothing down and the
/// text is still the text that was opened when the `q` is read. A `q` over a buffer nothing has
/// written is the one that quits, and what is being asked is whether a held `q` is still read at
/// all -- an application that dropped what it queued, or that answered for it without running it,
/// would leave a reader typing `q` at an editor that had stopped listening.
#[test]
fn a_quit_typed_while_a_put_waits_still_stops() -> Result<()> {
    let asked = Arc::new(AtomicU64::new(0));
    let mut app = stood_in(&asked, STALL);
    let before = written(&app);

    press(&mut app, "\"+p");

    assert!(app.awaits_clipboard(), "the put was not held at all");
    assert_eq!(
        Outcome::Continues,
        app.press(area(), typed('q')),
        "the `q` ended the session ahead of the put it was typed after"
    );

    let deadline = Instant::now() + SETTLE_BUDGET;
    let mut outcome = Outcome::Continues;
    while Outcome::Continues == outcome {
        ensure!(
            Instant::now() < deadline,
            "a `q` typed behind a held put never ended the session"
        );
        outcome = app.handle(area(), &Event::Redraw);
        thread::sleep(TICK);
    }

    assert_eq!(
        before,
        written(&app),
        "a put the clipboard never answered inserted something"
    );

    Ok(())
}

/// A stand-in clipboard: it answers after a wait of its own, with text nothing else here holds, and
/// counts what it was asked.
struct Stub {
    asked: Arc<AtomicU64>,
    delay: Duration,
}

impl Source for Stub {
    fn read_clipboard(&mut self) -> Result<Response, Error> {
        self.asked.fetch_add(1, Ordering::Relaxed);
        thread::sleep(self.delay);

        Ok(Response::Text(COPIED.to_owned()))
    }
}

/// A stand-in for the desktop a yank goes to, which takes everything and keeps nothing. What a
/// yank reaches Windows as is asked of Windows, so what is wanted here is a sink that is not one.
struct Discarded;

impl Sink for Discarded {
    fn write_clipboard(&mut self, _text: &str) -> Result<(), WriteError> {
        Ok(())
    }
}

/// # Returns
///
/// The editor over the two-line file, reaching the real Windows clipboard, as the binary builds it.
fn editing() -> App {
    App::new(Buffer::from_text(&format!("{FIRST}\n{SECOND}")))
        .with_status(true)
        .with_clipboard(Bridge::windows())
}

/// # Returns
///
/// The same editor with the transcript panel open, which is where a `yac` is typed.
fn reading() -> App {
    let mut app = editing().with_transcript(said());
    app.press(area(), control('t'));

    assert_eq!(Focus::Transcript, app.focus(), "`<C-T>` reached no panel");

    app
}

/// # Returns
///
/// The same editor reaching a desktop of stand-in programs: a helper process that answers every
/// read with `holding`, and a writer process that keeps what it is handed in `capture`. Everything
/// between those two programs and the keystroke is what the binary runs.
fn editing_through(capture: &Path, holding: &str) -> App {
    let launch = Launch::of(SHELL.into(), vec![HELPER_STUB.into()])
        .with_environment("VBC_STUB_TEXT".into(), holding.into());
    let writer = Clip::of(SHELL.into(), vec![CLIP_STUB.into(), capture.into()]);

    App::new(Buffer::from_text(&format!("{FIRST}\n{SECOND}")))
        .with_status(true)
        .with_clipboard(Bridge::served_by(move || Helper::launch(launch), writer))
}

/// # Returns
///
/// The same editor reaching a stand-in clipboard rather than the desktop's, which answers a read
/// after `delay` and adds one to `asked` every time it is asked.
fn stood_in(asked: &Arc<AtomicU64>, delay: Duration) -> App {
    let source = Stub {
        asked: Arc::clone(asked),
        delay,
    };

    App::new(Buffer::from_text(&format!("{FIRST}\n{SECOND}")))
        .with_status(true)
        .with_clipboard(Bridge::served_by(move || Ok(source), Discarded))
}

/// # Returns
///
/// The exchange the panel shows, which is a question and the answer fencing the code block.
fn said() -> Transcript {
    [
        Block::new(Kind::Message(Role::User), ASKED.to_owned()),
        Block::new(Kind::Message(Role::Assistant), ANSWERED.to_owned()),
    ]
    .into_iter()
    .collect()
}

/// Types `keys` at whichever half of the application has them.
fn press(app: &mut App, keys: &str) {
    for key in keys.chars() {
        app.press(area(), typed(key));
    }
}

/// Drives the editor's own loop until nothing is waiting on the clipboard any more.
///
/// # Errors
///
/// Returns an error if a put was still held after [`SETTLE_BUDGET`], which is long past every
/// deadline a read is held to.
fn settle(app: &mut App) -> Result<()> {
    let deadline = Instant::now() + SETTLE_BUDGET;
    while app.awaits_clipboard() {
        ensure!(
            Instant::now() < deadline,
            "a put was still waiting on the clipboard after {SETTLE_BUDGET:?}"
        );
        app.handle(area(), &Event::Redraw);
        thread::sleep(TICK);
    }

    Ok(())
}

/// # Returns
///
/// The text the editor now holds, with its lines separated by one line feed.
fn written(app: &App) -> String {
    app.text().text()
}

/// # Returns
///
/// The area every case is driven in.
fn area() -> Rect {
    Rect::new(0, 0, COLUMNS, ROWS)
}

/// # Returns
///
/// The key event a terminal reports when `character` is typed with control held.
fn control(character: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(character), KeyModifiers::CONTROL)
}

/// # Returns
///
/// The UTF-16LE bytes a text is spelled by, written out here rather than taken from the write path,
/// so that what a put is given to find shares no code with what put it there.
fn utf16le(text: &str) -> Vec<u8> {
    text.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

/// # Returns
///
/// What the writer's line ending rewrite leaves a text as, which is every line of it ended with a
/// CRLF. What the editor yanks is what the editor pastes; what sits on the clipboard in between is
/// this.
fn rewritten(text: &str) -> String {
    format!("{}\r\n", text.replace('\n', "\r\n"))
}
