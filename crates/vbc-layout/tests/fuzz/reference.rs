//! A trivial reference layout, which exercises the harness until the real layout lands.
//!
//! The layout character-wraps every logical line at the viewport's width, and the wrapping helpers
//! it is built from are shared with the deliberately broken layouts.

use std::ops::RangeInclusive;

use vbc_layout::invariants::{
    display_width, graphemes, DisplayPosition, Document, Layout, LogicalPosition, Row, Screen,
    Viewport,
};

/// A layout that character-wraps each logical line at the viewport's width, satisfying every
/// invariant.
pub struct Wrapped;

impl Layout for Wrapped {
    fn lay_out(&self, document: &Document, viewport: Viewport) -> Screen {
        Screen {
            rows: wrap(document, viewport),
        }
    }

    fn display_position(
        &self,
        document: &Document,
        viewport: Viewport,
        position: LogicalPosition,
    ) -> Option<DisplayPosition> {
        let len = document.line_len(position.line)?;
        if len < position.grapheme {
            return None;
        }

        let rows = wrap(document, viewport);
        let range = rows_of_line(&rows, position.line)?;
        if position.grapheme == len {
            let row = *range.end();
            let column = display_width(&rows[row].text);
            if viewport.width.get() <= column {
                return Some(DisplayPosition {
                    row: row + 1,
                    column: 0,
                });
            }
            return Some(DisplayPosition { row, column });
        }

        let row = range
            .rev()
            .find(|index| rows[*index].start <= position.grapheme)?;
        let column = graphemes(&rows[row].text)
            .take(position.grapheme - rows[row].start)
            .map(display_width)
            .sum();
        Some(DisplayPosition { row, column })
    }

    fn logical_position(
        &self,
        document: &Document,
        viewport: Viewport,
        position: DisplayPosition,
    ) -> Option<LogicalPosition> {
        logical_position_in(&wrap(document, viewport), position)
    }
}

/// # Returns
///
/// The rows `document` wraps into, top to bottom, with one empty row per empty logical line.
pub fn wrap(document: &Document, viewport: Viewport) -> Vec<Row> {
    let width = viewport.width.get();
    let mut rows = Vec::new();
    for (line, text) in document.lines().iter().enumerate() {
        let mut start = 0;
        let mut used = 0;
        let mut row_text = String::new();
        for (index, grapheme) in graphemes(text).enumerate() {
            let grapheme_width = display_width(grapheme);
            if 0 < used && width < used + grapheme_width {
                rows.push(Row {
                    line,
                    start,
                    text: std::mem::take(&mut row_text),
                });
                start = index;
                used = 0;
            }
            row_text.push_str(grapheme);
            used += grapheme_width;
        }
        rows.push(Row {
            line,
            start,
            text: row_text,
        });
    }

    rows
}

/// # Returns
///
/// The index range of the rows rendering `line`, or `None` if no row renders it.
pub fn rows_of_line(rows: &[Row], line: usize) -> Option<RangeInclusive<usize>> {
    let first = rows.iter().position(|row| row.line == line)?;
    let last = rows.iter().rposition(|row| row.line == line)?;
    Some(first..=last)
}

/// # Returns
///
/// The logical position of the grapheme starting at `position` in `rows`, or `None` if no grapheme
/// starts there.
pub fn logical_position_in(rows: &[Row], position: DisplayPosition) -> Option<LogicalPosition> {
    let row = rows.get(position.row)?;
    let mut column = 0;
    for (offset, grapheme) in graphemes(&row.text).enumerate() {
        if column == position.column {
            return Some(LogicalPosition {
                line: row.line,
                grapheme: row.start + offset,
            });
        }
        column += display_width(grapheme);
    }

    None
}
