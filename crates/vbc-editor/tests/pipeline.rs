//! The path a logical line takes from the layout to the cells of a terminal.
//!
//! The path is laid out, styled, numbered and drawn, and the rows the layout produced are handed
//! to each of those in turn without being rebuilt in between: what [`line::lay_out`] returns is
//! what the gutter is drawn beside and what the block styles. That is the whole of what these
//! tests are for. A caller that had to write out a second kind of row to get from one stage to the
//! next would not compile the calls below at all, so the file failing to compile is as much a
//! failure as an assertion that does not hold.
//!
//! What reaches the terminal is checked as cells rather than as symbols alone. A row whose spans
//! were dropped somewhere on the way draws exactly the text a row whose spans were kept draws, so
//! only the styles the cells carry tell the two apart.

mod screen;

use anyhow::Result;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::Terminal;
use std::num::NonZeroUsize;
use vbc_editor::gutter::{Gutter, Options as GutterOptions};
use vbc_editor::render::Renderer;
use vbc_editor::style::{Block, Span, StyledRow};
use vbc_layout::line::{self, DisplayRow, Options as LineOptions};
use vbc_layout::width::{AmbiWidth, Metrics};

use crate::screen::broken_claims;

/// The columns the text of a fixture is wrapped into, narrow enough that a six-grapheme line takes
/// two rows.
const TEXT_WIDTH: usize = 3;

/// The columns the gutter and the text together are drawn into.
const AREA_WIDTH: u16 = 7;

/// The columns a tab advances by in these fixtures.
const TAB_STOP: usize = 8;

/// # Returns
///
/// The style a terminal cell carries once `style` has been drawn onto it, which is `style` laid
/// over the cell's reset colours rather than `style` on its own.
fn painted(style: Style) -> Style {
    Style::new().fg(Color::Reset).bg(Color::Reset).patch(style)
}

/// # Returns
///
/// The metrics the fixtures are measured under.
fn metrics() -> Metrics {
    Metrics::new(
        AmbiWidth::Single,
        NonZeroUsize::new(TAB_STOP).expect("a fixture advances tabs by eight columns"),
    )
}

/// # Returns
///
/// One of the two styles the fixtures tell apart.
fn red() -> Style {
    Style::new().fg(Color::Red).add_modifier(Modifier::BOLD)
}

/// # Returns
///
/// The other of the two styles the fixtures tell apart.
fn blue() -> Style {
    Style::new().bg(Color::Blue)
}

/// # Returns
///
/// The rows the logical line `line` of `block`, whose text is `text`, lays out into `width`
/// columns.
///
/// # Panics
///
/// Panics if `width` is zero.
fn lay_out(line: usize, text: &str, width: usize) -> Vec<DisplayRow> {
    line::lay_out(
        line,
        text,
        NonZeroUsize::new(width).expect("a fixture is drawn in at least one column"),
        metrics(),
        &LineOptions::new(),
    )
}

/// # Returns
///
/// The symbol of every cell of `terminal`, one string per row, cells separated by pipes so that a
/// cell a wider grapheme beside it has claimed is read rather than hidden.
fn symbols(terminal: &Terminal<TestBackend>) -> Vec<String> {
    let buffer = terminal.backend().buffer();
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
/// The style of every cell of row `y` of `terminal`, left to right.
fn styles(terminal: &Terminal<TestBackend>, y: u16) -> Vec<Style> {
    let buffer = terminal.backend().buffer();
    (0..buffer.area.width)
        .map(|x| buffer[(x, y)].style())
        .collect()
}

/// Validation 1: one call path lays a line out, styles it, numbers it and draws it, with the rows
/// the layout produced handed to each stage as they came.
#[test]
fn a_line_is_laid_out_styled_numbered_and_drawn_in_one_call_path() -> Result<()> {
    let block = Block::with_spans(
        "abcdef\nghi".to_owned(),
        vec![Span::new(2..4, red()), Span::new(7..9, blue())],
    );
    let gutter_options = GutterOptions::new().with_number(true).with_min_width(3);
    let renderer = Renderer::new(metrics());
    let line_count = block.lines().count();

    let mut terminal = Terminal::new(TestBackend::new(AREA_WIDTH, 3))?;
    terminal.draw(|frame| {
        let area = frame.area();
        let gutter_width = gutter_options.width(line_count);
        let narrowed = u16::try_from(gutter_width).expect("the gutter fits in a terminal");
        let text_area = Rect::new(narrowed, area.y, area.width - narrowed, area.height);

        let mut rows: Vec<DisplayRow> = Vec::new();
        let mut top = 0;
        for (line, (start, text)) in block.lines().enumerate() {
            let laid_out = lay_out(line, text, usize::from(text_area.width));
            let styled = block.style_rows(start, &laid_out);
            top += renderer.draw_styled_line(frame.buffer_mut(), text_area, top, &styled);
            rows.extend(laid_out);
        }

        let gutter = Gutter::new(&gutter_options, &rows, 0, line_count);
        frame.render_widget(gutter, Rect::new(area.x, area.y, narrowed, area.height));
    })?;

    assert_eq!(
        vec![" |1| |a|b|c|d", " | | |e|f| | ", " |2| |g|h|i| "],
        symbols(&terminal)
    );
    assert_eq!(
        Vec::<String>::new(),
        broken_claims(terminal.backend().buffer())
    );

    let first = styles(&terminal, 0);
    assert_eq!(painted(Style::default()), first[3], "`a` was styled");
    assert_eq!(painted(red()), first[5], "`c` lost the span covering it");
    assert_eq!(painted(red()), first[6], "`d` lost the span covering it");

    let third = styles(&terminal, 2);
    assert_eq!(painted(blue()), third[3], "`g` lost the span covering it");
    assert_eq!(painted(blue()), third[4], "`h` lost the span covering it");
    assert_eq!(painted(Style::default()), third[5], "`i` was styled");

    Ok(())
}

/// Validation 4: the styles a row was drawn with reach the cells the terminal holds, across the
/// wrap boundary a span crosses and across the blanks a styled tab is drawn as.
#[test]
fn a_styled_row_carries_its_styles_into_the_terminal_grid() -> Result<()> {
    let block = Block::with_spans("abcdef".to_owned(), vec![Span::new(2..4, red())]);
    let terminal = draw(&block, TEXT_WIDTH, 2)?;

    assert_eq!(vec!["a|b|c", "d|e|f"], symbols(&terminal));
    assert_eq!(
        vec![
            painted(Style::default()),
            painted(Style::default()),
            painted(red())
        ],
        styles(&terminal, 0)
    );
    assert_eq!(
        vec![
            painted(red()),
            painted(Style::default()),
            painted(Style::default())
        ],
        styles(&terminal, 1)
    );

    let tabbed = Block::with_spans("a\tb".to_owned(), vec![Span::new(1..2, blue())]);
    let terminal = draw(&tabbed, TAB_STOP + 1, 1)?;

    assert_eq!(vec!["a| | | | | | | |b"], symbols(&terminal));
    let drawn = styles(&terminal, 0);
    assert_eq!(
        vec![painted(blue()); TAB_STOP - 1],
        drawn[1..TAB_STOP],
        "the blanks a styled tab is drawn as do not carry its span"
    );
    assert_eq!(painted(Style::default()), drawn[TAB_STOP], "`b` was styled");

    Ok(())
}

/// Validation 4: a styled row puts every grapheme in the cell the row it was built from puts it
/// in, so styling a row changes what its cells carry and nothing else.
///
/// The fixtures are the shapes the two draws could disagree on: a cluster measured as one
/// grapheme and stored as several bytes, a tab whose blanks one path fills and the other blanks, a
/// grapheme too wide for the cells its row had left, and a continuation row drawn behind a repeated
/// indent and a marker.
#[test]
fn a_styled_row_draws_the_cells_its_display_row_draws() -> Result<()> {
    let fixtures = [
        ("a\u{0301}\t中文 abcdef", 6, LineOptions::new()),
        ("ab中cd中ef", 3, LineOptions::new()),
        (
            "\tan indented line that wraps",
            10,
            LineOptions::new()
                .with_break_indent(true)
                .with_break_indent_min(2)
                .with_show_break("> ".to_owned()),
        ),
    ];

    for (source, width, options) in fixtures {
        let block = Block::with_spans(source.to_owned(), vec![Span::new(0..7, red())]);
        let rows = line::lay_out(
            0,
            source,
            NonZeroUsize::new(width).expect("a fixture is drawn in at least one column"),
            metrics(),
            &options,
        );
        let styled = block.style_rows(0, &rows);
        let height = u16::try_from(rows.len()).expect("a fixture fits on a screen");
        assert!(1 < rows.len(), "`{source}` must wrap to be worth drawing");

        let narrowed = u16::try_from(width).expect("a fixture fits in a terminal");
        let mut plain = Terminal::new(TestBackend::new(narrowed, height))?;
        plain.draw(|frame| {
            let area = frame.area();
            Renderer::new(metrics()).draw_line(frame.buffer_mut(), area, 0, &rows);
        })?;

        let mut painted_rows = Terminal::new(TestBackend::new(narrowed, height))?;
        painted_rows.draw(|frame| {
            let area = frame.area();
            Renderer::new(metrics()).draw_styled_line(frame.buffer_mut(), area, 0, &styled);
        })?;

        assert_eq!(
            symbols(&plain),
            symbols(&painted_rows),
            "`{source}` is drawn in different cells once styled"
        );
        assert_ne!(
            (0..height)
                .map(|y| styles(&plain, y))
                .collect::<Vec<Vec<Style>>>(),
            (0..height)
                .map(|y| styles(&painted_rows, y))
                .collect::<Vec<Vec<Style>>>(),
            "`{source}` was drawn with the styled path leaving every cell as the unstyled one did"
        );
    }

    Ok(())
}

/// Draws the first logical line of `block`, laid out `width` columns wide, through the styled
/// entry point of the renderer.
///
/// # Returns
///
/// The terminal the styled rows were drawn into on success.
///
/// # Errors
///
/// Returns an error if the terminal cannot be built or drawn to.
///
/// # Panics
///
/// Panics if the line lays out into more rows than `height`.
fn draw(block: &Block, width: usize, height: u16) -> Result<Terminal<TestBackend>> {
    let (start, text) = block.lines().next().expect("a block holds a first line");
    let rows = lay_out(0, text, width);
    let styled: Vec<StyledRow> = block.style_rows(start, &rows);
    assert_eq!(usize::from(height), styled.len());

    let narrowed = u16::try_from(width).expect("a fixture fits in a terminal");
    let mut terminal = Terminal::new(TestBackend::new(narrowed, height))?;
    terminal.draw(|frame| {
        let area = frame.area();
        Renderer::new(metrics()).draw_styled_line(frame.buffer_mut(), area, 0, &styled);
    })?;

    Ok(terminal)
}
