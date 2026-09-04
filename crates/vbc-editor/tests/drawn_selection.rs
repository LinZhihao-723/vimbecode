//! A selection is drawn, in the cells of a terminal, in both of the things the application draws.
//!
//! Everything about selecting was built and none of it was visible. `Selection::highlight` derived
//! the columns to paint and nothing painted them; `v`, `V` and `CTRL-V` moved a range around the
//! text and every frame drew the text exactly as it would have drawn it unselected. A reader could
//! type `viac` at a transcript and had no way to tell what it had taken.
//!
//! So nothing here asserts that a range exists. Every case reads the cells of a `TestBackend` grid
//! and asserts which of them carry the highlight and, as importantly, which of them do not: a
//! painting that reversed the whole window would satisfy "the selection is drawn" and satisfies
//! nothing below.
//!
//! The three shapes are checked because they cut a line differently: charwise takes a run of it,
//! linewise takes the whole of it, and blockwise takes the same virtual columns out of each of the
//! lines it spans. And the charwise case is run across a wrap boundary as well as inside one row,
//! because a selection is a range of the text and the rows are the layout's: a selection that
//! reached the screen through the rows rather than through the text would paint one row of a
//! wrapped line and stop.
//!
//! The panel's case is the one that matters most and the one that is easiest to get wrong. `viac`
//! selects the code a block fenced, in the coordinates of that block's own source, and that source
//! is drawn wherever the folds and the scroll have put it -- so the selection reaches the screen
//! through the rows the panel drew rather than through anything the selection knows. What is
//! asserted is that the rows of the fenced code are the rows that carry the highlight, and that the
//! prose above them does not.

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::Terminal;
use vbc_editor::app::{App, Focus};
use vbc_editor::chat::block::{Block, Kind, Role};
use vbc_editor::chat::transcript::Transcript;
use vbc_editor::engine::typed;
use vbc_editor::gutter::Options as GutterOptions;
use vbc_layout::buffer::Buffer;

/// The window the file's cases are driven in, wide enough that the fixture does not wrap.
const COLUMNS: u16 = 24;
const ROWS: u16 = 6;

/// The window the wrapping case is driven in, narrow enough that its one line takes three rows.
const NARROW: u16 = 8;

/// The text the file's cases are selected out of, whose lines are each shorter than the window.
const FIXTURE: &str = "abcdefg\nhijklmn\nopqrst";

/// The text the wrapping case is selected out of, which is one logical line of three rows.
const LONG: &str = "abcdefghijklmnopqrst";

/// The file the panel case leaves open behind the transcript.
const FILE: &str = "a file the reader left open";

/// What was asked, and the answer fencing the code `iac` takes.
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

/// The window the panel case is driven in, tall enough that nothing the case names is scrolled
/// out of it.
const PANEL_ROWS: u16 = 12;

/// The rows the panel draws the fenced code in, the row of prose above them, and the steps down
/// that put the cursor inside the code.
const CODE_ROWS: [u16; 3] = [4, 5, 6];
const PROSE_ROW: u16 = 1;
const INSIDE_THE_CODE: usize = 5;

/// Validation 4: a charwise selection paints the cells it covers and no others.
#[test]
fn a_charwise_selection_paints_the_cells_it_covers() -> Result<()> {
    let mut app = plain(FIXTURE);
    typing(&mut app, area(COLUMNS), "vll");

    assert_eq!(
        vec![0..3],
        highlighted(&mut app, COLUMNS)?,
        "`vll` painted something other than the three graphemes it covers"
    );

    typing(&mut app, area(COLUMNS), "j");

    assert_eq!(
        vec![0..7, 0..3],
        highlighted(&mut app, COLUMNS)?,
        "a charwise selection carried down a line did not paint to the end of the line above"
    );

    Ok(())
}

/// Validation 4: a linewise selection paints whole lines.
#[test]
fn a_linewise_selection_paints_whole_lines() -> Result<()> {
    let mut app = plain(FIXTURE);
    typing(&mut app, area(COLUMNS), "Vj");

    assert_eq!(
        vec![0..7, 0..7],
        highlighted(&mut app, COLUMNS)?,
        "`Vj` painted something other than the whole of the two lines it covers"
    );

    Ok(())
}

/// Validation 4: a blockwise selection paints the same columns out of each line it spans.
#[test]
fn a_blockwise_selection_paints_the_columns_it_takes_out_of_every_line() -> Result<()> {
    let mut app = plain(FIXTURE);
    typing(&mut app, area(COLUMNS), "ll");
    app.press(area(COLUMNS), control('v'));
    typing(&mut app, area(COLUMNS), "jl");

    assert_eq!(
        vec![2..4, 2..4],
        highlighted(&mut app, COLUMNS)?,
        "a blockwise selection painted something other than the columns it takes"
    );

    Ok(())
}

/// Validation 4: a selection that crosses a wrap boundary paints its part of every row it reaches.
#[test]
fn a_selection_across_a_wrap_boundary_paints_every_row_it_reaches() -> Result<()> {
    let mut app = plain(LONG);
    let area = area(NARROW);

    assert_eq!(
        3,
        drawn_rows(&mut app, NARROW)?,
        "the fixture is not drawn in the rows the case is about"
    );

    typing(&mut app, area, "v");
    for _ in 0..12 {
        app.press(area, typed('l'));
    }

    assert_eq!(
        vec![0..8, 0..5],
        highlighted(&mut app, NARROW)?,
        "a selection crossing a wrap boundary did not paint its part of both rows"
    );

    Ok(())
}

/// Validation 4: the selection a text object takes in the transcript panel is drawn, in the rows
/// the panel drew the block's own source in.
#[test]
fn the_selection_a_text_object_takes_in_the_panel_is_drawn() -> Result<()> {
    let panel = Rect::new(0, 0, COLUMNS, PANEL_ROWS);
    let mut app = reading(panel);
    for _ in 0..INSIDE_THE_CODE {
        app.press(panel, typed('j'));
    }
    typing(&mut app, panel, "viac");

    let painted = painted_rows(&mut app, COLUMNS, PANEL_ROWS)?;

    assert_eq!(
        CODE_ROWS.to_vec(),
        painted,
        "`viac` painted rows other than the ones the fenced code is drawn in"
    );
    assert!(
        !painted.contains(&PROSE_ROW),
        "`viac` painted the prose above the code as well"
    );

    Ok(())
}

/// # Returns
///
/// An application over `text` with no gutter, so that the columns a case asserts are the columns
/// the text is drawn in.
fn plain(text: &str) -> App {
    App::new(Buffer::from_text(text)).with_gutter(GutterOptions::new())
}

/// # Returns
///
/// An application whose keys the transcript panel has, over the exchange the panel case selects
/// out of.
fn reading(area: Rect) -> App {
    let mut app = App::new(Buffer::from_text(FILE)).with_transcript(said());
    app.press(area, control('t'));

    assert_eq!(Focus::Transcript, app.focus(), "`<C-T>` reached no panel");

    app
}

/// # Returns
///
/// The exchange the panel case is driven over.
fn said() -> Transcript {
    [
        Block::new(Kind::Message(Role::User), ASKED.to_owned()),
        Block::new(Kind::Message(Role::Assistant), ANSWERED.to_owned()),
    ]
    .into_iter()
    .collect()
}

/// Types the characters of `keys` at `app`, one at a time.
fn typing(app: &mut App, area: Rect, keys: &str) {
    for character in keys.chars() {
        app.press(area, typed(character));
    }
}

/// Draws a frame of `app` and reads the cells the selection was painted over.
///
/// # Returns
///
/// The columns painted in each row that carries any, top to bottom.
///
/// # Errors
///
/// Returns an error if the frame could not be drawn.
fn highlighted(app: &mut App, columns: u16) -> Result<Vec<std::ops::Range<u16>>> {
    Ok(painted(app, columns, ROWS)?
        .into_iter()
        .map(|(_, run)| run)
        .collect())
}

/// Draws a frame of `app` and reads which rows the selection was painted into.
///
/// # Returns
///
/// The rows carrying any painted cell, top to bottom.
///
/// # Errors
///
/// Returns an error if the frame could not be drawn.
fn painted_rows(app: &mut App, columns: u16, rows: u16) -> Result<Vec<u16>> {
    Ok(painted(app, columns, rows)?
        .into_iter()
        .map(|(row, _)| row)
        .collect())
}

/// Draws a frame of `app` and reads the cells drawn in the selection's own style.
///
/// # Returns
///
/// The row and the columns of each run of painted cells, top to bottom.
///
/// # Errors
///
/// Returns an error if the frame could not be drawn.
fn painted(app: &mut App, columns: u16, rows: u16) -> Result<Vec<(u16, std::ops::Range<u16>)>> {
    let mut terminal = Terminal::new(TestBackend::new(columns, rows))?;
    terminal.draw(|frame| app.render(frame))?;
    let cells = terminal.backend().buffer().clone();

    let mut runs = Vec::new();
    for row in 0..rows {
        let mut start = None;
        for column in 0..=columns {
            let reversed = column < columns
                && cells[(column, row)]
                    .style()
                    .add_modifier
                    .contains(Modifier::REVERSED);
            match (reversed, start) {
                (true, None) => start = Some(column),
                (false, Some(from)) => {
                    runs.push((row, from..column));
                    start = None;
                }
                _ => {}
            }
        }
    }

    Ok(runs)
}

/// # Returns
///
/// The number of rows a frame of `app` draws text in, which is what says a fixture wraps.
///
/// # Errors
///
/// Returns an error if the frame could not be drawn.
fn drawn_rows(app: &mut App, columns: u16) -> Result<usize> {
    let mut terminal = Terminal::new(TestBackend::new(columns, ROWS))?;
    terminal.draw(|frame| app.render(frame))?;
    let cells = terminal.backend().buffer().clone();

    Ok((0..ROWS)
        .filter(|row| (0..columns).any(|column| " " != cells[(column, *row)].symbol()))
        .count())
}

/// # Returns
///
/// The area a case `columns` columns wide is driven in.
fn area(columns: u16) -> Rect {
    Rect::new(0, 0, columns, ROWS)
}

/// # Returns
///
/// The key event a terminal reports when `character` is typed with control held.
fn control(character: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(character), KeyModifiers::CONTROL)
}
