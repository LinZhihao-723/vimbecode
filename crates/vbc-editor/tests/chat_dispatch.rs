//! The transcript panel driven the way a reader drives it: by typing keys at the application.
//!
//! Everything M5 built for a transcript -- the objects, the structural yanks, the folds, the
//! refusals -- was reachable only from its own tests. Each of those tests calls the thing it is
//! about directly, which is what let all of it pass while no run of the binary could arrive at any
//! of it. So nothing here calls `Object::resolve`, `Yank::structural` or `Folds::apply`. Every
//! case types the keys a reader types, at the application the binary runs, and reads back what a
//! reader would see: what a register holds, what the panel draws, what the status line says.
//!
//! What that is worth rests on the keys being the keys. A case that reached into the panel to
//! place its cursor, or that asked the panel for a command rather than typing one, would pass
//! against a dispatch nothing routes keys to, which is the fault this file exists to catch. So the
//! cursor is moved by `j` and `G`, the panel is entered by `<C-T>`, and the only thing read back
//! is what the application already shows.
//!
//! The patch a diff is yanked as is handed to `git apply` rather than compared against a string,
//! because a patch that looks right and does not apply is the failure that matters, and the file
//! it leaves behind is required to be the text the edit wrote.
//!
//! Two of the cases are about the shape of the code rather than about a run, and both are here
//! because the shape is what drifted. There is one machine in the workspace that reads a sequence
//! of typed keys through a table, and one type that holds a selection; the transcript half had
//! grown a second of each, neither reachable, and a scan is what stops a third appearing. What the
//! application reads for itself is a single key rather than a sequence -- the interrupt, `<C-T>`,
//! the scrolls -- and is no more a dispatcher than the terminal that reported it. Both scans read
//! the shipped source of every crate rather than a list, so a reader added in another crate fails
//! them too.
//!
//! The panel's own cost is measured rather than argued, as the block renderer's is. A panel
//! scrolled to the bottom of what a tool wrote asks for a window a hundred thousand rows down, and
//! a render that laid out everything above it would cost the scroll rather than the screen. The
//! window is therefore drawn from the top of a long block and from deep inside it and required to
//! ask the allocator for the same bytes in the same number of calls, which is the property W.1
//! established for a block held to here through the panel that draws one.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer as Cells;
use ratatui::layout::Rect;
use vbc_editor::app::{App, Focus};
use vbc_editor::chat::block::{Block, Kind, Role};
use vbc_editor::chat::fold::Position as Placed;
use vbc_editor::chat::policy::{Drawn, Panel, Policy};
use vbc_editor::chat::selection::{Mode, Source};
use vbc_editor::chat::transcript::Transcript;
use vbc_editor::engine::typed;
use vbc_editor::event::{Event, Paste};
use vbc_layout::buffer::Buffer;
use vbc_layout::width::Metrics;

/// The window every case is driven in, which is wide enough that the fixture's lines do not wrap
/// and tall enough to draw the whole of it.
const COLUMNS: u16 = 80;
const ROWS: u16 = 24;

/// The file the editor is over while the transcript is being read, which no case here touches.
const FILE: &str = "a file the reader left open";

/// What was asked, and the answer holding the code the reader came for.
const ASKED: &str = "why does it not build";
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

/// The code that answer fenced, which is what `yac` takes out of it.
const CODE: &str = "fn main() {\n    todo!();\n}";

/// What the tool answered, which is the block that folds.
const WROTE: &str = "   Compiling vimbecode\nerror: a semicolon was expected";

/// The edit that answer made: the file, the text it replaced, and the text it wrote.
const PATH: &str = "src/main.rs";
const BEFORE: &str = "fn main() {}\n";
const AFTER: &str = "fn main() {\n    todo!();\n}\n";

/// The index of each block of the fixture a case names.
const ANSWER: usize = 1;
const RAN: usize = 2;
const EDIT: usize = 3;

/// The line of the flattened transcript the code the reader wants is written on, and the line the
/// answer's last is.
const INSIDE_THE_CODE: usize = 5;
const LAST_OF_THE_ANSWER: usize = 9;

/// The name the patch is written under while `git apply` is asked to take it.
const PATCH_FILE: &str = "yanked.patch";

/// What `git apply` says when it had to move a hunk to make it fit.
const MOVED: &str = "with fuzz";

/// The file the workspace's one keybinding machine is declared in, and the one its one selection
/// type is.
const DISPATCHER: &str = "crates/vbc-editor/src/keys.rs";
const SELECTION: &str = "crates/vbc-editor/src/chat/selection.rs";

/// The shapes a second dispatcher takes: a machine that is handed a typed key of a sequence, and
/// the ad-hoc readers that turned a string of typed keys into the command it named.
const DISPATCHING: [&str; 4] = [
    "fn input_key(",
    "fn from_keys(",
    "fn read(keys: &str)",
    "enum Reading",
];

/// The shapes a type holding a selection is declared in.
const DECLARING: [&str; 3] = ["struct Selection", "enum Selection", "type Selection"];

/// The lines of the block a window is drawn out of, short and long, and the row deep inside the
/// long one that a scrolled panel asks for.
const SHORT: usize = 100;
const LONG: usize = 20_000;
const DEEP: usize = 19_000;

/// The rows a measured window asks for.
const WINDOW: usize = 20;

/// The allocator every measurement here is read through.
#[global_allocator]
static ALLOCATOR: Counting = Counting;

thread_local! {
    /// The bytes this thread has asked for since the last [`counted`] began, given back or not.
    static ASKED_FOR: Cell<usize> = const { Cell::new(0) };

    /// The number of times it asked for them.
    static CALLS: Cell<usize> = const { Cell::new(0) };
}

#[test]
fn yanking_a_code_block_from_a_keystroke_fills_the_register_with_its_source() -> Result<()> {
    let mut app = reading();
    for _ in 0..INSIDE_THE_CODE {
        app.press(area(), typed('j'));
    }
    for key in "yac".chars() {
        app.press(area(), typed(key));
    }

    let held = app
        .panel()
        .registers()
        .get(&'"')
        .cloned()
        .expect("`yac` filled the unnamed register");

    assert_eq!(format!("{CODE}\n"), held.text);
    assert_eq!(
        Some(format!("{CODE}\n")),
        app.panel().register('+').map(|held| held.text),
        "what a reader took never reached the clipboard's register"
    );

    Ok(())
}

#[test]
fn the_panel_draws_the_cursor_where_the_keys_left_it() {
    let mut app = reading();
    for _ in 0..INSIDE_THE_CODE {
        app.press(area(), typed('j'));
    }
    let mut cells = Cells::empty(area());
    let landed = app
        .draw(&mut cells, area())
        .expect("the panel draws the row the cursor is on");
    app.press(area(), typed('l'));
    let moved = app
        .draw(&mut cells, area())
        .expect("the panel draws the row the cursor is on");

    assert_eq!(
        u16::try_from(INSIDE_THE_CODE).expect("the fixture is short"),
        landed.y
    );
    assert_eq!(0, landed.x);
    assert_eq!(landed.y, moved.y);
    assert_eq!(1, moved.x);
}

#[test]
fn the_panel_draws_the_cursor_on_the_row_a_closed_fold_is_drawn_in() {
    let mut app = reading();
    for _ in 0..=LAST_OF_THE_ANSWER {
        app.press(area(), typed('j'));
    }
    let mut cells = Cells::empty(area());
    let on_the_fold = app
        .draw(&mut cells, area())
        .expect("the panel draws the cursor on the row the closed fold is");
    let row = drawn_row(&cells, on_the_fold.y);

    assert_eq!(RAN, app.panel().at().block(), "the cursor left the fold");
    assert_eq!(
        u16::try_from(LAST_OF_THE_ANSWER + 1).expect("the fixture is short"),
        on_the_fold.y
    );
    assert_eq!(0, on_the_fold.x);
    assert!(
        row.contains("2 lines"),
        "{row:?} is not the row the closed fold is drawn in"
    );

    for key in "za".chars() {
        app.press(area(), typed(key));
    }
    let opened = app
        .draw(&mut cells, area())
        .expect("the panel draws the cursor once the fold is open");

    assert_eq!(on_the_fold.y, opened.y);
    assert_eq!(
        "   Compiling vimbecode",
        drawn_row(&cells, opened.y),
        "the row the cursor is on is not the first of what the fold hid"
    );
}

#[test]
fn a_code_block_selected_from_a_keystroke_covers_the_lines_it_was_fenced_around() {
    let mut app = reading();
    for _ in 0..INSIDE_THE_CODE {
        app.press(area(), typed('j'));
    }
    for key in "viac".chars() {
        app.press(area(), typed(key));
    }

    let selected = app
        .panel()
        .selection()
        .expect("`viac` left a selection behind");
    let transcript = said();
    let block = transcript.block(selected.block()).expect("a block");
    let source = Source::new(block.source(), Metrics::default());

    assert_eq!(ANSWER, selected.block());
    assert_eq!(Mode::Charwise, selected.selection().mode());
    assert_eq!(CODE, selected.selection().text(source));
    assert_eq!(3, selected.selection().lines(source));
}

#[test]
fn a_selected_code_block_is_what_the_yank_after_it_takes() {
    let mut app = reading();
    for _ in 0..INSIDE_THE_CODE {
        app.press(area(), typed('j'));
    }
    for key in "viacy".chars() {
        app.press(area(), typed(key));
    }

    assert_eq!(
        Some(CODE.to_owned()),
        app.panel()
            .registers()
            .get(&'"')
            .map(|held| held.text.clone()),
        "the yank after `iac` took something other than what `iac` selected"
    );
}

#[test]
fn every_structural_yank_takes_what_the_cursor_is_in_from_a_keystroke() {
    for (down, keys, taken) in [
        (INSIDE_THE_CODE, "yac", CODE),
        (INSIDE_THE_CODE, "yam", ANSWERED),
        (LAST_OF_THE_ANSWER + 1, "yat", WROTE),
    ] {
        let mut app = reading();
        for _ in 0..down {
            app.press(area(), typed('j'));
        }
        for key in keys.chars() {
            app.press(area(), typed(key));
        }

        assert_eq!(
            Some(format!("{taken}\n")),
            app.panel()
                .registers()
                .get(&'"')
                .map(|held| held.text.clone()),
            "`{keys}` took something else"
        );
    }
}

#[test]
fn every_fold_at_every_depth_opens_and_closes_from_a_keystroke() {
    let mut app = reading();
    app.press(area(), typed('z'));
    app.press(area(), typed('R'));
    let opened = app.panel().folds().is_open(RAN);
    app.press(area(), typed('z'));
    app.press(area(), typed('M'));

    assert!(opened, "`zR` opened no fold");
    assert!(!app.panel().folds().is_open(RAN), "`zM` closed no fold");
}

#[test]
fn a_diff_yanked_from_a_keystroke_is_a_patch_git_apply_takes() -> Result<()> {
    let mut app = reading();
    app.press(area(), typed('G'));
    for key in "yad".chars() {
        app.press(area(), typed(key));
    }

    let held = app
        .panel()
        .registers()
        .get(&'"')
        .cloned()
        .expect("`yad` filled the unnamed register");

    assert!(
        held.text.contains("@@"),
        "what `yad` took is no unified patch: {:?}",
        held.text
    );
    assert_eq!(Some(AFTER.to_owned()), applied(PATH, BEFORE, &held.text)?);

    Ok(())
}

#[test]
fn a_fold_closed_from_a_keystroke_is_one_row_to_every_motion_that_crosses_it() {
    let mut closed = reading();
    for _ in 0..LAST_OF_THE_ANSWER {
        closed.press(area(), typed('j'));
    }
    let above = closed.panel().at().block();
    closed.press(area(), typed('j'));
    let onto = closed.panel().at().block();
    closed.press(area(), typed('j'));

    assert_eq!(ANSWER, above);
    assert_eq!(RAN, onto, "the row below the answer is not the fold's");
    assert_eq!(
        EDIT,
        closed.panel().at().block(),
        "a closed fold took more than one row to cross"
    );

    let mut opened = reading();
    for _ in 0..=LAST_OF_THE_ANSWER {
        opened.press(area(), typed('j'));
    }
    for key in "za".chars() {
        opened.press(area(), typed(key));
    }
    opened.press(area(), typed('k'));
    opened.press(area(), typed('j'));
    opened.press(area(), typed('j'));

    assert!(
        opened.panel().folds().is_open(RAN),
        "`za` left the fold closed"
    );
    assert_eq!(
        RAN,
        opened.panel().at().block(),
        "an opened fold was crossed in one row all the same"
    );
    assert!(
        opened.panel().text().len() > closed.panel().text().len(),
        "opening the fold drew none of what it hid"
    );
}

#[test]
fn a_fold_opened_and_closed_again_from_a_keystroke_comes_back_to_one_row() {
    let mut app = reading();
    for _ in 0..=LAST_OF_THE_ANSWER {
        app.press(area(), typed('j'));
    }
    let folded = app.panel().text();
    for key in "zazc".chars() {
        app.press(area(), typed(key));
    }

    assert!(!app.panel().folds().is_open(RAN));
    assert_eq!(folded, app.panel().text());
}

#[test]
fn every_keystroke_that_would_write_is_refused_and_says_why() {
    for keys in ["x", "dd", "dw", "cw", "p", "P", "D", "C", "J", "r!"] {
        let mut app = reading();
        let before = app.panel().transcript().clone();
        let drawn = app.panel().text();
        let mut said = false;
        for key in keys.chars() {
            app.press(area(), typed(key));
            said |= !app.status().is_empty() && app.status().contains("read-only");
            assert_eq!(
                &before,
                app.panel().transcript(),
                "`{keys}` changed what was said"
            );
            assert_eq!(drawn, app.panel().text(), "`{keys}` changed what is drawn");
        }

        assert!(said, "`{keys}` was dropped without a word");
    }
}

#[test]
fn every_keystroke_that_would_write_changes_the_transcript_once_the_policy_is_taken_out() {
    for keys in ["x", "dd", "dw", "cw", "p", "P", "D", "C", "J", "r!"] {
        let mut free = Panel::new(said()).governed_by(Policy::Unrestricted);
        let before = free.text();
        free.press_all("yyj".chars().map(typed))
            .expect("a yank and a motion run");
        free.press_all(keys.chars().map(typed))
            .expect("the keys run where nothing refuses them");

        assert_ne!(
            before,
            free.text(),
            "`{keys}` writes nothing, so refusing it proves nothing"
        );
    }
}

#[test]
fn a_paste_while_the_transcript_has_the_keys_reaches_neither_it_nor_the_file() {
    let mut app = reading();
    let drawn = app.panel().text();
    app.handle(
        area(),
        &Event::Paste(Paste {
            text: "pasted".to_owned(),
            dropped_keys: 0,
        }),
    );

    assert_eq!(drawn, app.panel().text());
    assert!(
        app.status().contains("read-only"),
        "a paste into the transcript was swallowed: {:?}",
        app.status()
    );

    app.press(area(), control('t'));
    assert_eq!(Focus::Text, app.focus());
    assert_eq!(FILE, app.text().text());
}

#[test]
fn the_workspace_holds_one_machine_that_reads_a_sequence_of_typed_keys() {
    for shape in DISPATCHING {
        let found = holding(shape);
        let expected: Vec<String> = if "fn input_key(" == shape {
            vec![DISPATCHER.to_owned()]
        } else {
            Vec::new()
        };

        assert_eq!(
            expected, found,
            "`{shape}` is a second reader of typed keys beside the one table"
        );
    }
}

#[test]
fn the_workspace_declares_one_type_that_holds_a_selection() {
    let mut declared = Vec::new();
    for shape in DECLARING {
        declared.extend(holding(shape));
    }

    assert_eq!(vec![SELECTION.to_owned()], declared);
}

#[test]
fn the_scan_for_a_second_reader_and_a_second_selection_reads_every_crate() {
    let sources = sources();

    assert!(
        sources.len() > 25,
        "the scan read {} sources of the workspace",
        sources.len()
    );
    for named in [DISPATCHER, SELECTION, "crates/vbc-layout/src/anchor.rs"] {
        assert!(
            sources.iter().any(|source| named == source),
            "the scan did not read {named}"
        );
    }
}

#[test]
fn the_panel_draws_the_blocks_that_were_said_and_the_row_a_closed_fold_is() {
    let panel = Panel::new(said());
    let drawn = panel.rows(Placed::new(0, 0), usize::from(ROWS));
    let rows: Vec<String> = drawn
        .iter()
        .map(|row| match row {
            Drawn::Summary(summary) => summary.text().to_owned(),
            Drawn::Body { row, .. } => row.styled().cells(),
        })
        .collect();

    assert_eq!(ASKED, rows[0]);
    assert_eq!("```rust", rows[3]);
    assert!(
        rows[LAST_OF_THE_ANSWER + 1].contains("2 lines"),
        "{:?} is not the row the closed fold is drawn in",
        rows[LAST_OF_THE_ANSWER + 1]
    );
    assert!(
        rows.iter().all(|row| !row.contains("semicolon")),
        "the closed fold drew what it hid"
    );
    assert!(
        rows.iter().any(|row| row.starts_with('+')),
        "the diff was not drawn"
    );
}

#[test]
fn a_window_deep_in_the_panel_asks_the_allocator_for_what_one_off_its_top_asks_for() {
    let short = panelled(SHORT);
    let long = panelled(LONG);
    let (_, off_the_top) = counted(|| long.rows(Placed::new(1, 0), WINDOW));
    let (_, deep) = counted(|| long.rows(Placed::new(1, DEEP), WINDOW));
    let (rows, over_the_short) = counted(|| short.rows(Placed::new(1, 0), WINDOW));

    assert_eq!(WINDOW, rows.len());
    assert_eq!(
        off_the_top, deep,
        "a window at row {DEEP} of the panel cost more than one off its top"
    );
    assert_eq!(
        over_the_short, deep,
        "a window of the panel cost what the block below it holds"
    );
}

#[test]
fn a_window_deep_in_the_panel_draws_the_rows_it_was_asked_for() {
    let long = panelled(LONG);
    let deep = long.rows(Placed::new(1, DEEP), WINDOW);
    let rows: Vec<String> = deep
        .iter()
        .map(|row| match row {
            Drawn::Summary(summary) => summary.text().to_owned(),
            Drawn::Body { row, .. } => row.styled().cells(),
        })
        .collect();

    assert_eq!(WINDOW, rows.len());
    assert_eq!(line(DEEP), rows[0]);
    assert_eq!(line(DEEP + WINDOW - 1), rows[WINDOW - 1]);
}

/// The allocator that records what a measurement asks for.
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

/// Runs `measure` and reads off everything it asked the allocator for.
///
/// # Returns
///
/// What it returned, and the bytes it asked for paired with the number of calls it asked in.
fn counted<ValueType>(measure: impl FnOnce() -> ValueType) -> (ValueType, (usize, usize)) {
    ASKED_FOR.with(|asked| asked.set(0));
    CALLS.with(|calls| calls.set(0));
    let value = measure();

    (value, (ASKED_FOR.with(Cell::get), CALLS.with(Cell::get)))
}

/// # Returns
///
/// The application a reader types at, with the transcript panel already reached by `<C-T>`.
fn reading() -> App {
    let mut app = App::new(Buffer::from_text(FILE))
        .with_status(true)
        .with_transcript(said());
    app.press(area(), control('t'));
    assert_eq!(Focus::Transcript, app.focus(), "`<C-T>` reached no panel");

    app
}

/// # Returns
///
/// What the row `row` of `cells` was drawn with, trailing blanks left off.
fn drawn_row(cells: &Cells, row: u16) -> String {
    let drawn: String = (0..COLUMNS)
        .map(|column| cells[(column, row)].symbol().to_owned())
        .collect();

    drawn.trim_end().to_owned()
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
/// The exchange every case is driven over: a question, the answer fencing [`CODE`], what a tool
/// answered, and the diff the answer wrote.
fn said() -> Transcript {
    [
        Block::new(Kind::Message(Role::User), ASKED.to_owned()),
        Block::new(Kind::Message(Role::Assistant), ANSWERED.to_owned()),
        Block::new(Kind::ToolResult, WROTE.to_owned()),
        Block::diff(PATH.to_owned(), BEFORE, AFTER),
    ]
    .into_iter()
    .collect()
}

/// # Returns
///
/// A panel over a message and a block of `count` lines of the printable ASCII, whose rows a width
/// divides out of a length.
fn panelled(count: usize) -> Panel {
    let source: Vec<String> = (0..count).map(line).collect();

    Panel::new(
        [
            Block::new(Kind::Message(Role::User), ASKED.to_owned()),
            Block::new(Kind::Code { language: None }, source.join("\n")),
        ]
        .into_iter()
        .collect(),
    )
}

/// # Returns
///
/// The line numbered `index` of a measured block, which is short enough to be drawn in one row
/// and the same length as every other, so that what a window asks the allocator for cannot follow
/// the length of the lines it happened to draw.
fn line(index: usize) -> String {
    format!("let line{index:06} = {index:06};")
}

/// # Returns
///
/// Every source that ships, of every crate of the workspace, named by the path the workspace
/// holds it at. A crate's tests are left out because a test that names one of these shapes in a
/// string is naming it in order to look for it, and because a reader nothing ships cannot be one
/// a keystroke reaches.
fn sources() -> Vec<String> {
    let mut found = Vec::new();
    let crates = workspace().join("crates");
    let Ok(entries) = fs::read_dir(&crates) else {
        return found;
    };
    for entry in entries.flatten() {
        read(&entry.path().join("src"), &workspace(), &mut found);
    }
    found.sort();

    found
}

/// Reads every Rust source under `directory` into `found`, named relative to `root`.
fn read(directory: &Path, root: &Path, found: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            read(&path, root, found);
        } else if Some("rs") == path.extension().and_then(|extension| extension.to_str()) {
            if let Ok(named) = path.strip_prefix(root) {
                found.push(named.to_string_lossy().replace('\\', "/"));
            }
        }
    }
}

/// # Returns
///
/// Every source of the workspace holding `shape`, named by the path the workspace holds it at.
fn holding(shape: &str) -> Vec<String> {
    sources()
        .into_iter()
        .filter(|source| {
            fs::read_to_string(workspace().join(source)).is_ok_and(|held| held.contains(shape))
        })
        .collect()
}

/// # Returns
///
/// The root of the workspace, which is two directories above this crate's own.
///
/// # Panics
///
/// Panics if this crate is not a crate of a workspace, which it is.
fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("this crate sits two directories below the workspace")
        .to_path_buf()
}

/// Writes `old` to `path` in a directory of its own, hands `written` to `git apply` there, and
/// reads back what the file holds afterwards.
///
/// # Returns
///
/// What the file holds once the patch was applied, and `None` where `git apply` refused it.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`tempfile::tempdir`]'s return values on failure.
/// * Forwards [`std::fs::create_dir_all`]'s return values on failure.
/// * Forwards [`std::fs::write`]'s return values on failure.
/// * Forwards [`std::process::Command::output`]'s return values on failure.
/// * Forwards [`std::fs::read_to_string`]'s return values on failure.
///
/// # Panics
///
/// Panics if `git apply` had to move a hunk away from the line the patch numbered it at.
fn applied(path: &str, old: &str, written: &str) -> Result<Option<String>> {
    let directory = tempfile::tempdir()?;
    let file = directory.path().join(path);
    fs::create_dir_all(file.parent().expect("a file in a directory has a parent"))?;
    fs::write(&file, old)?;
    fs::write(directory.path().join(PATCH_FILE), written)?;

    let ran = Command::new("git")
        .arg("apply")
        .arg("--verbose")
        .arg(PATCH_FILE)
        .current_dir(directory.path())
        .output()?;
    if !ran.status.success() {
        return Ok(None);
    }

    let reported = format!(
        "{}{}",
        String::from_utf8_lossy(&ran.stdout),
        String::from_utf8_lossy(&ran.stderr)
    );
    assert!(
        !reported.contains(MOVED),
        "`git apply` moved a hunk to make the patch fit: {reported}"
    );

    Ok(Some(fs::read_to_string(&file)?))
}
