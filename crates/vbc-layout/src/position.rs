//! The two coordinate spaces a layout maps between.
//!
//! A position is either a place in the logical text or a place on the screen, and a layout exists
//! to carry one onto the other. Neither is anything a layout owns, so both sit here rather than
//! beside the layout that maps them or beside the invariants that hold it to its mapping.

use std::fmt::{Display, Formatter, Result as FmtResult};

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
