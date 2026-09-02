//! What ratatui does with a cell a double-width grapheme claims, and what the renderer does about
//! it.
//!
//! The buffer's own semantics are recorded here first, because the renderer is built on them. A
//! grapheme wider than one cell is kept whole in the first cell it claims and the cells beside it
//! are blanked; the buffer's view, the terminal diff, and `TestBackend`'s grid all read the claim
//! back off the first cell by measuring its symbol. Nothing enforces the claim: a write to a
//! claimed cell is accepted, and the buffer then describes a screen no terminal can draw, which
//! `TestBackend` reports as `Hidden by multi-width symbols`. The measurement is `unicode-width`
//! alone, so a buffer left to measure for itself disagrees with the layout wherever `'ambiwidth'`
//! or a halfwidth sound mark does -- which is why the renderer places every grapheme at the column
//! the layout measured and marks the cell with the width the layout gave it.

mod screen;

use std::num::{NonZeroU16, NonZeroUsize};

use ratatui::backend::TestBackend;
use ratatui::buffer::{Buffer, CellDiffOption, CellWidth};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::Terminal;
use vbc_editor::render::{cursor_cell, Renderer, WIDE_CHARACTER_MARKER};
use vbc_layout::line::{self, DisplayRow, Options};
use vbc_layout::width::{AmbiWidth, Metrics};

use crate::screen::{broken_claims, BLANK};

/// A family emoji joined by zero-width joiners, which is five code points and one grapheme.
const ZWJ_FAMILY: &str = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";

/// A halfwidth katakana letter followed by a halfwidth voiced sound mark, which `unicode-width`
/// measures as one column and ratatui as two.
const HALFWIDTH_DAKUTEN: &str = "\u{FF76}\u{FF9E}";

#[test]
fn ratatui_keeps_a_double_width_grapheme_in_the_first_cell_it_claims() {
    let mut buffer = Buffer::empty(Rect::new(0, 0, 10, 1));
    buffer.set_string(0, 0, "中文abc", Style::new());

    assert_eq!(vec!["中| |文| |a|b|c| | | "], grid(&buffer));
    assert_eq!(2, "中".cell_width());
}

#[test]
fn ratatui_lets_a_claimed_cell_be_written_and_reports_the_buffer_as_corrupt() {
    let mut buffer = Buffer::empty(Rect::new(0, 0, 4, 1));
    buffer.set_string(0, 0, "中", Style::new());
    buffer[(1, 0)].set_symbol("X");

    assert_eq!(vec!["中|X| | "], grid(&buffer));
    assert!(
        format!("{buffer:?}").contains("hidden by multi-width symbols"),
        "the buffer reports the claimed cell it let through: {buffer:?}"
    );
}

#[test]
fn a_rendered_cjk_row_matches_the_expected_grid() {
    let terminal = draw("中文abc", 10, Metrics::default(), &Options::new());

    assert_eq!(
        vec!["中| |文| |a|b|c| | | "],
        grid(terminal.backend().buffer())
    );
    assert_eq!(
        "\"中文abc   \" Hidden by multi-width symbols: [(1, \" \"), (3, \" \")]\n",
        terminal.backend().to_string()
    );
}

#[test]
fn a_rendered_zwj_cluster_fills_one_cell_and_claims_the_next() {
    let line = format!("{ZWJ_FAMILY}ab");
    let terminal = draw(&line, 8, Metrics::default(), &Options::new());

    assert_eq!(
        vec![format!("{ZWJ_FAMILY}| |a|b| | | | ")],
        grid(terminal.backend().buffer())
    );
    assert_eq!(
        format!("\"{ZWJ_FAMILY}ab    \" Hidden by multi-width symbols: [(1, \" \")]\n"),
        terminal.backend().to_string()
    );
}

#[test]
fn a_rendered_row_leaves_every_claimed_cell_blank() {
    let buffer = rendered("中a文b", 10, Metrics::default(), &Options::new());

    assert_eq!(BLANK, buffer[(1, 0)].symbol());
    assert_eq!(BLANK, buffer[(4, 0)].symbol());
    assert_eq!(Vec::<String>::new(), broken_claims(&buffer));
}

#[test]
fn a_grapheme_the_layout_measures_wider_than_ratatui_claims_the_layout_s_cells() {
    let metrics = Metrics::new(
        AmbiWidth::Double,
        NonZeroUsize::new(8).expect("the tab stop is not zero"),
    );
    let terminal = draw("αβc", 8, metrics, &Options::new());
    let buffer = rendered("αβc", 8, metrics, &Options::new());

    assert_eq!(1, "α".cell_width());
    assert_eq!(vec!["α| |β| |c| | | "], grid(terminal.backend().buffer()));
    assert_eq!(
        CellDiffOption::ForcedWidth(two()),
        buffer[(0, 0)].diff_option
    );
    assert_eq!(Vec::<String>::new(), broken_claims(&buffer));
}

#[test]
fn a_grapheme_the_layout_measures_narrower_than_ratatui_claims_one_cell() {
    let line = format!("{HALFWIDTH_DAKUTEN}ab");
    let terminal = draw(&line, 8, Metrics::default(), &Options::new());
    let buffer = rendered(&line, 8, Metrics::default(), &Options::new());

    assert_eq!(2, HALFWIDTH_DAKUTEN.cell_width());
    assert_eq!(
        vec![format!("{HALFWIDTH_DAKUTEN}|a|b| | | | | ")],
        grid(terminal.backend().buffer())
    );
    assert_eq!(
        CellDiffOption::ForcedWidth(one()),
        buffer[(0, 0)].diff_option
    );
    assert_eq!(Vec::<String>::new(), broken_claims(&buffer));
}

#[test]
fn the_cursor_rests_on_the_first_cell_of_a_wide_grapheme() {
    let rows = rows_of("中文abc", 10, Metrics::default(), &Options::new());
    let columns: Vec<Option<u16>> = (0..=rows[0].end())
        .map(|grapheme| {
            cursor_cell(Rect::new(0, 0, 10, 1), 0, &rows[0], grapheme).map(|cell| cell.x)
        })
        .collect();

    assert_eq!(
        vec![Some(0), Some(2), Some(4), Some(5), Some(6), Some(7)],
        columns
    );
}

#[test]
fn the_cursor_rests_in_the_cell_after_a_line_that_ends_in_a_wide_grapheme() {
    let rows = rows_of("ab中", 10, Metrics::default(), &Options::new());
    let terminal = place_cursor(10, 1, &rows[0], rows[0].end());

    assert_eq!(4, terminal.backend().cursor_position().x);
}

#[test]
fn a_row_that_fills_its_last_cell_draws_no_cell_past_its_text() {
    let rows = rows_of("abcde", 5, Metrics::default(), &Options::new());
    let row = &rows[0];

    assert_eq!(5, row.width());
    assert_eq!(
        Some(4),
        cursor_cell(Rect::new(0, 0, 5, 1), 0, row, 4).map(|cell| cell.x)
    );
    assert_eq!(None, cursor_cell(Rect::new(0, 0, 5, 1), 0, row, row.end()));
}

#[test]
fn a_row_draws_no_cursor_for_a_grapheme_of_another_row() {
    let rows = rows_of("abcdefgh", 4, Metrics::default(), &Options::new());
    let area = Rect::new(0, 0, 4, 2);

    assert_eq!(2, rows.len());
    assert_eq!(None, cursor_cell(area, 0, &rows[0], 4));
    assert_eq!(None, cursor_cell(area, 1, &rows[1], 3));
}

#[test]
fn a_wide_grapheme_that_does_not_fit_leaves_the_cells_it_would_have_split_marked() {
    let terminal = draw("abcd中文", 5, Metrics::default(), &Options::new());

    assert_eq!(
        vec![
            format!("a|b|c|d|{WIDE_CHARACTER_MARKER}"),
            "中| |文| | ".to_owned(),
        ],
        grid(terminal.backend().buffer())
    );
}

#[test]
fn a_grapheme_wider_than_the_whole_area_is_not_drawn_at_all() {
    let buffer = rendered("中", 1, Metrics::default(), &Options::new());

    assert_eq!(
        2,
        rows_of("中", 1, Metrics::default(), &Options::new())[0].width()
    );
    assert_eq!(vec![" "], grid(&buffer));
    assert_eq!(Vec::<String>::new(), broken_claims(&buffer));
}

#[test]
fn a_row_drawn_into_a_narrower_area_splits_no_grapheme() {
    let metrics = Metrics::default();
    let renderer = Renderer::new(metrics);
    let area = Rect::new(0, 0, 3, 1);
    let mut buffer = Buffer::empty(area);
    let rows = rows_of("ab中cd", 10, metrics, &Options::new());

    renderer.draw_row(&mut buffer, area, 0, &rows[0], None);

    assert_eq!(vec!["a|b| "], grid(&buffer));
    assert_eq!(Vec::<String>::new(), broken_claims(&buffer));
}

#[test]
fn a_tab_wider_than_the_whole_row_is_not_drawn_at_all() {
    let metrics = Metrics::new(
        AmbiWidth::Single,
        NonZeroUsize::new(8).expect("the tab stop is not zero"),
    );
    let rows = rows_of("\tx", 4, metrics, &Options::new());
    let terminal = draw("\tx", 4, metrics, &Options::new());

    assert_eq!(8, rows[0].width());
    assert_eq!(
        vec![" | | | ", "x| | | "],
        grid(terminal.backend().buffer())
    );
}

#[test]
fn a_redrawn_row_keeps_nothing_of_the_row_it_replaced() {
    let metrics = Metrics::default();
    let renderer = Renderer::new(metrics);
    let area = Rect::new(0, 0, 8, 1);
    let mut buffer = Buffer::empty(area);
    let wide = rows_of("中文中文", 8, metrics, &Options::new());
    let narrow = rows_of("ab", 8, metrics, &Options::new());

    renderer.draw_row(&mut buffer, area, 0, &wide[0], None);
    renderer.draw_row(&mut buffer, area, 0, &narrow[0], None);

    assert_eq!(vec!["a|b| | | | | | "], grid(&buffer));
    assert_eq!(CellDiffOption::None, buffer[(0, 0)].diff_option);
}

#[test]
fn a_continuation_row_is_drawn_behind_the_decoration_the_layout_measured() {
    let options = Options::new()
        .with_break_indent(true)
        .with_break_indent_min(4)
        .with_show_break(">>".to_owned());
    let terminal = draw("    abcdefghij", 12, Metrics::default(), &options);

    assert_eq!(
        vec![" | | | |a|b|c|d|e|f|g|h", " | | | |>|>|i|j| | | | ",],
        grid(terminal.backend().buffer())
    );
}

#[test]
fn a_tab_in_a_continuation_marker_is_drawn_as_the_blanks_it_advances_by() {
    let options = Options::new().with_show_break("\t".to_owned());
    let terminal = draw("abcdefghijklmnop", 12, Metrics::default(), &options);

    assert_eq!(
        vec![
            "a|b|c|d|e|f|g|h|i|j|k|l".to_owned(),
            " | | | | | | | |m|n|o|p".to_owned(),
        ],
        grid(terminal.backend().buffer())
    );
}

#[test]
fn every_cell_a_row_owns_is_drawn_in_the_renderer_s_style() {
    let metrics = Metrics::default();
    let renderer = Renderer::new(metrics).with_style(Style::new().fg(Color::Red));
    let area = Rect::new(0, 0, 4, 1);
    let mut buffer = Buffer::empty(area);
    let rows = rows_of("ab", 4, metrics, &Options::new());

    renderer.draw_row(&mut buffer, area, 0, &rows[0], None);

    assert_eq!(Color::Red, buffer[(0, 0)].fg);
    assert_eq!(Color::Red, buffer[(3, 0)].fg);
}

#[test]
fn a_line_stops_at_the_bottom_of_the_area() {
    let metrics = Metrics::default();
    let renderer = Renderer::new(metrics);
    let area = Rect::new(0, 0, 4, 2);
    let mut buffer = Buffer::empty(area);
    let rows = rows_of("abcdefghijkl", 4, metrics, &Options::new());

    assert_eq!(3, rows.len());
    assert_eq!(2, renderer.draw_line(&mut buffer, area, 0, &rows));
    assert_eq!(vec!["a|b|c|d", "e|f|g|h"], grid(&buffer));
}

#[test]
fn a_row_is_drawn_at_the_offset_of_the_area_it_is_given() {
    let metrics = Metrics::default();
    let renderer = Renderer::new(metrics);
    let area = Rect::new(2, 1, 4, 2);
    let mut buffer = Buffer::empty(Rect::new(0, 0, 8, 3));
    let rows = rows_of("wxyz", 4, metrics, &Options::new());

    renderer.draw_line(&mut buffer, area, 1, &rows);

    assert_eq!(
        vec![" | | | | | | | ", " | | | | | | | ", " | |w|x|y|z| | ",],
        grid(&buffer)
    );
}

/// # Returns
///
/// The rows `line` lays out into when it is drawn `width` columns wide.
///
/// # Panics
///
/// Panics if `width` is zero.
fn rows_of(line: &str, width: usize, metrics: Metrics, options: &Options) -> Vec<DisplayRow> {
    line::lay_out(
        line,
        NonZeroUsize::new(width).expect("a test's width is not zero"),
        metrics,
        options,
    )
}

/// Lays a line out and draws every row of it into a terminal of the same width.
///
/// # Returns
///
/// The terminal the rows were drawn into.
///
/// # Panics
///
/// Panics if the terminal cannot be built or drawn to.
fn draw(line: &str, width: u16, metrics: Metrics, options: &Options) -> Terminal<TestBackend> {
    let rows = rows_of(line, usize::from(width), metrics, options);
    let height = u16::try_from(rows.len()).expect("a test's rows fit on a screen");
    let renderer = Renderer::new(metrics);
    let mut terminal =
        Terminal::new(TestBackend::new(width, height)).expect("a test terminal is built");
    terminal
        .draw(|frame| {
            let area = frame.area();
            renderer.draw_line(frame.buffer_mut(), area, 0, &rows);
        })
        .expect("a test frame is drawn");

    terminal
}

/// Lays a line out and draws every row of it into a buffer of the same width, which is the buffer
/// the renderer itself filled rather than the one a terminal diff has already been through.
///
/// # Returns
///
/// The buffer the rows were drawn into.
///
/// # Panics
///
/// Panics if the line needs more rows than a screen holds.
fn rendered(line: &str, width: u16, metrics: Metrics, options: &Options) -> Buffer {
    let rows = rows_of(line, usize::from(width), metrics, options);
    let height = u16::try_from(rows.len()).expect("a test's rows fit on a screen");
    let area = Rect::new(0, 0, width, height);
    let mut buffer = Buffer::empty(area);
    Renderer::new(metrics).draw_line(&mut buffer, area, 0, &rows);

    buffer
}

/// Draws one row into a terminal and puts the cursor on one of its graphemes.
///
/// # Returns
///
/// The terminal the row was drawn into.
///
/// # Panics
///
/// Panics if the terminal cannot be built or drawn to, or if the row does not draw `grapheme`.
fn place_cursor(
    width: u16,
    height: u16,
    row: &DisplayRow,
    grapheme: usize,
) -> Terminal<TestBackend> {
    let renderer = Renderer::new(Metrics::default());
    let mut terminal =
        Terminal::new(TestBackend::new(width, height)).expect("a test terminal is built");
    terminal
        .draw(|frame| {
            let area = frame.area();
            renderer.draw_row(frame.buffer_mut(), area, 0, row, None);
            frame.set_cursor_position(
                cursor_cell(area, 0, row, grapheme).expect("the row draws the cursor's grapheme"),
            );
        })
        .expect("a test frame is drawn");

    terminal
}

/// # Returns
///
/// One string per row of `buffer`, holding the symbol of every cell of that row separated by
/// pipes, so that a cell a wider grapheme beside it has claimed is read rather than hidden.
fn grid(buffer: &Buffer) -> Vec<String> {
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<Vec<&str>>()
                .join("|")
        })
        .collect()
}

/// # Returns
///
/// A forced width of one cell.
fn one() -> NonZeroU16 {
    NonZeroU16::new(1).expect("one is not zero")
}

/// # Returns
///
/// A forced width of two cells.
fn two() -> NonZeroU16 {
    NonZeroU16::new(2).expect("two is not zero")
}
