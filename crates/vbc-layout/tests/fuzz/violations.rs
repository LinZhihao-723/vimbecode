//! Layouts that each break exactly one invariant, so that the harness can be shown to catch it.
//!
//! Every layout here is the real layout with one deliberate defect, planted where it breaks
//! nothing else. Exclusivity is what makes each of them evidence: a layout that broke several
//! invariants at once would fail every one of the harness's tests without any of them exercising
//! the check it is named for.

use vbc_layout::invariants::{
    graphemes, DisplayPosition, Layout, LogicalPosition, Row, Screen, View,
};
use vbc_layout::screen::WrappedLayout;

/// A layout that draws one column more than the viewport holds, in blanks the text does not
/// account for.
pub struct OverdrawsRows;

impl Layout for OverdrawsRows {
    fn lay_out(&self, view: View<'_>) -> Screen {
        let occupied = view.viewport.width() + 1;
        let metrics = view.viewport.wrapping.metrics();
        let mut screen = WrappedLayout.lay_out(view);
        for row in &mut screen.rows {
            let drawn = metrics.text_width(&row.cells, 0);
            row.cells
                .push_str(&" ".repeat(occupied.saturating_sub(drawn)));
            if let Some(last) = row.columns.last_mut() {
                *last = occupied;
            }
        }

        screen
    }

    fn display_position(
        &self,
        view: View<'_>,
        position: LogicalPosition,
    ) -> Option<DisplayPosition> {
        WrappedLayout.display_position(view, position)
    }

    fn logical_position(
        &self,
        view: View<'_>,
        position: DisplayPosition,
    ) -> Option<LogicalPosition> {
        WrappedLayout.logical_position(view, position)
    }
}

/// A layout that swallows the last grapheme of the screen's first row.
///
/// The defect is planted on a row the screen still reaches past, so that the missing grapheme is
/// one the rows were meant to show rather than one the window ran out of room for, and on a row
/// that keeps a grapheme of its own, so that it does not fall empty.
pub struct DropsAGrapheme;

impl Layout for DropsAGrapheme {
    fn lay_out(&self, view: View<'_>) -> Screen {
        let mut screen = WrappedLayout.lay_out(view);
        if screen.rows.len() < 2 {
            return screen;
        }

        let row = &mut screen.rows[0];
        let count = graphemes(&row.text).count();
        if count < 2 {
            return screen;
        }
        row.text = graphemes(&row.text).take(count - 1).collect();
        row.columns.truncate(count);

        screen
    }

    fn display_position(
        &self,
        view: View<'_>,
        position: LogicalPosition,
    ) -> Option<DisplayPosition> {
        WrappedLayout.display_position(view, position)
    }

    fn logical_position(
        &self,
        view: View<'_>,
        position: DisplayPosition,
    ) -> Option<LogicalPosition> {
        WrappedLayout.logical_position(view, position)
    }
}

/// A layout that follows the screen's first row with an empty continuation row.
///
/// The defect is planted only on a screen with a row to spare, so that the padded screen still
/// fits the window and still shows the whole document.
pub struct PadsWithAnEmptyRow;

impl PadsWithAnEmptyRow {
    /// # Returns
    ///
    /// Whether the empty row is planted on the screen `view` draws.
    fn plants(view: View<'_>, screen: &Screen) -> bool {
        let Some(first) = screen.rows.first() else {
            return false;
        };

        2 <= screen.rows.len()
            && screen.rows.len() < view.viewport.height.get()
            && !first.text.is_empty()
    }
}

impl Layout for PadsWithAnEmptyRow {
    fn lay_out(&self, view: View<'_>) -> Screen {
        let mut screen = WrappedLayout.lay_out(view);
        if !Self::plants(view, &screen) {
            return screen;
        }

        let first = &screen.rows[0];
        let padding = Row {
            line: first.line,
            start: first.end(),
            text: String::new(),
            cells: String::new(),
            columns: vec![0],
        };
        screen.rows.insert(1, padding);

        screen
    }

    fn display_position(
        &self,
        view: View<'_>,
        position: LogicalPosition,
    ) -> Option<DisplayPosition> {
        let display = WrappedLayout.display_position(view, position)?;
        if !Self::plants(view, &WrappedLayout.lay_out(view)) || 0 == display.row {
            return Some(display);
        }

        Some(DisplayPosition {
            row: display.row + 1,
            ..display
        })
    }

    fn logical_position(
        &self,
        view: View<'_>,
        position: DisplayPosition,
    ) -> Option<LogicalPosition> {
        if !Self::plants(view, &WrappedLayout.lay_out(view)) {
            return WrappedLayout.logical_position(view, position);
        }

        match position.row {
            0 => WrappedLayout.logical_position(view, position),
            1 => None,
            row => WrappedLayout.logical_position(
                view,
                DisplayPosition {
                    row: row - 1,
                    ..position
                },
            ),
        }
    }
}

/// A layout that parks the cursor resting past the end of a line in the cell after that line's
/// last, instead of wrapping it onto the row below where the row it would rest past is full.
pub struct OverflowsEndOfLine;

impl Layout for OverflowsEndOfLine {
    fn lay_out(&self, view: View<'_>) -> Screen {
        WrappedLayout.lay_out(view)
    }

    fn display_position(
        &self,
        view: View<'_>,
        position: LogicalPosition,
    ) -> Option<DisplayPosition> {
        let line_len = view.document.line_len(position.line)?;
        if position.grapheme != line_len {
            return WrappedLayout.display_position(view, position);
        }

        let screen = WrappedLayout.lay_out(view);
        let row = screen
            .rows
            .iter()
            .rposition(|row| row.line == position.line && row.end() == line_len)?;
        let column = *screen.rows[row].columns.last()?;

        Some(DisplayPosition { row, column })
    }

    fn logical_position(
        &self,
        view: View<'_>,
        position: DisplayPosition,
    ) -> Option<LogicalPosition> {
        WrappedLayout.logical_position(view, position)
    }
}

/// A layout that reports the last cell of a row as the cell before it, so that two display
/// positions share one logical position.
pub struct MergesLastCell;

impl Layout for MergesLastCell {
    fn lay_out(&self, view: View<'_>) -> Screen {
        WrappedLayout.lay_out(view)
    }

    fn display_position(
        &self,
        view: View<'_>,
        position: LogicalPosition,
    ) -> Option<DisplayPosition> {
        WrappedLayout.display_position(view, position)
    }

    fn logical_position(
        &self,
        view: View<'_>,
        position: DisplayPosition,
    ) -> Option<LogicalPosition> {
        let logical = WrappedLayout.logical_position(view, position)?;
        let screen = WrappedLayout.lay_out(view);
        let row = screen.rows.get(position.row)?;
        if graphemes(&row.text).count() < 2 || logical.grapheme + 1 != row.end() {
            return Some(logical);
        }

        Some(LogicalPosition {
            line: logical.line,
            grapheme: logical.grapheme - 1,
        })
    }
}
