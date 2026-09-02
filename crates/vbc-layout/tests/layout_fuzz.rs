//! The invariant search over the real layout: that it survives a hundred thousand generated views,
//! that the harness catches each invariant being broken, that a failure is reported as a shrunk
//! case with a replayable seed, and that the generator really draws the text and the options the
//! search is only useful for covering.

mod fuzz;

use proptest::strategy::{Strategy, ValueTree};
use proptest::test_runner::{Config, TestRunner};

use fuzz::harness::{
    layout_input, search, FuzzFailure, LayoutInput, Seed, DEFAULT_CASES, FLAG, ZWJ_FAMILY,
};
use fuzz::violations::{
    DropsAGrapheme, MergesLastCell, OverdrawsRows, OverflowsEndOfLine, PadsWithAnEmptyRow,
};
use vbc_layout::invariants::{graphemes, Invariant, Layout};
use vbc_layout::line;
use vbc_layout::screen::WrappedLayout;
use vbc_layout::width::{AmbiWidth, DEFAULT_TAB_STOP};

/// The seed every search in these tests starts from, chosen so each planted defect is found.
const SEED: Seed = Seed::new(0x7669_6D62_6563_6F64);

/// A second seed, used to check that a search follows the seed it is given.
const ALTERNATE_SEED: Seed = Seed::new(0x6C61_796F_7574_0001);

/// The number of cases the search over the real layout runs. A layout defect that shows up on one
/// case in ten thousand is a defect a reader meets, so the search is run at a scale that meets it
/// too.
const SOAK_CASES: u32 = 100_000;

/// The seed the coverage tests draw their cases from, and the number they draw.
const COVERAGE_SEED: Seed = Seed::new(0x636F_7665_7261_6765);
const COVERAGE_CASES: usize = 2_000;

/// The number of the drawn cases that must carry each of the two shapes layouts break on, set
/// above what the generator's unconstrained arm reaches on its own so that dropping a deliberate
/// arm turns this into a failing test rather than a smaller number nobody reads.
const HARD_SHAPE_CASES: usize = 420;

/// Draws cases from the generator a search runs over, so that what a search covers can be measured
/// rather than assumed.
///
/// # Returns
///
/// `count` cases, generated from `seed`.
///
/// # Panics
///
/// Panics if the case generator cannot produce a case.
fn cases(seed: Seed, count: usize) -> Vec<LayoutInput> {
    let mut runner = TestRunner::new_with_rng(
        Config {
            failure_persistence: None,
            ..Config::default()
        },
        seed.rng(),
    );
    let strategy = layout_input();

    (0..count)
        .map(|_| {
            strategy
                .new_tree(&mut runner)
                .expect("the case generator produces a case")
                .current()
        })
        .collect()
}

/// Searches a layout that is known to be broken.
///
/// # Type Parameters
///
/// * `LayoutType` - The broken layout.
///
/// # Returns
///
/// The failure the harness found.
///
/// # Panics
///
/// Panics if the harness cleared the layout.
fn expect_failure<LayoutType: Layout>(layout: &LayoutType, seed: Seed) -> FuzzFailure {
    *search(layout, seed, DEFAULT_CASES)
        .expect_err("the harness must catch a layout that breaks an invariant")
}

/// Asserts that a failure breaks the invariant its layout was built to break, and only that one.
///
/// A layout that breaks several invariants at once would satisfy every one of these tests without
/// any of them exercising the check it is named for, so exclusivity is what makes each test
/// evidence that its own invariant is enforced.
fn assert_violates(failure: &FuzzFailure, invariant: Invariant) {
    let broken: Vec<Invariant> = failure
        .violations
        .iter()
        .map(|violation| violation.invariant)
        .collect();

    assert_eq!(broken, vec![invariant], "the harness reported:\n{failure}");
}

/// Asserts that enough of the generated cases carry a shape the search is only useful for
/// covering.
fn assert_covers(
    drawn: &[LayoutInput],
    shape: &str,
    minimum: usize,
    covered: impl Fn(&LayoutInput) -> bool,
) {
    let count = drawn.iter().filter(|input| covered(input)).count();

    assert!(
        minimum <= count,
        "only {count} of {} generated cases carry {shape}, fewer than the {minimum} a search needs",
        drawn.len()
    );
}

/// # Returns
///
/// Whether any line of the case holds `text`.
fn holds(input: &LayoutInput, text: &str) -> bool {
    input
        .document
        .lines()
        .iter()
        .any(|line| line.contains(text))
}

/// # Returns
///
/// Whether the case draws a line whose continuation rows are decorated with under two columns left
/// beside the decoration, and whose text meets that decoration with a two-column cluster.
fn squeezes_a_wide_cluster(input: &LayoutInput) -> bool {
    let wrapping = &input.viewport.wrapping;
    let metrics = wrapping.metrics();
    let width = input.viewport.width();

    input.document.lines().iter().any(|line| {
        let decoration =
            line::continuation_decoration(line, wrapping.width(), metrics, wrapping.options());
        let decoration_width = metrics.text_width(&decoration, 0);
        if decoration.is_empty() || decoration_width + 2 <= width {
            return false;
        }

        line::lay_out(line, wrapping.width(), metrics, wrapping.options())
            .iter()
            .skip(1)
            .any(|row| {
                graphemes(row.text())
                    .next()
                    .is_some_and(|grapheme| 2 == metrics.grapheme_width(grapheme, 0))
            })
    })
}

/// # Returns
///
/// Whether the case's cursor rests past the last grapheme of its line.
fn rests_past_a_line(input: &LayoutInput) -> bool {
    input.document.line_len(input.cursor.line) == Some(input.cursor.grapheme)
}

/// # Returns
///
/// Whether the case's cursor rests past the last grapheme of a line whose last row is exactly
/// full, where the cell drawing it belongs to the row below.
fn rests_past_a_full_row(input: &LayoutInput) -> bool {
    if !rests_past_a_line(input) {
        return false;
    }
    let wrapping = &input.viewport.wrapping;
    let Some(line) = input.document.line(input.cursor.line) else {
        return false;
    };

    line::lay_out(
        line,
        wrapping.width(),
        wrapping.metrics(),
        wrapping.options(),
    )
    .last()
    .is_some_and(|row| input.viewport.width() <= row.width())
}

#[test]
fn the_real_layout_satisfies_every_invariant() {
    for seed in 0..8 {
        if let Err(failure) = search(&WrappedLayout, Seed::new(seed), DEFAULT_CASES) {
            panic!("the real layout broke an invariant:\n{failure}");
        }
    }
}

#[test]
fn the_real_layout_survives_a_hundred_thousand_cases() {
    if let Err(failure) = search(&WrappedLayout, SEED, SOAK_CASES) {
        panic!("the real layout broke an invariant:\n{failure}");
    }
}

#[test]
fn row_width_violation_is_caught() {
    assert_violates(&expect_failure(&OverdrawsRows, SEED), Invariant::RowWidth);
}

#[test]
fn grapheme_conservation_violation_is_caught() {
    assert_violates(
        &expect_failure(&DropsAGrapheme, SEED),
        Invariant::GraphemeConservation,
    );
}

#[test]
fn no_empty_rows_violation_is_caught() {
    assert_violates(
        &expect_failure(&PadsWithAnEmptyRow, SEED),
        Invariant::NoEmptyRows,
    );
}

#[test]
fn cursor_visible_violation_is_caught() {
    assert_violates(
        &expect_failure(&OverflowsEndOfLine, SEED),
        Invariant::CursorVisible,
    );
}

#[test]
fn round_trip_violation_is_caught() {
    assert_violates(&expect_failure(&MergesLastCell, SEED), Invariant::RoundTrip);
}

#[test]
fn printed_seed_replays_the_same_failure() {
    let failure = expect_failure(&MergesLastCell, SEED);
    let replayed_seed: Seed = failure
        .seed
        .to_string()
        .parse()
        .expect("the printed seed must parse back");
    let replayed = expect_failure(&MergesLastCell, replayed_seed);

    assert_eq!(failure, replayed);
}

#[test]
fn different_seeds_search_different_cases() {
    let failure = expect_failure(&OverdrawsRows, SEED);
    let alternate = expect_failure(&OverdrawsRows, ALTERNATE_SEED);

    assert_ne!(
        failure.original, alternate.original,
        "both seeds searched the same cases, so a reported seed replays nothing"
    );
}

#[test]
fn shrinking_reduces_the_failing_case() {
    let failure = expect_failure(&MergesLastCell, SEED);

    assert!(
        failure.minimal.size() < failure.original.size(),
        "shrinking did not reduce the case:\n{failure}"
    );
}

#[test]
fn the_generator_draws_the_text_a_layout_is_hard_on() {
    let drawn = cases(COVERAGE_SEED, COVERAGE_CASES);

    assert_covers(&drawn, "a tab", 100, |input| holds(input, "\t"));
    assert_covers(&drawn, "a joined emoji", 100, |input| {
        holds(input, ZWJ_FAMILY)
    });
    assert_covers(&drawn, "a flag", 100, |input| holds(input, FLAG));
    assert_covers(&drawn, "a combining mark", 100, |input| {
        holds(input, "e\u{0301}") || holds(input, "a\u{301}\u{302}\u{323}")
    });
    assert_covers(&drawn, "a double-width cluster", 100, |input| {
        holds(input, "漢")
    });
    assert_covers(&drawn, "an ambiguous-width letter", 100, |input| {
        holds(input, "α")
    });
}

#[test]
fn the_generator_draws_the_display_options() {
    let drawn = cases(COVERAGE_SEED, COVERAGE_CASES);

    assert_covers(&drawn, "breakindent", 100, |input| {
        input.viewport.wrapping.options().break_indent()
    });
    assert_covers(&drawn, "a showbreak marker", 100, |input| {
        !input.viewport.wrapping.options().show_break().is_empty()
    });
    assert_covers(&drawn, "linebreak", 100, |input| {
        input.viewport.wrapping.options().line_break()
    });
    assert_covers(&drawn, "a tab stop vim does not default to", 100, |input| {
        DEFAULT_TAB_STOP != input.viewport.wrapping.metrics().tab_stop().get()
    });
    assert_covers(&drawn, "vim's default tab stop", 20, |input| {
        DEFAULT_TAB_STOP == input.viewport.wrapping.metrics().tab_stop().get()
    });
    assert_covers(&drawn, "ambiwidth=double", 100, |input| {
        AmbiWidth::Double == input.viewport.wrapping.metrics().ambiwidth()
    });
    assert_covers(&drawn, "a window one row tall", 100, |input| {
        1 == input.viewport.height.get()
    });
    assert_covers(&drawn, "a window several rows tall", 100, |input| {
        1 < input.viewport.height.get()
    });
}

#[test]
fn the_generator_draws_the_shapes_layouts_break_on() {
    let drawn = cases(COVERAGE_SEED, COVERAGE_CASES);

    assert_covers(
        &drawn,
        "a two-column cluster with under two columns beside a continuation decoration",
        HARD_SHAPE_CASES,
        squeezes_a_wide_cluster,
    );
    assert_covers(&drawn, "a cursor past the end of its line", 100, |input| {
        rests_past_a_line(input)
    });
    assert_covers(
        &drawn,
        "a cursor past the end of a line whose last row is full",
        HARD_SHAPE_CASES,
        rests_past_a_full_row,
    );
}

#[test]
fn every_generated_grapheme_fits_a_row_and_fills_a_cell() {
    for input in cases(COVERAGE_SEED, COVERAGE_CASES) {
        let metrics = input.viewport.wrapping.metrics();
        let width = input.viewport.width();
        for line in input.document.lines() {
            let mut column = 0;
            for grapheme in graphemes(line) {
                let occupied = metrics.grapheme_width(grapheme, column);
                assert!(
                    0 < occupied && occupied <= width,
                    "`{}` occupies {occupied} columns of a viewport {width} wide: {input}",
                    grapheme.escape_debug()
                );
                column += occupied;
            }
        }
    }
}
