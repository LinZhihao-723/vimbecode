//! The display rows one screenful of text is drawn from, walked out from the row a viewport is
//! anchored to.
//!
//! This is the seam between the layout engine and the code that draws: everything below it
//! addresses the text by logical position and knows nothing of cells, and everything above it is
//! handed the rows it draws and asks the layout nothing. A screen is therefore the only place a
//! frame's rows are laid out, and it lays out exactly the logical lines the window reaches --
//! never the text they sit in -- so a frame of a hundred lines and a frame of fifty thousand cost
//! the same.
//!
//! A screen carries one row more than the window shows wherever the window cut a logical line
//! short. Nothing draws that row; it is what says whether the row above it ended because the
//! grapheme coming next was too wide for the cells it had left, which is the difference between a
//! blank at the right edge of a row and the marker vim leaves there.
//!
//! The cursor is placed while the walk is passing its own logical line, whose rows are in hand
//! already, rather than looked up afterwards. That is why nothing above this module needs the
//! anchor mapping to find the row the cursor is drawn on.

use std::num::NonZeroUsize;

pub use vbc_layout::anchor::Error;
pub use vbc_layout::viewport::Scrolled;

use vbc_layout::anchor::Wrapping;
use vbc_layout::buffer::Buffer;
use vbc_layout::line::{self, DisplayRow, Options};
use vbc_layout::position::LogicalPosition;
use vbc_layout::viewport::{Command, Viewport, Window};
use vbc_layout::width::Metrics;

/// The shape a screenful of text is drawn to: the columns a display row is wrapped into, the rows
/// the window holds, and how the text between them is measured and decorated.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Geometry {
    wrapping: Wrapping,
    window: Window,
}

impl Geometry {
    /// Factory function.
    ///
    /// # Returns
    ///
    /// A newly created geometry `columns` columns wide and `rows` rows tall, measuring and
    /// wrapping its text as vim's own defaults do and keeping no rows beside the cursor.
    #[must_use]
    pub fn new(columns: NonZeroUsize, rows: NonZeroUsize) -> Self {
        Self {
            wrapping: Wrapping::new(columns, Metrics::default(), Options::new()),
            window: Window::new(rows),
        }
    }

    /// # Returns
    ///
    /// This geometry measuring its text under `metrics`.
    #[must_use]
    pub fn with_metrics(self, metrics: Metrics) -> Self {
        let options = self.wrapping.options().clone();

        Self {
            wrapping: Wrapping::new(self.wrapping.width(), metrics, options),
            ..self
        }
    }

    /// # Returns
    ///
    /// This geometry wrapping its text as `options` says.
    #[must_use]
    pub fn with_options(self, options: Options) -> Self {
        Self {
            wrapping: Wrapping::new(self.wrapping.width(), self.wrapping.metrics(), options),
            ..self
        }
    }

    /// # Returns
    ///
    /// This geometry keeping `rows` rows between the cursor and an edge, as vim's `'scrolloff'`.
    #[must_use]
    pub fn with_scrolloff(self, rows: usize) -> Self {
        Self {
            window: self.window.with_scrolloff(rows),
            ..self
        }
    }

    /// # Returns
    ///
    /// The columns a display row is wrapped into.
    #[must_use]
    pub fn columns(&self) -> NonZeroUsize {
        self.wrapping.width()
    }

    /// # Returns
    ///
    /// The window the text is scrolled inside.
    #[must_use]
    pub fn window(&self) -> Window {
        self.window
    }
}

/// The display rows a window draws, top to bottom, and where the cursor sits among them.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Screen {
    rows: Vec<DisplayRow>,
    drawn: usize,
    cursor_row: Option<usize>,
}

impl Screen {
    /// Walks the rows a viewport draws, from the row it is anchored to down to the bottom of the
    /// window.
    ///
    /// A viewport anchored past the end of the text draws the text's last line, so a screen can
    /// always be drawn of a buffer whatever the viewport was left pointing at.
    ///
    /// # Returns
    ///
    /// The screen `viewport` draws of `text`, with `cursor` placed among its rows.
    #[must_use]
    pub fn of(
        text: &Buffer,
        viewport: &Viewport,
        cursor: LogicalPosition,
        geometry: &Geometry,
    ) -> Self {
        let count = text.line_count();
        let wanted = geometry.window.height().get();
        let mut rows = Vec::with_capacity(wanted + 1);
        let mut index = viewport.anchor().min(count - 1);
        let mut hidden = viewport.vertical_offset();
        let mut cursor_row = None;
        let mut drawn = 0;

        while index < count && drawn < wanted {
            let source = text
                .line(index)
                .expect("the walk stops at the last line of the text");
            let laid_out = line::lay_out(
                index,
                source,
                geometry.wrapping.width(),
                geometry.wrapping.metrics(),
                geometry.wrapping.options(),
            );
            let above = hidden.min(laid_out.len() - 1);
            hidden = 0;

            if index == cursor.line {
                let at = row_index(&laid_out, cursor.grapheme);
                if above <= at && drawn + at - above < wanted {
                    cursor_row = Some(drawn + at - above);
                }
            }

            for row in laid_out.into_iter().skip(above) {
                rows.push(row);
                if wanted == drawn {
                    break;
                }
                drawn += 1;
            }
            index += 1;
        }

        Self {
            rows,
            drawn,
            cursor_row,
        }
    }

    /// # Returns
    ///
    /// The rows the window shows, top to bottom.
    #[must_use]
    pub fn rows(&self) -> &[DisplayRow] {
        &self.rows[..self.drawn]
    }

    /// # Returns
    ///
    /// The rows of each logical line the window shows, top to bottom, each group carrying the row
    /// that continues it below the window wherever the window cut its line short.
    #[must_use]
    pub fn lines(&self) -> Vec<&[DisplayRow]> {
        let mut groups = Vec::new();
        let mut start = 0;
        for (index, row) in self.rows.iter().enumerate() {
            if row.line() != self.rows[start].line() {
                groups.push(&self.rows[start..index]);
                start = index;
            }
        }
        if !self.rows.is_empty() {
            groups.push(&self.rows[start..]);
        }

        groups
    }

    /// # Returns
    ///
    /// The row of the window the cursor is drawn on, or [`None`] where the window does not draw
    /// the cursor's own row.
    #[must_use]
    pub fn cursor_row(&self) -> Option<usize> {
        self.cursor_row
    }
}

/// Scrolls a viewport by one command, under the geometry the screen it draws is laid out to.
///
/// # Returns
///
/// Where the scroll left the screen and the cursor on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`Viewport::scroll`]'s return values on failure.
pub fn scroll(
    text: &Buffer,
    viewport: &Viewport,
    cursor: LogicalPosition,
    geometry: &Geometry,
    command: Command,
) -> Result<Scrolled, Error> {
    viewport.scroll(
        text.lines(),
        &geometry.wrapping,
        geometry.window,
        cursor,
        command,
    )
}

/// # Returns
///
/// The index of the row of `rows` that draws the grapheme at `grapheme`, which is the last row for
/// the position past the line's last grapheme.
///
/// # Panics
///
/// Panics if `rows` is empty, which no line lays out into.
fn row_index(rows: &[DisplayRow], grapheme: usize) -> usize {
    assert!(!rows.is_empty(), "a line lays out into at least one row");

    rows.partition_point(|row| row.end() <= grapheme)
        .min(rows.len() - 1)
}
