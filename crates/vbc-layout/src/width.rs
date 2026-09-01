//! The display width of text, which every other part of vimbecode measures with.
//!
//! Text is measured in grapheme clusters rather than in characters: a cluster is what an editor
//! moves the cursor over and what a terminal draws as one glyph, so a joined emoji, a flag, and a
//! letter carrying combining marks each occupy the columns of one glyph rather than the sum of
//! their code points'. A width also depends on the state around it -- on how the characters whose
//! East Asian width is ambiguous are configured to be measured, and on the column a tab starts at
//! -- so widths are asked of a [`Metrics`] carrying that configuration rather than of a free
//! function that has to guess it.

use std::cmp::Ordering;
use std::num::NonZeroUsize;

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// The number of columns a control character occupies, tabs excepted: none, since a control
/// character addresses the terminal rather than filling a cell of it. NUL is one of them.
pub const CONTROL_WIDTH: usize = 0;

/// The number of columns between tab stops when a caller states none, which is vim's own
/// `'tabstop'` default.
pub const DEFAULT_TAB_STOP: usize = 8;

/// How the characters whose East Asian width is ambiguous are measured, mirroring vim's
/// `'ambiwidth'` option.
///
/// The two settings disagree about several hundred characters -- Greek and Cyrillic letters,
/// accented Latin letters, arrows, box drawing, and the enclosed alphanumerics among them -- and
/// measuring one of them wrongly shifts every glyph drawn after it on the row, so the setting is
/// carried rather than assumed.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AmbiWidth {
    /// An ambiguous character occupies one column.
    #[default]
    Single,

    /// An ambiguous character occupies two columns.
    Double,
}

/// The configuration text is measured under, and the authority every width in the application is
/// asked of.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Metrics {
    ambiwidth: AmbiWidth,
    tab_stop: NonZeroUsize,
}

impl Metrics {
    /// Factory function.
    ///
    /// # Returns
    ///
    /// A newly created metrics measuring ambiguous characters as `ambiwidth` says and advancing
    /// tabs to multiples of `tab_stop`.
    #[must_use]
    pub fn new(ambiwidth: AmbiWidth, tab_stop: NonZeroUsize) -> Self {
        Self {
            ambiwidth,
            tab_stop,
        }
    }

    #[must_use]
    pub fn ambiwidth(&self) -> AmbiWidth {
        self.ambiwidth
    }

    #[must_use]
    pub fn tab_stop(&self) -> NonZeroUsize {
        self.tab_stop
    }

    /// # Returns
    ///
    /// The number of columns a tab starting at `column` occupies, which is what it takes to reach
    /// the next multiple of the tab stop and so is between one column and the tab stop itself.
    #[must_use]
    pub fn tab_width(&self, column: usize) -> usize {
        let tab_stop = self.tab_stop.get();
        tab_stop - column % tab_stop
    }

    /// Measures one grapheme cluster drawn at a given column.
    ///
    /// A tab is measured against the column it starts at, so the same tab is between one and
    /// [`Metrics::tab_stop`] columns wide depending on where it is drawn. Every other control
    /// character, NUL included, is [`CONTROL_WIDTH`] columns wide.
    ///
    /// # Returns
    ///
    /// The number of columns `grapheme` occupies when it is drawn starting at `column`.
    #[must_use]
    pub fn grapheme_width(&self, grapheme: &str, column: usize) -> usize {
        let Some(first) = grapheme.chars().next() else {
            return 0;
        };
        if '\t' == first {
            return self.tab_width(column);
        }
        if first.is_control() {
            return CONTROL_WIDTH;
        }

        match self.ambiwidth {
            AmbiWidth::Single => UnicodeWidthStr::width(grapheme),
            AmbiWidth::Double => {
                let width = UnicodeWidthStr::width_cjk(grapheme);
                if 1 == width && is_ambiguous_letter(first) {
                    2
                } else {
                    width
                }
            }
        }
    }

    /// Measures a run of text drawn from a given column.
    ///
    /// # Returns
    ///
    /// The number of columns `text` occupies when it is drawn starting at `column`.
    #[must_use]
    pub fn text_width(&self, text: &str, column: usize) -> usize {
        graphemes(text).fold(0, |width, grapheme| {
            width + self.grapheme_width(grapheme, column + width)
        })
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new(
            AmbiWidth::default(),
            NonZeroUsize::new(DEFAULT_TAB_STOP).expect("the default tab stop is not zero"),
        )
    }
}

/// # Returns
///
/// The grapheme clusters of `text`, in order.
pub fn graphemes(text: &str) -> impl Iterator<Item = &str> {
    text.graphemes(true)
}

/// The characters vim widens under `ambiwidth=double` that `unicode-width` leaves one column wide,
/// as the inclusive ranges of a sorted, non-overlapping table.
///
/// `unicode-width` applies the ambiguous-width rule only to characters that are neither letters nor
/// modifier symbols, which leaves the Greek, Cyrillic, and accented Latin letters one column wide
/// in either context. vim widens all of them, and vim is this project's oracle, so they are carried
/// here: every character with an `East_Asian_Width` of `Ambiguous` or a `Line_Break` of `AI` whose
/// `General_Category` is a letter or a modifier symbol, taken from Unicode 16.0.0.
const AMBIGUOUS_LETTERS: [(char, char); 61] = [
    ('\u{00A8}', '\u{00A8}'),
    ('\u{00AA}', '\u{00AA}'),
    ('\u{00B4}', '\u{00B4}'),
    ('\u{00B8}', '\u{00B8}'),
    ('\u{00BA}', '\u{00BA}'),
    ('\u{00C6}', '\u{00C6}'),
    ('\u{00D0}', '\u{00D0}'),
    ('\u{00D8}', '\u{00D8}'),
    ('\u{00DE}', '\u{00E1}'),
    ('\u{00E6}', '\u{00E6}'),
    ('\u{00E8}', '\u{00EA}'),
    ('\u{00EC}', '\u{00ED}'),
    ('\u{00F0}', '\u{00F0}'),
    ('\u{00F2}', '\u{00F3}'),
    ('\u{00F8}', '\u{00FA}'),
    ('\u{00FC}', '\u{00FC}'),
    ('\u{00FE}', '\u{00FE}'),
    ('\u{0101}', '\u{0101}'),
    ('\u{0111}', '\u{0111}'),
    ('\u{0113}', '\u{0113}'),
    ('\u{011B}', '\u{011B}'),
    ('\u{0126}', '\u{0127}'),
    ('\u{012B}', '\u{012B}'),
    ('\u{0131}', '\u{0133}'),
    ('\u{0138}', '\u{0138}'),
    ('\u{013F}', '\u{0142}'),
    ('\u{0144}', '\u{0144}'),
    ('\u{0148}', '\u{014B}'),
    ('\u{014D}', '\u{014D}'),
    ('\u{0152}', '\u{0153}'),
    ('\u{0166}', '\u{0167}'),
    ('\u{016B}', '\u{016B}'),
    ('\u{01CE}', '\u{01CE}'),
    ('\u{01D0}', '\u{01D0}'),
    ('\u{01D2}', '\u{01D2}'),
    ('\u{01D4}', '\u{01D4}'),
    ('\u{01D6}', '\u{01D6}'),
    ('\u{01D8}', '\u{01D8}'),
    ('\u{01DA}', '\u{01DA}'),
    ('\u{01DC}', '\u{01DC}'),
    ('\u{0251}', '\u{0251}'),
    ('\u{0261}', '\u{0261}'),
    ('\u{02C4}', '\u{02C4}'),
    ('\u{02C7}', '\u{02C7}'),
    ('\u{02C9}', '\u{02CB}'),
    ('\u{02CD}', '\u{02CD}'),
    ('\u{02D0}', '\u{02D0}'),
    ('\u{02D8}', '\u{02DB}'),
    ('\u{02DD}', '\u{02DD}'),
    ('\u{02DF}', '\u{02DF}'),
    ('\u{0391}', '\u{03A1}'),
    ('\u{03A3}', '\u{03A9}'),
    ('\u{03B1}', '\u{03C1}'),
    ('\u{03C3}', '\u{03C9}'),
    ('\u{0401}', '\u{0401}'),
    ('\u{0410}', '\u{044F}'),
    ('\u{0451}', '\u{0451}'),
    ('\u{207F}', '\u{207F}'),
    ('\u{2113}', '\u{2113}'),
    ('\u{2126}', '\u{2126}'),
    ('\u{212B}', '\u{212B}'),
];

/// # Returns
///
/// Whether `character` is one of the ambiguous-width letters [`AMBIGUOUS_LETTERS`] holds.
fn is_ambiguous_letter(character: char) -> bool {
    AMBIGUOUS_LETTERS
        .binary_search_by(|&(first, last)| {
            if character < first {
                Ordering::Greater
            } else if last < character {
                Ordering::Less
            } else {
                Ordering::Equal
            }
        })
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ZWJ sequence for the family of a man, a woman, a girl, and a boy: seven code points and
    /// one glyph.
    const ZWJ_FAMILY: &str = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{200D}\u{1F466}";

    /// The flag of Japan, a pair of regional indicators.
    const FLAG: &str = "\u{1F1EF}\u{1F1F5}";

    /// # Returns
    ///
    /// A metrics measuring ambiguous characters as `ambiwidth` says, with the default tab stop.
    fn metrics(ambiwidth: AmbiWidth) -> Metrics {
        Metrics::new(
            ambiwidth,
            NonZeroUsize::new(DEFAULT_TAB_STOP).expect("the default tab stop is not zero"),
        )
    }

    /// # Returns
    ///
    /// A metrics with the default ambiguous width and a tab stop of `tab_stop`.
    fn tab_stop_metrics(tab_stop: usize) -> Metrics {
        Metrics::new(
            AmbiWidth::default(),
            NonZeroUsize::new(tab_stop).expect("a test's tab stop is not zero"),
        )
    }

    #[test]
    fn width_table_holds_for_both_ambiwidths() {
        // The expected width of a grapheme under `ambiwidth=single` and under `ambiwidth=double`.
        let table: [(&str, usize, usize); 27] = [
            ("", 0, 0),
            ("a", 1, 1),
            (" ", 1, 1),
            ("~", 1, 1),
            ("中", 2, 2),
            ("，", 2, 2),
            ("한", 2, 2),
            ("\u{1112}\u{1161}\u{11AB}", 2, 2),
            ("α", 1, 2),
            ("Ω", 1, 2),
            ("я", 1, 2),
            ("à", 1, 2),
            // Not every accented Latin letter is ambiguous, and one that is not stays narrow
            // under either setting.
            ("ç", 1, 1),
            ("±", 1, 2),
            ("…", 1, 2),
            ("│", 1, 2),
            ("→", 1, 2),
            ("①", 1, 2),
            (ZWJ_FAMILY, 2, 2),
            ("\u{1F469}\u{200D}\u{1F4BB}", 2, 2),
            ("\u{1F44D}\u{1F3FD}", 2, 2),
            ("\u{2764}\u{FE0F}", 2, 2),
            (FLAG, 2, 2),
            ("\u{1F1EF}", 1, 1),
            ("e\u{301}", 1, 1),
            ("a\u{301}\u{302}\u{323}", 1, 1),
            ("\u{200B}", 0, 0),
        ];

        for (text, single, double) in table {
            assert_eq!(
                single,
                metrics(AmbiWidth::Single).text_width(text, 0),
                "the single width of {text:?}"
            );
            assert_eq!(
                double,
                metrics(AmbiWidth::Double).text_width(text, 0),
                "the double width of {text:?}"
            );
        }
    }

    #[test]
    fn ambiguous_letters_are_only_widened_by_the_double_setting() {
        let single = metrics(AmbiWidth::Single);
        let double = metrics(AmbiWidth::Double);
        for (first, last) in AMBIGUOUS_LETTERS {
            for character in [first, last] {
                let text = character.to_string();
                assert_eq!(
                    1,
                    single.text_width(&text, 0),
                    "the single width of {text:?}"
                );
                assert_eq!(
                    2,
                    double.text_width(&text, 0),
                    "the double width of {text:?}"
                );
            }
        }

        // The table carries only what `unicode-width` does not widen by itself, so a character
        // that enters its own ambiguous set has to leave the table.
        for (first, last) in AMBIGUOUS_LETTERS {
            for character in [first, last] {
                let text = character.to_string();
                assert_eq!(
                    1,
                    UnicodeWidthStr::width_cjk(text.as_str()),
                    "`unicode-width` now widens {text:?} by itself"
                );
            }
        }
    }

    #[test]
    fn ambiguous_letter_table_is_sorted_and_disjoint() {
        for (first, last) in AMBIGUOUS_LETTERS {
            assert!(first <= last, "the range {first:?}..={last:?} is reversed");
        }
        for window in AMBIGUOUS_LETTERS.windows(2) {
            let [(_, earlier_last), (later_first, _)] = window else {
                panic!("a window of two holds two ranges");
            };
            assert!(
                earlier_last < later_first,
                "the range ending at {earlier_last:?} is not before the one at {later_first:?}"
            );
        }
    }

    #[test]
    fn tab_advances_to_the_next_tab_stop() {
        let eight = tab_stop_metrics(8);
        let expected_at_eight = [8, 7, 6, 5, 4, 3, 2, 1, 8];
        for (column, expected) in expected_at_eight.into_iter().enumerate() {
            assert_eq!(
                expected,
                eight.tab_width(column),
                "a tab at column {column}"
            );
            assert_eq!(
                expected,
                eight.grapheme_width("\t", column),
                "a tab grapheme at column {column}"
            );
        }

        let four = tab_stop_metrics(4);
        let expected_at_four = [4, 3, 2, 1, 4, 3, 2, 1, 4];
        for (column, expected) in expected_at_four.into_iter().enumerate() {
            assert_eq!(expected, four.tab_width(column), "a tab at column {column}");
            assert_eq!(
                expected,
                four.grapheme_width("\t", column),
                "a tab grapheme at column {column}"
            );
        }

        let one = tab_stop_metrics(1);
        for column in 0..9 {
            assert_eq!(1, one.tab_width(column), "a tab at column {column}");
        }
    }

    #[test]
    fn tabs_in_a_run_are_measured_against_the_columns_they_start_at() {
        let metrics = tab_stop_metrics(4);
        assert_eq!(9, metrics.text_width("a\tvalue", 0));
        assert_eq!(9, metrics.text_width("abc\tvalue", 0));
        assert_eq!(13, metrics.text_width("abcd\tvalue", 0));
        assert_eq!(13, metrics.text_width("中文\tvalue", 0));

        // The same run drawn from another column reaches other tab stops.
        assert_eq!(8, metrics.text_width("a\tvalue", 1));
        assert_eq!(7, metrics.text_width("a\tvalue", 2));
    }

    #[test]
    fn tab_and_nul_are_measured_by_policy_rather_than_by_the_width_tables() {
        // What the width tables answer for a tab and for NUL, which is what falling through to
        // them would measure and is wrong for both.
        assert_eq!(1, UnicodeWidthStr::width("\t"));
        assert_eq!(1, UnicodeWidthStr::width("\0"));
        assert_eq!(1, UnicodeWidthStr::width_cjk("\t"));
        assert_eq!(1, UnicodeWidthStr::width_cjk("\0"));

        for ambiwidth in [AmbiWidth::Single, AmbiWidth::Double] {
            let metrics = metrics(ambiwidth);
            assert_eq!(CONTROL_WIDTH, metrics.grapheme_width("\0", 0));
            assert_eq!(2 + CONTROL_WIDTH, metrics.text_width("a\0b", 0));
            for column in 0..DEFAULT_TAB_STOP {
                assert_eq!(
                    DEFAULT_TAB_STOP - column,
                    metrics.grapheme_width("\t", column),
                    "a tab at column {column}"
                );
            }
        }
    }

    #[test]
    fn control_characters_other_than_tab_report_no_width() {
        let metrics = Metrics::default();
        for character in ['\0', '\u{1}', '\u{7}', '\u{1B}', '\u{7F}', '\u{9F}'] {
            let text = character.to_string();
            assert_eq!(
                CONTROL_WIDTH,
                metrics.grapheme_width(&text, 0),
                "the width of {text:?}"
            );
        }
    }

    #[test]
    fn cluster_iteration_keeps_sequences_whole() {
        let cases: [(&str, &[&str]); 6] = [
            (ZWJ_FAMILY, &[ZWJ_FAMILY]),
            (
                "\u{1F1EF}\u{1F1F5}\u{1F1FA}\u{1F1F8}",
                &["\u{1F1EF}\u{1F1F5}", "\u{1F1FA}\u{1F1F8}"],
            ),
            ("cafe\u{301}", &["c", "a", "f", "e\u{301}"]),
            (
                "a\u{301}\u{302}\u{323} b",
                &["a\u{301}\u{302}\u{323}", " ", "b"],
            ),
            (
                "#\u{FE0F}\u{20E3}\u{2764}\u{FE0F}",
                &["#\u{FE0F}\u{20E3}", "\u{2764}\u{FE0F}"],
            ),
            ("\u{0915}\u{093E}", &["\u{0915}\u{093E}"]),
        ];

        for (text, expected) in cases {
            let clusters: Vec<&str> = graphemes(text).collect();
            assert_eq!(expected, clusters, "the clusters of {text:?}");
        }
    }

    #[test]
    fn a_run_is_as_wide_as_the_clusters_it_holds() {
        let metrics = tab_stop_metrics(4);
        let text = "\tα中\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{200D}\u{1F466}e\u{301}\t!";
        let mut column = 0;
        for grapheme in graphemes(text) {
            column += metrics.grapheme_width(grapheme, column);
        }

        assert_eq!(column, metrics.text_width(text, 0));
    }
}
