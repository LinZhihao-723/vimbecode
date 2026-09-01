//! The invariants every vimbecode layout must satisfy.
//!
//! Defines the two coordinate spaces a layout maps between, the [`Layout`] trait the real layout
//! will implement, and the checks that decide whether a layout obeys the invariants for a given
//! document, viewport, and cursor.

use std::collections::BTreeMap;
use std::fmt::{Display, Formatter, Result as FmtResult};
use std::num::NonZeroUsize;

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// The invariants a layout is checked against, in the order [`check`] reports them.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Invariant {
    /// No visual row occupies more display columns than the viewport is wide.
    RowWidth,

    /// Every non-space grapheme of the document appears exactly once across the visual rows.
    GraphemeConservation,

    /// No visual row is empty, except a row rendering an empty logical line and the screen's last
    /// row.
    NoEmptyRows,

    /// The cursor's display column lies inside the viewport, including where the cursor rests past
    /// the last grapheme of its line.
    CursorVisible,

    /// Mapping a position into the other coordinate space and back returns the original position,
    /// in both directions.
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

/// The area a document is laid out into.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Viewport {
    /// The number of display columns a visual row may occupy.
    pub width: NonZeroUsize,
}

/// The logical text a layout renders, holding one entry per logical line with newlines excluded.
///
/// A document always holds at least one line, so every document has a position the cursor can rest
/// at.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Document {
    lines: Vec<String>,
}

impl Document {
    /// Factory function.
    ///
    /// # Returns
    ///
    /// A document holding `lines`, or a document holding one empty line if `lines` is empty.
    #[must_use]
    pub fn new(lines: Vec<String>) -> Self {
        if lines.is_empty() {
            return Self {
                lines: vec![String::new()],
            };
        }
        Self { lines }
    }

    #[must_use]
    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    #[must_use]
    pub fn line(&self, index: usize) -> Option<&str> {
        self.lines.get(index).map(String::as_str)
    }

    /// # Returns
    ///
    /// The number of graphemes on the line at `index`, or `None` if the document has no such line.
    #[must_use]
    pub fn line_len(&self, index: usize) -> Option<usize> {
        self.line(index).map(|line| graphemes(line).count())
    }

    /// Moves a position onto the nearest position the document holds.
    ///
    /// # Returns
    ///
    /// `position` if the document holds it, otherwise the closest position it does hold.
    #[must_use]
    pub fn clamp(&self, position: LogicalPosition) -> LogicalPosition {
        let line = position.line.min(self.lines.len() - 1);
        let len = self.line_len(line).unwrap_or(0);
        LogicalPosition {
            line,
            grapheme: position.grapheme.min(len),
        }
    }
}

/// A position in the logical text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Row {
    /// The zero-based index of the logical line this row shows a slice of.
    pub line: usize,

    /// The grapheme offset within that logical line at which the row's text starts.
    pub start: usize,

    /// The text the row shows.
    pub text: String,
}

/// The visual rows a document was laid out into, top to bottom.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Screen {
    /// The rows, in the order they are drawn.
    pub rows: Vec<Row>,
}

/// A text layout, which wraps a document into visual rows and maps positions between the logical
/// and the display coordinate space.
///
/// Implementations are expected to be pure: the same document and viewport must always yield the
/// same rows and the same mappings.
pub trait Layout {
    /// Wraps a document into the visual rows that render it.
    ///
    /// # Parameters
    ///
    /// * `document` - The logical text to render.
    /// * `viewport` - The area the text is wrapped into.
    ///
    /// # Returns
    ///
    /// The visual rows rendering `document`.
    fn lay_out(&self, document: &Document, viewport: Viewport) -> Screen;

    /// Maps a logical position onto the screen.
    ///
    /// # Parameters
    ///
    /// * `document` - The logical text being rendered.
    /// * `viewport` - The area the text is wrapped into.
    /// * `position` - The logical position to map, which may address the position past a line's
    ///   last grapheme.
    ///
    /// # Returns
    ///
    /// The display position rendering `position`, or `None` if the document holds no such position.
    fn display_position(
        &self,
        document: &Document,
        viewport: Viewport,
        position: LogicalPosition,
    ) -> Option<DisplayPosition>;

    /// Maps a display position back into the logical text.
    ///
    /// # Parameters
    ///
    /// * `document` - The logical text being rendered.
    /// * `viewport` - The area the text is wrapped into.
    /// * `position` - The display position to map.
    ///
    /// # Returns
    ///
    /// The logical position rendered at `position`, or `None` if no grapheme starts there.
    fn logical_position(
        &self,
        document: &Document,
        viewport: Viewport,
        position: DisplayPosition,
    ) -> Option<LogicalPosition>;
}

/// Checks a layout against every invariant for one document, viewport, and cursor.
///
/// The round trip is required of the positions that address a grapheme; where the cursor rests past
/// the last grapheme of a line, [`Invariant::CursorVisible`] governs instead. The cursor is clamped
/// into the document before it is checked.
///
/// # Type Parameters
///
/// * `LayoutType` - The layout under check.
///
/// # Returns
///
/// Every invariant the layout breaks for this input, ordered as in [`Invariant`] and empty if the
/// layout breaks none.
pub fn check<LayoutType: Layout>(
    layout: &LayoutType,
    document: &Document,
    viewport: Viewport,
    cursor: LogicalPosition,
) -> Vec<Violation> {
    let screen = layout.lay_out(document, viewport);
    [
        check_row_width(&screen, viewport),
        check_grapheme_conservation(document, &screen),
        check_no_empty_rows(document, &screen),
        check_cursor_visible(layout, document, viewport, cursor),
        check_round_trip(layout, document, viewport, &screen),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// # Returns
///
/// The graphemes of `text`, in order.
pub fn graphemes(text: &str) -> impl Iterator<Item = &str> {
    text.graphemes(true)
}

/// # Returns
///
/// The number of display columns `text` occupies.
#[must_use]
pub fn display_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

/// # Returns
///
/// An [`Invariant::RowWidth`] violation if a row is wider than the viewport, otherwise `None`.
fn check_row_width(screen: &Screen, viewport: Viewport) -> Option<Violation> {
    let width = viewport.width.get();
    screen.rows.iter().enumerate().find_map(|(index, row)| {
        let row_width = display_width(&row.text);
        (width < row_width).then(|| {
            Violation::new(
                Invariant::RowWidth,
                format!("row {index} occupies {row_width} columns in a viewport {width} wide"),
            )
        })
    })
}

/// # Returns
///
/// An [`Invariant::GraphemeConservation`] violation if the rows do not show each of the document's
/// non-space graphemes exactly once, otherwise `None`.
fn check_grapheme_conservation(document: &Document, screen: &Screen) -> Option<Violation> {
    let mut counts: BTreeMap<&str, i64> = BTreeMap::new();
    for line in document.lines() {
        for grapheme in graphemes(line).filter(|grapheme| !is_space(grapheme)) {
            *counts.entry(grapheme).or_default() += 1;
        }
    }
    for row in &screen.rows {
        for grapheme in graphemes(&row.text).filter(|grapheme| !is_space(grapheme)) {
            *counts.entry(grapheme).or_default() -= 1;
        }
    }

    counts.into_iter().find_map(|(grapheme, difference)| {
        (0 != difference).then(|| {
            let detail = if 0 < difference {
                format!("the rows show `{grapheme}` {difference} times too few")
            } else {
                format!("the rows show `{grapheme}` {} times too many", -difference)
            };
            Violation::new(Invariant::GraphemeConservation, detail)
        })
    })
}

/// # Returns
///
/// An [`Invariant::NoEmptyRows`] violation if a row shows no text without rendering an empty logical
/// line or closing the screen, otherwise `None`.
fn check_no_empty_rows(document: &Document, screen: &Screen) -> Option<Violation> {
    let last_index = screen.rows.len().saturating_sub(1);
    screen.rows.iter().enumerate().find_map(|(index, row)| {
        if !row.text.is_empty() || index == last_index {
            return None;
        }
        let renders_empty_line =
            0 == row.start && document.line(row.line).is_some_and(str::is_empty);
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

/// # Returns
///
/// An [`Invariant::CursorVisible`] violation if the cursor has no display position or sits outside
/// the viewport, otherwise `None`.
fn check_cursor_visible<LayoutType: Layout>(
    layout: &LayoutType,
    document: &Document,
    viewport: Viewport,
    cursor: LogicalPosition,
) -> Option<Violation> {
    let cursor = document.clamp(cursor);
    let width = viewport.width.get();
    let Some(display) = layout.display_position(document, viewport, cursor) else {
        return Some(Violation::new(
            Invariant::CursorVisible,
            format!("the cursor at {cursor} has no display position"),
        ));
    };

    (width <= display.column).then(|| {
        Violation::new(
            Invariant::CursorVisible,
            format!("the cursor at {cursor} maps to {display}, outside a viewport {width} wide"),
        )
    })
}

/// # Returns
///
/// An [`Invariant::RoundTrip`] violation if mapping a position into the other coordinate space and
/// back does not return it, in either direction, otherwise `None`.
fn check_round_trip<LayoutType: Layout>(
    layout: &LayoutType,
    document: &Document,
    viewport: Viewport,
    screen: &Screen,
) -> Option<Violation> {
    for (line, text) in document.lines().iter().enumerate() {
        for grapheme in 0..graphemes(text).count() {
            let logical = LogicalPosition { line, grapheme };
            let Some(display) = layout.display_position(document, viewport, logical) else {
                return Some(Violation::new(
                    Invariant::RoundTrip,
                    format!("{logical} has no display position"),
                ));
            };
            let Some(mapped_back) = layout.logical_position(document, viewport, display) else {
                return Some(Violation::new(
                    Invariant::RoundTrip,
                    format!("{logical} maps to {display}, which maps back to nothing"),
                ));
            };
            if mapped_back != logical {
                return Some(Violation::new(
                    Invariant::RoundTrip,
                    format!("{logical} maps to {display}, which maps back to {mapped_back}"),
                ));
            }
        }
    }

    for (row, text) in screen.rows.iter().map(|row| &row.text).enumerate() {
        let mut column = 0;
        for grapheme in graphemes(text) {
            let display = DisplayPosition { row, column };
            column += display_width(grapheme);

            let Some(logical) = layout.logical_position(document, viewport, display) else {
                return Some(Violation::new(
                    Invariant::RoundTrip,
                    format!("{display} shows `{grapheme}` but maps back to nothing"),
                ));
            };
            let Some(mapped_back) = layout.display_position(document, viewport, logical) else {
                return Some(Violation::new(
                    Invariant::RoundTrip,
                    format!("{display} maps to {logical}, which maps back to nothing"),
                ));
            };
            if mapped_back != display {
                return Some(Violation::new(
                    Invariant::RoundTrip,
                    format!("{display} maps to {logical}, which maps back to {mapped_back}"),
                ));
            }
        }
    }

    None
}

/// # Returns
///
/// Whether `grapheme` renders as blank space.
fn is_space(grapheme: &str) -> bool {
    grapheme.chars().all(char::is_whitespace)
}
