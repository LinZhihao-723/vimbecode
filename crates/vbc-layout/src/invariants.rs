//! The invariants every vimbecode layout must satisfy.
//!
//! Defines the two coordinate spaces a layout maps between, the [`Layout`] trait the real layout
//! implements, and the checks that decide whether a layout obeys the invariants for a given view.
//!
//! A view is a buffer, the window it is drawn into, and the cursor. The cursor belongs there
//! because a window shows a slice of a text taller than itself, and which slice it shows is
//! whichever one holds the cursor: a layout that could not see the cursor could not choose.
//!
//! Two of the invariants are therefore stated against that slice rather than against the whole
//! document. The rows show every grapheme they reach exactly once and leave text out only where
//! the window ran out of rows; and a position is round tripped only while it is drawn, since the
//! text above the slice and below it is on no row to be mapped onto.

use std::collections::BTreeMap;
use std::fmt::{Display, Formatter, Result as FmtResult};
use std::num::NonZeroUsize;

pub use crate::width::graphemes;

use crate::anchor::Wrapping;
use crate::buffer::Buffer;

/// The invariants a layout is checked against, in the order [`check`] reports them.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Invariant {
    /// No visual row occupies more display columns than the viewport is wide.
    RowWidth,

    /// Every non-space grapheme the screen reaches appears exactly once across the visual rows,
    /// and the screen leaves a grapheme out only where the window ran out of rows.
    GraphemeConservation,

    /// No visual row is empty, except a row rendering an empty logical line and the screen's last
    /// row.
    NoEmptyRows,

    /// The cursor is drawn on a row the screen holds and in a column inside the viewport,
    /// including where the cursor rests past the last grapheme of its line.
    CursorVisible,

    /// Mapping a position the screen draws into the other coordinate space and back returns the
    /// original position, in both directions.
    RoundTrip,
}

impl Display for Invariant {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        match self {
            Self::RowWidth => write!(f, "row width"),
            Self::GraphemeConservation => write!(f, "grapheme conservation"),
            Self::NoEmptyRows => write!(f, "no empty rows"),
            Self::CursorVisible => write!(f, "cursor visible"),
            Self::RoundTrip => write!(f, "round trip"),
        }
    }
}

/// One way in which a layout breaks an invariant, naming the invariant and what the layout did.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Violation {
    /// The invariant that was broken.
    pub invariant: Invariant,

    /// What the layout did instead of what the invariant requires.
    pub detail: String,
}

impl Violation {
    /// # Returns
    ///
    /// A violation of `invariant`, described by `detail`.
    #[must_use]
    pub fn new(invariant: Invariant, detail: String) -> Self {
        Self { invariant, detail }
    }
}

impl Display for Violation {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        write!(f, "{}: {}", self.invariant, self.detail)
    }
}

/// The window a document is laid out into: the rows the screen draws, and the way their text is
/// wrapped and measured.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Viewport {
    /// How the text is wrapped into rows and measured, the display options included.
    pub wrapping: Wrapping,

    /// The number of display rows the window draws.
    pub height: NonZeroUsize,
}

impl Viewport {
    /// # Returns
    ///
    /// The number of display columns a visual row may occupy.
    #[must_use]
    pub fn width(&self) -> usize {
        self.wrapping.width().get()
    }
}

impl Display for Viewport {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        let options = self.wrapping.options();
        write!(
            f,
            "viewport {} columns by {} rows, tabstop {}, ambiwidth {:?}, breakindent {} (min {}), \
             showbreak `{}`, linebreak {}",
            self.width(),
            self.height,
            self.wrapping.metrics().tab_stop(),
            self.wrapping.metrics().ambiwidth(),
            options.break_indent(),
            options.break_indent_min(),
            options.show_break().escape_debug(),
            options.line_break(),
        )
    }
}

/// A position in the logical text.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct LogicalPosition {
    /// The zero-based index of the logical line.
    pub line: usize,

    /// The zero-based grapheme offset within the line. An offset equal to the line's grapheme count
    /// addresses the position past the line's last grapheme.
    pub grapheme: usize,
}

impl Display for LogicalPosition {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        write!(f, "logical(line {}, grapheme {})", self.line, self.grapheme)
    }
}

/// A position on the screen.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DisplayPosition {
    /// The zero-based index of the visual row.
    pub row: usize,

    /// The zero-based display column within the row.
    pub column: usize,
}

impl Display for DisplayPosition {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        write!(f, "display(row {}, column {})", self.row, self.column)
    }
}

/// One visual row of a laid-out screen, together with the logical text it shows.
///
/// A row carries both the slice of the logical line it renders and the cells it is drawn in, which
/// differ wherever a continuation row is decorated or a tab stands for the blanks it advances by.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Row {
    /// The zero-based index of the logical line this row shows a slice of.
    pub line: usize,

    /// The grapheme offset within that logical line at which the row's text starts.
    pub start: usize,

    /// The slice of the logical line the row shows.
    pub text: String,

    /// The cells the row is drawn in, its decoration included and every tab spelled as the blanks
    /// it advances by.
    pub cells: String,

    /// The column each grapheme of [`Row::text`] is drawn at, followed by the column just past the
    /// row's last grapheme.
    pub columns: Vec<usize>,
}

impl Row {
    /// # Returns
    ///
    /// The grapheme offset within the logical line just past the row's text.
    #[must_use]
    pub fn end(&self) -> usize {
        self.start + graphemes(&self.text).count()
    }
}

/// The visual rows a window draws, top to bottom.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Screen {
    /// The rows, in the order they are drawn.
    pub rows: Vec<Row>,
}

/// What a layout is asked to draw: the text, the window it is drawn into, and where the cursor
/// rests.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct View<'view> {
    /// The logical text being rendered.
    pub buffer: &'view Buffer,

    /// The window the text is drawn into.
    pub viewport: &'view Viewport,

    /// The cursor's logical position, which may rest past the last grapheme of its line.
    pub cursor: LogicalPosition,
}

/// A text layout, which draws a view into visual rows and maps positions between the logical and
/// the display coordinate space.
///
/// Implementations are expected to be pure: the same view must always yield the same rows and the
/// same mappings.
pub trait Layout {
    /// Draws a view into the visual rows the window shows.
    ///
    /// # Parameters
    ///
    /// * `view` - The text, window, and cursor to draw.
    ///
    /// # Returns
    ///
    /// The visual rows the window shows, top to bottom.
    fn lay_out(&self, view: View<'_>) -> Screen;

    /// Maps a logical position onto the screen.
    ///
    /// # Parameters
    ///
    /// * `view` - The text, window, and cursor being drawn.
    /// * `position` - The logical position to map, which may address the position past a line's
    ///   last grapheme.
    ///
    /// # Returns
    ///
    /// The display position rendering `position`, or `None` if the screen does not draw it.
    fn display_position(
        &self,
        view: View<'_>,
        position: LogicalPosition,
    ) -> Option<DisplayPosition>;

    /// Maps a display position back into the logical text.
    ///
    /// # Parameters
    ///
    /// * `view` - The text, window, and cursor being drawn.
    /// * `position` - The display position to map.
    ///
    /// # Returns
    ///
    /// The logical position rendered at `position`, or `None` where the screen renders no position
    /// there, such as a cell inside a wider cluster or one the rows do not reach.
    fn logical_position(
        &self,
        view: View<'_>,
        position: DisplayPosition,
    ) -> Option<LogicalPosition>;
}

/// Checks a layout against every invariant for one view.
///
/// The round trip is required of the positions the screen draws; where the cursor rests past the
/// last grapheme of a line, [`Invariant::CursorVisible`] governs instead. The cursor is clamped
/// into the buffer before anything is drawn.
///
/// # Type Parameters
///
/// * `LayoutType` - The layout under check.
///
/// # Returns
///
/// Every invariant the layout breaks for this view, ordered as in [`Invariant`] and empty if the
/// layout breaks none.
pub fn check<LayoutType: Layout>(layout: &LayoutType, view: View<'_>) -> Vec<Violation> {
    let view = View {
        cursor: view.buffer.clamp(view.cursor),
        ..view
    };
    let screen = layout.lay_out(view);
    [
        check_row_width(&screen, view.viewport),
        check_grapheme_conservation(&screen, view),
        check_no_empty_rows(&screen, view.buffer),
        check_cursor_visible(layout, &screen, view),
        check_round_trip(layout, &screen, view),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// # Returns
///
/// An [`Invariant::RowWidth`] violation if a row is wider than the viewport, otherwise `None`.
fn check_row_width(screen: &Screen, viewport: &Viewport) -> Option<Violation> {
    let width = viewport.width();
    let metrics = viewport.wrapping.metrics();
    screen.rows.iter().enumerate().find_map(|(index, row)| {
        let drawn = metrics.text_width(&row.cells, 0);
        let reported = row.columns.last().copied().unwrap_or(0);
        let occupied = drawn.max(reported);
        (width < occupied).then(|| {
            Violation::new(
                Invariant::RowWidth,
                format!("row {index} occupies {occupied} columns in a viewport {width} wide"),
            )
        })
    })
}

/// # Returns
///
/// An [`Invariant::GraphemeConservation`] violation if the rows do not show each non-space
/// grapheme they reach exactly once, or if they leave text out while the window still had rows to
/// draw it on, otherwise `None`.
fn check_grapheme_conservation(screen: &Screen, view: View<'_>) -> Option<Violation> {
    let Some(first) = screen.rows.first() else {
        return Some(Violation::new(
            Invariant::GraphemeConservation,
            "the screen draws no row at all".to_owned(),
        ));
    };
    let last = screen
        .rows
        .last()
        .expect("a screen with a first row has a last one");
    let start = LogicalPosition {
        line: first.line,
        grapheme: first.start,
    };
    let end = LogicalPosition {
        line: last.line,
        grapheme: last.end(),
    };

    let mut counts: BTreeMap<&str, i64> = BTreeMap::new();
    for grapheme in reached(view.buffer, start, end).filter(|grapheme| !is_space(grapheme)) {
        *counts.entry(grapheme).or_default() += 1;
    }
    for row in &screen.rows {
        for grapheme in graphemes(&row.text).filter(|grapheme| !is_space(grapheme)) {
            *counts.entry(grapheme).or_default() -= 1;
        }
    }
    let miscounted = counts.into_iter().find_map(|(grapheme, difference)| {
        (0 != difference).then(|| {
            let grapheme = grapheme.escape_debug();
            let detail = if 0 < difference {
                format!("the rows show `{grapheme}` {difference} times too few")
            } else {
                format!("the rows show `{grapheme}` {} times too many", -difference)
            };
            Violation::new(Invariant::GraphemeConservation, detail)
        })
    });
    if let Some(violation) = miscounted {
        return Some(violation);
    }

    let top = LogicalPosition {
        line: 0,
        grapheme: 0,
    };
    let whole_document = top == start && view.buffer.end() == end;
    let height = view.viewport.height.get();
    (!whole_document && screen.rows.len() != height).then(|| {
        Violation::new(
            Invariant::GraphemeConservation,
            format!(
                "the rows reach from {start} to {end} of a document ending at {}, drawn on {} of \
                 the window's {height} rows",
                view.buffer.end(),
                screen.rows.len()
            ),
        )
    })
}

/// # Returns
///
/// An [`Invariant::NoEmptyRows`] violation if a row shows no text without rendering an empty
/// logical line or closing the screen, otherwise `None`.
fn check_no_empty_rows(screen: &Screen, buffer: &Buffer) -> Option<Violation> {
    let last_index = screen.rows.len().saturating_sub(1);
    screen.rows.iter().enumerate().find_map(|(index, row)| {
        if !row.text.is_empty() || index == last_index {
            return None;
        }
        let renders_empty_line = 0 == row.start && buffer.line(row.line).is_some_and(str::is_empty);
        (!renders_empty_line).then(|| {
            Violation::new(
                Invariant::NoEmptyRows,
                format!(
                    "row {index} of {} is empty but continues line {}",
                    screen.rows.len(),
                    row.line
                ),
            )
        })
    })
}

/// # Type Parameters
///
/// * `LayoutType` - The layout under check.
///
/// # Returns
///
/// An [`Invariant::CursorVisible`] violation if the cursor has no display position or is drawn
/// outside the rows and columns the window holds, otherwise `None`.
fn check_cursor_visible<LayoutType: Layout>(
    layout: &LayoutType,
    screen: &Screen,
    view: View<'_>,
) -> Option<Violation> {
    let cursor = view.cursor;
    let width = view.viewport.width();
    let Some(display) = layout.display_position(view, cursor) else {
        return Some(Violation::new(
            Invariant::CursorVisible,
            format!("the cursor at {cursor} has no display position"),
        ));
    };

    (width <= display.column || screen.rows.len() <= display.row).then(|| {
        Violation::new(
            Invariant::CursorVisible,
            format!(
                "the cursor at {cursor} maps to {display}, outside the {} rows of a viewport \
                 {width} columns wide",
                screen.rows.len()
            ),
        )
    })
}

/// # Type Parameters
///
/// * `LayoutType` - The layout under check.
///
/// # Returns
///
/// An [`Invariant::RoundTrip`] violation if mapping a position the screen draws into the other
/// coordinate space and back does not return it, in either direction, otherwise `None`.
fn check_round_trip<LayoutType: Layout>(
    layout: &LayoutType,
    screen: &Screen,
    view: View<'_>,
) -> Option<Violation> {
    for (index, row) in screen.rows.iter().enumerate() {
        for (offset, grapheme) in graphemes(&row.text).enumerate() {
            let Some(&column) = row.columns.get(offset) else {
                return Some(Violation::new(
                    Invariant::RoundTrip,
                    format!(
                        "row {index} says nothing about the column `{}` is drawn in",
                        grapheme.escape_debug()
                    ),
                ));
            };
            let display = DisplayPosition { row: index, column };
            let logical = LogicalPosition {
                line: row.line,
                grapheme: row.start + offset,
            };
            let violation = round_trip_from_display(layout, view, display)
                .or_else(|| round_trip_from_logical(layout, view, logical));
            if let Some(violation) = violation {
                return Some(violation);
            }
        }
    }

    None
}

/// # Type Parameters
///
/// * `LayoutType` - The layout under check.
///
/// # Returns
///
/// An [`Invariant::RoundTrip`] violation if `display` does not map back onto itself through the
/// logical position drawn there, otherwise `None`.
fn round_trip_from_display<LayoutType: Layout>(
    layout: &LayoutType,
    view: View<'_>,
    display: DisplayPosition,
) -> Option<Violation> {
    let Some(logical) = layout.logical_position(view, display) else {
        return Some(Violation::new(
            Invariant::RoundTrip,
            format!("{display} draws a grapheme but maps back to nothing"),
        ));
    };
    let Some(mapped_back) = layout.display_position(view, logical) else {
        return Some(Violation::new(
            Invariant::RoundTrip,
            format!("{display} maps to {logical}, which maps back to nothing"),
        ));
    };

    (mapped_back != display).then(|| {
        Violation::new(
            Invariant::RoundTrip,
            format!("{display} maps to {logical}, which maps back to {mapped_back}"),
        )
    })
}

/// # Type Parameters
///
/// * `LayoutType` - The layout under check.
///
/// # Returns
///
/// An [`Invariant::RoundTrip`] violation if `logical` does not map back onto itself through the
/// cell that draws it, otherwise `None`.
fn round_trip_from_logical<LayoutType: Layout>(
    layout: &LayoutType,
    view: View<'_>,
    logical: LogicalPosition,
) -> Option<Violation> {
    let Some(display) = layout.display_position(view, logical) else {
        return Some(Violation::new(
            Invariant::RoundTrip,
            format!("{logical} is drawn on the screen but has no display position"),
        ));
    };
    let Some(mapped_back) = layout.logical_position(view, display) else {
        return Some(Violation::new(
            Invariant::RoundTrip,
            format!("{logical} maps to {display}, which maps back to nothing"),
        ));
    };

    (mapped_back != logical).then(|| {
        Violation::new(
            Invariant::RoundTrip,
            format!("{logical} maps to {display}, which maps back to {mapped_back}"),
        )
    })
}

/// # Returns
///
/// The graphemes of the buffer from `start` up to `end`, in order, which is nothing at all where
/// `end` does not come after `start`.
fn reached(
    buffer: &Buffer,
    start: LogicalPosition,
    end: LogicalPosition,
) -> impl Iterator<Item = &str> {
    (start.line..=end.line)
        .filter_map(move |line| buffer.line(line).map(|text| (line, text)))
        .flat_map(move |(line, text)| {
            let from = if line == start.line {
                start.grapheme
            } else {
                0
            };
            let to = if line == end.line {
                end.grapheme
            } else {
                usize::MAX
            };
            graphemes(text).skip(from).take(to.saturating_sub(from))
        })
}

/// # Returns
///
/// Whether `grapheme` renders as blank space.
fn is_space(grapheme: &str) -> bool {
    grapheme.chars().all(char::is_whitespace)
}
