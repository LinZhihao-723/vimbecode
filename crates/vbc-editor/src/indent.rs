//! The whitespace vim's shift operators lay down, and the column they leave the cursor in.
//!
//! `>>` and `<<` do not add or remove characters: they measure the blanks a line begins with in
//! screen columns, move that measurement by a step, and write the whole indent out again in
//! whatever whitespace the options ask for. That is what makes a tab-indented line come back
//! spelled in spaces under `'expandtab'`, and what makes a line indented with a mixture of tabs
//! and spaces come back spelled the one way, and it is why the two options an indent is spelled by
//! -- `'shiftwidth'` and `'expandtab'` -- decide the answer as much as the keys do.
//!
//! What is measured here is a column rather than a character, because a tab is worth as many
//! columns as it takes to reach the next tab stop and no fixed number of them. What is written out
//! is a character again, so the round trip is lossy in exactly the way vim's is: an indent that
//! went in as eight spaces comes out as one tab where a tab stop is eight and `'expandtab'` is
//! off.
//!
//! A shift is measured in columns rather than in steps so that the counted forms and the two
//! directions are one arithmetic. `>` and `<` differ only in the sign of the columns they ask for,
//! and an outdent that would carry an indent past the left margin stops at it rather than wrapping
//! around, which is the whole of what vim does at column zero.

use std::num::NonZeroUsize;

/// vim's own `'shiftwidth'` and `'tabstop'`, which are the widths an indent is measured and
/// written in where nothing says otherwise.
const DEFAULT_WIDTH: usize = 8;

/// The blanks a line is indented with: how far one step of a shift carries it, how wide a tab
/// carries it, and whether the indent is written in tabs at all.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Shift {
    width: usize,
    tab_stop: NonZeroUsize,
    expand_tabs: bool,
}

impl Shift {
    /// Factory function.
    ///
    /// # Returns
    ///
    /// A newly created shift carrying a line `width` columns per step, drawing a tab to every
    /// multiple of `tab_stop` columns, and writing its indent in spaces alone where `expand_tabs`
    /// says so. A zero `width` follows the tab stop, as vim's own `'shiftwidth'` does.
    #[must_use]
    pub fn new(width: usize, tab_stop: NonZeroUsize, expand_tabs: bool) -> Self {
        Self {
            width,
            tab_stop,
            expand_tabs,
        }
    }

    /// # Returns
    ///
    /// The same shift drawing a tab to every multiple of `tab_stop` columns.
    #[must_use]
    pub fn with_tab_stop(self, tab_stop: NonZeroUsize) -> Self {
        Self { tab_stop, ..self }
    }

    /// # Returns
    ///
    /// The columns one step of the shift carries a line by, which is the tab stop where the shift
    /// holds no width of its own.
    #[must_use]
    pub fn step(&self) -> usize {
        if 0 == self.width {
            self.tab_stop.get()
        } else {
            self.width
        }
    }

    /// Shifts one line's indent.
    ///
    /// # Returns
    ///
    /// * The line with its indent moved `columns` columns, written in the whitespace the shift
    ///   spells an indent in.
    /// * `None` for a line vim leaves alone, which is the line holding nothing at all.
    #[must_use]
    pub fn shifted(&self, line: &str, columns: isize) -> Option<String> {
        if line.is_empty() {
            return None;
        }

        let indent = indent_of(line);
        let width = self.columns_of(indent);
        let shifted = if columns < 0 {
            width.saturating_sub(columns.unsigned_abs())
        } else {
            width.saturating_add(columns.unsigned_abs())
        };

        Some(format!("{}{}", self.blanks(shifted), &line[indent.len()..]))
    }

    /// # Returns
    ///
    /// The screen columns `indent` carries a line by, in which a tab is worth the columns it takes
    /// to reach the next tab stop.
    fn columns_of(&self, indent: &str) -> usize {
        let tab_stop = self.tab_stop.get();

        indent.chars().fold(0, |columns, blank| match blank {
            '\t' => columns + tab_stop - columns % tab_stop,
            _ => columns + 1,
        })
    }

    /// # Returns
    ///
    /// The whitespace carrying a line `columns` columns: spaces alone where the shift is written
    /// in spaces, and otherwise as many tabs as reach a tab stop below it followed by the spaces
    /// that make up the rest.
    fn blanks(&self, columns: usize) -> String {
        if self.expand_tabs {
            return " ".repeat(columns);
        }
        let tab_stop = self.tab_stop.get();

        format!(
            "{}{}",
            "\t".repeat(columns / tab_stop),
            " ".repeat(columns % tab_stop)
        )
    }
}

impl Default for Shift {
    fn default() -> Self {
        Self {
            width: DEFAULT_WIDTH,
            tab_stop: NonZeroUsize::new(DEFAULT_WIDTH).expect("the default tab stop is not zero"),
            expand_tabs: false,
        }
    }
}

/// # Returns
///
/// The character of `line` the cursor rests on once the line has been shifted, counted the way
/// modalkit counts a column: the line's first non-blank, the last character of a line holding
/// nothing but blanks, and the first column of a line holding nothing.
#[must_use]
pub fn resting_column(line: &str) -> usize {
    let blanks = indent_of(line).chars().count();
    if blanks < line.chars().count() {
        blanks
    } else {
        blanks.saturating_sub(1)
    }
}

/// # Returns
///
/// The blanks `line` begins with, which is the whole of a line holding nothing else.
fn indent_of(line: &str) -> &str {
    let end = line
        .find(|held| !matches!(held, ' ' | '\t'))
        .unwrap_or(line.len());

    &line[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// # Returns
    ///
    /// A shift carrying a line `width` columns per step against a tab stop of `tab_stop`.
    ///
    /// # Panics
    ///
    /// Panics if `tab_stop` is zero.
    fn shift(width: usize, tab_stop: usize, expand_tabs: bool) -> Shift {
        Shift::new(
            width,
            NonZeroUsize::new(tab_stop).expect("the tab stop is not zero"),
            expand_tabs,
        )
    }

    #[test]
    fn an_indent_narrower_than_a_tab_stop_is_written_in_spaces() {
        assert_eq!(
            Some("    alpha".to_owned()),
            shift(4, 8, false).shifted("alpha", 4)
        );
    }

    #[test]
    fn an_indent_reaching_a_tab_stop_is_written_in_tabs() {
        assert_eq!(
            Some("\talpha".to_owned()),
            shift(8, 8, false).shifted("alpha", 8)
        );
        assert_eq!(
            Some("\t    alpha".to_owned()),
            shift(4, 8, false).shifted("\talpha", 4)
        );
    }

    #[test]
    fn an_indent_is_written_in_spaces_alone_where_tabs_are_expanded() {
        assert_eq!(
            Some("            alpha".to_owned()),
            shift(4, 8, true).shifted("\talpha", 4)
        );
    }

    #[test]
    fn a_mixed_indent_is_measured_in_columns_and_written_out_again() {
        assert_eq!(
            Some("\t    mixed".to_owned()),
            shift(4, 8, false).shifted("  \tmixed", 4)
        );
        assert_eq!(
            Some("\t\tmixed".to_owned()),
            shift(4, 4, false).shifted("  \tmixed", 4)
        );
    }

    #[test]
    fn an_outdent_stops_at_the_left_margin() {
        assert_eq!(
            Some("alpha".to_owned()),
            shift(4, 8, false).shifted("alpha", -4)
        );
        assert_eq!(
            Some("alpha".to_owned()),
            shift(8, 8, false).shifted("    alpha", -8)
        );
    }

    #[test]
    fn a_line_of_blanks_keeps_its_blanks_and_a_line_of_nothing_is_left_alone() {
        assert_eq!(
            Some("       ".to_owned()),
            shift(4, 8, false).shifted("   ", 4)
        );
        assert_eq!(Some(" ".to_owned()), shift(2, 8, false).shifted("   ", -2));
        assert_eq!(None, shift(4, 8, false).shifted("", 4));
    }

    #[test]
    fn an_outdent_wider_than_any_indent_could_be_stops_at_the_left_margin() {
        assert_eq!(
            Some("alpha".to_owned()),
            shift(4, 8, false).shifted("\t\talpha", isize::MIN)
        );
    }

    #[test]
    fn a_shift_of_no_width_carries_a_line_a_tab_stop() {
        assert_eq!(3, shift(0, 3, false).step());
        assert_eq!(2, shift(2, 3, false).step());
    }

    #[test]
    fn a_cursor_rests_on_the_first_non_blank_of_the_line_it_was_left_on() {
        assert_eq!(4, resting_column("    alpha"));
        assert_eq!(0, resting_column("alpha"));
        assert_eq!(6, resting_column("       "));
        assert_eq!(0, resting_column(""));
    }
}
