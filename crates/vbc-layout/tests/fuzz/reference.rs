//! The reference layout the invariant search runs over: vim's wrapped rows for a whole
//! document, scrolled to the slice of the text the cursor is on, and the mapping between that
//! screen and the text.
//!
//! It lives in the test tree rather than in the crate, and its name says what it costs, because
//! laying every line of the buffer out on every call is the cost the anchored mapping exists to
//! avoid. A search can afford it over a generated document of a few lines; a renderer cannot
//! afford it over a transcript, and must not be able to reach for it by accident.
//!
//! The layout owns no rule of its own beyond which rows the window shows:
//! [`vbc_layout::line`] decides where a logical line breaks and [`vbc_layout::anchor`] decides
//! which cell draws a position.
//!
//! A layout is a pure function of the view, so it carries none of the scroll state an editor keeps
//! between draws. The window it shows is therefore the one a reader lands on when the text is
//! revealed from its first row: the rows are scrolled down only as far as it takes to bring the
//! cursor onto the last of them, and never further.
//!
//! A cursor resting past the end of a line whose last row is exactly full is drawn in the first
//! cell of the row below, which for the last line of the text is a row no text reaches. The window
//! draws that row as an empty one rather than leaving the cursor off the screen, which is the row
//! vim draws the same cursor on.

use vbc_layout::anchor::{
    char_idx_at_visual_offset, visual_offset_from_anchor, VisualOffset, Wrapping,
};
use vbc_layout::invariants::{DisplayPosition, Layout, LogicalPosition, Row, Screen, View};
use vbc_layout::line::{self, DisplayRow};

/// A layout that draws a view by laying every line of its buffer out.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WholeDocumentLayout;

impl Layout for WholeDocumentLayout {
    fn lay_out(&self, view: View<'_>) -> Screen {
        Screen {
            rows: Drawn::of(view).rows,
        }
    }

    fn display_position(
        &self,
        view: View<'_>,
        position: LogicalPosition,
    ) -> Option<DisplayPosition> {
        Drawn::of(view).display_position(view, position)
    }

    fn logical_position(
        &self,
        view: View<'_>,
        position: DisplayPosition,
    ) -> Option<LogicalPosition> {
        Drawn::of(view).logical_position(view, position)
    }
}

/// The rows a window shows, together with the anchor they are measured from.
///
/// The anchor is a position drawn on a row of the screen, and `anchor_row` is the row of the
/// screen that draws it, which is negative for the screen that holds nothing but the empty row a
/// cursor past the end of the text rests on.
struct Drawn {
    rows: Vec<Row>,
    anchor: LogicalPosition,
    anchor_row: isize,
}

impl Drawn {
    /// # Returns
    ///
    /// The rows `view` draws, scrolled to the slice of the text holding its cursor.
    ///
    /// # Panics
    ///
    /// Panics if a line of the document lays out into no rows, which none does.
    fn of(view: View<'_>) -> Self {
        let height = view.viewport.height.get();
        let wrapped = wrap(view);
        let cursor_row = cursor_row(view, wrapped.len());
        let top = cursor_row.saturating_sub(height - 1);

        let mut rows: Vec<Row> = wrapped
            .iter()
            .skip(top)
            .take(height)
            .map(Row::from)
            .collect();
        if wrapped.len() == cursor_row {
            let end = view.buffer.end();
            rows.push(Row {
                line: end.line,
                start: end.grapheme,
                text: String::new(),
                cells: String::new(),
                columns: vec![0],
            });
        }

        let last_row = wrapped
            .last()
            .expect("a document lays out into at least one row");
        let (anchor, anchor_row) = match wrapped.get(top) {
            Some(row) => (
                LogicalPosition {
                    line: row.line(),
                    grapheme: row.start(),
                },
                0,
            ),
            None => (
                LogicalPosition {
                    line: last_row.line(),
                    grapheme: last_row.start(),
                },
                -1,
            ),
        };

        Self {
            rows,
            anchor,
            anchor_row,
        }
    }

    /// # Returns
    ///
    /// The cell of this screen that draws `position`, or `None` if the screen does not draw it.
    fn display_position(
        &self,
        view: View<'_>,
        position: LogicalPosition,
    ) -> Option<DisplayPosition> {
        let offset = visual_offset_from_anchor(
            view.buffer.lines(),
            self.anchor,
            position,
            &view.viewport.wrapping,
            self.rows.len(),
        )
        .ok()?;
        let row = usize::try_from(self.anchor_row + offset.rows).ok()?;

        (row < self.rows.len()).then_some(DisplayPosition {
            row,
            column: offset.column,
        })
    }

    /// # Returns
    ///
    /// The position this screen draws at `position`, or `None` where it draws no position there.
    fn logical_position(
        &self,
        view: View<'_>,
        position: DisplayPosition,
    ) -> Option<LogicalPosition> {
        if self.rows.len() <= position.row {
            return None;
        }

        let asked = VisualOffset {
            rows: signed(position.row) - self.anchor_row,
            column: position.column,
        };
        let landing = char_idx_at_visual_offset(
            view.buffer.lines(),
            self.anchor,
            asked,
            &view.viewport.wrapping,
        )
        .ok()?;

        (asked == landing.offset).then_some(landing.position)
    }
}

/// # Returns
///
/// Every row the document wraps into, top to bottom.
fn wrap(view: View<'_>) -> Vec<DisplayRow> {
    let wrapping: &Wrapping = &view.viewport.wrapping;

    view.buffer
        .lines()
        .iter()
        .enumerate()
        .flat_map(|(line, text)| {
            line::lay_out(
                line,
                text,
                wrapping.width(),
                wrapping.metrics(),
                wrapping.options(),
            )
        })
        .collect()
}

/// # Returns
///
/// The index, among the rows the whole document wraps into, of the row drawing the view's cursor,
/// which is one row past the last where the cursor rests past the end of a text whose last row is
/// full.
///
/// # Panics
///
/// Panics if the cursor cannot be mapped, which a position of the document always can be.
fn cursor_row(view: View<'_>, wrapped_rows: usize) -> usize {
    let start = LogicalPosition {
        line: 0,
        grapheme: 0,
    };
    let offset = visual_offset_from_anchor(
        view.buffer.lines(),
        start,
        view.buffer.clamp(view.cursor),
        &view.viewport.wrapping,
        wrapped_rows,
    )
    .expect("a clamped cursor is drawn within the rows of its own document");

    usize::try_from(offset.rows).expect("no position is drawn above the first row of the text")
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

/// The rows and mappings the reference layout is pinned to by hand, which is what keeps the
/// invariant search from being checked against a layout nobody ever read.
mod tests {
    use std::num::NonZeroUsize;

    use super::*;

    use vbc_layout::buffer::Buffer;
    use vbc_layout::invariants::{check, Viewport, Violation};
    use vbc_layout::line::Options;
    use vbc_layout::width::Metrics;

    /// # Returns
    ///
    /// A viewport `width` columns wide and `height` rows tall, wrapping its text as `options` say
    /// under the default metrics.
    fn viewport(width: usize, height: usize, options: Options) -> Viewport {
        Viewport {
            wrapping: Wrapping::new(
                NonZeroUsize::new(width).expect("a test's width is not zero"),
                Metrics::default(),
                options,
            ),
            height: NonZeroUsize::new(height).expect("a test's height is not zero"),
        }
    }

    /// # Returns
    ///
    /// The cells of each row the view draws, top to bottom.
    fn cells(view: View<'_>) -> Vec<String> {
        WholeDocumentLayout
            .lay_out(view)
            .rows
            .into_iter()
            .map(|row| row.cells)
            .collect()
    }

    #[test]
    fn a_text_shorter_than_the_window_is_drawn_from_its_first_row() {
        let buffer = Buffer::from_lines(vec!["abcdef".to_owned(), "gh".to_owned()]);
        let viewport = viewport(4, 5, Options::new());
        let view = View {
            buffer: &buffer,
            viewport: &viewport,
            cursor: LogicalPosition {
                line: 0,
                grapheme: 0,
            },
        };

        assert_eq!(vec!["abcd", "ef", "gh"], cells(view));
    }

    #[test]
    fn the_window_scrolls_down_only_as_far_as_the_cursor() {
        let buffer = Buffer::from_lines(vec!["abcdefghij".to_owned()]);
        let viewport = viewport(2, 2, Options::new());
        let at = |grapheme| View {
            buffer: &buffer,
            viewport: &viewport,
            cursor: LogicalPosition { line: 0, grapheme },
        };

        assert_eq!(vec!["ab", "cd"], cells(at(0)));
        assert_eq!(vec!["ab", "cd"], cells(at(3)));
        assert_eq!(vec!["cd", "ef"], cells(at(4)));
        assert_eq!(vec!["gh", "ij"], cells(at(9)));
    }

    #[test]
    fn a_cursor_past_a_full_last_row_is_drawn_on_a_row_of_its_own() {
        let buffer = Buffer::from_lines(vec!["abcd".to_owned()]);
        let viewport = viewport(4, 3, Options::new());
        let view = View {
            buffer: &buffer,
            viewport: &viewport,
            cursor: LogicalPosition {
                line: 0,
                grapheme: 4,
            },
        };

        assert_eq!(vec!["abcd", ""], cells(view));
        assert_eq!(
            Some(DisplayPosition { row: 1, column: 0 }),
            WholeDocumentLayout.display_position(view, view.cursor)
        );
        assert_eq!(Vec::<Violation>::new(), check(&WholeDocumentLayout, view));
    }

    #[test]
    fn a_window_one_row_tall_gives_that_row_to_the_cursor_past_the_text() {
        let buffer = Buffer::from_lines(vec!["abcd".to_owned()]);
        let viewport = viewport(4, 1, Options::new());
        let view = View {
            buffer: &buffer,
            viewport: &viewport,
            cursor: LogicalPosition {
                line: 0,
                grapheme: 4,
            },
        };

        assert_eq!(vec![""], cells(view));
        assert_eq!(
            Some(DisplayPosition { row: 0, column: 0 }),
            WholeDocumentLayout.display_position(view, view.cursor)
        );
        assert_eq!(Vec::<Violation>::new(), check(&WholeDocumentLayout, view));
    }

    #[test]
    fn a_cell_drawing_no_position_maps_to_nothing() {
        let buffer = Buffer::from_lines(vec!["漢a漢".to_owned()]);
        let viewport = viewport(4, 4, Options::new().with_show_break(">".to_owned()));
        let view = View {
            buffer: &buffer,
            viewport: &viewport,
            cursor: LogicalPosition {
                line: 0,
                grapheme: 0,
            },
        };
        let at = |row, column| {
            WholeDocumentLayout.logical_position(view, DisplayPosition { row, column })
        };
        let position = |grapheme| Some(LogicalPosition { line: 0, grapheme });

        assert_eq!(vec!["漢a", ">漢"], cells(view));

        // The second cell of a two-column cluster, and the cell past the text of a row the line
        // goes on past, draw no position of their own.
        assert_eq!(None, at(0, 1));
        assert_eq!(None, at(0, 3));
        assert_eq!(position(1), at(0, 2));

        // Nor does a cell of the continuation marker, which is drawn from no text at all.
        assert_eq!(None, at(1, 0));
        assert_eq!(None, at(1, 2));
        assert_eq!(position(2), at(1, 1));

        // The cell past the last grapheme of the line draws the position the cursor rests at, and
        // the rows the window does not draw draw nothing.
        assert_eq!(position(3), at(1, 3));
        assert_eq!(None, at(1, 4));
        assert_eq!(None, at(2, 0));
    }

    #[test]
    fn a_position_the_window_scrolled_past_has_no_display_position() {
        let buffer = Buffer::from_lines(vec!["abcdefgh".to_owned(), "ij".to_owned()]);
        let viewport = viewport(2, 2, Options::new());
        let view = View {
            buffer: &buffer,
            viewport: &viewport,
            cursor: LogicalPosition {
                line: 0,
                grapheme: 0,
            },
        };
        let at = |line, grapheme| {
            WholeDocumentLayout.display_position(view, LogicalPosition { line, grapheme })
        };

        assert_eq!(vec!["ab", "cd"], cells(view));
        assert_eq!(Some(DisplayPosition { row: 1, column: 1 }), at(0, 3));
        assert_eq!(None, at(0, 4));
        assert_eq!(None, at(1, 0));
    }

    #[test]
    fn the_window_never_draws_more_rows_than_it_holds() {
        let buffer = Buffer::from_lines(vec!["abcdefghij".to_owned(), "klmnop".to_owned()]);
        for height in 1..=8 {
            let viewport = viewport(3, height, Options::new());
            for line in 0..2 {
                for grapheme in 0..=6 {
                    let view = View {
                        buffer: &buffer,
                        viewport: &viewport,
                        cursor: LogicalPosition { line, grapheme },
                    };
                    let rows = WholeDocumentLayout.lay_out(view).rows.len();
                    assert!(
                        rows <= height,
                        "a window {height} rows tall drew {rows} rows"
                    );
                }
            }
        }
    }

    #[test]
    fn a_continuation_row_keeps_a_decoration_only_while_its_text_fits_beside_it() {
        let buffer = Buffer::from_lines(vec!["a漢a漢漢a".to_owned()]);
        let viewport = viewport(4, 6, Options::new().with_show_break(">>>".to_owned()));
        let view = View {
            buffer: &buffer,
            viewport: &viewport,
            cursor: LogicalPosition {
                line: 0,
                grapheme: 0,
            },
        };

        // A row starting on a two-column cluster has one column too few for the marker beside it
        // and drops it; the row starting on `a` keeps it.
        assert_eq!(vec!["a漢a", "漢漢", ">>>a"], cells(view));
        assert_eq!(Vec::<Violation>::new(), check(&WholeDocumentLayout, view));
    }
}
