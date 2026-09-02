//! The whole path from a buffer to a scrolled frame, drawn into a terminal and read back cell by
//! cell.
//!
//! Everything below has been checked on its own before now: a line lays out, a viewport scrolls, a
//! gutter numbers, a renderer fills cells. What was never checked is that they compose, and the
//! only thing that can check it is a frame -- a buffer taller than the window, held at an offset
//! the window did not start at, wrapped into a width that breaks its double-width text, and read
//! back off a terminal.
//!
//! The frames are read as symbols per cell rather than as lines of text, because a row that drew
//! its double-width graphemes in the wrong cells draws the same line of text as one that drew them
//! in the right ones. The cursor is read off the terminal for the same reason: the cell a terminal
//! was told to rest it in is the only thing that says the cursor followed the scroll.
//!
//! A scroll is asserted as a relation between two frames rather than as a second grid: the frame
//! after `CTRL-D` has to be the frame before it moved up by the rows a half page is, which a
//! renderer that redrew the same screen or scrolled by logical lines cannot manage.

mod screen;

use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer as Cells;
use ratatui::layout::{Position, Rect};
use ratatui::Terminal;
use vbc_editor::app::App;
use vbc_editor::screen::Screen;
use vbc_layout::buffer::Buffer;
use vbc_layout::position::LogicalPosition;
use vbc_layout::viewport::Command;

use crate::screen::broken_claims;

/// The columns and rows the fixture is drawn into: four columns of gutter and eight of text, and a
/// window far shorter than the text it shows.
const WIDTH: u16 = 12;
const HEIGHT: u16 = 5;

/// The columns the gutter occupies, which is vim's own `'numberwidth'` and is what the text is
/// drawn to the right of.
const GUTTER: u16 = 4;

/// The lines the fixture holds, which is more lines than the window has rows and far more display
/// rows than that.
const LINES: usize = 9;

/// The eight characters every line of the fixture ends in, each of them two columns wide, so that
/// a line is nine characters and eighteen columns and wraps into three rows of four, four and one.
const TAIL: &str = "行中文字段落文本";

/// The character each line of the fixture begins with, which is what tells one line's rows from
/// another's.
const HEADS: [&str; LINES] = ["一", "二", "三", "四", "五", "六", "七", "八", "九"];

/// The two texts a frame is timed over, to check that drawing one costs the window rather than the
/// text behind it.
const SMALL_TEXT: usize = 100;
const LARGE_TEXT: usize = 50_000;

/// The number of frames one timed run draws.
const FRAMES: usize = 64;

/// The number of runs a measurement takes the fastest of, which is what keeps a machine's own
/// noise out of the ratio.
const RUNS: usize = 9;

/// The factor by which the larger text is allowed to cost more than the smaller one, and the
/// smaller more than the larger. A frame that laid the whole text out is hundreds of times slower
/// over the larger one.
const TOLERANCE: u32 = 4;

/// Validation 1: a buffer taller than its window, held at a non-zero vertical offset, is drawn as
/// wrapped rows with the gutter numbering its logical lines and the cursor on the cell the layout
/// put it in.
#[test]
fn a_scrolled_frame_of_wrapped_cjk_draws_its_gutter_rows_and_cursor() -> Result<()> {
    let mut app = App::new(fixture());
    app.scroll(area(), Command::HalfPageDown)?;

    assert_eq!(0, app.viewport().anchor());
    assert_eq!(
        2,
        app.viewport().vertical_offset(),
        "the frame is drawn from the top of a line, so the viewport is never exercised"
    );
    assert_eq!(
        LogicalPosition {
            line: 0,
            grapheme: 8
        },
        app.cursor()
    );

    let mut terminal = Terminal::new(TestBackend::new(WIDTH, HEIGHT))?;
    terminal.draw(|frame| app.render(frame))?;

    assert_eq!(
        vec![
            " | | | |本| | | | | | | ",
            " | |2| |二| |行| |中| |文| ",
            " | | | |字| |段| |落| |文| ",
            " | | | |本| | | | | | | ",
            " | |3| |三| |行| |中| |文| ",
        ],
        grid(terminal.backend().buffer())
    );
    assert_eq!(
        Vec::<String>::new(),
        broken_claims(terminal.backend().buffer())
    );
    assert_eq!(
        Position { x: GUTTER, y: 0 },
        terminal.get_cursor_position()?
    );

    Ok(())
}

/// Validation 3: a scroll moves the frame rather than redrawing it, so the frame after a half page
/// is the frame before it with its top rows dropped.
#[test]
fn scrolling_moves_the_frame_up_by_the_rows_the_command_asks_for() -> Result<()> {
    const HALF_PAGE: usize = HEIGHT as usize / 2;

    let mut app = App::new(fixture());
    let mut terminal = Terminal::new(TestBackend::new(WIDTH, HEIGHT))?;
    app.scroll(area(), Command::HalfPageDown)?;
    terminal.draw(|frame| app.render(frame))?;
    let before = grid(terminal.backend().buffer());

    app.scroll(area(), Command::HalfPageDown)?;
    terminal.draw(|frame| app.render(frame))?;
    let after = grid(terminal.backend().buffer());

    assert_ne!(before, after);
    assert_eq!(
        before[HALF_PAGE..],
        after[..before.len() - HALF_PAGE],
        "the rows the scroll kept are not the rows it drew"
    );
    assert_eq!(1, app.viewport().anchor());
    assert_eq!(1, app.viewport().vertical_offset());

    Ok(())
}

/// The frame the window shows is the frame the screen was walked for, so a rendered frame and the
/// rows behind it never fall out of step.
#[test]
fn the_frame_draws_the_rows_the_screen_walked() -> Result<()> {
    let mut app = App::new(fixture());
    app.scroll(area(), Command::HalfPageDown)?;
    let geometry = app
        .geometry(area())
        .ok_or_else(|| anyhow::anyhow!("the fixture's area draws text"))?;
    let screen = Screen::of(app.text(), &app.viewport(), app.cursor(), &geometry);

    assert_eq!(usize::from(HEIGHT), screen.rows().len());
    assert_eq!(
        vec![(0, 8), (1, 0), (1, 4), (1, 8), (2, 0)],
        screen
            .rows()
            .iter()
            .map(|row| (row.line(), row.start()))
            .collect::<Vec<(usize, usize)>>()
    );
    assert_eq!(Some(0), screen.cursor_row());

    Ok(())
}

/// A window taller than the text it shows draws what there is and leaves the rest of its rows
/// blank, rather than running off the end of the buffer.
#[test]
fn a_window_taller_than_its_text_draws_the_rows_there_are() -> Result<()> {
    let app = App::new(Buffer::from_text("短"));
    let mut terminal = Terminal::new(TestBackend::new(WIDTH, HEIGHT))?;
    terminal.draw(|frame| app.render(frame))?;

    assert_eq!(
        vec![
            " | |1| |短| | | | | | | ",
            " | | | | | | | | | | | ",
            " | | | | | | | | | | | ",
            " | | | | | | | | | | | ",
            " | | | | | | | | | | | ",
        ],
        grid(terminal.backend().buffer())
    );

    Ok(())
}

/// The bottom row of a window is drawn as vim draws it, marker and all, which only a frame that
/// walked one row further than it draws can manage: whether a row ended early is a question about
/// the row below it, and at the bottom of a window that row is off the screen.
#[test]
fn the_bottom_row_of_a_window_is_marked_where_the_row_below_is_too_wide_for_it() -> Result<()> {
    const ODD_WIDTH: u16 = 13;

    let app = App::new(Buffer::from_text("一二三四五"));
    let mut terminal = Terminal::new(TestBackend::new(ODD_WIDTH, 1))?;
    terminal.draw(|frame| app.render(frame))?;

    assert_eq!(
        vec![" | |1| |一| |二| |三| |四| |>"],
        grid(terminal.backend().buffer())
    );

    Ok(())
}

/// The last row of a logical line is never marked, however wide the grapheme the line below it
/// begins with: whether a row ended early is a question about the row that continues its own line,
/// which is what the rows being grouped by line answers.
#[test]
fn the_last_row_of_a_line_is_not_marked_by_the_line_below_it() -> Result<()> {
    const ODD_WIDTH: u16 = 13;

    let app = App::new(Buffer::from_lines(vec![
        "\u{4e00}\u{4e8c}\u{4e09}\u{56db}".to_owned(),
        "\u{4e94}\u{516d}".to_owned(),
    ]));
    let mut terminal = Terminal::new(TestBackend::new(ODD_WIDTH, 2))?;
    terminal.draw(|frame| app.render(frame))?;

    assert_eq!(
        vec![
            " | |1| |\u{4e00}| |\u{4e8c}| |\u{4e09}| |\u{56db}| | ",
            " | |2| |\u{4e94}| |\u{516d}| | | | | | ",
        ],
        grid(terminal.backend().buffer())
    );

    Ok(())
}

/// A frame drawn over an earlier one leaves nothing of it behind, so the rows a shorter text does
/// not reach are the terminal's own blanks rather than the rows that were drawn there before.
#[test]
fn a_frame_drawn_over_a_taller_one_keeps_none_of_its_rows() -> Result<()> {
    let mut cells = Cells::empty(area());
    App::new(fixture()).draw(&mut cells, area());
    App::new(Buffer::from_text("\u{77ed}")).draw(&mut cells, area());

    assert_eq!(
        vec![
            " | |1| |\u{77ed}| | | | | | | ",
            " | | | | | | | | | | | ",
            " | | | | | | | | | | | ",
            " | | | | | | | | | | | ",
            " | | | | | | | | | | | ",
        ],
        grid(&cells)
    );

    Ok(())
}

/// Validation 5: a frame costs the window it draws rather than the text behind it, which is the
/// property the anchor-relative layout was built for and the one a frame that laid the whole
/// buffer out would lose.
#[test]
fn a_frame_costs_the_same_over_a_short_text_and_a_long_one() {
    let small = App::new(paragraphs(SMALL_TEXT));
    let large = App::new(paragraphs(LARGE_TEXT));

    // The first run of each pays for the pages the text was just written to.
    cost(&small);
    cost(&large);
    let small_cost = cost(&small);
    let large_cost = cost(&large);

    assert!(
        large_cost <= small_cost * TOLERANCE,
        "{FRAMES} frames cost {large_cost:?} over {LARGE_TEXT} lines and {small_cost:?} over \
         {SMALL_TEXT}, so a frame's cost follows the text"
    );
    assert!(
        small_cost <= large_cost * TOLERANCE,
        "{FRAMES} frames cost {small_cost:?} over {SMALL_TEXT} lines and {large_cost:?} over \
         {LARGE_TEXT}, so the measurement is not comparing the same work"
    );
}

/// # Returns
///
/// The area a fixture is drawn into.
fn area() -> Rect {
    Rect::new(0, 0, WIDTH, HEIGHT)
}

/// # Returns
///
/// The buffer every frame in this file is drawn from: nine logical lines, each of them nine
/// double-width characters and so three display rows of the fixture's width.
fn fixture() -> Buffer {
    Buffer::from_lines(HEADS.iter().map(|head| format!("{head}{TAIL}")).collect())
}

/// # Returns
///
/// A buffer of `lines` paragraphs, each of them wider than the window a frame is timed in.
fn paragraphs(lines: usize) -> Buffer {
    Buffer::from_lines(
        (0..lines)
            .map(|index| format!("{}{TAIL}{TAIL}{TAIL}", HEADS[index % LINES]))
            .collect(),
    )
}

/// Draws [`FRAMES`] frames of an application into cells of the fixture's own size.
///
/// # Returns
///
/// The fastest of [`RUNS`] runs.
fn cost(app: &App) -> Duration {
    let mut fastest = Duration::MAX;
    for _ in 0..RUNS {
        let mut cells = Cells::empty(area());
        let start = Instant::now();
        for _ in 0..FRAMES {
            std::hint::black_box(app.draw(&mut cells, area()));
        }
        fastest = fastest.min(start.elapsed());
    }

    fastest
}

/// # Returns
///
/// The symbol of every cell of `cells`, one string per row, cells separated by pipes so that a
/// cell a wider grapheme beside it has claimed is read rather than hidden.
fn grid(cells: &Cells) -> Vec<String> {
    (0..cells.area.height)
        .map(|y| {
            (0..cells.area.width)
                .map(|x| cells[(x, y)].symbol())
                .collect::<Vec<&str>>()
                .join("|")
        })
        .collect()
}
