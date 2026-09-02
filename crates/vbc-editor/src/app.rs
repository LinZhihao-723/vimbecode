//! The editor as a program: the text it holds, the window it is scrolled inside, and the frame it
//! paints into a terminal.
//!
//! An application is where the pieces meet. It owns the one [`Buffer`] the workspace edits, the
//! [`Viewport`] that says which part of it is on screen, and the cursor's logical position; a
//! frame is the three of them turned into cells. Nothing about that turning consults the anchor
//! mapping: the rows come from [`Screen`], already laid out and already carrying the row the
//! cursor is drawn on, and the drawing spends itself on the cells it fills.
//!
//! The window is measured from the area a frame is drawn into rather than stored, so a terminal
//! that was resized between two frames draws the second one at its new size without being told.
//! The gutter takes its columns off the left of that area and the text wraps into what is left, so
//! a wider gutter narrows the text rather than pushing it off the screen.

use std::num::NonZeroUsize;

use ratatui::buffer::Buffer as Cells;
use ratatui::layout::{Position, Rect};
use ratatui::widgets::Widget;
use ratatui::Frame;
use vbc_layout::buffer::Buffer;
use vbc_layout::line::Options;
use vbc_layout::position::LogicalPosition;
use vbc_layout::viewport::{Command, Viewport};
use vbc_layout::width::Metrics;

use crate::gutter::{Gutter, Options as GutterOptions};
use crate::render::{cursor_cell, Renderer};
use crate::screen::{self, Error, Geometry, Screen};

/// An editor: the text being edited, the part of it the window shows, and where the cursor rests.
///
/// The gutter numbers lines by default, as vim with `'number'` set, because a wrapped transcript
/// is unreadable without the blanks that say which rows continue a line.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct App {
    text: Buffer,
    viewport: Viewport,
    cursor: LogicalPosition,
    metrics: Metrics,
    options: Options,
    gutter: GutterOptions,
    scrolloff: usize,
}

impl App {
    /// Factory function.
    ///
    /// # Returns
    ///
    /// A newly created application showing `text` from its first row, with the cursor on its first
    /// grapheme.
    #[must_use]
    pub fn new(text: Buffer) -> Self {
        Self {
            text,
            viewport: Viewport::new(),
            cursor: LogicalPosition {
                line: 0,
                grapheme: 0,
            },
            metrics: Metrics::default(),
            options: Options::new(),
            gutter: GutterOptions::new().with_number(true),
            scrolloff: 0,
        }
    }

    /// # Returns
    ///
    /// This application measuring its text under `metrics`.
    #[must_use]
    pub fn with_metrics(mut self, metrics: Metrics) -> Self {
        self.metrics = metrics;
        self
    }

    /// # Returns
    ///
    /// This application wrapping its text as `options` says.
    #[must_use]
    pub fn with_options(mut self, options: Options) -> Self {
        self.options = options;
        self
    }

    /// # Returns
    ///
    /// This application drawing the gutter `gutter` describes.
    #[must_use]
    pub fn with_gutter(mut self, gutter: GutterOptions) -> Self {
        self.gutter = gutter;
        self
    }

    /// # Returns
    ///
    /// This application keeping `rows` rows between the cursor and an edge, as vim's `'scrolloff'`.
    #[must_use]
    pub fn with_scrolloff(mut self, rows: usize) -> Self {
        self.scrolloff = rows;
        self
    }

    /// # Returns
    ///
    /// The text being edited.
    #[must_use]
    pub fn text(&self) -> &Buffer {
        &self.text
    }

    /// # Returns
    ///
    /// The part of the text the window shows.
    #[must_use]
    pub fn viewport(&self) -> Viewport {
        self.viewport
    }

    /// # Returns
    ///
    /// Where the cursor rests in the text.
    #[must_use]
    pub fn cursor(&self) -> LogicalPosition {
        self.cursor
    }

    /// Measures the window an area draws, which is the area's rows and the columns the gutter
    /// leaves the text.
    ///
    /// # Returns
    ///
    /// The geometry a frame drawn into `area` is laid out to, or [`None`] where the area is too
    /// small to draw a column of text or a row of one in.
    #[must_use]
    pub fn geometry(&self, area: Rect) -> Option<Geometry> {
        let columns = usize::from(area.width).checked_sub(self.gutter_columns())?;

        Some(
            Geometry::new(
                NonZeroUsize::new(columns)?,
                NonZeroUsize::new(usize::from(area.height))?,
            )
            .with_metrics(self.metrics)
            .with_options(self.options.clone())
            .with_scrolloff(self.scrolloff),
        )
    }

    /// Draws one frame: the gutter down the left of the area, the rows of text beside it, and
    /// nothing at all where the area is too small to hold either.
    ///
    /// # Returns
    ///
    /// The cell of `area` a terminal should rest the cursor in, or [`None`] where the frame does
    /// not draw the cursor's own row.
    pub fn draw(&self, cells: &mut Cells, area: Rect) -> Option<Position> {
        let geometry = self.geometry(area)?;
        let screen = Screen::of(&self.text, &self.viewport, self.cursor, &geometry);
        let gutter = Rect {
            width: narrowed(self.gutter_columns()).min(area.width),
            ..area
        };
        let text = Rect {
            x: area.x + gutter.width,
            width: area.width - gutter.width,
            ..area
        };

        Gutter::new(
            &self.gutter,
            screen.rows(),
            self.cursor.line,
            self.text.line_count(),
        )
        .render(gutter, cells);

        let renderer = Renderer::new(self.metrics);
        let mut top = 0;
        for rows in screen.lines() {
            top += renderer.draw_line(cells, text, top, rows);
        }
        blank(cells, text, top);

        let row = screen.cursor_row()?;
        cursor_cell(
            text,
            narrowed(row),
            screen.rows().get(row)?,
            self.cursor.grapheme,
        )
    }

    /// Draws one frame into a terminal's own frame, leaving the cursor where the frame draws it.
    pub fn render(&self, frame: &mut Frame<'_>) {
        let area = frame.area();
        if let Some(position) = self.draw(frame.buffer_mut(), area) {
            frame.set_cursor_position(position);
        }
    }

    /// Scrolls the window by one command, as it would be scrolled in an area of `area`.
    ///
    /// An area too small to draw text in scrolls nothing, because there is no window for a scroll
    /// to count the rows of.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * Forwards [`screen::scroll`]'s return values on failure.
    pub fn scroll(&mut self, area: Rect, command: Command) -> Result<(), Error> {
        let Some(geometry) = self.geometry(area) else {
            return Ok(());
        };
        let scrolled = screen::scroll(&self.text, &self.viewport, self.cursor, &geometry, command)?;
        self.viewport = scrolled.viewport;
        self.cursor = scrolled.cursor;

        Ok(())
    }

    /// # Returns
    ///
    /// The display columns the gutter takes off the left of an area.
    fn gutter_columns(&self) -> usize {
        self.gutter.width(self.text.line_count())
    }
}

/// Resets the rows of `area` from `top` down, so that no row the text does not reach keeps what an
/// earlier frame drew there.
fn blank(cells: &mut Cells, area: Rect, top: u16) {
    for y in (area.y + top)..area.bottom() {
        for x in area.x..area.right() {
            cells[(x, y)].reset();
        }
    }
}

/// # Returns
///
/// `columns` as a terminal coordinate, saturated at the widest a terminal can be.
fn narrowed(columns: usize) -> u16 {
    u16::try_from(columns).unwrap_or(u16::MAX)
}
