//! The viewport a text is scrolled inside, and the scrolling that moves it.
//!
//! A viewport says what the top left of the screen shows: the logical line its top display row
//! belongs to, the rows of that line hidden above it, and the columns hidden to its left. Every
//! other row is found by walking outwards from there, so a scroll costs the rows it moves rather
//! than the text it moves over, and a text of any length scrolls at the same price.
//!
//! Scrolling counts display rows rather than logical lines, which is a deliberate deviation from
//! vim rather than an oversight. vim's `CTRL-D` scrolls half a window of logical lines, which
//! suits source code, where a logical line is a screen line. A vimbecode paragraph is one logical
//! line that may be wrapped over forty rows, so scrolling it by logical lines would send a single
//! `CTRL-D` past several screens of text. Every command here therefore moves the viewport by rows,
//! which is what neovim's `'smoothscroll'` does and what a reader of a wrapped paragraph expects.
//! Nothing about addressing the text follows the viewport: scrolling moves what is drawn, while a
//! position still names a grapheme of a logical line.
//!
//! `'scrolloff'` is kept only against an edge the viewport can still scroll past. Where it cannot
//! -- at the first row of the text, at its last, or in a window too short to hold the kept rows
//! above the cursor and below it both -- the cursor is let closer to the edge rather than pushed
//! onto a row that is not there.
//!
//! A resize reflows the viewport by keeping the position drawn at the top left, so the text a
//! reader is looking at stays where they are looking. That position's row is found again under the
//! new wrapping, which is why a viewport narrowed and widened back returns to the row it started
//! on wherever both widths break its line in the same place.

use std::num::NonZeroUsize;

use crate::anchor::{
    char_idx_at_visual_offset, visual_offset_from_anchor, Error, VisualOffset, Wrapping,
};
use crate::line::{self, DisplayRow};
use crate::position::LogicalPosition;

/// A scroll of the viewport, named after the vim key that asks for it.
///
/// Every command moves the viewport by display rows. The paging commands carry the cursor along
/// with the viewport, leaving it on the screen row it started on; the row commands leave the
/// cursor in the text until `'scrolloff'` pushes it; and the placing commands leave the cursor
/// alone altogether and move the viewport around it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Command {
    /// `CTRL-D`: half a window of rows further down the text.
    HalfPageDown,

    /// `CTRL-U`: half a window of rows further up the text.
    HalfPageUp,

    /// `CTRL-F`: a whole window of rows further down the text.
    PageDown,

    /// `CTRL-B`: a whole window of rows further up the text.
    PageUp,

    /// `CTRL-E`: one row further down the text.
    RowDown,

    /// `CTRL-Y`: one row further up the text.
    RowUp,

    /// `zz`: the cursor's row to the middle of the window.
    CenterCursor,

    /// `zt`: the cursor's row to the top of the window.
    CursorToTop,

    /// `zb`: the cursor's row to the bottom of the window.
    CursorToBottom,
}

/// The window a text is scrolled inside: the display rows it draws, and how close to the top and
/// the bottom of those rows the cursor may come.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Window {
    height: NonZeroUsize,
    scrolloff: usize,
}

impl Window {
    /// Factory function.
    ///
    /// # Returns
    ///
    /// A newly created window `height` rows tall, keeping no rows beside the cursor.
    #[must_use]
    pub fn new(height: NonZeroUsize) -> Self {
        Self {
            height,
            scrolloff: 0,
        }
    }

    /// # Returns
    ///
    /// This window with `'scrolloff'` asking for the given number of rows.
    #[must_use]
    pub fn with_scrolloff(mut self, rows: usize) -> Self {
        self.scrolloff = rows;
        self
    }

    /// # Returns
    ///
    /// The number of display rows the window draws.
    #[must_use]
    pub fn height(&self) -> NonZeroUsize {
        self.height
    }

    /// # Returns
    ///
    /// The rows `'scrolloff'` asks to be kept between the cursor and an edge.
    #[must_use]
    pub fn scrolloff(&self) -> usize {
        self.scrolloff
    }

    /// # Returns
    ///
    /// The number of display rows the window draws.
    fn rows(&self) -> usize {
        self.height.get()
    }

    /// # Returns
    ///
    /// The rows actually kept between the cursor and an edge the viewport can scroll past, which
    /// is fewer than [`Window::scrolloff`] asks for in a window too short to hold that many rows
    /// above the cursor and below it both.
    fn kept_rows(&self) -> usize {
        self.scrolloff.min((self.rows() - 1) / 2)
    }

    /// # Returns
    ///
    /// The rows a half-page scroll moves, which is at least one however short the window is.
    fn half_page(&self) -> usize {
        (self.rows() / 2).max(1)
    }
}

/// Where a scroll left the screen and the cursor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Scrolled {
    /// The viewport the scroll moved to.
    pub viewport: Viewport,

    /// The cursor, moved only where the scroll carried it along or `'scrolloff'` pushed it.
    pub cursor: LogicalPosition,
}

/// What a viewport shows.
///
/// A viewport is anchored to a logical line rather than to a row of the whole text, so the text
/// above it is never laid out and an edit far away costs nothing to scroll past.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Viewport {
    anchor: usize,
    vertical_offset: usize,
    horizontal_offset: usize,
}

impl Viewport {
    /// Factory function.
    ///
    /// # Returns
    ///
    /// A newly created viewport showing the text from its very first row.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Factory function.
    ///
    /// # Returns
    ///
    /// A newly created viewport whose top display row is the row `rows` rows into the logical line
    /// at `line`, which is where a scroll that stopped part-way down a wrapped line leaves one.
    #[must_use]
    pub fn anchored_at(line: usize, rows: usize) -> Self {
        Self {
            anchor: line,
            vertical_offset: rows,
            horizontal_offset: 0,
        }
    }

    /// # Returns
    ///
    /// This viewport scrolled `columns` columns to the right, which a viewport whose rows are all
    /// wrapped into its own width never is.
    #[must_use]
    pub fn with_horizontal_offset(mut self, columns: usize) -> Self {
        self.horizontal_offset = columns;
        self
    }

    /// # Returns
    ///
    /// The logical line the viewport's top display row belongs to.
    #[must_use]
    pub fn anchor(&self) -> usize {
        self.anchor
    }

    /// # Returns
    ///
    /// The display rows of the anchored line hidden above the top of the viewport.
    #[must_use]
    pub fn vertical_offset(&self) -> usize {
        self.vertical_offset
    }

    /// # Returns
    ///
    /// The display columns hidden to the left of the viewport.
    #[must_use]
    pub fn horizontal_offset(&self) -> usize {
        self.horizontal_offset
    }

    /// # Returns
    ///
    /// The position drawn at the start of the viewport's top display row on success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * Forwards [`rows_of`]'s return values on failure.
    pub fn top_position(
        &self,
        lines: &[String],
        wrapping: &Wrapping,
    ) -> Result<LogicalPosition, Error> {
        let rows = rows_of(lines, self.anchor, wrapping)?;
        let row = &rows[self.vertical_offset.min(rows.len() - 1)];

        Ok(LogicalPosition {
            line: self.anchor,
            grapheme: row.start(),
        })
    }

    /// Reflows the viewport from the wrapping it was laid out under onto another one.
    ///
    /// # Returns
    ///
    /// The viewport drawing the same position at its top left under `to` on success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * Forwards [`Viewport::top_position`]'s return values on failure.
    /// * Forwards [`rows_of`]'s return values on failure.
    pub fn resize(&self, lines: &[String], from: &Wrapping, to: &Wrapping) -> Result<Self, Error> {
        let top = self.top_position(lines, from)?;
        let rows = rows_of(lines, top.line, to)?;

        Ok(Self {
            anchor: top.line,
            vertical_offset: row_index(&rows, top.grapheme),
            horizontal_offset: self.horizontal_offset,
        })
    }

    /// Scrolls the viewport by one command.
    ///
    /// The viewport stops at the first row of the text and at its last, and the cursor is left on
    /// a row that the window draws and the text holds.
    ///
    /// # Returns
    ///
    /// Where the scroll left the screen and the cursor on success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * Forwards [`drawn_at`]'s return values on failure.
    /// * Forwards [`Viewport::paged_by`]'s return values on failure.
    /// * Forwards [`Viewport::scrolled_by`]'s return values on failure.
    /// * Forwards [`Viewport::placed_around`]'s return values on failure.
    /// * Forwards [`Viewport::hold_scrolloff`]'s return values on failure.
    pub fn scroll(
        &self,
        lines: &[String],
        wrapping: &Wrapping,
        window: Window,
        cursor: LogicalPosition,
        command: Command,
    ) -> Result<Scrolled, Error> {
        let drawn = drawn_at(lines, cursor, wrapping)?;
        let half_page = signed(window.half_page());
        let page = signed(window.rows());
        let (viewport, cursor) = match command {
            Command::HalfPageDown => self.paged_by(lines, wrapping, drawn, half_page)?,
            Command::HalfPageUp => self.paged_by(lines, wrapping, drawn, -half_page)?,
            Command::PageDown => self.paged_by(lines, wrapping, drawn, page)?,
            Command::PageUp => self.paged_by(lines, wrapping, drawn, -page)?,
            Command::RowDown => (self.scrolled_by(lines, wrapping, 1)?, cursor),
            Command::RowUp => (self.scrolled_by(lines, wrapping, -1)?, cursor),
            Command::CenterCursor => (
                self.placed_around(lines, wrapping, drawn, (window.rows() - 1) / 2)?,
                cursor,
            ),
            Command::CursorToTop => (
                self.placed_around(lines, wrapping, drawn, window.kept_rows())?,
                cursor,
            ),
            Command::CursorToBottom => (
                self.placed_around(
                    lines,
                    wrapping,
                    drawn,
                    window.rows() - 1 - window.kept_rows(),
                )?,
                cursor,
            ),
        };
        let cursor = viewport.hold_scrolloff(lines, wrapping, window, cursor, drawn.column)?;

        Ok(Scrolled { viewport, cursor })
    }

    /// Scrolls the viewport and carries the cursor along with it.
    ///
    /// The cursor moves by the rows that were asked for rather than by the rows the viewport
    /// managed, so a page scroll that runs into an end of the text still walks the cursor towards
    /// that end instead of leaving it where it was.
    ///
    /// # Returns
    ///
    /// * The viewport `rows` rows away, stopped at the end of the text it ran into.
    /// * The cursor, moved `rows` rows and stopped at the same end.
    ///
    /// on success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * Forwards [`Viewport::scrolled_by`]'s return values on failure.
    /// * Forwards [`moved_by`]'s return values on failure.
    fn paged_by(
        &self,
        lines: &[String],
        wrapping: &Wrapping,
        cursor: Drawn,
        rows: isize,
    ) -> Result<(Self, LogicalPosition), Error> {
        Ok((
            self.scrolled_by(lines, wrapping, rows)?,
            moved_by(lines, wrapping, cursor, rows)?,
        ))
    }

    /// # Returns
    ///
    /// The viewport `rows` rows further down the text, or further up it where `rows` is negative,
    /// stopped at the end of the text it ran into, on success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * Forwards [`Viewport::walked`]'s return values on failure.
    fn scrolled_by(
        &self,
        lines: &[String],
        wrapping: &Wrapping,
        rows: isize,
    ) -> Result<Self, Error> {
        Ok(self.walked(lines, wrapping, rows)?.0)
    }

    /// Moves the viewport by display rows, reporting how far it got.
    ///
    /// # Returns
    ///
    /// * The viewport `rows` rows away, stopped at the end of the text it ran into.
    /// * The rows it moved, which is fewer than were asked for only where it ran into an end.
    ///
    /// on success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * Forwards [`rows_of`]'s return values on failure.
    fn walked(
        &self,
        lines: &[String],
        wrapping: &Wrapping,
        rows: isize,
    ) -> Result<(Self, usize), Error> {
        let mut anchor = self.anchor;
        let mut count = rows_of(lines, anchor, wrapping)?.len();
        let mut offset = self.vertical_offset.min(count - 1);
        let wanted = rows.unsigned_abs();
        let mut moved = 0;

        while moved < wanted && rows.is_negative() {
            let left = wanted - moved;
            if left <= offset {
                offset -= left;
                moved = wanted;
                break;
            }
            if 0 == anchor {
                moved += offset;
                offset = 0;
                break;
            }
            moved += offset + 1;
            anchor -= 1;
            count = rows_of(lines, anchor, wrapping)?.len();
            offset = count - 1;
        }

        while moved < wanted && rows.is_positive() {
            let left = wanted - moved;
            let room = count - 1 - offset;
            if left <= room {
                offset += left;
                moved = wanted;
                break;
            }
            if lines.len() - 1 == anchor {
                moved += room;
                offset = count - 1;
                break;
            }
            moved += room + 1;
            anchor += 1;
            count = rows_of(lines, anchor, wrapping)?.len();
            offset = 0;
        }

        Ok((
            Self {
                anchor,
                vertical_offset: offset,
                horizontal_offset: self.horizontal_offset,
            },
            moved,
        ))
    }

    /// # Returns
    ///
    /// The viewport whose top display row is `rows_above` rows above the row drawing `cursor`, or
    /// the first row of the text where there are fewer rows than that above it, on success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * Forwards [`Viewport::scrolled_by`]'s return values on failure.
    fn placed_around(
        &self,
        lines: &[String],
        wrapping: &Wrapping,
        cursor: Drawn,
        rows_above: usize,
    ) -> Result<Self, Error> {
        let on_the_cursor = Self {
            anchor: cursor.position.line,
            vertical_offset: cursor.row,
            horizontal_offset: self.horizontal_offset,
        };

        on_the_cursor.scrolled_by(lines, wrapping, -signed(rows_above))
    }

    /// Pushes the cursor back inside the rows `'scrolloff'` leaves it.
    ///
    /// The rows are kept only against an edge the viewport can still scroll past, so the cursor is
    /// let onto the first row of the text and onto its last rather than pushed off the text.
    ///
    /// # Returns
    ///
    /// The cursor, moved onto the nearest row it is allowed on and drawn as close to `column` as
    /// that row reaches, on success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * Forwards [`Viewport::top_position`]'s return values on failure.
    /// * Forwards [`Viewport::walked`]'s return values on failure.
    /// * Forwards [`visual_offset_from_anchor`]'s return values on failure.
    /// * Forwards [`char_idx_at_visual_offset`]'s return values on failure.
    fn hold_scrolloff(
        &self,
        lines: &[String],
        wrapping: &Wrapping,
        window: Window,
        cursor: LogicalPosition,
        column: usize,
    ) -> Result<LogicalPosition, Error> {
        let top = self.top_position(lines, wrapping)?;
        let bottom_row = window.rows() - 1;
        let kept = window.kept_rows();
        let (_, rows_below) = self.walked(lines, wrapping, signed(bottom_row) + 1)?;
        let last_allowed = if rows_below <= bottom_row {
            rows_below
        } else {
            bottom_row - kept
        };
        let first_allowed = if 0 == self.anchor && 0 == self.vertical_offset {
            0
        } else {
            kept
        };

        let row = match visual_offset_from_anchor(lines, top, cursor, wrapping, window.rows()) {
            Ok(offset) => offset.rows,
            Err(Error::OutOfView { .. }) => {
                if (cursor.line, cursor.grapheme) < (top.line, top.grapheme) {
                    -1
                } else {
                    signed(window.rows())
                }
            }
            Err(error) => return Err(error),
        };
        let held = row.max(signed(first_allowed)).min(signed(last_allowed));
        if held == row {
            return Ok(cursor);
        }

        let landing =
            char_idx_at_visual_offset(lines, top, VisualOffset { rows: held, column }, wrapping)?;

        Ok(landing.position)
    }
}

/// A position together with where its own line draws it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Drawn {
    position: LogicalPosition,
    row: usize,
    column: usize,
}

/// # Returns
///
/// `position` together with the row of its own line that draws it and the column it is drawn in,
/// on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`Error::GraphemeOutOfBounds`] if the position names a grapheme past the end of its line.
/// * Forwards [`rows_of`]'s return values on failure.
///
/// # Panics
///
/// Panics if a line lays out into no rows, which none does.
fn drawn_at(
    lines: &[String],
    position: LogicalPosition,
    wrapping: &Wrapping,
) -> Result<Drawn, Error> {
    let rows = rows_of(lines, position.line, wrapping)?;
    let line_len = rows
        .last()
        .expect("a line lays out into at least one row")
        .end();
    if line_len < position.grapheme {
        return Err(Error::GraphemeOutOfBounds { position, line_len });
    }
    let index = row_index(&rows, position.grapheme);
    let row = &rows[index];

    Ok(Drawn {
        position,
        row: index,
        column: row.columns()[position.grapheme - row.start()],
    })
}

/// Moves a drawn position by display rows, keeping it as close to the column it was drawn in as
/// the row it lands on reaches.
///
/// # Returns
///
/// The position `rows` rows further down the text, or further up it where `rows` is negative,
/// stopped at the end of the text it ran into, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`char_idx_at_visual_offset`]'s return values on failure.
fn moved_by(
    lines: &[String],
    wrapping: &Wrapping,
    drawn: Drawn,
    rows: isize,
) -> Result<LogicalPosition, Error> {
    if 0 == rows {
        return Ok(drawn.position);
    }

    let landing = char_idx_at_visual_offset(
        lines,
        LogicalPosition {
            line: drawn.position.line,
            grapheme: 0,
        },
        VisualOffset {
            rows: signed(drawn.row) + rows,
            column: drawn.column,
        },
        wrapping,
    )?;

    Ok(landing.position)
}

/// # Returns
///
/// The rows rendering the line at `line` on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`Error::LineOutOfBounds`] if the text holds no such line.
fn rows_of(lines: &[String], line: usize, wrapping: &Wrapping) -> Result<Vec<DisplayRow>, Error> {
    let text = lines.get(line).ok_or(Error::LineOutOfBounds {
        line,
        line_count: lines.len(),
    })?;

    Ok(line::lay_out(
        line,
        text,
        wrapping.width(),
        wrapping.metrics(),
        wrapping.options(),
    ))
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

/// # Returns
///
/// `count` as a signed row count.
///
/// # Panics
///
/// Panics if `count` does not fit in an `isize`.
fn signed(count: usize) -> isize {
    isize::try_from(count).expect("a row count fits in an `isize`")
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::line::Options;
    use crate::width::Metrics;

    /// The commands a paragraph is scrolled through by, so that a scroll of every kind is checked
    /// against the same text.
    const EVERY_COMMAND: [Command; 9] = [
        Command::HalfPageDown,
        Command::HalfPageUp,
        Command::PageDown,
        Command::PageUp,
        Command::RowDown,
        Command::RowUp,
        Command::CenterCursor,
        Command::CursorToTop,
        Command::CursorToBottom,
    ];

    /// # Returns
    ///
    /// A wrapping drawing rows `width` columns wide, with vim's own defaults for everything else.
    fn wrapping(width: usize) -> Wrapping {
        Wrapping::new(
            NonZeroUsize::new(width).expect("a test's width is not zero"),
            Metrics::default(),
            Options::new(),
        )
    }

    /// # Returns
    ///
    /// A window `height` rows tall, keeping no rows beside the cursor.
    fn window(height: usize) -> Window {
        Window::new(NonZeroUsize::new(height).expect("a test's height is not zero"))
    }

    /// # Returns
    ///
    /// A text holding `lines`.
    fn text(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|&line| line.to_owned()).collect()
    }

    /// # Returns
    ///
    /// A text of `count` paragraphs, each one logical line of `graphemes` graphemes drawn from a
    /// letter of its own, so that a row can be told apart from the rows of every other paragraph.
    fn paragraphs(count: usize, graphemes: usize) -> Vec<String> {
        (0..count)
            .map(|index| {
                let letter =
                    char::from(b'a' + u8::try_from(index).expect("a test's text is short"));
                std::iter::repeat_n(letter, graphemes).collect()
            })
            .collect()
    }

    /// # Returns
    ///
    /// A text of `count` lines, each short enough to be drawn on one row.
    fn short_lines(count: usize) -> Vec<String> {
        (0..count).map(|index| format!("line{index}")).collect()
    }

    /// # Returns
    ///
    /// The position of the grapheme at `grapheme` on the line at `line`.
    fn at(line: usize, grapheme: usize) -> LogicalPosition {
        LogicalPosition { line, grapheme }
    }

    /// # Returns
    ///
    /// Where the given scroll left the screen and the cursor.
    ///
    /// # Panics
    ///
    /// Panics if the scroll failed.
    fn scroll(
        lines: &[String],
        wrapping: &Wrapping,
        window: Window,
        from: Scrolled,
        command: Command,
    ) -> Scrolled {
        from.viewport
            .scroll(lines, wrapping, window, from.cursor, command)
            .expect("a test's scroll is taken")
    }

    /// # Returns
    ///
    /// The screen and cursor a run of `commands` from the top of the text leaves.
    ///
    /// # Panics
    ///
    /// Panics if a scroll failed.
    fn scroll_all(
        lines: &[String],
        wrapping: &Wrapping,
        window: Window,
        commands: &[Command],
    ) -> Scrolled {
        let mut state = Scrolled {
            viewport: Viewport::new(),
            cursor: at(0, 0),
        };
        for &command in commands {
            state = scroll(lines, wrapping, window, state, command);
        }

        state
    }

    /// # Returns
    ///
    /// The position drawn at the start of a viewport's top row.
    ///
    /// # Panics
    ///
    /// Panics if the viewport's top row is not drawn.
    fn top_of(viewport: Viewport, lines: &[String], wrapping: &Wrapping) -> LogicalPosition {
        viewport
            .top_position(lines, wrapping)
            .expect("a test's viewport is drawn")
    }

    #[test]
    fn a_half_page_scrolls_display_rows_rather_than_logical_lines() {
        let lines = paragraphs(4, 200);
        let wrapping = wrapping(20);
        let window = window(10);

        let once = scroll_all(&lines, &wrapping, window, &[Command::HalfPageDown]);

        assert_eq!(
            Viewport {
                anchor: 0,
                vertical_offset: 5,
                horizontal_offset: 0
            },
            once.viewport,
            "half a window of a ten-row window is five rows, and a two hundred grapheme paragraph \
             wrapped at twenty columns holds ten of them, so the scroll stays inside its first \
             logical line"
        );
        assert_eq!(at(0, 100), top_of(once.viewport, &lines, &wrapping));
        assert_eq!(at(0, 100), once.cursor);

        let twice = scroll(&lines, &wrapping, window, once, Command::HalfPageDown);

        assert_eq!(
            Viewport {
                anchor: 1,
                vertical_offset: 0,
                horizontal_offset: 0
            },
            twice.viewport,
            "ten rows of scrolling crosses exactly one ten-row paragraph"
        );
        assert_eq!(at(1, 0), top_of(twice.viewport, &lines, &wrapping));
    }

    #[test]
    fn a_whole_page_scrolls_display_rows_rather_than_logical_lines() {
        let lines = paragraphs(4, 200);
        let wrapping = wrapping(20);

        let scrolled = scroll_all(&lines, &wrapping, window(6), &[Command::PageDown]);

        assert_eq!(
            Viewport {
                anchor: 0,
                vertical_offset: 6,
                horizontal_offset: 0
            },
            scrolled.viewport
        );
        assert_eq!(at(0, 120), top_of(scrolled.viewport, &lines, &wrapping));
    }

    #[test]
    fn one_row_at_a_time_walks_the_rows_of_a_wrapped_line() {
        let lines = paragraphs(2, 60);
        let wrapping = wrapping(20);
        let window = window(4);

        let down = scroll_all(
            &lines,
            &wrapping,
            window,
            &[Command::RowDown, Command::RowDown],
        );

        assert_eq!(2, down.viewport.vertical_offset());
        assert_eq!(0, down.viewport.anchor());
        assert_eq!(at(0, 40), top_of(down.viewport, &lines, &wrapping));

        let up = scroll(&lines, &wrapping, window, down, Command::RowUp);

        assert_eq!(1, up.viewport.vertical_offset());
        assert_eq!(at(0, 20), top_of(up.viewport, &lines, &wrapping));
    }

    #[test]
    fn scrolling_stops_at_the_first_row_of_the_text() {
        let lines = paragraphs(3, 60);
        let wrapping = wrapping(20);
        let window = window(4);

        let scrolled = scroll_all(
            &lines,
            &wrapping,
            window,
            &[Command::RowDown, Command::PageUp, Command::PageUp],
        );

        assert_eq!(Viewport::new(), scrolled.viewport);
        assert_eq!(at(0, 0), scrolled.cursor);
    }

    #[test]
    fn scrolling_stops_with_the_last_row_of_the_text_at_the_top() {
        let lines = paragraphs(2, 60);
        let wrapping = wrapping(20);

        let scrolled = scroll_all(
            &lines,
            &wrapping,
            window(4),
            &[Command::PageDown, Command::PageDown, Command::PageDown],
        );

        assert_eq!(
            Viewport {
                anchor: 1,
                vertical_offset: 2,
                horizontal_offset: 0
            },
            scrolled.viewport
        );
        assert_eq!(at(1, 40), top_of(scrolled.viewport, &lines, &wrapping));
        assert_eq!(at(1, 40), scrolled.cursor);
    }

    #[test]
    fn a_page_scroll_carries_the_cursor_with_the_screen() {
        let lines = paragraphs(3, 200);
        let wrapping = wrapping(20);
        let window = window(10);

        let scrolled = scroll_all(&lines, &wrapping, window, &[Command::HalfPageDown]);

        assert_eq!(at(0, 100), scrolled.cursor, "the cursor moved five rows");

        let back = scroll(&lines, &wrapping, window, scrolled, Command::HalfPageUp);

        assert_eq!(Viewport::new(), back.viewport);
        assert_eq!(at(0, 0), back.cursor);
    }

    #[test]
    fn a_row_scroll_leaves_the_cursor_where_it_was() {
        let lines = paragraphs(2, 200);
        let wrapping = wrapping(20);

        let scrolled = Viewport::new()
            .scroll(&lines, &wrapping, window(10), at(0, 60), Command::RowDown)
            .expect("a test's scroll is taken");

        assert_eq!(1, scrolled.viewport.vertical_offset());
        assert_eq!(
            at(0, 60),
            scrolled.cursor,
            "the screen moved under a cursor that is still on it"
        );
    }

    #[test]
    fn a_row_scroll_pushes_a_cursor_the_screen_left_behind() {
        let lines = paragraphs(2, 200);
        let wrapping = wrapping(20);

        let scrolled = scroll_all(&lines, &wrapping, window(10), &[Command::RowDown]);

        assert_eq!(1, scrolled.viewport.vertical_offset());
        assert_eq!(
            at(0, 20),
            scrolled.cursor,
            "the cursor was on the row the scroll hid, so it moved onto the new top row"
        );
    }

    #[test]
    fn resizing_to_a_narrower_viewport_and_back_returns_the_same_anchor() {
        let lines = paragraphs(3, 500);
        let wide = wrapping(120);
        let narrow = wrapping(20);
        let scrolled = scroll_all(
            &lines,
            &wide,
            window(10),
            &[Command::RowDown, Command::RowDown],
        );
        let anchored = scrolled.viewport;
        let top = top_of(anchored, &lines, &wide);

        let narrowed = anchored
            .resize(&lines, &wide, &narrow)
            .expect("a test's viewport is reflowed");

        assert_eq!(
            top,
            top_of(narrowed, &lines, &narrow),
            "the position at the top left is what a reflow keeps"
        );
        assert_eq!(12, narrowed.vertical_offset());

        let widened = narrowed
            .resize(&lines, &narrow, &wide)
            .expect("a test's viewport is reflowed");

        assert_eq!(anchored, widened);
        assert_eq!(top, top_of(widened, &lines, &wide));
    }

    #[test]
    fn a_resize_keeps_the_horizontal_offset() {
        let lines = paragraphs(1, 500);
        let wide = wrapping(120);
        let narrow = wrapping(20);
        let viewport = Viewport::new().with_horizontal_offset(7);

        let narrowed = viewport
            .resize(&lines, &wide, &narrow)
            .expect("a test's viewport is reflowed");

        assert_eq!(7, narrowed.horizontal_offset());
    }

    #[test]
    fn a_scroll_keeps_the_horizontal_offset() {
        let lines = paragraphs(2, 200);
        let wrapping = wrapping(20);
        let window = window(10);
        let viewport = Viewport::new().with_horizontal_offset(4);

        for command in EVERY_COMMAND {
            let scrolled = viewport
                .scroll(&lines, &wrapping, window, at(0, 0), command)
                .expect("a test's scroll is taken");

            assert_eq!(
                4,
                scrolled.viewport.horizontal_offset(),
                "{command:?} moved the viewport sideways"
            );
        }
    }

    #[test]
    fn scrolloff_holds_the_cursor_away_from_the_top_of_the_window() {
        let lines = short_lines(20);
        let wrapping = wrapping(20);
        let window = window(10).with_scrolloff(3);

        let scrolled = scroll_all(&lines, &wrapping, window, &[Command::RowDown]);

        assert_eq!(at(1, 0), top_of(scrolled.viewport, &lines, &wrapping));
        assert_eq!(
            at(4, 0),
            scrolled.cursor,
            "the cursor is pushed three rows below a top the viewport can still scroll past"
        );
    }

    #[test]
    fn scrolloff_does_not_hold_the_cursor_off_the_first_row_of_the_text() {
        let lines = short_lines(20);
        let wrapping = wrapping(20);
        let window = window(10).with_scrolloff(3);

        let held = scroll_all(&lines, &wrapping, window, &[Command::RowUp]);

        assert_eq!(Viewport::new(), held.viewport);
        assert_eq!(
            at(0, 0),
            held.cursor,
            "no rows are kept above a cursor on the first row of the text"
        );

        let returned = scroll_all(
            &lines,
            &wrapping,
            window,
            &[Command::RowDown, Command::RowUp],
        );

        assert_eq!(Viewport::new(), returned.viewport);
        assert_eq!(
            at(4, 0),
            returned.cursor,
            "the cursor the first scroll pushed down is not pulled back up by the second"
        );
    }

    #[test]
    fn scrolloff_does_not_hold_the_cursor_off_the_last_row_of_the_text() {
        let lines = short_lines(12);
        let wrapping = wrapping(20);
        let window = window(10).with_scrolloff(3);

        let scrolled = scroll_all(&lines, &wrapping, window, &[Command::PageDown]);

        assert_eq!(at(10, 0), top_of(scrolled.viewport, &lines, &wrapping));
        assert_eq!(
            at(11, 0),
            scrolled.cursor,
            "the last row of the text is one below the top, and no row below it exists to keep"
        );
    }

    #[test]
    fn scrolloff_keeps_no_rows_below_the_last_row_of_the_text() {
        let lines = short_lines(14);
        let wrapping = wrapping(20);
        let window = window(10).with_scrolloff(3);

        let scrolled = scroll_all(
            &lines,
            &wrapping,
            window,
            &[
                Command::PageDown,
                Command::RowUp,
                Command::RowUp,
                Command::RowUp,
                Command::RowUp,
            ],
        );

        assert_eq!(at(6, 0), top_of(scrolled.viewport, &lines, &wrapping));
        assert_eq!(
            at(13, 0),
            scrolled.cursor,
            "the cursor sits seven rows below the top, past the row scrolloff would keep, because \
             the rows below it are not text but the end of the text"
        );
    }

    #[test]
    fn scrolloff_degrades_in_a_window_shorter_than_the_rows_it_asks_for() {
        let lines = short_lines(20);
        let wrapping = wrapping(20);
        let window = window(3).with_scrolloff(5);

        let scrolled = scroll_all(&lines, &wrapping, window, &[Command::RowDown]);

        assert_eq!(at(1, 0), top_of(scrolled.viewport, &lines, &wrapping));
        assert_eq!(
            at(2, 0),
            scrolled.cursor,
            "a three row window keeps one row beside the cursor, not the five that were asked for"
        );
    }

    #[test]
    fn scrolloff_degrades_in_a_window_taller_than_the_text() {
        let lines = short_lines(3);
        let wrapping = wrapping(20);
        let window = window(10).with_scrolloff(4);

        let scrolled = scroll_all(&lines, &wrapping, window, &[Command::RowDown]);

        assert_eq!(at(1, 0), top_of(scrolled.viewport, &lines, &wrapping));
        assert_eq!(
            at(2, 0),
            scrolled.cursor,
            "the cursor stops on the last row of the text rather than on the row scrolloff asks for"
        );
    }

    #[test]
    fn the_cursor_is_put_at_the_top_of_the_window_with_wrapped_lines_present() {
        let lines = paragraphs(4, 100);
        let wrapping = wrapping(20);

        let scrolled = Viewport::new()
            .scroll(
                &lines,
                &wrapping,
                window(7),
                at(2, 50),
                Command::CursorToTop,
            )
            .expect("a test's scroll is taken");

        assert_eq!(
            Viewport {
                anchor: 2,
                vertical_offset: 2,
                horizontal_offset: 0
            },
            scrolled.viewport
        );
        assert_eq!(at(2, 40), top_of(scrolled.viewport, &lines, &wrapping));
        assert_eq!(at(2, 50), scrolled.cursor, "a placing scroll moves nothing");
    }

    #[test]
    fn the_cursor_is_put_in_the_middle_of_the_window_with_wrapped_lines_present() {
        let lines = paragraphs(4, 100);
        let wrapping = wrapping(20);

        let scrolled = Viewport::new()
            .scroll(
                &lines,
                &wrapping,
                window(7),
                at(2, 50),
                Command::CenterCursor,
            )
            .expect("a test's scroll is taken");

        assert_eq!(
            Viewport {
                anchor: 1,
                vertical_offset: 4,
                horizontal_offset: 0
            },
            scrolled.viewport,
            "three rows above the cursor's row leaves the middle row of seven"
        );
        assert_eq!(at(1, 80), top_of(scrolled.viewport, &lines, &wrapping));
        assert_eq!(at(2, 50), scrolled.cursor);
    }

    #[test]
    fn the_cursor_is_put_at_the_bottom_of_the_window_with_wrapped_lines_present() {
        let lines = paragraphs(4, 100);
        let wrapping = wrapping(20);

        let scrolled = Viewport::new()
            .scroll(
                &lines,
                &wrapping,
                window(7),
                at(2, 50),
                Command::CursorToBottom,
            )
            .expect("a test's scroll is taken");

        assert_eq!(
            Viewport {
                anchor: 1,
                vertical_offset: 1,
                horizontal_offset: 0
            },
            scrolled.viewport,
            "six rows above the cursor's row leaves the last row of seven"
        );
        assert_eq!(at(1, 20), top_of(scrolled.viewport, &lines, &wrapping));
        assert_eq!(at(2, 50), scrolled.cursor);
    }

    #[test]
    fn a_placing_scroll_leaves_the_rows_scrolloff_asks_for_beside_the_cursor() {
        let lines = paragraphs(4, 100);
        let wrapping = wrapping(20);
        let window = window(7).with_scrolloff(2);

        let top = Viewport::new()
            .scroll(&lines, &wrapping, window, at(2, 50), Command::CursorToTop)
            .expect("a test's scroll is taken");

        assert_eq!(
            at(2, 0),
            top_of(top.viewport, &lines, &wrapping),
            "the cursor's row is put two rows below the top rather than on it"
        );
        assert_eq!(at(2, 50), top.cursor);

        let bottom = Viewport::new()
            .scroll(
                &lines,
                &wrapping,
                window,
                at(2, 50),
                Command::CursorToBottom,
            )
            .expect("a test's scroll is taken");

        assert_eq!(at(1, 60), top_of(bottom.viewport, &lines, &wrapping));
        assert_eq!(at(2, 50), bottom.cursor);
    }

    #[test]
    fn a_placing_scroll_stops_at_the_first_row_of_the_text() {
        let lines = paragraphs(3, 100);
        let wrapping = wrapping(20);

        let scrolled = Viewport::new()
            .scroll(
                &lines,
                &wrapping,
                window(9),
                at(0, 10),
                Command::CursorToBottom,
            )
            .expect("a test's scroll is taken");

        assert_eq!(Viewport::new(), scrolled.viewport);
        assert_eq!(at(0, 10), scrolled.cursor);
    }

    #[test]
    fn a_scroll_of_a_text_of_one_short_line_moves_nothing() {
        let lines = text(&["short"]);
        let wrapping = wrapping(20);
        let window = window(5).with_scrolloff(2);

        for command in EVERY_COMMAND {
            let scrolled = Viewport::new()
                .scroll(&lines, &wrapping, window, at(0, 3), command)
                .expect("a test's scroll is taken");

            assert_eq!(
                Viewport::new(),
                scrolled.viewport,
                "{command:?} moved a viewport with nowhere to go"
            );
            assert_eq!(at(0, 3), scrolled.cursor, "{command:?} moved the cursor");
        }
    }

    #[test]
    fn a_scroll_reports_a_cursor_the_text_does_not_hold() {
        let lines = text(&["short"]);
        let wrapping = wrapping(20);

        assert_eq!(
            Err(Error::GraphemeOutOfBounds {
                position: at(0, 9),
                line_len: 5
            }),
            Viewport::new().scroll(
                &lines,
                &wrapping,
                window(5),
                at(0, 9),
                Command::HalfPageDown
            )
        );
        assert_eq!(
            Err(Error::LineOutOfBounds {
                line: 4,
                line_count: 1
            }),
            Viewport::new().scroll(
                &lines,
                &wrapping,
                window(5),
                at(4, 0),
                Command::HalfPageDown
            )
        );
    }
}
