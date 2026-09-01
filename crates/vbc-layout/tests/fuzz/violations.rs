//! Layouts that each break exactly one invariant, so that the harness can be shown to catch it.
//!
//! Every layout here is the reference layout with one deliberate defect, which keeps the other
//! invariants intact and the harness's report unambiguous.

use vbc_layout::invariants::{
    display_width, graphemes, DisplayPosition, Document, Layout, LogicalPosition, Row, Screen,
    Viewport,
};

use crate::fuzz::reference::{logical_position_in, rows_of_line, wrap, Wrapped};

/// A layout that never wraps, so a line longer than the viewport overflows its row.
///
/// The end-of-line cursor is held inside the viewport, so the overflow shows up as a row-width
/// violation rather than a cursor one.
pub struct NeverWraps;

impl Layout for NeverWraps {
    fn lay_out(&self, document: &Document, _viewport: Viewport) -> Screen {
        Screen {
            rows: document
                .lines()
                .iter()
                .enumerate()
                .map(|(line, text)| Row {
                    line,
                    start: 0,
                    text: text.clone(),
                })
                .collect(),
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

        let text = document.line(position.line)?;
        let column: usize = graphemes(text)
            .take(position.grapheme)
            .map(display_width)
            .sum();
        let column = if position.grapheme == len {
            column.min(viewport.width.get() - 1)
        } else {
            column
        };
        Some(DisplayPosition {
            row: position.line,
            column,
        })
    }

    fn logical_position(
        &self,
        document: &Document,
        viewport: Viewport,
        position: DisplayPosition,
    ) -> Option<LogicalPosition> {
        logical_position_in(&self.lay_out(document, viewport).rows, position)
    }
}

/// A layout that swallows the last grapheme of every logical line it renders.
pub struct DropsLastGrapheme;

impl Layout for DropsLastGrapheme {
    fn lay_out(&self, document: &Document, viewport: Viewport) -> Screen {
        let mut rows = wrap(document, viewport);
        for line in 0..document.lines().len() {
            let Some(range) = rows_of_line(&rows, line) else {
                continue;
            };
            let last = *range.end();
            let count = graphemes(&rows[last].text).count();
            if count < 2 {
                continue;
            }
            rows[last].text = graphemes(&rows[last].text).take(count - 1).collect();
        }

        Screen { rows }
    }

    fn display_position(
        &self,
        document: &Document,
        viewport: Viewport,
        position: LogicalPosition,
    ) -> Option<DisplayPosition> {
        Wrapped.display_position(document, viewport, position)
    }

    fn logical_position(
        &self,
        document: &Document,
        viewport: Viewport,
        position: DisplayPosition,
    ) -> Option<LogicalPosition> {
        Wrapped.logical_position(document, viewport, position)
    }
}

/// A layout that reports the last cell of a row as the cell before it, so two display positions
/// share one logical position.
pub struct MergesLastCell;

impl Layout for MergesLastCell {
    fn lay_out(&self, document: &Document, viewport: Viewport) -> Screen {
        Wrapped.lay_out(document, viewport)
    }

    fn display_position(
        &self,
        document: &Document,
        viewport: Viewport,
        position: LogicalPosition,
    ) -> Option<DisplayPosition> {
        Wrapped.display_position(document, viewport, position)
    }

    fn logical_position(
        &self,
        document: &Document,
        viewport: Viewport,
        position: DisplayPosition,
    ) -> Option<LogicalPosition> {
        let logical = Wrapped.logical_position(document, viewport, position)?;
        let rows = wrap(document, viewport);
        let row = rows.get(position.row)?;
        let count = graphemes(&row.text).count();
        if count < 2 || logical.grapheme + 1 != row.start + count {
            return Some(logical);
        }

        Some(LogicalPosition {
            line: logical.line,
            grapheme: logical.grapheme - 1,
        })
    }
}

/// A layout that closes every logical line with an empty continuation row.
pub struct PadsWithEmptyRows;

impl Layout for PadsWithEmptyRows {
    fn lay_out(&self, document: &Document, viewport: Viewport) -> Screen {
        let wrapped = wrap(document, viewport);
        let mut rows = Vec::new();
        for line in 0..document.lines().len() {
            if let Some(range) = rows_of_line(&wrapped, line) {
                rows.extend_from_slice(&wrapped[range]);
            }
            rows.push(Row {
                line,
                start: document.line_len(line).unwrap_or(0),
                text: String::new(),
            });
        }

        Screen { rows }
    }

    fn display_position(
        &self,
        document: &Document,
        viewport: Viewport,
        position: LogicalPosition,
    ) -> Option<DisplayPosition> {
        let display = Wrapped.display_position(document, viewport, position)?;
        Some(DisplayPosition {
            row: display.row + position.line,
            column: display.column,
        })
    }

    fn logical_position(
        &self,
        document: &Document,
        viewport: Viewport,
        position: DisplayPosition,
    ) -> Option<LogicalPosition> {
        logical_position_in(&self.lay_out(document, viewport).rows, position)
    }
}

/// A layout that parks the end-of-line cursor just past a full row instead of wrapping it onto the
/// next one.
pub struct OverflowsEndOfLine;

impl Layout for OverflowsEndOfLine {
    fn lay_out(&self, document: &Document, viewport: Viewport) -> Screen {
        Wrapped.lay_out(document, viewport)
    }

    fn display_position(
        &self,
        document: &Document,
        viewport: Viewport,
        position: LogicalPosition,
    ) -> Option<DisplayPosition> {
        let len = document.line_len(position.line)?;
        if position.grapheme != len {
            return Wrapped.display_position(document, viewport, position);
        }

        let rows = wrap(document, viewport);
        let row = *rows_of_line(&rows, position.line)?.end();
        Some(DisplayPosition {
            row,
            column: display_width(&rows[row].text),
        })
    }

    fn logical_position(
        &self,
        document: &Document,
        viewport: Viewport,
        position: DisplayPosition,
    ) -> Option<LogicalPosition> {
        Wrapped.logical_position(document, viewport, position)
    }
}
