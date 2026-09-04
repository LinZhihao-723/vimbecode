//! Both of the things the application draws follow the cursor they are moved by.
//!
//! A window that does not follow its cursor is an editor that answers keys nobody can see the
//! result of. The file's own window has followed since the frame existed; the transcript panel
//! never did, so `j` past the bottom row of a panel moved a cursor off the screen and every
//! keystroke after it acted somewhere the reader was not looking.
//!
//! What is asserted is the frame rather than a scroll counter: after every `j` and every `k`, the
//! frame is drawn and required to place the cursor, and the row it places it on is required to be
//! inside the window. A panel that scrolled by the wrong amount, or by the right amount in the
//! wrong direction, fails that as surely as one that does not scroll at all -- and a panel that
//! never drew the cursor would have failed it before the first `j`.
//!
//! The fixture is walked the whole way down and the whole way back up, over a closed fold and over
//! a line drawn in more than one row, because those are the two places a follow counted in logical
//! lines rather than in drawn rows goes wrong: a fold is many blocks drawn in one row, and a
//! wrapped line is one line drawn in many.
//!
//! A scroll is not a follow, and the two are checked against each other: `CTRL-E` moves the panel
//! away from its cursor on purpose, and a follow that ran on every key would drag it straight back.

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer as Cells;
use ratatui::layout::{Position, Rect};
use vbc_editor::app::{App, Focus};
use vbc_editor::chat::block::{Block, Kind, Role};
use vbc_editor::chat::transcript::Transcript;
use vbc_editor::engine::typed;
use vbc_layout::buffer::Buffer;

/// The window every case is driven in, which is short enough that the fixtures are taller than it
/// and narrow enough that their longest lines wrap.
const COLUMNS: u16 = 40;
const ROWS: u16 = 6;

/// The keystrokes each walk takes, which is more than the window holds rows.
const STEPS: usize = 14;

/// The file the panel cases leave open behind the transcript, which none of them touches.
const FILE: &str = "a file the reader left open";

/// The text the file's own window is walked down: more lines than the window draws, one of them
/// long enough to be drawn in three rows.
const TALL: &str = "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\nten\neleven\ntwelve";
const WRAPPED: &str = "a line long enough that a forty-column window draws it in three rows \
                       rather than in one of its own";

/// What was asked, and the answer that takes more rows than the window has.
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

/// What a tool answered, which is the block a fold covers.
const WROTE: &str = "   Compiling vimbecode\nerror: a semicolon was expected\nerror: aborting";

/// The edit that answer made: the file, the text it replaced, and the text it wrote.
const PATH: &str = "src/main.rs";
const BEFORE: &str = "fn main() {}\n";
const AFTER: &str = "fn main() {\n    todo!();\n}\n";

/// Validation 3: `j` past the last visible row of the file's window scrolls it, and the cursor
/// stays on the screen the whole way down and the whole way back up.
#[test]
fn the_files_window_follows_its_cursor_down_and_back_up() {
    let mut app = App::new(Buffer::from_text(&format!("{TALL}\n{WRAPPED}\n{TALL}")));
    let mut cells = Cells::empty(area());
    let mut tops = Vec::new();

    for step in 0..STEPS {
        app.press(area(), typed('j'));
        let at = app.draw(&mut cells, area()).unwrap_or_else(|| {
            panic!(
                "the window drew no cursor after {} downward steps",
                step + 1
            )
        });

        assert!(
            at.y < ROWS,
            "the cursor was drawn at row {} of {ROWS}",
            at.y
        );
        tops.push(row(&cells, 0));
    }

    assert!(
        tops.first() != tops.last(),
        "the window never scrolled while the cursor walked {STEPS} lines down it"
    );

    for step in 0..STEPS {
        app.press(area(), typed('k'));
        let at = app
            .draw(&mut cells, area())
            .unwrap_or_else(|| panic!("the window drew no cursor after {} upward steps", step + 1));

        assert!(
            at.y < ROWS,
            "the cursor was drawn at row {} of {ROWS}",
            at.y
        );
    }
}

/// Validation 3: `j` past the last visible row of the transcript panel scrolls it, over a closed
/// fold and over the wrapped rows of a block, and the cursor stays on the screen.
#[test]
fn the_transcript_panel_follows_its_cursor_over_folds_and_wrapped_rows() {
    let mut app = reading();
    let mut cells = Cells::empty(area());
    let mut tops = Vec::new();

    for step in 0..STEPS {
        app.press(area(), typed('j'));
        let at = drawn(&mut app, &mut cells, step + 1);

        assert!(
            at.y < ROWS,
            "the cursor was drawn at row {} of {ROWS}",
            at.y
        );
        tops.push(row(&cells, 0));
    }

    assert!(
        tops.first() != tops.last(),
        "the panel never scrolled while its cursor walked {STEPS} rows down it"
    );

    for step in 0..STEPS {
        app.press(area(), typed('k'));
        let at = drawn(&mut app, &mut cells, step + 1);

        assert!(
            at.y < ROWS,
            "the cursor was drawn at row {} of {ROWS}",
            at.y
        );
    }

    assert_eq!(
        tops.first().cloned(),
        Some(row(&cells, 0)),
        "the panel did not come back to the top it started at"
    );
}

/// Validation 3: a fold the panel is scrolled over is one row, and the cursor is drawn on it.
#[test]
fn the_panel_follows_its_cursor_onto_the_one_row_a_closed_fold_is_drawn_in() {
    let mut app = reading();
    let mut cells = Cells::empty(area());
    let mut folded = None;
    for step in 0..STEPS {
        app.press(area(), typed('j'));
        let at = drawn(&mut app, &mut cells, step + 1);
        if row(&cells, at.y).contains("lines") {
            folded = Some(at);

            break;
        }
    }

    let at = folded.expect("the walk never reached the row a closed fold is drawn in");

    assert!(
        at.y < ROWS,
        "the fold's row was drawn at row {} of {ROWS}",
        at.y
    );
    assert_eq!(
        0, at.x,
        "the cursor on a fold's row is not in its first column"
    );
}

/// A scroll moves the panel away from its cursor on purpose, so nothing follows it back.
#[test]
fn a_scroll_of_the_panel_is_not_undone_by_the_follow() -> Result<()> {
    let mut app = reading();
    let mut cells = Cells::empty(area());
    app.draw(&mut cells, area());
    let top = row(&cells, 0);

    for _ in 0..3 {
        app.press(area(), control('e'));
    }
    app.draw(&mut cells, area());

    assert_ne!(
        top,
        row(&cells, 0),
        "`CTRL-E` scrolled the panel nowhere at all"
    );

    Ok(())
}

/// # Returns
///
/// A frame of `app` with the cursor it drew, which every step of a walk is required to draw.
///
/// # Panics
///
/// Panics if the frame drew no cursor.
fn drawn(app: &mut App, cells: &mut Cells, step: usize) -> Position {
    app.draw(cells, area())
        .unwrap_or_else(|| panic!("the panel drew no cursor after {step} steps"))
}

/// # Returns
///
/// An application whose keys the transcript panel has, over a transcript taller than the window.
fn reading() -> App {
    let mut app = App::new(Buffer::from_text(FILE)).with_transcript(said());
    app.press(area(), control('t'));

    assert_eq!(Focus::Transcript, app.focus(), "`<C-T>` reached no panel");

    app
}

/// # Returns
///
/// The exchange the panel cases are driven over: a question, an answer taller than the window,
/// what a tool answered, and the diff the answer wrote.
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
/// What the row `at` of `cells` was drawn with, trailing blanks left off.
fn row(cells: &Cells, at: u16) -> String {
    let drawn: String = (0..COLUMNS)
        .map(|column| cells[(column, at)].symbol().to_owned())
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
