//! What the line-number gutter draws beside a wrapped screen.
//!
//! The rows the gutter is drawn beside come from the real line layout rather than from rows
//! written out by hand, so a line that the tests call four rows tall is one the layout engine
//! actually wraps over four rows.

use anyhow::Result;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::Terminal;
use std::num::NonZeroUsize;
use vbc_editor::gutter::{Gutter, Label, Options, DEFAULT_MIN_WIDTH};
use vbc_layout::invariants::Row;
use vbc_layout::line::{self, Options as LineOptions};
use vbc_layout::width::Metrics;

/// A line 32 columns wide, which wraps into exactly four rows of a window eight columns wide.
const FOUR_ROW_LINE: &str = "abcdefghijklmnopqrstuvwxyz012345";

/// The columns the text of [`FOUR_ROW_LINE`] is wrapped into.
const TEXT_WIDTH: usize = 8;

/// The window the gutter is drawn into during a render.
const AREA_WIDTH: u16 = 16;

/// # Returns
///
/// The style a terminal cell carries once `style` has been drawn onto it, which is `style` laid
/// over the cell's reset colours rather than `style` on its own.
fn painted(style: Style) -> Style {
    Style::new().fg(Color::Reset).bg(Color::Reset).patch(style)
}

/// What one render produced, read back cell by cell.
struct Rendered {
    buffer: ratatui::buffer::Buffer,
    width: usize,
}

impl Rendered {
    /// # Returns
    ///
    /// The symbols the gutter drew on display row `y`, joined left to right.
    fn cells(&self, y: u16) -> String {
        (0..self.width)
            .map(|x| {
                let x = u16::try_from(x).expect("the gutter is narrower than a terminal");
                self.buffer[(x, y)].symbol()
            })
            .collect()
    }

    /// # Returns
    ///
    /// The style of each cell the gutter drew on display row `y`, left to right.
    fn styles(&self, y: u16) -> Vec<Style> {
        (0..self.width)
            .map(|x| {
                let x = u16::try_from(x).expect("the gutter is narrower than a terminal");
                self.buffer[(x, y)].style()
            })
            .collect()
    }

    /// # Returns
    ///
    /// The style every cell of display row `y` shares.
    ///
    /// # Panics
    ///
    /// Panics if the row's cells do not all share one style.
    fn style(&self, y: u16) -> Style {
        let styles = self.styles(y);
        let first = *styles
            .first()
            .expect("the gutter is at least one column wide");
        assert!(
            styles.iter().all(|style| *style == first),
            "row {y} is drawn in more than one style: {styles:?}"
        );
        first
    }

    /// # Returns
    ///
    /// The symbol drawn in the first cell to the right of the gutter on display row `y`.
    fn beyond(&self, y: u16) -> String {
        let x = u16::try_from(self.width).expect("the gutter is narrower than a terminal");
        self.buffer[(x, y)].symbol().to_owned()
    }
}

/// # Returns
///
/// The display rows `lines` wraps into at a window [`TEXT_WIDTH`] columns wide, top to bottom.
fn wrap(lines: &[&str]) -> Vec<Row> {
    let width = NonZeroUsize::new(TEXT_WIDTH).expect("the text width is non-zero");
    let options = LineOptions::new();
    lines
        .iter()
        .enumerate()
        .flat_map(|(index, text)| {
            line::lay_out(text, width, Metrics::default(), &options)
                .into_iter()
                .map(move |row| Row {
                    line: index,
                    start: row.start(),
                    text: row.text().to_owned(),
                    cells: row.cells().to_owned(),
                    columns: row.columns().to_vec(),
                })
        })
        .collect()
}

/// # Returns
///
/// The gutter `options` draws beside `rows` with the cursor on `cursor_line`, read back cell by
/// cell.
///
/// # Errors
///
/// Returns an error if the terminal cannot be built or drawn to.
fn render(
    options: &Options,
    rows: &[Row],
    cursor_line: usize,
    line_count: usize,
    height: u16,
) -> Result<Rendered> {
    let mut terminal = Terminal::new(TestBackend::new(AREA_WIDTH, height))?;
    terminal.draw(|frame| {
        let gutter = Gutter::new(options, rows, cursor_line, line_count);
        frame.render_widget(gutter, Rect::new(0, 0, AREA_WIDTH, height));
    })?;
    Ok(Rendered {
        buffer: terminal.backend().buffer().clone(),
        width: options.width(line_count),
    })
}

/// Validation 1: a logical line wrapped over four rows is numbered once and blank three times.
#[test]
fn wrapped_line_is_numbered_once() -> Result<()> {
    let rows = wrap(&["one", FOUR_ROW_LINE, "three"]);
    assert_eq!(rows.len(), 6, "the middle line must wrap over four rows");

    let options = Options::new().with_number(true);
    let drawn = render(&options, &rows, 0, 3, 6)?;

    assert_eq!(drawn.width, 4);
    assert_eq!(drawn.cells(0), "  1 ");
    assert_eq!(drawn.cells(1), "  2 ");
    assert_eq!(drawn.cells(2), "    ");
    assert_eq!(drawn.cells(3), "    ");
    assert_eq!(drawn.cells(4), "    ");
    assert_eq!(drawn.cells(5), "  3 ");

    let labels = Gutter::new(&options, &rows, 0, 3).labels();
    assert_eq!(
        labels,
        vec![
            Label::Absolute(1),
            Label::Absolute(2),
            Label::Continuation,
            Label::Continuation,
            Label::Continuation,
            Label::Absolute(3),
        ]
    );
    Ok(())
}

/// Validation 2: the same wrapped line under `relativenumber`, and under both numberings.
#[test]
fn wrapped_line_is_numbered_once_under_every_numbering() -> Result<()> {
    let rows = wrap(&["one", FOUR_ROW_LINE, "three"]);

    let relative = Options::new().with_relative_number(true);
    let drawn = render(&relative, &rows, 0, 3, 6)?;
    assert_eq!(drawn.cells(0), "  0 ");
    assert_eq!(drawn.cells(1), "  1 ");
    assert_eq!(drawn.cells(2), "    ");
    assert_eq!(drawn.cells(3), "    ");
    assert_eq!(drawn.cells(4), "    ");
    assert_eq!(drawn.cells(5), "  2 ");

    let both = Options::new().with_number(true).with_relative_number(true);
    let drawn = render(&both, &rows, 1, 3, 6)?;
    assert_eq!(drawn.cells(0), "  1 ");
    assert_eq!(drawn.cells(1), "2   ");
    assert_eq!(drawn.cells(2), "    ");
    assert_eq!(drawn.cells(3), "    ");
    assert_eq!(drawn.cells(4), "    ");
    assert_eq!(drawn.cells(5), "  1 ");
    assert_eq!(drawn.style(1), painted(both.current_style()));
    assert_ne!(drawn.style(1), painted(both.number_style()));

    let both_off_cursor = render(&both, &rows, 0, 3, 6)?;
    assert_eq!(both_off_cursor.cells(0), "1   ");
    assert_eq!(both_off_cursor.cells(1), "  1 ");
    assert_eq!(both_off_cursor.cells(5), "  2 ");
    Ok(())
}

/// Validation 2: a continuation row stays blank whichever numbering drew the row above it.
#[test]
fn continuation_rows_are_blank_under_every_numbering() -> Result<()> {
    let rows = wrap(&[FOUR_ROW_LINE]);
    let settings = [
        Options::new().with_number(true),
        Options::new().with_relative_number(true),
        Options::new().with_number(true).with_relative_number(true),
    ];
    for options in &settings {
        let drawn = render(options, &rows, 0, 1, 4)?;
        let numbered = drawn.cells(0);
        assert_ne!(numbered.trim(), "", "{options:?} left the first row blank");
        for y in 1..4 {
            assert_eq!(drawn.cells(y), "    ", "{options:?} numbered row {y}");
        }
    }
    Ok(())
}

/// Validation 3: the gutter is as wide as the largest line number needs.
#[test]
fn width_adapts_to_the_largest_line_number() -> Result<()> {
    let adapting = Options::new().with_number(true).with_min_width(1);
    let expected = [(9, 2), (10, 3), (99, 3), (100, 4), (1000, 5)];
    for (line_count, width) in expected {
        assert_eq!(
            adapting.width(line_count),
            width,
            "{line_count} lines must need {width} columns"
        );
    }

    let default = Options::new().with_number(true);
    assert_eq!(default.min_width(), DEFAULT_MIN_WIDTH);
    for (line_count, width) in expected {
        assert_eq!(default.width(line_count), width.max(DEFAULT_MIN_WIDTH));
    }

    assert_eq!(
        Options::new().width(1000),
        0,
        "an off gutter takes no columns"
    );
    Ok(())
}

/// Validation 3: the columns a render actually paints follow the adapted width.
#[test]
fn rendered_width_adapts_to_the_largest_line_number() -> Result<()> {
    let options = Options::new().with_number(true).with_min_width(1);
    let expected = [
        (9, "9 "),
        (10, "10 "),
        (99, "99 "),
        (100, "100 "),
        (1000, "1000 "),
    ];
    for (line_count, last) in expected {
        let lines: Vec<String> = (1..=line_count).map(|index| format!("l{index}")).collect();
        let borrowed: Vec<&str> = lines.iter().map(String::as_str).collect();
        let rows = wrap(&borrowed);
        let height = u16::try_from(line_count).expect("the line count fits a terminal");
        let drawn = render(&options, &rows, 0, line_count, height)?;

        assert_eq!(drawn.width, last.len(), "{line_count} lines");
        assert_eq!(drawn.cells(height - 1), last, "{line_count} lines");
        assert_eq!(
            drawn.beyond(height - 1),
            " ",
            "{line_count} lines: the gutter spilled past its width"
        );
    }
    Ok(())
}

/// Validation 4: a continuation row's blanks are styled apart from a numbered row's.
#[test]
fn continuation_blanks_are_styled_distinctly() -> Result<()> {
    let rows = wrap(&["one", FOUR_ROW_LINE]);
    let options = Options::new().with_number(true);
    let drawn = render(&options, &rows, 0, 2, 5)?;

    assert_eq!(drawn.style(0), painted(options.number_style()));
    assert_eq!(drawn.style(1), painted(options.number_style()));
    for y in 2..5 {
        assert_eq!(drawn.cells(y), "    ", "row {y} is not blank");
        assert_eq!(
            drawn.style(y),
            painted(options.continuation_style()),
            "row {y} is not styled as a continuation"
        );
        assert_ne!(
            drawn.style(y),
            painted(options.number_style()),
            "row {y} is styled as a numbered row"
        );
        assert_ne!(
            drawn.style(y),
            painted(Style::default()),
            "row {y} is blank in the terminal's own style, so it is merely spaces"
        );
    }
    Ok(())
}

/// Validation 4: the styles a caller sets are the styles the cells are drawn in.
#[test]
fn styles_are_configurable() -> Result<()> {
    let number = Style::new().fg(Color::Blue);
    let current = Style::new().fg(Color::Red).add_modifier(Modifier::BOLD);
    let continuation = Style::new().bg(Color::Green).add_modifier(Modifier::ITALIC);
    let options = Options::new()
        .with_number(true)
        .with_relative_number(true)
        .with_number_style(number)
        .with_current_style(current)
        .with_continuation_style(continuation);

    let rows = wrap(&["one", FOUR_ROW_LINE]);
    let drawn = render(&options, &rows, 0, 2, 5)?;
    assert_eq!(drawn.style(0), painted(current));
    assert_eq!(drawn.style(1), painted(number));
    assert_eq!(drawn.style(2), painted(continuation));
    Ok(())
}

/// A gutter with neither numbering on paints nothing at all.
#[test]
fn an_off_gutter_draws_nothing() -> Result<()> {
    let rows = wrap(&["one", FOUR_ROW_LINE]);
    let options = Options::new();
    let mut terminal = Terminal::new(TestBackend::new(AREA_WIDTH, 5))?;
    terminal.draw(|frame| {
        let gutter = Gutter::new(&options, &rows, 0, 2);
        frame.render_widget(gutter, Rect::new(0, 0, AREA_WIDTH, 5));
    })?;
    let buffer = terminal.backend().buffer();
    for y in 0..5 {
        for x in 0..AREA_WIDTH {
            assert_eq!(buffer[(x, y)].symbol(), " ", "cell ({x}, {y}) was painted");
            assert_eq!(buffer[(x, y)].style(), painted(Style::default()));
        }
    }
    assert_eq!(Gutter::new(&options, &rows, 0, 2).labels(), Vec::new());
    Ok(())
}

/// Rows the text does not reach carry no number and no gutter style.
#[test]
fn rows_past_the_text_are_left_bare() -> Result<()> {
    let rows = wrap(&["one", "two"]);
    let options = Options::new().with_number(true);
    let drawn = render(&options, &rows, 0, 2, 5)?;

    assert_eq!(drawn.cells(0), "  1 ");
    assert_eq!(drawn.cells(1), "  2 ");
    for y in 2..5 {
        assert_eq!(drawn.cells(y), "    ");
        assert_eq!(
            drawn.style(y),
            painted(Style::default()),
            "row {y} was styled"
        );
    }
    Ok(())
}

/// A screen scrolled onto the middle of a wrapped line numbers none of the rows it shows.
#[test]
fn a_screen_starting_mid_line_shows_no_number() -> Result<()> {
    let rows = wrap(&[FOUR_ROW_LINE]);
    let scrolled = &rows[1..];
    let options = Options::new().with_number(true);
    let drawn = render(&options, scrolled, 0, 1, 3)?;

    for y in 0..3 {
        assert_eq!(drawn.cells(y), "    ", "row {y} was numbered");
        assert_eq!(drawn.style(y), painted(options.continuation_style()));
    }
    Ok(())
}
