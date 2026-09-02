//! The line-number gutter drawn down the left of a window.
//!
//! The gutter numbers logical lines rather than display rows. A logical line wrapped over four
//! rows therefore carries its number on the first of them and blanks on the other three, and that
//! blanking is the whole of what the numbering tells a reader: the four rows are one line. A
//! gutter that numbered rows instead would say the opposite, and in a transcript, where a
//! paragraph is one logical line wrapped over dozens of rows, it would say it dozens of times.
//!
//! Continuation blanks are drawn in a style of their own rather than as spaces, so a theme can
//! mark the run of rows a wrapped line covers instead of leaving a reader to infer it from a gap
//! in the numbers.
//!
//! Which number a row shows follows vim: `number` alone shows the absolute line number,
//! `relativenumber` alone shows the distance from the cursor's line and `0` on the cursor's own
//! line, and the two together show the distance everywhere except the cursor's line, which shows
//! its absolute number left aligned. With neither set the gutter is zero columns wide and draws
//! nothing.
//!
//! The gutter is as wide as its largest number needs, subject to a configurable minimum that
//! stands in for vim's `'numberwidth'`.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::Widget;

use vbc_layout::invariants::Row;

/// The narrowest a gutter is drawn, in display columns, matching vim's default `'numberwidth'`.
pub const DEFAULT_MIN_WIDTH: usize = 4;

/// What a gutter shows beside one display row.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Label {
    /// The absolute number of the logical line the row starts.
    Absolute(usize),

    /// The distance in logical lines between the row's line and the cursor's.
    Relative(usize),

    /// The absolute number of the cursor's own logical line, shown when both numberings are on.
    Current(usize),

    /// The row continues a logical line numbered on an earlier row.
    Continuation,
}

impl Label {
    /// # Parameters
    ///
    /// * `width` - The number of display columns the gutter occupies.
    ///
    /// # Returns
    ///
    /// The cells the label is drawn in, which are `width` columns wide unless the number needs
    /// more room than the gutter was given.
    #[must_use]
    pub fn cells(self, width: usize) -> String {
        match self {
            Self::Absolute(number) | Self::Relative(number) => {
                let digits = width.saturating_sub(1);
                format!("{number:>digits$} ")
            }
            Self::Current(number) => format!("{number:<width$}"),
            Self::Continuation => " ".repeat(width),
        }
    }
}

/// How a gutter numbers lines and how it is styled.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Options {
    number: bool,
    relative_number: bool,
    min_width: usize,
    number_style: Style,
    current_style: Style,
    continuation_style: Style,
}

impl Options {
    /// # Returns
    ///
    /// A gutter with both numberings off, which draws nothing until one is turned on.
    #[must_use]
    pub fn new() -> Self {
        Self {
            number: false,
            relative_number: false,
            min_width: DEFAULT_MIN_WIDTH,
            number_style: Style::new().fg(Color::DarkGray),
            current_style: Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            continuation_style: Style::new().fg(Color::DarkGray).add_modifier(Modifier::DIM),
        }
    }

    /// # Parameters
    ///
    /// * `enabled` - Whether absolute line numbers are shown, as vim's `'number'`.
    ///
    /// # Returns
    ///
    /// The options with the setting applied.
    #[must_use]
    pub fn with_number(mut self, enabled: bool) -> Self {
        self.number = enabled;
        self
    }

    /// # Parameters
    ///
    /// * `enabled` - Whether distances from the cursor's line are shown, as vim's
    ///   `'relativenumber'`.
    ///
    /// # Returns
    ///
    /// The options with the setting applied.
    #[must_use]
    pub fn with_relative_number(mut self, enabled: bool) -> Self {
        self.relative_number = enabled;
        self
    }

    /// # Parameters
    ///
    /// * `columns` - The narrowest the gutter is drawn, as vim's `'numberwidth'`.
    ///
    /// # Returns
    ///
    /// The options with the setting applied.
    #[must_use]
    pub fn with_min_width(mut self, columns: usize) -> Self {
        self.min_width = columns;
        self
    }

    /// # Parameters
    ///
    /// * `style` - The style a numbered row's cells are drawn in.
    ///
    /// # Returns
    ///
    /// The options with the setting applied.
    #[must_use]
    pub fn with_number_style(mut self, style: Style) -> Self {
        self.number_style = style;
        self
    }

    /// # Parameters
    ///
    /// * `style` - The style the cursor's own line is drawn in when both numberings are on.
    ///
    /// # Returns
    ///
    /// The options with the setting applied.
    #[must_use]
    pub fn with_current_style(mut self, style: Style) -> Self {
        self.current_style = style;
        self
    }

    /// # Parameters
    ///
    /// * `style` - The style a continuation row's blanks are drawn in.
    ///
    /// # Returns
    ///
    /// The options with the setting applied.
    #[must_use]
    pub fn with_continuation_style(mut self, style: Style) -> Self {
        self.continuation_style = style;
        self
    }

    /// # Returns
    ///
    /// Whether absolute line numbers are shown.
    #[must_use]
    pub fn number(&self) -> bool {
        self.number
    }

    /// # Returns
    ///
    /// Whether distances from the cursor's line are shown.
    #[must_use]
    pub fn relative_number(&self) -> bool {
        self.relative_number
    }

    /// # Returns
    ///
    /// The narrowest the gutter is drawn, in display columns.
    #[must_use]
    pub fn min_width(&self) -> usize {
        self.min_width
    }

    /// # Returns
    ///
    /// The style a numbered row's cells are drawn in.
    #[must_use]
    pub fn number_style(&self) -> Style {
        self.number_style
    }

    /// # Returns
    ///
    /// The style the cursor's own line is drawn in when both numberings are on.
    #[must_use]
    pub fn current_style(&self) -> Style {
        self.current_style
    }

    /// # Returns
    ///
    /// The style a continuation row's blanks are drawn in.
    #[must_use]
    pub fn continuation_style(&self) -> Style {
        self.continuation_style
    }

    /// # Parameters
    ///
    /// * `line_count` - The number of logical lines in the buffer, which is the largest absolute
    ///   number the gutter may have to show.
    ///
    /// # Returns
    ///
    /// The display columns the gutter occupies, which is zero while neither numbering is on.
    #[must_use]
    pub fn width(&self, line_count: usize) -> usize {
        if !self.number && !self.relative_number {
            return 0;
        }
        let digits = decimal_digits(line_count.max(1));
        self.min_width.max(digits + 1)
    }

    /// # Parameters
    ///
    /// * `row` - The display row the label is drawn beside.
    /// * `cursor_line` - The zero-based logical line the cursor rests on.
    ///
    /// # Returns
    ///
    /// What the gutter shows beside `row`, or [`None`] while neither numbering is on.
    #[must_use]
    pub fn label(&self, row: &Row, cursor_line: usize) -> Option<Label> {
        match (self.number, self.relative_number) {
            (false, false) => None,
            _ if row.start != 0 => Some(Label::Continuation),
            (true, false) => Some(Label::Absolute(row.line + 1)),
            (false, true) => Some(Label::Relative(row.line.abs_diff(cursor_line))),
            (true, true) if row.line == cursor_line => Some(Label::Current(row.line + 1)),
            (true, true) => Some(Label::Relative(row.line.abs_diff(cursor_line))),
        }
    }

    /// # Parameters
    ///
    /// * `label` - The label whose style is wanted.
    ///
    /// # Returns
    ///
    /// The style `label` is drawn in.
    #[must_use]
    pub fn style(&self, label: Label) -> Style {
        match label {
            Label::Absolute(_) | Label::Relative(_) => self.number_style,
            Label::Current(_) => self.current_style,
            Label::Continuation => self.continuation_style,
        }
    }
}

impl Default for Options {
    fn default() -> Self {
        Self::new()
    }
}

/// The gutter of one drawn screen: the rows it labels, and the cursor they are numbered against.
///
/// Rows of the drawing area the screen does not reach are blanked in the terminal's own style, so
/// nothing of an earlier draw is left beside them.
#[derive(Clone, Copy, Debug)]
pub struct Gutter<'gutter> {
    options: &'gutter Options,
    rows: &'gutter [Row],
    cursor_line: usize,
    line_count: usize,
}

impl<'gutter> Gutter<'gutter> {
    /// # Parameters
    ///
    /// * `options` - How the gutter numbers lines and how it is styled.
    /// * `rows` - The display rows the screen draws, top to bottom.
    /// * `cursor_line` - The zero-based logical line the cursor rests on.
    /// * `line_count` - The number of logical lines in the buffer.
    ///
    /// # Returns
    ///
    /// A gutter drawn beside `rows`.
    #[must_use]
    pub fn new(
        options: &'gutter Options,
        rows: &'gutter [Row],
        cursor_line: usize,
        line_count: usize,
    ) -> Self {
        Self {
            options,
            rows,
            cursor_line,
            line_count,
        }
    }

    /// # Returns
    ///
    /// The display columns the gutter occupies.
    #[must_use]
    pub fn width(&self) -> usize {
        self.options.width(self.line_count)
    }

    /// # Returns
    ///
    /// What the gutter shows beside each of its rows, top to bottom, which is empty while neither
    /// numbering is on.
    #[must_use]
    pub fn labels(&self) -> Vec<Label> {
        self.rows
            .iter()
            .filter_map(|row| self.options.label(row, self.cursor_line))
            .collect()
    }
}

impl Widget for Gutter<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let width = self.width().min(usize::from(area.width));
        if width == 0 || area.is_empty() {
            return;
        }
        let labels = self.labels();
        let blank = " ".repeat(width);
        for offset in 0..area.height {
            let (cells, style) = match labels.get(usize::from(offset)) {
                Some(label) => (label.cells(width), self.options.style(*label)),
                None => (blank.clone(), Style::default()),
            };
            buf.set_stringn(area.x, area.y + offset, &cells, width, style);
        }
    }
}

/// # Parameters
///
/// * `value` - The number whose decimal length is wanted.
///
/// # Returns
///
/// How many decimal digits `value` is written in.
fn decimal_digits(value: usize) -> usize {
    value.checked_ilog10().unwrap_or(0) as usize + 1
}
