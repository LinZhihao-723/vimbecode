//! `"+` and `"*` as the desktop's clipboard, driven the way a reader drives them.
//!
//! Everything under `clipboard/` was built and tested before any of it was reachable. The frames,
//! the helper's life, the deadlines and the write path each had a file of their own and each
//! passed, and no keystroke could arrive at one of them: `"+` was a name the keybinding table knew
//! and modalkit threw the writes to it away. A suite can go on being green over that for as long
//! as nobody types the keys, which is why nothing here constructs a `Bridge`, a `Reader`, a
//! `Writer` or a `Helper` for the product cases. Every one of them builds the conversation screen
//! the binary opens -- the history panel over the prompt -- types what a reader types, and asks
//! Windows itself what happened.
//!
//! Windows itself is the point. The write path's central claim -- that `clip.exe` fed UTF-16LE
//! puts a yank where another application can paste it -- had never once been executed against a
//! real clipboard when it was written, because the station's session was locked. It is executed
//! here: `Get-Clipboard` is the oracle for what a yank left behind, and `clip.exe` is what puts
//! something there for a put to find. Where there is no Windows the cases that need one skip
//! loudly; where there is one whose clipboard will not answer they fail, because those two results
//! are not the same and only one of them is nobody's fault.
//!
//! The two panels are asked different things. Nothing is written in the history panel, so every
//! yank there is asked to reach the desktop whether or not it names `"+`; the prompt keeps vim's
//! own registers, so a plain yank there is asked to leave what another window copied where it was.
//!
//! Most of the cases need no Windows at all and are the ones that would still be worth running on
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

/// The draft in the prompt, whose first line is what a `"+yy` sends to the desktop and whose
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

/// The line of the code the cursor is walked onto, the line under it, and the word a yank over a
/// motion takes out of the first of them.
const TODO: &str = "    todo!();";
const CLOSED: &str = "}";
const WORD: &str = "todo";

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

/// `"+yy` in the prompt puts the line on the Windows clipboard.
#[test]
fn a_yank_to_the_clipboard_register_reaches_windows() -> Result<()> {
    let _turn = turn();
    let Some(oracle) = Oracle::open()? else {
        return Ok(());
    };

    put_raw(&[])?;
    let mut app = prompting(Bridge::windows());
    press(&mut app, "\"+yy");
    drop(app);

    assert_eq!(
        format!("{FIRST}\r\n"),
        oracle.text()?,
        "`\"+yy` left the line somewhere other than the Windows clipboard"
    );

    Ok(())
}

/// A plain `yy` in the prompt leaves on the Windows clipboard what another window put there.
#[test]
fn a_plain_yank_in_the_prompt_leaves_windows_alone() -> Result<()> {
    let _turn = turn();
    let Some(oracle) = Oracle::open()? else {
        return Ok(());
    };

    let mut app = prompting(Bridge::windows());
    put_raw(&utf16le(COPIED))?;
    press(&mut app, "yy");
    drop(app);

    assert_eq!(
        COPIED,
        oracle.text()?,
        "a plain `yy` in the prompt replaced what another window had copied"
    );

    Ok(())
}

/// `"+p` in the prompt puts what Windows holds into the draft.
#[test]
fn a_put_from_the_clipboard_register_inserts_what_windows_holds() -> Result<()> {
    let _turn = turn();
    let Some(_oracle) = Oracle::open()? else {
        return Ok(());
    };

    let mut app = prompting(Bridge::windows());
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

/// A `yac` in the history panel reaches the Windows clipboard, so a code block Claude wrote can be
/// pasted into another Windows application.
#[test]
fn a_code_block_yanked_in_the_history_reaches_windows() -> Result<()> {
    let Some(held) = yanked_on_windows("yac")? else {
        return Ok(());
    };

    assert_eq!(
        rewritten(CODE),
        held,
        "`yac` in the history left the code block nowhere Windows could see it"
    );

    Ok(())
}

/// A plain `yy` in the history panel reaches the Windows clipboard.
#[test]
fn a_line_yanked_in_the_history_reaches_windows() -> Result<()> {
    let Some(held) = yanked_on_windows("yy")? else {
        return Ok(());
    };

    assert_eq!(
        rewritten(TODO),
        held,
        "`yy` in the history left the line nowhere Windows could see it"
    );

    Ok(())
}

/// A visual `Vjy` in the history panel reaches the Windows clipboard.
#[test]
fn lines_yanked_visually_in_the_history_reach_windows() -> Result<()> {
    let Some(held) = yanked_on_windows("Vjy")? else {
        return Ok(());
    };

    assert_eq!(
        rewritten(&format!("{TODO}\n{CLOSED}")),
        held,
        "`Vjy` in the history left the lines nowhere Windows could see them"
    );

    Ok(())
}

/// A code block yanked a second time in the history panel reaches the Windows clipboard again,
/// after another window has copied something over the first yank.
#[test]
fn a_code_block_yanked_again_after_another_window_copied_reaches_windows() -> Result<()> {
    let _turn = turn();
    let Some(oracle) = Oracle::open()? else {
        return Ok(());
    };

    let code = rewritten(CODE);
    let mut app = reading(Bridge::windows());
    press(&mut app, "yac");
    let deadline = Instant::now() + SETTLE_BUDGET;
    while code != oracle.text()? {
        ensure!(
            Instant::now() < deadline,
            "the first `yac` never reached the Windows clipboard"
        );
        thread::sleep(TICK);
    }
    put_raw(&utf16le(COPIED))?;

    ensure!(
        COPIED == oracle.text()?,
        "what another window copied never reached the Windows clipboard"
    );

    press(&mut app, "yac");
    drop(app);

    assert_eq!(
        code,
        oracle.text()?,
        "a second `yac` of the same block left what another window copied on the clipboard"
    );

    Ok(())
}

/// `"*` is the same clipboard as `"+`.
///
/// This asks nothing of Windows, because the two names being one register is a fact about the
/// editor rather than about a desktop, and a station whose clipboard is unavailable is no reason
/// to stop checking it.
#[test]
fn a_yank_to_the_alias_reaches_the_same_clipboard() -> Result<()> {
    let directory = Directory::create()?;
    let capture = directory.join("capture.bin");
    let mut app = prompting(captured(&capture, ""));

    press(&mut app, "j\"*yy");
    drop(app);

    assert_eq!(
        format!("{SECOND}\n"),
        decoded(&fs::read(&capture)?)?,
        "`\"*yy` reached a register of the editor's own rather than the desktop"
    );

    Ok(())
}

/// A plain `yy` in the prompt hands the desktop's writer nothing at all.
#[test]
fn a_plain_yank_in_the_prompt_hands_the_writer_nothing() -> Result<()> {
    let directory = Directory::create()?;
    let capture = directory.join("capture.bin");
    let mut app = prompting(captured(&capture, ""));

    press(&mut app, "yy");

    assert_eq!(
        Some(0),
        app.clipboard().map(Bridge::writes_issued),
        "a plain `yy` in the prompt was handed to the desktop"
    );

    drop(app);

    assert!(
        !capture.exists(),
        "a plain `yy` in the prompt reached the desktop's writer"
    );

    Ok(())
}

/// What leaves the editor for the desktop's writer is the line, encoded the way the desktop's
/// writer has to be fed.
///
/// The desktop is the only thing stood in for. The keystrokes, the register, the mirror, the worker
/// thread, the spawn and the pipe are all the real ones, and what is read back is the bytes a real
/// program was handed on its real standard input.
#[test]
fn a_yank_to_the_clipboard_register_reaches_the_writer_as_utf16le() -> Result<()> {
    let directory = Directory::create()?;
    let capture = directory.join("capture.bin");
    let mut app = prompting(captured(&capture, ""));

    press(&mut app, "\"+yy");
    drop(app);

    assert_eq!(
        format!("{FIRST}\n"),
        decoded(&fs::read(&capture)?)?,
        "`\"+yy` handed the writer something other than the line it yanked"
    );

    Ok(())
}

/// A `yac` in the history panel reaches the desktop's writer without anyone naming a register.
#[test]
fn a_code_block_yanked_in_the_history_reaches_the_writer() -> Result<()> {
    assert_eq!(
        format!("{CODE}\n"),
        yanked_through_writer("yac")?,
        "`yac` in the history handed the writer something other than the code block"
    );

    Ok(())
}

/// A plain `yy` in the history panel reaches the desktop's writer.
#[test]
fn a_line_yanked_in_the_history_reaches_the_writer() -> Result<()> {
    assert_eq!(
        format!("{TODO}\n"),
        yanked_through_writer("yy")?,
        "`yy` in the history handed the writer something other than the line"
    );

    Ok(())
}

/// A visual `Vjy` in the history panel reaches the desktop's writer.
#[test]
fn lines_yanked_visually_in_the_history_reach_the_writer() -> Result<()> {
    assert_eq!(
        format!("{TODO}\n{CLOSED}\n"),
        yanked_through_writer("Vjy")?,
        "`Vjy` in the history handed the writer something other than the lines"
    );

    Ok(())
}

/// A `y` over a motion in the history panel reaches the desktop's writer.
#[test]
fn a_word_yanked_over_a_motion_in_the_history_reaches_the_writer() -> Result<()> {
    assert_eq!(
        WORD,
        yanked_through_writer("wye")?,
        "`ye` in the history handed the writer something other than the word"
    );

    Ok(())
}

/// A yank in the history panel that names `"+` reaches the desktop's writer as well.
#[test]
fn a_yank_naming_the_clipboard_in_the_history_reaches_the_writer() -> Result<()> {
    assert_eq!(
        format!("{TODO}\n"),
        yanked_through_writer("\"+yy")?,
        "`\"+yy` in the history handed the writer something other than the line"
    );

    Ok(())
}

/// The same text yanked twice is handed to the desktop's writer twice, because another window may
/// have copied something over the first yank in between.
#[test]
fn the_same_text_yanked_twice_reaches_the_writer_twice() -> Result<()> {
    for (keys, in_the_history) in [("yacyac", true), ("yyyy", true), ("\"+yy\"+yy", false)] {
        let directory = Directory::create()?;
        let capture = directory.join("capture.bin");
        let clipboard = captured(&capture, "");
        let mut app = if in_the_history {
            reading(clipboard)
        } else {
            prompting(clipboard)
        };
        press(&mut app, keys);

        assert_eq!(
            Some(2),
            app.clipboard().map(Bridge::writes_issued),
            "`{keys}` handed the writer its second yank some number of times other than once"
        );
    }

    Ok(())
}

/// Neither a put from the desktop nor a keystroke that names `"+` and writes nothing into it hands
/// the desktop's writer anything, so what another window copied is not replaced by what the editor
/// last held.
#[test]
fn a_clipboard_register_named_but_not_written_hands_the_writer_nothing() -> Result<()> {
    let directory = Directory::create()?;
    let capture = directory.join("capture.bin");
    let mut app = prompting(captured(&capture, COPIED));
    press(&mut app, "\"+p");
    settle(&mut app)?;
    press(&mut app, "\"+");
    app.press(area(), key(KeyCode::Esc));
    press(&mut app, "\"+j");

    assert_eq!(
        Some(0),
        app.clipboard().map(Bridge::writes_issued),
        "a keystroke that wrote nothing into `\"+` was handed to the desktop"
    );

    Ok(())
}

/// What a helper process answers a read with is what `"+p` inserts.
///
/// The helper here is a shell script rather than PowerShell, and everything between it and the
/// keystroke is the real thing: the framed protocol over its real pipes, the worker thread, the
/// deadlines, and the register the put reads.
#[test]
fn a_put_from_the_clipboard_register_inserts_what_the_helper_answered() -> Result<()> {
    let directory = Directory::create()?;
    let capture = directory.join("capture.bin");
    let mut app = prompting(captured(&capture, COPIED));

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

/// A plain `p` never asks the desktop anything.
///
/// What is asserted is that the desktop was not asked, rather than that what it holds was not
/// pasted. Those are different claims and only the first one holds whatever the desktop happens to
/// hold: a read that ran and came back with the same bytes the reader had just yanked would be
/// invisible in the buffer.
#[test]
fn a_plain_put_never_reads_the_desktop() -> Result<()> {
    let asked = Arc::new(AtomicU64::new(0));
    let mut app = prompting(stood_in(&asked, Duration::ZERO));

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

/// The read a `"+p` does make is the only one made.
#[test]
fn only_a_put_from_the_clipboard_register_reads_the_desktop() -> Result<()> {
    let asked = Arc::new(AtomicU64::new(0));
    let mut app = prompting(stood_in(&asked, Duration::ZERO));

    press(&mut app, "yyjdd\"+p");
    settle(&mut app)?;

    assert_eq!(
        1,
        asked.load(Ordering::Relaxed),
        "the desktop was asked once per put rather than once per `\"+p`"
    );

    Ok(())
}

/// A clipboard that takes five seconds to answer leaves the editor drawing, and leaves the put
/// inserting nothing at all.
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
    let mut app = prompting(stood_in(&asked, STALL));

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

/// A key typed while a put is held runs after the put rather than ahead of it.
///
/// The `j` here is what tells the two orders apart. Run after the put, it leaves the cursor on the
/// second line of a draft whose first line was pasted into; run ahead of it, it would have carried
/// the put down a line and the paste would have landed in the second line instead. Both the text
/// and the cursor are read back, because either one alone is passable by the wrong order.
#[test]
fn a_key_typed_while_a_put_waits_runs_after_it() -> Result<()> {
    let asked = Arc::new(AtomicU64::new(0));
    let mut app = prompting(stood_in(&asked, WAIT));

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

/// A `:qa!` typed while a put is held ends the program, on the frame the put is over rather than
/// ahead of it.
///
/// The clipboard here is the one that never answers, so that the put lays nothing down and the
/// draft is still the draft it was when the `:qa!` is read. What is being asked is whether a held
/// command line is still read at all -- an application that dropped what it queued, or that
/// answered for it without running it, would leave a reader typing `:qa!` at an editor that had
/// stopped listening.
#[test]
fn a_quit_typed_while_a_put_waits_still_stops() -> Result<()> {
    let asked = Arc::new(AtomicU64::new(0));
    let mut app = prompting(stood_in(&asked, STALL));
    let before = written(&app);

    press(&mut app, "\"+p");

    assert!(app.awaits_clipboard(), "the put was not held at all");

    for key in ":qa!".chars().map(typed).chain([key(KeyCode::Enter)]) {
        assert_eq!(
            Outcome::Continues,
            app.press(area(), key),
            "the `:qa!` ended the program ahead of the put it was typed after"
        );
    }

    let deadline = Instant::now() + SETTLE_BUDGET;
    let mut outcome = Outcome::Continues;
    while Outcome::Continues == outcome {
        ensure!(
            Instant::now() < deadline,
            "a `:qa!` typed behind a held put never ended the program"
        );
        outcome = app.handle(area(), &Event::Redraw);
        thread::sleep(TICK);
    }

    assert_eq!(
        Outcome::Stops,
        outcome,
        "the held `:qa!` came back as something other than the end of the program"
    );
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
/// The conversation screen the binary opens, with the two-line draft in the prompt and the exchange
/// in the history panel, reaching the desktop through `clipboard`, and with the keys at the prompt
/// in normal mode.
fn prompting(clipboard: Bridge) -> App {
    let mut app = App::new(Buffer::from_text(&format!("{FIRST}\n{SECOND}")))
        .composing()
        .with_transcript(said())
        .with_clipboard(clipboard);
    app.press(area(), key(KeyCode::Esc));

    app
}

/// # Returns
///
/// The same screen with the keys moved up to the history panel by `<C-W>k`, and its cursor walked
/// down into the code the answer fenced.
fn reading(clipboard: Bridge) -> App {
    let mut app = prompting(clipboard);
    app.press(area(), control('w'));
    app.press(area(), typed('k'));

    assert_eq!(Focus::History, app.focus(), "`<C-W>k` reached no panel");

    press(&mut app, &"j".repeat(INSIDE_THE_CODE));

    app
}

/// # Returns
///
/// A bridge to a desktop of stand-in programs: a helper process that answers every read with
/// `holding`, and a writer process that keeps what it is handed in `capture`. Everything between
/// those two programs and the keystroke is what the binary runs.
fn captured(capture: &Path, holding: &str) -> Bridge {
    let launch = Launch::of(SHELL.into(), vec![HELPER_STUB.into()])
        .with_environment("VBC_STUB_TEXT".into(), holding.into());
    let writer = Clip::of(SHELL.into(), vec![CLIP_STUB.into(), capture.into()]);

    Bridge::served_by(move || Helper::launch(launch), writer)
}

/// # Returns
///
/// A bridge to a stand-in clipboard rather than the desktop's, which answers a read after `delay`
/// and adds one to `asked` every time it is asked.
fn stood_in(asked: &Arc<AtomicU64>, delay: Duration) -> Bridge {
    let source = Stub {
        asked: Arc::clone(asked),
        delay,
    };

    Bridge::served_by(move || Ok(source), Discarded)
}

/// Types `keys` in the history panel of a screen reaching the real Windows clipboard, and asks
/// Windows what they left there.
///
/// # Returns
///
/// What the Windows clipboard holds afterwards on success, or [`None`] where there is no Windows
/// to ask.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`Oracle::open`]'s return values on failure.
/// * Forwards [`put_raw`]'s return values on failure.
/// * Forwards [`Oracle::text`]'s return values on failure.
fn yanked_on_windows(keys: &str) -> Result<Option<String>> {
    let _turn = turn();
    let Some(oracle) = Oracle::open()? else {
        return Ok(None);
    };

    put_raw(&[])?;
    let mut app = reading(Bridge::windows());
    press(&mut app, keys);
    drop(app);

    Ok(Some(oracle.text()?))
}

/// Types `keys` in the history panel of a screen reaching a stand-in writer.
///
/// # Returns
///
/// What the writer was handed, decoded, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`Directory::create`]'s return values on failure.
/// * Forwards [`std::fs::read`]'s return values on failure.
/// * Forwards [`decoded`]'s return values on failure.
fn yanked_through_writer(keys: &str) -> Result<String> {
    let directory = Directory::create()?;
    let capture = directory.join("capture.bin");
    let mut app = reading(captured(&capture, ""));
    press(&mut app, keys);
    drop(app);

    decoded(&fs::read(&capture)?)
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

/// Types `keys` at whichever panel has them.
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
/// The draft the prompt now holds, with its lines separated by one line feed.
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
/// The key event a terminal reports when `code` is typed with nothing held.
fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
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
