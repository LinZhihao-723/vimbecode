//! Laying one logical line out into the display rows that render it.
//!
//! Wrapping is a pure function of the line, the number of columns it is drawn in, the [`Metrics`]
//! it is measured under, and the display options it is drawn with: no viewport, no cursor, and no
//! state of any kind. The rows are the rows vim draws for the same line, so this module carries
//! vim's rules rather than rules of its own -- where a row breaks, how far a continuation row is
//! indented, and where the marker announcing one goes.
//!
//! Four of vim's rules are worth stating before the code states them. A line is measured against
//! the columns already drawn rather than against the columns of its own row, so a tab reaches the
//! tab stop it would reach on an unwrapped screen and a continuation row's decoration pushes the
//! text after it along. A tab that runs off the end of a row is drawn across the boundary rather
//! than moved whole, so what it owes the next row is measured from where it began and is not
//! shortened by that row's decoration. An indent is repeated onto a continuation row only while
//! at least [`Options::break_indent_min`] columns remain beside it, which is why `'breakindent'`
//! does nothing at all in a viewport as narrow as its own threshold. And the leading run of
//! `'breakat'` characters of a line is not a place a word-wrapped row may end, so an indented
//! first word is split rather than pushed off the row it starts on.
//!
//! `'linebreak'` together with tabs is the one combination this module does not reproduce: vim
//! measures a word from the column the break character in front of it starts at, so a tab
//! separating two words does not count against the word behind it, and a row therefore holds more
//! than the rules here give it. The test
//! `word_wrapping_beside_a_tab_is_not_measured_the_way_vim_measures_it` pins what this module
//! draws instead.
//!
//! Where a decoration would leave no room for the text it decorates, vim draws the same row
//! forever. This module drops the decoration from such a row instead, so that a layout always
//! ends and always advances.
//!
//! `min` is the only `'breakindentopt'` field carried here; `shift`, `sbr`, `list` and `column`
//! are left at the defaults under which vim adds nothing of its own.

use std::num::NonZeroUsize;

use crate::width::{grapheme_indices, graphemes, Metrics};

/// The characters vim's `'breakat'` default allows a word-wrapped row to end on.
pub const DEFAULT_BREAK_AT: &str = " \t!@*-+;:,./?";

/// The number of columns vim's `'breakindentopt'` default, `min:20`, keeps for text beside a
/// repeated indent.
pub const DEFAULT_BREAK_INDENT_MIN: usize = 20;

/// How a logical line is wrapped, mirroring the vim options that decide it.
///
/// The defaults are vim's own: character wrapping, no repeated indent, and no continuation marker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Options {
    break_indent: bool,
    break_indent_min: usize,
    show_break: String,
    line_break: bool,
    break_at: String,
}

impl Options {
    /// Factory function.
    ///
    /// # Returns
    ///
    /// A newly created set of options holding vim's own defaults.
    #[must_use]
    pub fn new() -> Self {
        Self {
            break_indent: false,
            break_indent_min: DEFAULT_BREAK_INDENT_MIN,
            show_break: String::new(),
            line_break: false,
            break_at: DEFAULT_BREAK_AT.to_owned(),
        }
    }

    /// # Returns
    ///
    /// These options with `'breakindent'` set as given.
    #[must_use]
    pub fn with_break_indent(mut self, enabled: bool) -> Self {
        self.break_indent = enabled;
        self
    }

    /// # Returns
    ///
    /// These options with the `'breakindentopt'` `min` threshold set as given.
    #[must_use]
    pub fn with_break_indent_min(mut self, columns: usize) -> Self {
        self.break_indent_min = columns;
        self
    }

    /// # Returns
    ///
    /// These options with `'showbreak'` set to the given marker, which is empty for none.
    #[must_use]
    pub fn with_show_break(mut self, marker: String) -> Self {
        self.show_break = marker;
        self
    }

    /// # Returns
    ///
    /// These options with `'linebreak'` set as given.
    #[must_use]
    pub fn with_line_break(mut self, enabled: bool) -> Self {
        self.line_break = enabled;
        self
    }

    /// # Returns
    ///
    /// These options with `'breakat'` set to the given characters.
    #[must_use]
    pub fn with_break_at(mut self, characters: String) -> Self {
        self.break_at = characters;
        self
    }

    /// # Returns
    ///
    /// Whether a continuation row repeats the line's indent.
    #[must_use]
    pub fn break_indent(&self) -> bool {
        self.break_indent
    }

    /// # Returns
    ///
    /// The columns kept for the text beside a repeated indent.
    #[must_use]
    pub fn break_indent_min(&self) -> usize {
        self.break_indent_min
    }

    /// # Returns
    ///
    /// The marker put in front of a continuation row, empty for none.
    #[must_use]
    pub fn show_break(&self) -> &str {
        &self.show_break
    }

    /// # Returns
    ///
    /// Whether a row ends on a break character rather than wherever it runs out of columns.
    #[must_use]
    pub fn line_break(&self) -> bool {
        self.line_break
    }

    /// # Returns
    ///
    /// The characters a word-wrapped row may end on.
    #[must_use]
    pub fn break_at(&self) -> &str {
        &self.break_at
    }
}

impl Default for Options {
    fn default() -> Self {
        Self::new()
    }
}

/// One display row of a wrapped logical line: the slice of the line the row shows, together with
/// the decoration drawn in front of it.
///
/// The decoration is the repeated indent and the continuation marker, in that order, and is empty
/// on the row that starts a line. A row carries the line's own bytes as well as the cells they are
/// drawn in, which differ wherever a tab stands for the blanks it advances by: only the layout
/// knows the column a tab was measured against, so only the layout can spell one out.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DisplayRow {
    start: usize,
    end: usize,
    prefix: String,
    text: String,
    cells: String,
    columns: Vec<usize>,
}

impl DisplayRow {
    /// # Returns
    ///
    /// The grapheme offset within the logical line at which the row's text starts.
    #[must_use]
    pub fn start(&self) -> usize {
        self.start
    }

    /// # Returns
    ///
    /// The grapheme offset within the logical line just past the row's text.
    #[must_use]
    pub fn end(&self) -> usize {
        self.end
    }

    /// # Returns
    ///
    /// The decoration drawn in front of the row's text, empty on the row that starts a line.
    #[must_use]
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// # Returns
    ///
    /// The slice of the logical line the row shows.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// # Returns
    ///
    /// The cells the row is drawn in, its decoration included and every tab spelled as the blanks
    /// it advances by.
    #[must_use]
    pub fn cells(&self) -> &str {
        &self.cells
    }

    /// # Returns
    ///
    /// The column each of the row's graphemes is drawn at, its decoration accounted for, followed
    /// by the column just past the row's last grapheme.
    #[must_use]
    pub fn columns(&self) -> &[usize] {
        &self.columns
    }

    /// # Returns
    ///
    /// The number of columns the row occupies, its decoration included.
    ///
    /// # Panics
    ///
    /// Panics if the row carries no columns, which no laid-out row does.
    #[must_use]
    pub fn width(&self) -> usize {
        *self
            .columns
            .last()
            .expect("a row's columns end with the column past its text")
    }
}

/// Lays one logical line out into the display rows that render it.
///
/// A row never holds more than `width` columns and always shows at least one grapheme, so the rows
/// partition the line and there are always finitely many of them. The one exception is a grapheme
/// too wide for a whole row, which is placed alone on a row wider than `width` because no other
/// placement would ever advance: vim spills a tab that wide over as many rows as it takes, which
/// rows that partition a line by grapheme cannot express. A tab that merely runs off the end of
/// the row it starts on is not an exception -- it is carried to the next row and measured there
/// from where it began.
///
/// An empty line lays out into one empty row, so that every logical line is rendered by at least
/// one row.
///
/// # Returns
///
/// The rows rendering `line`, top to bottom.
#[must_use]
pub fn lay_out(
    line: &str,
    width: NonZeroUsize,
    metrics: Metrics,
    options: &Options,
) -> Vec<DisplayRow> {
    let decoration = continuation_decoration(line, width, metrics, options);
    let width = width.get();
    let clusters: Vec<(usize, &str)> = grapheme_indices(line).collect();
    if clusters.is_empty() {
        return vec![DisplayRow {
            start: 0,
            end: 0,
            prefix: String::new(),
            text: String::new(),
            cells: String::new(),
            columns: vec![0],
        }];
    }

    let decoration_width = metrics.text_width(&decoration, 0);
    let marker_skipped = if options.line_break {
        0
    } else {
        metrics.text_width(&options.show_break, 0)
    };

    let first_word = clusters
        .iter()
        .position(|&(_, grapheme)| !is_break_at(grapheme, &options.break_at))
        .unwrap_or(clusters.len());

    let mut rows: Vec<DisplayRow> = Vec::new();
    let mut start = 0;
    while start < clusters.len() {
        let drawn = rows.len() * width;
        let first = clusters[start].1;
        let carried = carried_tab_columns(rows.last(), first, drawn, width, metrics);
        let beside_decoration = carried.unwrap_or_else(|| {
            grapheme_columns(first, drawn + decoration_width, marker_skipped, metrics)
        });
        let decorated = !rows.is_empty() && decoration_width + beside_decoration <= width;
        let first_width = if decorated {
            beside_decoration
        } else {
            carried.unwrap_or_else(|| grapheme_columns(first, drawn, 0, metrics))
        };
        let prefix = if decorated {
            decoration.clone()
        } else {
            String::new()
        };
        let mut cells = prefix.clone();
        let mut column = if decorated { decoration_width } else { 0 };
        let mut columns = Vec::new();
        let mut index = start;
        while index < clusters.len() {
            let grapheme = clusters[index].1;
            if start < index
                && first_word < index
                && options.line_break
                && is_break_at(clusters[index - 1].1, &options.break_at)
                && !is_break_at(grapheme, &options.break_at)
                && !word_fits(
                    &clusters[index..],
                    drawn + column,
                    width.saturating_sub(column),
                    metrics,
                    &options.break_at,
                )
            {
                break;
            }

            let grapheme_width = if index == start {
                first_width
            } else {
                metrics.grapheme_width(grapheme, drawn + column)
            };
            if width < column + grapheme_width && start < index {
                break;
            }
            if "\t" == grapheme {
                cells.extend(std::iter::repeat_n(' ', grapheme_width));
            } else {
                cells.push_str(grapheme);
            }
            columns.push(column);
            column += grapheme_width;
            index += 1;
        }
        columns.push(column);

        let text_start = clusters[start].0;
        let text_end = clusters
            .get(index)
            .map_or_else(|| line.len(), |&(offset, _)| offset);
        rows.push(DisplayRow {
            start,
            end: index,
            prefix,
            text: line[text_start..text_end].to_owned(),
            cells,
            columns,
        });
        start = index;
    }

    rows
}

/// Builds the decoration a continuation row of a line carries: the repeated indent followed by
/// the continuation marker.
///
/// vim repeats an indent only while `min` columns remain for the text beside it, and shortens the
/// repeated indent rather than dropping it when fewer do. A row drops the decoration altogether
/// where it would leave no room for the text it decorates.
///
/// # Returns
///
/// The decoration, empty if the options ask for none.
#[must_use]
pub fn continuation_decoration(
    line: &str,
    width: NonZeroUsize,
    metrics: Metrics,
    options: &Options,
) -> String {
    let mut decoration = String::new();
    if options.break_indent {
        let repeated =
            indent_width(line, metrics).min(width.get().saturating_sub(options.break_indent_min));
        decoration.push_str(&" ".repeat(repeated));
    }
    decoration.push_str(&options.show_break);

    decoration
}

/// Measures one grapheme drawn at a given column of the screen.
///
/// A tab drawn as the first grapheme after a continuation marker is measured from the column the
/// marker itself started at, which is how vim draws one: the marker does not push the tab on to a
/// later tab stop. `marker_skipped` is that marker's width, and is zero both for every other
/// grapheme, which is measured from where it is drawn, and under `'linebreak'`, where vim does let
/// the marker push the tab along.
///
/// # Returns
///
/// The number of columns `grapheme` occupies.
fn grapheme_columns(
    grapheme: &str,
    column: usize,
    marker_skipped: usize,
    metrics: Metrics,
) -> usize {
    if "\t" == grapheme {
        return metrics.tab_width(column.saturating_sub(marker_skipped));
    }

    metrics.grapheme_width(grapheme, column)
}

/// # Returns
///
/// The number of columns the leading whitespace of `line` occupies, which is what a repeated
/// indent is measured from.
fn indent_width(line: &str, metrics: Metrics) -> usize {
    let mut column = 0;
    for grapheme in graphemes(line) {
        match grapheme {
            " " => column += 1,
            "\t" => column += metrics.tab_width(column),
            _ => break,
        }
    }

    column
}

/// Measures what is left of a tab that ran off the end of the row before this one.
///
/// vim draws such a tab across the row boundary rather than moving it whole, so the columns it
/// still owes are the columns between the row this one follows and the tab stop the tab reaches.
/// A row's decoration is drawn in front of them and does not shorten them.
///
/// # Returns
///
/// The columns the tab owes this row, or `None` if `first` is not a tab carried from the row
/// before it.
fn carried_tab_columns(
    previous: Option<&DisplayRow>,
    first: &str,
    drawn: usize,
    width: usize,
    metrics: Metrics,
) -> Option<usize> {
    if "\t" != first {
        return None;
    }
    let previous = previous?;
    if width <= previous.width() {
        return None;
    }
    let started = drawn - width + previous.width();
    let reached = started + metrics.tab_width(started);
    (drawn < reached).then(|| reached - drawn)
}

/// Measures the word a word-wrapped row would have to keep whole.
///
/// A word is measured together with the break characters that follow it, as vim measures one: a
/// word is kept off a row that could hold its letters but not the separator behind them.
///
/// # Returns
///
/// Whether the word starting `clusters` fits in `room` columns when it is drawn from `column`.
fn word_fits(
    clusters: &[(usize, &str)],
    column: usize,
    room: usize,
    metrics: Metrics,
    break_at: &str,
) -> bool {
    let mut used = 0;
    let mut separating = false;
    for &(_, grapheme) in clusters {
        if is_break_at(grapheme, break_at) {
            separating = true;
        } else if separating {
            break;
        }
        used += metrics.grapheme_width(grapheme, column + used);
        if room < used {
            return false;
        }
    }

    true
}

/// # Returns
///
/// Whether `grapheme` is one of the characters a word-wrapped row may end on.
fn is_break_at(grapheme: &str, break_at: &str) -> bool {
    let mut characters = grapheme.chars();
    match (characters.next(), characters.next()) {
        (Some(character), None) => break_at.contains(character),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::width::{AmbiWidth, DEFAULT_TAB_STOP};

    /// The indented sentence the corpus wraps at several widths, which is the line the golden rows
    /// below were captured from vim for.
    const SENTENCE: &str =
        "    the indented sentence that wraps a few times before it finally ends here";

    /// One tab-measuring case: a line, the width it is laid out in, its tab stop, its
    /// continuation marker, whether its indent is repeated, and the two rows vim draws for it.
    type TabCase = (
        &'static str,
        usize,
        usize,
        &'static str,
        bool,
        [&'static str; 2],
    );

    /// # Returns
    ///
    /// A metrics with the default ambiguous width and a tab stop of `tab_stop`.
    fn metrics(tab_stop: usize) -> Metrics {
        Metrics::new(
            AmbiWidth::default(),
            NonZeroUsize::new(tab_stop).expect("a test's tab stop is not zero"),
        )
    }

    /// # Returns
    ///
    /// The cells each row of `line` is drawn in when it is laid out `width` columns wide.
    fn rows(line: &str, width: usize, metrics: Metrics, options: &Options) -> Vec<String> {
        lay_out(
            line,
            NonZeroUsize::new(width).expect("a test's width is not zero"),
            metrics,
            options,
        )
        .iter()
        .map(|row| row.cells().to_owned())
        .collect()
    }

    #[test]
    fn an_empty_line_is_one_empty_row() {
        let laid_out = lay_out(
            "",
            NonZeroUsize::new(20).expect("a test's width is not zero"),
            Metrics::default(),
            &Options::new(),
        );

        assert_eq!(1, laid_out.len());
        assert_eq!("", laid_out[0].text());
        assert_eq!("", laid_out[0].cells());
        assert_eq!(0, laid_out[0].width());
        assert_eq!(0, laid_out[0].start());
        assert_eq!(0, laid_out[0].end());
    }

    #[test]
    fn rows_partition_the_line_they_render() {
        let line = "\tthe indented 中文 line that wraps a few times over";
        let laid_out = lay_out(
            line,
            NonZeroUsize::new(13).expect("a test's width is not zero"),
            metrics(4),
            &Options::new()
                .with_break_indent(true)
                .with_break_indent_min(4),
        );

        let mut offset = 0;
        let mut rejoined = String::new();
        for row in &laid_out {
            assert_eq!(offset, row.start());
            assert!(row.start() < row.end(), "a row shows no grapheme");
            rejoined.push_str(row.text());
            offset = row.end();
        }

        assert_eq!(line, rejoined);
        assert_eq!(graphemes(line).count(), offset);
    }

    #[test]
    fn a_line_hard_splits_at_the_column_the_row_runs_out_at() {
        assert_eq!(
            rows(SENTENCE, 24, Metrics::default(), &Options::new()),
            [
                "    the indented sentenc",
                "e that wraps a few times",
                " before it finally ends ",
                "here",
            ]
        );
    }

    #[test]
    fn break_indent_repeats_the_indent_onto_every_continuation_row() {
        let options = Options::new().with_break_indent(true);

        assert_eq!(
            rows(SENTENCE, 24, Metrics::default(), &options),
            [
                "    the indented sentenc",
                "    e that wraps a few t",
                "    imes before it final",
                "    ly ends here",
            ]
        );
    }

    #[test]
    fn break_indent_keeps_min_columns_for_the_text_beside_it() {
        // The repeated indent and the first continuation row at each width, for a line indented
        // four columns and the default `min:20`. vim shortens the repeated indent rather than
        // dropping it, and is left with none at all once the viewport is as narrow as the
        // threshold itself. Every row is the row vim draws.
        let expected: [(usize, usize, &str); 7] = [
            (20, 0, "tence that wraps a f"),
            (21, 1, " ence that wraps a fe"),
            (22, 2, "  nce that wraps a few"),
            (23, 3, "   ce that wraps a few "),
            (24, 4, "    e that wraps a few t"),
            (25, 4, "     that wraps a few tim"),
            (26, 4, "    that wraps a few times"),
        ];
        let options = Options::new().with_break_indent(true);

        for (width, indent, drawn) in expected {
            let laid_out = lay_out(
                SENTENCE,
                NonZeroUsize::new(width).expect("a test's width is not zero"),
                Metrics::default(),
                &options,
            );
            let continuation = laid_out
                .get(1)
                .unwrap_or_else(|| panic!("a width of {width} wraps the sentence"));

            assert_eq!(
                " ".repeat(indent),
                continuation.prefix(),
                "the indent repeated at a width of {width}"
            );
            assert_eq!(
                drawn,
                continuation.cells(),
                "the first continuation row at a width of {width}"
            );
        }

        assert_eq!(DEFAULT_BREAK_INDENT_MIN, Options::new().break_indent_min());
    }

    #[test]
    fn break_indent_measures_a_tab_indent_in_the_columns_it_draws() {
        let line = "\t\tthe indented line that has to wrap several times in a narrow viewport";
        let options = Options::new().with_break_indent(true);

        // Two tabs indent sixteen columns, which `min:20` shortens to four in a viewport
        // twenty-four columns wide.
        assert_eq!(
            rows(line, 24, metrics(DEFAULT_TAB_STOP), &options),
            [
                "                the inde",
                "    nted line that has t",
                "    o wrap several times",
                "     in a narrow viewpor",
                "    t",
            ]
        );
    }

    #[test]
    fn show_break_marks_every_continuation_row() {
        let options = Options::new().with_show_break("> ".to_owned());

        assert_eq!(
            rows(SENTENCE, 24, Metrics::default(), &options),
            [
                "    the indented sentenc",
                "> e that wraps a few tim",
                "> es before it finally e",
                "> nds here",
            ]
        );
    }

    #[test]
    fn show_break_is_drawn_after_the_repeated_indent() {
        let options = Options::new()
            .with_break_indent(true)
            .with_show_break("> ".to_owned());

        assert_eq!(
            rows(SENTENCE, 24, Metrics::default(), &options),
            [
                "    the indented sentenc",
                "    > e that wraps a few",
                "    >  times before it f",
                "    > inally ends here",
            ]
        );
        assert_eq!(
            rows(
                SENTENCE,
                40,
                Metrics::default(),
                &Options::new()
                    .with_break_indent(true)
                    .with_show_break("+++ ".to_owned()),
            ),
            [
                "    the indented sentence that wraps a f",
                "    +++ ew times before it finally ends ",
                "    +++ here",
            ]
        );
    }

    #[test]
    fn a_tab_is_measured_against_the_columns_already_drawn() {
        // Every expected row is what vim draws for the same line, options and tab stop. The
        // corpus holds no case with a tab on a continuation row, so these rows are the only thing
        // pinning the rule down.
        let cases: [TabCase; 7] = [
            (
                "aaaaaaaaaaaaaaaaaaaaaa\tX",
                20,
                8,
                "",
                false,
                ["aaaaaaaaaaaaaaaaaaaa", "aa  X"],
            ),
            (
                "aaaaaaaaaaaaaaaaaaaaaa\tX",
                20,
                8,
                ">>",
                false,
                ["aaaaaaaaaaaaaaaaaaaa", ">>aa        X"],
            ),
            (
                "    aaaaaaaaaaaaaaaaaaaaaa\tX",
                24,
                8,
                "",
                true,
                ["    aaaaaaaaaaaaaaaaaaaa", "    aa  X"],
            ),
            (
                "    aaaaaaaaaaaaaaaaaaaaa\tX",
                25,
                8,
                "",
                true,
                ["    aaaaaaaaaaaaaaaaaaaaa", "       X"],
            ),
            (
                "aaaaaaaaaaaaaaaaa\tX",
                20,
                8,
                "",
                false,
                ["aaaaaaaaaaaaaaaaa", "    X"],
            ),
            (
                "中中中中中中中中中\tX",
                21,
                8,
                "",
                false,
                ["中中中中中中中中中", "   X"],
            ),
            ("ab\tX", 12, 20, "", false, ["ab", "        X"]),
        ];

        for (line, width, tab_stop, marker, break_indent, expected) in cases {
            let options = Options::new()
                .with_break_indent(break_indent)
                .with_show_break(marker.to_owned());

            assert_eq!(
                rows(line, width, metrics(tab_stop), &options),
                expected,
                "`{line}` at a width of {width} with a tab stop of {tab_stop}"
            );
        }
    }

    #[test]
    fn a_tab_opening_a_row_is_measured_from_where_its_marker_started() {
        // vim does not let a continuation marker push a tab drawn right after it on to a later tab
        // stop, so the tab reaches the stop it would have reached without one. Every expected row
        // is what vim draws.
        let cases: [TabCase; 4] = [
            (
                "aaaaaaaaaaaaaaaaaaaa\tX",
                20,
                8,
                ">>",
                false,
                ["aaaaaaaaaaaaaaaaaaaa", ">>    X"],
            ),
            (
                "aaaaaaaaaaaaaaaaaaaab\tX",
                20,
                8,
                ">>",
                false,
                ["aaaaaaaaaaaaaaaaaaaa", ">>b X"],
            ),
            (
                "    aaaaaaaaaaaaaaaaaaaaa\tX",
                25,
                8,
                ">>",
                true,
                ["    aaaaaaaaaaaaaaaaaaaaa", "    >>   X"],
            ),
            ("ab\tX", 12, 20, ">>", false, ["ab", ">>        X"]),
        ];

        for (line, width, tab_stop, marker, break_indent, expected) in cases {
            let options = Options::new()
                .with_break_indent(break_indent)
                .with_show_break(marker.to_owned());

            assert_eq!(
                rows(line, width, metrics(tab_stop), &options),
                expected,
                "`{line}` at a width of {width} with a tab stop of {tab_stop}"
            );
        }
    }

    #[test]
    fn word_wrapping_ends_a_row_on_a_break_character() {
        let options = Options::new().with_line_break(true);

        assert_eq!(
            rows(SENTENCE, 20, Metrics::default(), &options),
            [
                "    the indented ",
                "sentence that wraps ",
                "a few times before ",
                "it finally ends here",
            ]
        );
        assert_eq!(
            rows(
                "data-source-name-and-other-value end",
                20,
                Metrics::default(),
                &options,
            ),
            ["data-source-name-", "and-other-value end"]
        );
    }

    #[test]
    fn word_wrapping_only_ends_a_row_on_a_character_break_at_names() {
        // With only the hyphen named, the spaces stop being places a row may end. These are the
        // rows vim draws under `breakat=-`.
        let options = Options::new()
            .with_line_break(true)
            .with_break_at("-".to_owned());

        assert_eq!(
            rows(
                "alpha beta-gamma delta-epsilon zeta",
                20,
                Metrics::default(),
                &options,
            ),
            ["alpha beta-", "gamma delta-", "epsilon zeta"]
        );
    }

    #[test]
    fn word_wrapping_carries_the_decoration_a_character_wrapped_row_carries() {
        let options = Options::new()
            .with_line_break(true)
            .with_break_indent(true)
            .with_show_break("> ".to_owned());

        assert_eq!(
            rows(SENTENCE, 24, Metrics::default(), &options),
            [
                "    the indented ",
                "    > sentence that ",
                "    > wraps a few times ",
                "    > before it finally ",
                "    > ends here",
            ]
        );
    }

    #[test]
    fn word_wrapping_hard_splits_a_run_it_cannot_break() {
        assert_eq!(
            rows(
                "short aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa end",
                20,
                Metrics::default(),
                &Options::new().with_line_break(true),
            ),
            ["short ", "aaaaaaaaaaaaaaaaaaaa", "aaaaaaaaaa end"]
        );
    }

    #[test]
    fn a_decoration_with_no_room_beside_it_is_dropped_rather_than_drawn_forever() {
        // vim redraws such a row for as long as the screen lasts and never advances through the
        // line. Dropping the decoration is what lets a layout end.
        for columns in [12, 15] {
            let marker = ">".repeat(columns);
            let options = Options::new().with_show_break(marker);

            assert_eq!(
                rows(&"a".repeat(30), 12, Metrics::default(), &options),
                ["a".repeat(12), "a".repeat(12), "a".repeat(6)],
                "a marker of {columns} columns in a viewport twelve wide"
            );
        }
    }

    #[test]
    fn a_double_width_cluster_meeting_a_full_indent_still_advances() {
        let line = "中".repeat(8);

        // Two columns are left beside the marker, which is exactly one cluster. This is the row
        // vim draws too.
        let wider = Options::new().with_show_break(">".repeat(10));
        assert_eq!(
            rows(&line, 12, Metrics::default(), &wider),
            ["中中中中中中", ">>>>>>>>>>中", ">>>>>>>>>>中"]
        );

        // One column is left, which is not enough for a cluster two columns wide. vim redraws the
        // marker alone for as long as the screen lasts; dropping it for a row that cannot hold a
        // cluster beside it keeps every row inside the viewport and every row advancing.
        let narrow = Options::new().with_show_break(">".repeat(11));
        assert_eq!(
            rows(&line, 12, Metrics::default(), &narrow),
            ["中中中中中中", "中中"]
        );

        // The smallest case of the same shape, which is what the fuzz search shrinks a width
        // violation to: a two-column marker in a viewport two columns wide.
        assert_eq!(
            rows(
                "漢α",
                2,
                Metrics::default(),
                &Options::new().with_show_break("> ".to_owned()),
            ),
            ["漢", "α"]
        );
    }

    #[test]
    fn a_grapheme_wider_than_the_whole_viewport_is_placed_alone() {
        let laid_out = lay_out(
            "中中",
            NonZeroUsize::new(1).expect("a test's width is not zero"),
            Metrics::default(),
            &Options::new(),
        );

        assert_eq!(2, laid_out.len());
        for row in &laid_out {
            assert_eq!("中", row.text());
            assert_eq!(2, row.width());
        }

        // vim spills a tab this wide over as many rows as it takes, which rows that partition a
        // line by grapheme cannot express. The tab is placed whole instead, on a row wider than
        // the viewport.
        let spilling = lay_out(
            "\tX",
            NonZeroUsize::new(12).expect("a test's width is not zero"),
            metrics(20),
            &Options::new(),
        );

        assert_eq!(2, spilling.len());
        assert_eq!("\t", spilling[0].text());
        assert_eq!(20, spilling[0].width());
        assert_eq!("X", spilling[1].text());
    }

    #[test]
    fn word_wrapping_does_not_end_a_row_on_the_leading_indent() {
        // vim splits the first word of an indented line rather than pushing it off the row its
        // indent starts on, though a run of the same break characters anywhere else does end a
        // row. Every expected row is what vim draws.
        let options = Options::new().with_line_break(true);

        assert_eq!(
            rows(
                &format!("        {} end", "b".repeat(15)),
                22,
                Metrics::default(),
                &options
            ),
            ["        bbbbbbbbbbbbbb", "b end"]
        );
        assert_eq!(
            rows(
                &format!("    {} end", "b".repeat(20)),
                22,
                Metrics::default(),
                &options
            ),
            ["    bbbbbbbbbbbbbbbbbb", "bb end"]
        );
        assert_eq!(
            rows(
                &format!("aa        {} end", "b".repeat(15)),
                22,
                Metrics::default(),
                &options
            ),
            ["aa        ", "bbbbbbbbbbbbbbb end"]
        );
        assert_eq!(
            rows(
                &format!("        {} end", "b".repeat(15)),
                22,
                Metrics::default(),
                &options
                    .clone()
                    .with_break_indent(true)
                    .with_show_break("> ".to_owned()),
            ),
            ["        bbbbbbbbbbbbbb", "  > b end"]
        );
    }

    #[test]
    fn a_tab_that_runs_off_a_row_is_drawn_across_the_boundary() {
        // A tab too wide for the columns left on its row is not moved whole to the next one: vim
        // draws it up to the boundary and owes the rest to the row below, where a repeated indent
        // and a marker are drawn in front of what is owed rather than shortening it. Every
        // expected row is what vim draws.
        assert_eq!(
            rows(
                &format!("{}\tX", "a".repeat(17)),
                20,
                metrics(8),
                &Options::new()
            ),
            ["aaaaaaaaaaaaaaaaa", "    X"]
        );

        let indented = Options::new().with_break_indent(true);
        assert_eq!(
            rows(
                &format!("    {}\tX", "a".repeat(18)),
                24,
                metrics(16),
                &indented
            ),
            ["    aaaaaaaaaaaaaaaaaa", "            X"]
        );
        assert_eq!(
            rows(
                &format!("    {}\tX", "a".repeat(18)),
                24,
                metrics(16),
                &indented.clone().with_show_break(">>".to_owned()),
            ),
            ["    aaaaaaaaaaaaaaaaaa", "    >>        X"]
        );

        // A tab that starts on the continuation row rather than being carried onto it is measured
        // from the columns already drawn, as before.
        assert_eq!(
            rows(
                &format!("    {}\tX", "a".repeat(20)),
                24,
                metrics(8),
                &indented
            ),
            ["    aaaaaaaaaaaaaaaaaaaa", "        X"]
        );
    }

    #[test]
    fn a_marker_pushes_a_tab_opening_a_row_along_under_word_wrapping() {
        // Under `linebreak` vim stops measuring such a tab from where the marker started and lets
        // the marker push it on to a later tab stop, which is the opposite of what it does
        // without. Every expected row is what vim draws.
        let options = Options::new()
            .with_line_break(true)
            .with_show_break(">>".to_owned());

        assert_eq!(
            rows(&format!("{}\tX", "a".repeat(20)), 20, metrics(8), &options),
            ["aaaaaaaaaaaaaaaaaaaa", ">>  X"]
        );
        assert_eq!(
            rows(
                &format!("    {}\tX", "a".repeat(21)),
                25,
                metrics(8),
                &options.clone().with_break_indent(true),
            ),
            ["    aaaaaaaaaaaaaaaaaaaaa", "    >> X"]
        );
    }

    #[test]
    fn word_wrapping_beside_a_tab_is_not_measured_the_way_vim_measures_it() {
        // vim draws this line as `bc ±    α` followed by the blanks the last tab owes, because it
        // measures `α` from the column the tab in front of it starts at rather than from the
        // column the tab ends at. These are the rows this module draws instead, and they are the
        // shape of every difference left between the two under `linebreak` with tabs.
        assert_eq!(
            rows(
                "bc ±\tα\t",
                12,
                metrics(8),
                &Options::new().with_line_break(true)
            ),
            ["bc ±    ", "α   "]
        );
    }

    #[test]
    fn ambiguous_width_text_wraps_where_it_is_measured_to() {
        let line = "α ± … │ 中文 ambiguous width test";

        assert_eq!(
            rows(line, 20, Metrics::default(), &Options::new()),
            ["α ± … │ 中文 ambiguo", "us width test"]
        );
        assert_eq!(
            rows(
                line,
                20,
                Metrics::new(
                    AmbiWidth::Double,
                    NonZeroUsize::new(DEFAULT_TAB_STOP).expect("the default tab stop is not zero"),
                ),
                &Options::new(),
            ),
            ["α ± … │ 中文 amb", "iguous width test"]
        );
    }
}
