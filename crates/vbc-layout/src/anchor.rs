//! Mapping between a position in the text and the cell that draws it, in both directions.
//!
//! Every mapping is anchored: it starts from a position whose display row the caller already
//! knows -- the top left of the viewport, in a renderer -- and walks outwards from it one logical
//! line at a time, laying each out and throwing it away again. Nothing is laid out ahead of time,
//! nothing is remembered between calls, and a walk stops as soon as it has covered more rows than
//! the caller asked to look at, so a mapping costs what the rows between the anchor and the
//! position cost rather than what the text around them costs. A renderer only ever asks about the
//! rows it is drawing, which is what leaves the rest of the text untouched however long it grows.
//!
//! A position addressing the grapheme past the end of a line -- where `A` leaves the cursor -- is
//! drawn in the cell after that line's last. Where the line's last row has no such cell left, the
//! position is drawn at the start of the row below, which is the next logical line's own first
//! row and is where vim draws the same cursor.
//!
//! An anchor is measured from the row that holds its own grapheme, which is its line's last row
//! for a position past the end of that line even where that position is itself drawn on the row
//! below. Both directions read the anchor's row that way, so they compose, but an anchor resting
//! past the end of a full row is reported one row above the cell it is drawn in: a caller that
//! anchors on the cursor rather than on the top of its viewport gets offsets measured from the
//! cursor's line rather than from the cursor's cell.

use std::error::Error as StdError;
use std::fmt::{Display, Formatter, Result as FmtResult};
use std::num::NonZeroUsize;

use crate::invariants::LogicalPosition;
use crate::line::{self, DisplayRow, Options};
use crate::width::Metrics;

/// The ways a position can fail to be mapped.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    /// A position named a line the text does not hold.
    LineOutOfBounds {
        /// The line that was named.
        line: usize,

        /// The number of lines the text holds.
        line_count: usize,
    },

    /// A position named a grapheme past the end of its line.
    GraphemeOutOfBounds {
        /// The position that was named.
        position: LogicalPosition,

        /// The number of graphemes the line holds.
        line_len: usize,
    },

    /// A position is drawn further from the anchor than the caller asked to look.
    OutOfView {
        /// The number of rows the walk had covered when it gave up.
        rows: usize,

        /// The number of rows the caller asked to look over.
        max_rows: usize,
    },
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        match self {
            Self::LineOutOfBounds { line, line_count } => {
                write!(f, "line {line} of a text holding {line_count} lines")
            }
            Self::GraphemeOutOfBounds { position, line_len } => {
                write!(f, "{position} of a line holding {line_len} graphemes")
            }
            Self::OutOfView { rows, max_rows } => write!(
                f,
                "a position {rows} rows from the anchor, past the {max_rows} rows asked for"
            ),
        }
    }
}

impl StdError for Error {}

/// How text is drawn: the columns a display row occupies, the metrics the text is measured under,
/// and the options it is wrapped by.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Wrapping {
    width: NonZeroUsize,
    metrics: Metrics,
    options: Options,
}

impl Wrapping {
    /// Factory function.
    ///
    /// # Returns
    ///
    /// A newly created wrapping drawing rows `width` columns wide, measuring their text under
    /// `metrics`, and wrapping it as `options` says.
    #[must_use]
    pub fn new(width: NonZeroUsize, metrics: Metrics, options: Options) -> Self {
        Self {
            width,
            metrics,
            options,
        }
    }

    /// # Returns
    ///
    /// The number of columns a display row occupies.
    #[must_use]
    pub fn width(&self) -> NonZeroUsize {
        self.width
    }

    /// # Returns
    ///
    /// The metrics text is measured under.
    #[must_use]
    pub fn metrics(&self) -> Metrics {
        self.metrics
    }

    /// # Returns
    ///
    /// The options text is wrapped by.
    #[must_use]
    pub fn options(&self) -> &Options {
        &self.options
    }

    /// # Returns
    ///
    /// The rows rendering `line` under this wrapping.
    fn lay_out(&self, line: &str) -> Vec<DisplayRow> {
        line::lay_out(line, self.width, self.metrics, &self.options)
    }
}

/// Where a position is drawn relative to an anchor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VisualOffset {
    /// The display rows between the row drawing the anchor and the row drawing the position,
    /// negative where the position is drawn above the anchor.
    pub rows: isize,

    /// The column of that row the position is drawn in, counted from the left of the viewport.
    pub column: usize,
}

impl Display for VisualOffset {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        write!(f, "offset(rows {}, column {})", self.rows, self.column)
    }
}

/// Where a walk to a visual offset ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Landing {
    /// The position drawn there.
    pub position: LogicalPosition,

    /// The offset the position is drawn at, which is the offset that was asked for except where
    /// the walk ran off the end of the text or the column fell inside a grapheme rather than at
    /// its start.
    pub offset: VisualOffset,
}

/// Maps a position onto the screen, relative to an anchor whose row the caller already knows.
///
/// The walk lays out the anchor's line, the position's line, and the lines between them, and gives
/// up as soon as those come to more than `max_rows` rows, so the lines past that point are never
/// laid out.
///
/// # Returns
///
/// Where `position` is drawn relative to `anchor` on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`rows_of`]'s return values on failure.
/// * Forwards [`check_grapheme`]'s return values on failure.
/// * Forwards [`check_reach`]'s return values on failure.
/// * Forwards [`in_view`]'s return values on failure.
pub fn visual_offset_from_anchor(
    lines: &[String],
    anchor: LogicalPosition,
    position: LogicalPosition,
    wrapping: &Wrapping,
    max_rows: usize,
) -> Result<VisualOffset, Error> {
    let width = wrapping.width.get();
    let anchor_rows = rows_of(lines, anchor.line, wrapping)?;
    check_grapheme(&anchor_rows, anchor)?;
    let anchor_row = row_index(&anchor_rows, anchor.grapheme);

    if position.line == anchor.line {
        check_grapheme(&anchor_rows, position)?;
        let (row, column) = place(&anchor_rows, position.grapheme, width);
        return in_view(signed(row) - signed(anchor_row), column, max_rows);
    }

    let position_rows = rows_of(lines, position.line, wrapping)?;
    check_grapheme(&position_rows, position)?;
    let (row, column) = place(&position_rows, position.grapheme, width);
    let first_row = if anchor.line < position.line {
        let mut walked = anchor_rows.len() - anchor_row;
        for line in (anchor.line + 1)..position.line {
            check_reach(walked, max_rows)?;
            walked += rows_of(lines, line, wrapping)?.len();
        }
        signed(walked)
    } else {
        let mut walked = anchor_row;
        for line in ((position.line + 1)..anchor.line).rev() {
            check_reach(walked, max_rows)?;
            walked += rows_of(lines, line, wrapping)?.len();
        }
        -signed(walked + position_rows.len())
    };

    in_view(first_row + signed(row), column, max_rows)
}

/// Maps a cell of the screen back into the text, relative to an anchor whose row the caller
/// already knows.
///
/// A cell the text does not reach -- one past the end of a row, or past the end of the text --
/// lands on the nearest position that is drawn, and the offset the landing reports says where
/// that position is drawn rather than repeating what was asked for.
///
/// # Returns
///
/// Where the walk to `offset` ended on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`rows_of`]'s return values on failure.
/// * Forwards [`check_grapheme`]'s return values on failure.
pub fn char_idx_at_visual_offset(
    lines: &[String],
    anchor: LogicalPosition,
    offset: VisualOffset,
    wrapping: &Wrapping,
) -> Result<Landing, Error> {
    let mut rows = rows_of(lines, anchor.line, wrapping)?;
    check_grapheme(&rows, anchor)?;
    let mut line = anchor.line;
    let mut row = row_index(&rows, anchor.grapheme);
    let mut remaining = offset.rows;
    let mut moved = 0;

    while 0 != remaining {
        let wanted = remaining.unsigned_abs();
        if 0 < remaining {
            let room = rows.len() - 1 - row;
            if wanted <= room {
                row += wanted;
                moved += remaining;
                break;
            }
            if lines.len() - 1 == line {
                row = rows.len() - 1;
                moved += signed(room);
                break;
            }
            remaining -= signed(room + 1);
            moved += signed(room + 1);
            line += 1;
            rows = rows_of(lines, line, wrapping)?;
            row = 0;
        } else {
            if wanted <= row {
                row -= wanted;
                moved += remaining;
                break;
            }
            if 0 == line {
                moved -= signed(row);
                row = 0;
                break;
            }
            remaining += signed(row + 1);
            moved -= signed(row + 1);
            line -= 1;
            rows = rows_of(lines, line, wrapping)?;
            row = rows.len() - 1;
        }
    }

    let drawn = &rows[row];
    let index = grapheme_at(drawn, offset.column, rows.len() - 1 == row);

    Ok(Landing {
        position: LogicalPosition {
            line,
            grapheme: drawn.start() + index,
        },
        offset: VisualOffset {
            rows: moved,
            column: drawn.columns()[index],
        },
    })
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

    Ok(wrapping.lay_out(text))
}

/// Checks that a position addresses a grapheme of the line `rows` renders, or the position just
/// past that line's last grapheme.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`Error::GraphemeOutOfBounds`] if the line is shorter than the position's grapheme offset.
///
/// # Panics
///
/// Panics if `rows` is empty, which no line lays out into.
fn check_grapheme(rows: &[DisplayRow], position: LogicalPosition) -> Result<(), Error> {
    let line_len = rows
        .last()
        .expect("a line lays out into at least one row")
        .end();
    if line_len < position.grapheme {
        return Err(Error::GraphemeOutOfBounds { position, line_len });
    }

    Ok(())
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

/// Places one grapheme of a line on the rows rendering it.
///
/// # Returns
///
/// * The index of the row the grapheme is drawn on, which is one past the line's last row for a
///   position past the end of a line whose last row is full.
/// * The column of that row the grapheme is drawn in.
fn place(rows: &[DisplayRow], grapheme: usize, width: usize) -> (usize, usize) {
    let index = row_index(rows, grapheme);
    let row = &rows[index];
    let column = row.columns()[grapheme - row.start()];
    if row.end() == grapheme && width <= column {
        return (index + 1, 0);
    }

    (index, column)
}

/// # Returns
///
/// The offset within a row's own graphemes of the one drawn at `column`: the grapheme `column`
/// falls inside where no grapheme starts there, and the nearest position the row draws where the
/// row has no cell at `column` at all.
fn grapheme_at(row: &DisplayRow, column: usize, last_of_line: bool) -> usize {
    let columns = row.columns();
    let past_the_text = columns.len() - 1;
    if columns[past_the_text] <= column {
        return if last_of_line {
            past_the_text
        } else {
            past_the_text - 1
        };
    }

    let index = columns.partition_point(|&start| start < column);
    if column == columns[index] {
        return index;
    }

    index.saturating_sub(1)
}

/// # Returns
///
/// The offset `rows` and `column` name on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`check_reach`]'s return values on failure.
fn in_view(rows: isize, column: usize, max_rows: usize) -> Result<VisualOffset, Error> {
    check_reach(rows.unsigned_abs(), max_rows)?;

    Ok(VisualOffset { rows, column })
}

/// Checks that a walk of `rows` rows is one the caller asked to look over.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`Error::OutOfView`] if `rows` is more rows than `max_rows`.
fn check_reach(rows: usize, max_rows: usize) -> Result<(), Error> {
    if max_rows < rows {
        return Err(Error::OutOfView { rows, max_rows });
    }

    Ok(())
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

    use crate::width::graphemes;

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
    /// A text holding `lines`.
    fn text(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|&line| line.to_owned()).collect()
    }

    /// # Returns
    ///
    /// The position of the grapheme at `grapheme` on the line at `line`.
    fn at(line: usize, grapheme: usize) -> LogicalPosition {
        LogicalPosition { line, grapheme }
    }

    /// # Returns
    ///
    /// The offset `rows` rows from the anchor's row, in the column at `column`.
    fn offset(rows: isize, column: usize) -> VisualOffset {
        VisualOffset { rows, column }
    }

    /// # Returns
    ///
    /// Where `position` is drawn relative to `anchor`.
    ///
    /// # Panics
    ///
    /// Panics if the position is not mapped.
    fn drawn_at(
        lines: &[String],
        anchor: LogicalPosition,
        position: LogicalPosition,
        wrapping: &Wrapping,
    ) -> VisualOffset {
        visual_offset_from_anchor(lines, anchor, position, wrapping, usize::MAX)
            .expect("a test's position is mapped")
    }

    /// # Returns
    ///
    /// Where the walk to `offset` from `anchor` ends.
    ///
    /// # Panics
    ///
    /// Panics if the offset is not mapped.
    fn landed_at(
        lines: &[String],
        anchor: LogicalPosition,
        offset: VisualOffset,
        wrapping: &Wrapping,
    ) -> Landing {
        char_idx_at_visual_offset(lines, anchor, offset, wrapping)
            .expect("a test's offset is mapped")
    }

    #[test]
    fn a_position_on_the_anchor_row_is_offset_by_its_column_alone() {
        let lines = text(&["abcde", "fg"]);
        let wrapping = wrapping(5);

        assert_eq!(
            offset(0, 3),
            drawn_at(&lines, at(0, 0), at(0, 3), &wrapping)
        );
        assert_eq!(
            offset(0, 2),
            drawn_at(&lines, at(0, 1), at(0, 2), &wrapping)
        );
    }

    #[test]
    fn a_line_too_long_for_its_row_continues_on_the_row_below() {
        let lines = text(&["abcdefg"]);
        let wrapping = wrapping(5);

        assert_eq!(
            offset(1, 0),
            drawn_at(&lines, at(0, 0), at(0, 5), &wrapping)
        );
        assert_eq!(
            offset(1, 1),
            drawn_at(&lines, at(0, 0), at(0, 6), &wrapping)
        );
        assert_eq!(
            at(0, 6),
            landed_at(&lines, at(0, 0), offset(1, 1), &wrapping).position
        );
    }

    #[test]
    fn an_anchor_is_measured_from_the_row_that_holds_its_grapheme() {
        let lines = text(&["abcde", "fg"]);
        let wrapping = wrapping(5);
        let anchor = at(0, 5);

        assert_eq!(offset(1, 0), drawn_at(&lines, anchor, anchor, &wrapping));
        assert_eq!(
            Landing {
                position: at(0, 0),
                offset: offset(0, 0)
            },
            landed_at(&lines, anchor, offset(0, 0), &wrapping)
        );
    }

    #[test]
    fn a_walk_runs_backwards_as_well_as_forwards() {
        let lines = text(&["ab", "cd", "ef"]);
        let wrapping = wrapping(5);
        let anchor = at(2, 0);

        assert_eq!(offset(-2, 1), drawn_at(&lines, anchor, at(0, 1), &wrapping));
        assert_eq!(offset(-1, 0), drawn_at(&lines, anchor, at(1, 0), &wrapping));
        assert_eq!(
            at(0, 1),
            landed_at(&lines, anchor, offset(-2, 1), &wrapping).position
        );
    }

    #[test]
    fn a_walk_past_the_end_of_the_text_stops_on_its_last_row() {
        let lines = text(&["ab", "cd"]);
        let wrapping = wrapping(5);

        assert_eq!(
            Landing {
                position: at(1, 0),
                offset: offset(1, 0)
            },
            landed_at(&lines, at(0, 0), offset(9, 0), &wrapping)
        );
        assert_eq!(
            Landing {
                position: at(0, 0),
                offset: offset(-1, 0)
            },
            landed_at(&lines, at(1, 0), offset(-9, 0), &wrapping)
        );
    }

    #[test]
    fn the_position_past_the_end_of_a_full_row_is_drawn_on_the_row_below() {
        let lines = text(&["abcde", "fg"]);
        let wrapping = wrapping(5);

        assert_eq!(
            offset(1, 0),
            drawn_at(&lines, at(0, 0), at(0, 5), &wrapping)
        );
        assert_eq!(
            offset(1, 2),
            drawn_at(&lines, at(0, 0), at(1, 2), &wrapping)
        );
    }

    #[test]
    fn a_tab_is_drawn_across_the_cells_it_advances_by() {
        let lines = text(&["\tx"]);
        let wrapping = wrapping(20);

        assert_eq!(
            offset(0, 0),
            drawn_at(&lines, at(0, 0), at(0, 0), &wrapping)
        );
        assert_eq!(
            offset(0, 8),
            drawn_at(&lines, at(0, 0), at(0, 1), &wrapping)
        );
        assert_eq!(
            Landing {
                position: at(0, 0),
                offset: offset(0, 0)
            },
            landed_at(&lines, at(0, 0), offset(0, 4), &wrapping)
        );
    }

    #[test]
    fn a_cell_inside_a_wide_grapheme_maps_to_the_grapheme_covering_it() {
        let lines = text(&["漢字"]);
        let wrapping = wrapping(20);

        assert_eq!(
            offset(0, 2),
            drawn_at(&lines, at(0, 0), at(0, 1), &wrapping)
        );
        assert_eq!(
            Landing {
                position: at(0, 0),
                offset: offset(0, 0)
            },
            landed_at(&lines, at(0, 0), offset(0, 1), &wrapping)
        );
    }

    #[test]
    fn a_continuation_marker_pushes_the_text_beside_it_along() {
        let lines = text(&["abcdefghij"]);
        let wrapping = Wrapping::new(
            NonZeroUsize::new(8).expect("a test's width is not zero"),
            Metrics::default(),
            Options::new().with_show_break(">>".to_owned()),
        );

        assert_eq!(
            offset(1, 2),
            drawn_at(&lines, at(0, 0), at(0, 8), &wrapping)
        );
        assert_eq!(
            offset(1, 3),
            drawn_at(&lines, at(0, 0), at(0, 9), &wrapping)
        );
        assert_eq!(
            at(0, 8),
            landed_at(&lines, at(0, 0), offset(1, 2), &wrapping).position
        );
    }

    #[test]
    fn an_empty_line_holds_the_one_position_past_its_end() {
        let lines = text(&[""]);
        let wrapping = wrapping(5);

        assert_eq!(0, graphemes(&lines[0]).count());
        assert_eq!(
            offset(0, 0),
            drawn_at(&lines, at(0, 0), at(0, 0), &wrapping)
        );
        assert_eq!(
            Landing {
                position: at(0, 0),
                offset: offset(0, 0)
            },
            landed_at(&lines, at(0, 0), offset(0, 4), &wrapping)
        );
    }
}
