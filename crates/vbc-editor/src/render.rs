//! Drawing display rows into the cells of a terminal buffer.
//!
//! A renderer is given rows that have already been laid out and puts them in the cells they were
//! laid out for: it reads the column the layout measured every grapheme at and writes the grapheme
//! there, rather than handing a row's text to ratatui and letting the buffer measure it again.
//! That is what keeps the screen agreeing with the rest of the application, because the two
//! measure differently. ratatui measures with `unicode-width` alone, while [`Metrics`] carries
//! vim's `'ambiwidth'` and `'tabstop'` as well, so a row drawn by width rather than by column
//! would put every grapheme after the first ambiguous-width one in the wrong cell.
//!
//! A grapheme wider than one cell claims the cells beside it: ratatui keeps the grapheme in the
//! first of them and blanks the rest, and nothing stops a later write from putting a symbol in a
//! cell that is already claimed, which leaves the buffer describing a screen no terminal can draw.
//! A renderer therefore owns the whole row it draws -- it blanks the row and then fills it left to
//! right -- and where the layout and ratatui disagree about how many cells a grapheme claims, the
//! layout wins: the cell is marked with the width the layout measured, so the diff that reaches
//! the terminal leaves the claimed cells alone.
//!
//! Two of vim's rules about the right edge of a window are carried here. A grapheme is drawn only
//! where the whole of it fits, so the last cell of a row is left blank rather than filled with
//! half of a double-width character. And where a row ended early because the grapheme coming next
//! was too wide for the cells it had left, those cells are filled with
//! [`WIDE_CHARACTER_MARKER`], which is what vim leaves there.

use ratatui::buffer::{Buffer, CellDiffOption, CellWidth};
use ratatui::layout::{Position, Rect};
use ratatui::style::Style;
use vbc_layout::line::DisplayRow;
use vbc_layout::width::{graphemes, Metrics};

/// The character vim leaves in the cells a row has left over when the grapheme coming next is too
/// wide to fit in them.
pub const WIDE_CHARACTER_MARKER: char = '>';

/// Draws laid-out display rows into the cells of a terminal buffer.
///
/// The renderer carries the metrics the rows were laid out under, which it needs to measure the
/// decoration a continuation row is drawn behind and the grapheme a row could not fit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Renderer {
    metrics: Metrics,
    style: Style,
}

impl Renderer {
    /// Factory function.
    ///
    /// # Returns
    ///
    /// A newly created renderer drawing rows measured under `metrics`, in the terminal's own
    /// style.
    #[must_use]
    pub fn new(metrics: Metrics) -> Self {
        Self {
            metrics,
            style: Style::new(),
        }
    }

    /// # Returns
    ///
    /// This renderer drawing every cell it fills in `style`.
    #[must_use]
    pub fn with_style(mut self, style: Style) -> Self {
        self.style = style;
        self
    }

    /// # Returns
    ///
    /// The metrics the rows this renderer draws are measured under.
    #[must_use]
    pub fn metrics(&self) -> Metrics {
        self.metrics
    }

    /// # Returns
    ///
    /// The style every cell this renderer fills is drawn in.
    #[must_use]
    pub fn style(&self) -> Style {
        self.style
    }

    /// Draws the rows rendering one logical line, top to bottom, stopping at the bottom of the
    /// area.
    ///
    /// # Returns
    ///
    /// The number of rows drawn, which is fewer than `rows` holds where the area filled up.
    ///
    /// # Panics
    ///
    /// Panics if `area` is not inside `buffer`.
    pub fn draw_line(&self, buffer: &mut Buffer, area: Rect, top: u16, rows: &[DisplayRow]) -> u16 {
        let mut drawn = 0;
        for (index, row) in rows.iter().enumerate() {
            let Some(screen_row) = top.checked_add(drawn).filter(|row| *row < area.height) else {
                break;
            };
            self.draw_row(buffer, area, screen_row, row, rows.get(index + 1));
            drawn += 1;
        }

        drawn
    }

    /// Draws one display row into the cells of `screen_row`, blanking the row first so that
    /// nothing drawn there before shows through.
    ///
    /// `next` is the row that follows this one within the same logical line, and is what says
    /// whether the cells this row has left over are the ones vim marks with
    /// [`WIDE_CHARACTER_MARKER`].
    ///
    /// # Panics
    ///
    /// Panics if `area` is not inside `buffer`.
    pub fn draw_row(
        &self,
        buffer: &mut Buffer,
        area: Rect,
        screen_row: u16,
        row: &DisplayRow,
        next: Option<&DisplayRow>,
    ) {
        if area.height <= screen_row {
            return;
        }

        let y = area.y + screen_row;
        self.blank(buffer, area, y);

        let mut column = 0;
        for grapheme in graphemes(row.prefix()) {
            let width = self.metrics.grapheme_width(grapheme, column);
            self.draw_grapheme(buffer, area, y, column, grapheme, width);
            column += width;
        }

        let columns = row.columns();
        for (index, grapheme) in graphemes(row.text()).enumerate() {
            if "\t" == grapheme {
                continue;
            }
            let start = columns[index];
            self.draw_grapheme(buffer, area, y, start, grapheme, columns[index + 1] - start);
        }

        self.mark_wide_gap(buffer, area, y, row, next);
    }

    /// Draws one grapheme into the cells it claims, leaving the row untouched where the grapheme
    /// occupies no columns at all or where the whole of it does not fit.
    fn draw_grapheme(
        &self,
        buffer: &mut Buffer,
        area: Rect,
        y: u16,
        column: usize,
        grapheme: &str,
        width: usize,
    ) {
        if 0 == width || usize::from(area.width) < column + width {
            return;
        }
        let Some(position) = cell(area, y, column) else {
            return;
        };

        let claimed =
            u16::try_from(width).expect("a grapheme that fits in an area fits in a `u16`");
        buffer[position].set_symbol(grapheme).set_style(self.style);
        if grapheme.cell_width() != claimed {
            buffer[position].set_diff_option(CellDiffOption::ForcedWidth(
                claimed.try_into().expect("a drawn grapheme claims a cell"),
            ));
        }
    }

    /// Fills the cells a row has left over with [`WIDE_CHARACTER_MARKER`] where the row ended
    /// because the grapheme coming next was too wide for them.
    ///
    /// A tab is drawn across a row boundary rather than moved whole, so a row that ends in front
    /// of one is not marked.
    fn mark_wide_gap(
        &self,
        buffer: &mut Buffer,
        area: Rect,
        y: u16,
        row: &DisplayRow,
        next: Option<&DisplayRow>,
    ) {
        let Some(carried) = next
            .and_then(|next| graphemes(next.text()).next())
            .filter(|grapheme| "\t" != *grapheme)
            .map(|grapheme| self.metrics.grapheme_width(grapheme, row.width()))
        else {
            return;
        };

        let leftover = usize::from(area.width).saturating_sub(row.width());
        if 0 == leftover || carried <= leftover {
            return;
        }

        let drawn =
            u16::try_from(row.width()).expect("a row narrower than its area fits in a `u16`");
        for x in (area.x + drawn)..area.right() {
            buffer[(x, y)]
                .set_char(WIDE_CHARACTER_MARKER)
                .set_style(self.style);
        }
    }

    /// Resets a row's cells to blanks drawn in this renderer's style, so that neither a symbol nor
    /// a claim left there by an earlier frame survives.
    fn blank(&self, buffer: &mut Buffer, area: Rect, y: u16) {
        for x in area.x..area.right() {
            buffer[(x, y)].reset();
            buffer[(x, y)].set_style(self.style);
        }
    }
}

/// Places the cursor on a row, which is what says where a terminal draws it.
///
/// A grapheme is drawn in the first of the cells it claims, so a cursor on a double-width
/// character rests on that character's own cell rather than on the blank beside it. The position
/// past a line's last grapheme -- where `A` leaves the cursor -- rests in the cell after the row's
/// text, and a row whose text fills its last cell has no such cell: the cursor belongs at the
/// start of the row below, which is the caller's to place because this row does not draw it.
///
/// # Returns
///
/// The cell of `area` the cursor rests in, or `None` where `row` does not draw `grapheme` or
/// where the cell falls outside the area.
#[must_use]
pub fn cursor_cell(
    area: Rect,
    screen_row: u16,
    row: &DisplayRow,
    grapheme: usize,
) -> Option<Position> {
    if area.height <= screen_row || grapheme < row.start() || row.end() < grapheme {
        return None;
    }

    cell(
        area,
        area.y + screen_row,
        row.columns()[grapheme - row.start()],
    )
}

/// # Returns
///
/// The cell of `area` at `column` of the screen line `y`, or `None` where the area has no such
/// column.
fn cell(area: Rect, y: u16, column: usize) -> Option<Position> {
    let column = u16::try_from(column).ok()?;
    if area.width <= column {
        return None;
    }

    Some(Position {
        x: area.x + column,
        y,
    })
}
