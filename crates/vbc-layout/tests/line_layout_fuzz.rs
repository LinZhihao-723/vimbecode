//! Searches for a line, a width, and a set of display options on which the line layout breaks one
//! of the invariants a layout owes its caller.
//!
//! The two invariants the search is worth running for are that no row is wider than the viewport
//! and that the rows show every grapheme of the line exactly once. Both are checked here rather
//! than through [`vbc_layout::invariants::Layout`], whose rows have nowhere to put the decoration
//! a continuation row carries: a marker counted as buffer text would break grapheme conservation
//! for a reason that has nothing to do with wrapping.
//!
//! The generated options include markers as wide as the viewport and wider, which is the shape a
//! layout stops advancing on, and double-width clusters meeting them, which is the shape a layout
//! overflows the viewport on.

use std::collections::BTreeMap;
use std::num::NonZeroUsize;

use proptest::prelude::*;
use vbc_layout::line::{self, DisplayRow, Options};
use vbc_layout::width::{graphemes, AmbiWidth, Metrics};

/// The graphemes lines are generated from, covering plain, break, tab, combining, ambiguous, and
/// double-width text as well as the spaces grapheme conservation ignores.
const ALPHABET: [&str; 9] = ["a", "b", " ", "-", "\t", "é", "e\u{0301}", "漢", "α"];

/// The continuation markers the search runs with. A marker as wide as the viewport, and one wider
/// than it, leave a continuation row no room for the text it decorates.
const MARKERS: [&str; 6] = ["", "> ", "+", "\u{21B3} ", "##########", "###############"];

/// The bounds a generated case is drawn from. A viewport is never narrower than the widest
/// grapheme [`ALPHABET`] holds, and a tab never advances by more than a viewport is wide, so every
/// grapheme a case generates fits in a row of its own and no row has to overflow to advance.
const MAX_LINE_LEN: usize = 24;
const MIN_WIDTH: usize = 2;
const MAX_WIDTH: usize = 10;
const MAX_BREAK_INDENT_MIN: usize = 12;

/// The number of cases each search runs. A prototype of this layout broke the width invariant on
/// roughly one case in three hundred, all of them a double-width cluster meeting a decoration with
/// fewer than two columns left, so a search has to be long enough to meet that shape many times
/// over rather than merely to have a chance of meeting it.
const CASES: u32 = 16_384;

/// One generated case: the line laid out, the columns it is laid out in, how it is measured, and
/// the options it is wrapped under.
#[derive(Clone, Debug)]
struct LineInput {
    line: String,
    width: NonZeroUsize,
    metrics: Metrics,
    options: Options,
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: CASES,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    #[test]
    fn no_row_is_wider_than_the_viewport(input in line_input()) {
        let width = input.width.get();
        for (index, row) in lay_out(&input).iter().enumerate() {
            prop_assert!(
                row.width() <= width,
                "row {index} occupies {} columns in a viewport {width} wide: {input:?}",
                row.width()
            );
            prop_assert_eq!(
                row.width(),
                input.metrics.text_width(row.cells(), 0),
                "row {} is drawn in a different number of columns than it reports: {:?}",
                index,
                input
            );
        }
    }

    #[test]
    fn every_non_space_grapheme_is_shown_exactly_once(input in line_input()) {
        let mut counts: BTreeMap<&str, i64> = BTreeMap::new();
        for grapheme in graphemes(&input.line).filter(|grapheme| !is_space(grapheme)) {
            *counts.entry(grapheme).or_default() += 1;
        }
        let rows = lay_out(&input);
        for row in &rows {
            for grapheme in graphemes(row.text()).filter(|grapheme| !is_space(grapheme)) {
                *counts.entry(grapheme).or_default() -= 1;
            }
        }

        for (grapheme, difference) in counts {
            prop_assert_eq!(
                0,
                difference,
                "the rows show `{}` {} times too often: {:?}",
                grapheme,
                -difference,
                input
            );
        }
    }

    #[test]
    fn the_rows_partition_the_line_and_every_row_advances(input in line_input()) {
        let rows = lay_out(&input);
        prop_assert!(!rows.is_empty(), "a line lays out into no row at all: {input:?}");

        let mut offset = 0;
        let mut rejoined = String::new();
        for (index, row) in rows.iter().enumerate() {
            prop_assert_eq!(
                offset,
                row.start(),
                "row {} does not start where row {} ended: {:?}",
                index,
                index.saturating_sub(1),
                input
            );
            prop_assert!(
                row.start() < row.end() || input.line.is_empty(),
                "row {index} shows no grapheme, so the layout never advances: {input:?}"
            );
            rejoined.push_str(row.text());
            offset = row.end();
        }

        prop_assert_eq!(&rejoined, &input.line);
        prop_assert_eq!(offset, graphemes(&input.line).count());
    }

    #[test]
    fn only_a_continuation_row_carries_a_decoration(input in line_input()) {
        let rows = lay_out(&input);
        prop_assert_eq!(
            "",
            rows.first().map_or("", DisplayRow::prefix),
            "the row that starts a line carries a decoration: {:?}",
            input
        );
        for row in &rows {
            prop_assert!(
                row.cells().starts_with(row.prefix()),
                "a row is not drawn with the decoration it carries: {input:?}"
            );
        }
    }
}

/// # Returns
///
/// The rows `input` lays out into.
fn lay_out(input: &LineInput) -> Vec<DisplayRow> {
    line::lay_out(&input.line, input.width, input.metrics, &input.options)
}

/// # Returns
///
/// Whether `grapheme` is the space that grapheme conservation does not count, which a repeated
/// indent and a wrapped row's blank cells are both made of.
fn is_space(grapheme: &str) -> bool {
    " " == grapheme
}

/// # Returns
///
/// A strategy generating the lines, widths, metrics, and options the layout is searched over.
fn line_input() -> impl Strategy<Value = LineInput> {
    let line = proptest::collection::vec(
        proptest::sample::select(ALPHABET.as_slice()),
        0..=MAX_LINE_LEN,
    )
    .prop_map(|line_graphemes| line_graphemes.concat());
    (
        line,
        MIN_WIDTH..=MAX_WIDTH,
        any::<bool>(),
        proptest::sample::select(MARKERS.as_slice()),
        any::<bool>(),
        0..=MAX_BREAK_INDENT_MIN,
        any::<bool>(),
    )
        .prop_flat_map(
            |(line, width, break_indent, marker, line_break, break_indent_min, wide_ambiguous)| {
                (
                    Just((
                        line,
                        width,
                        break_indent,
                        marker,
                        line_break,
                        break_indent_min,
                        wide_ambiguous,
                    )),
                    1..=width,
                )
            },
        )
        .prop_map(
            |(
                (line, width, break_indent, marker, line_break, break_indent_min, wide_ambiguous),
                tab_stop,
            )| {
                let ambiwidth = if wide_ambiguous {
                    AmbiWidth::Double
                } else {
                    AmbiWidth::Single
                };
                LineInput {
                    line,
                    width: NonZeroUsize::new(width).expect("a generated width is at least two"),
                    metrics: Metrics::new(
                        ambiwidth,
                        NonZeroUsize::new(tab_stop).expect("a generated tab stop is at least one"),
                    ),
                    options: Options::new()
                        .with_break_indent(break_indent)
                        .with_break_indent_min(break_indent_min)
                        .with_show_break(marker.to_owned())
                        .with_line_break(line_break),
                }
            },
        )
}
