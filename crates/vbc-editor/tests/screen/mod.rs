//! Checking that a drawn terminal buffer describes a screen a terminal can draw.
//!
//! A cell holding a grapheme wider than one column claims the cells beside it, and every reader of
//! a buffer -- ratatui's own view, its terminal diff, and the check here -- finds the claim by
//! measuring the first cell's symbol rather than by asking the cells it claimed. Nothing enforces
//! it, so a renderer that wrote into a claimed cell would leave a buffer no terminal can draw and
//! no reader would report.

use ratatui::buffer::{Buffer, CellWidth};

/// The symbol a cell holds when nothing is drawn in it.
pub const BLANK: &str = " ";

/// Checks that the cells of `buffer` describe a screen a terminal can draw: every cell a wider
/// grapheme beside it has claimed holds a blank, and no claim runs past the right edge of a row.
///
/// # Returns
///
/// One description per cell that breaks the rule, empty where the buffer is sound.
///
/// # Panics
///
/// Panics if a column of the buffer does not fit in a `u16`, which no buffer is wide enough for.
pub fn broken_claims(buffer: &Buffer) -> Vec<String> {
    let width = usize::from(buffer.area.width);
    let mut broken = Vec::new();
    for y in 0..buffer.area.height {
        let mut column = 0;
        while column < width {
            let claimed = usize::from(buffer[(narrowed(column), y)].cell_width().max(1));
            if width < column + claimed {
                broken.push(format!(
                    "row {y} claims {claimed} cells at column {column} of {width}"
                ));
            }
            for inside in (column + 1)..(column + claimed).min(width) {
                if BLANK != buffer[(narrowed(inside), y)].symbol() {
                    broken.push(format!(
                        "row {y} draws `{}` in the cell at column {inside} claimed at column \
                         {column}",
                        buffer[(narrowed(inside), y)].symbol()
                    ));
                }
            }
            column += claimed;
        }
    }

    broken
}

/// # Returns
///
/// `column` as a buffer coordinate.
///
/// # Panics
///
/// Panics if `column` does not fit in a `u16`, which no buffer is wide enough for.
fn narrowed(column: usize) -> u16 {
    u16::try_from(column).expect("a column of a buffer fits in a `u16`")
}
