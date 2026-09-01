//! Checks of the layout fuzz harness itself: that it clears a correct layout, that it catches each
//! invariant being broken, and that a failure is reported as a shrunk case with a replayable seed.

mod fuzz;

use fuzz::harness::{search, FuzzFailure, Seed, DEFAULT_CASES};
use fuzz::reference::Wrapped;
use fuzz::violations::{
    DropsLastGrapheme, MergesLastCell, NeverWraps, OverflowsEndOfLine, PadsWithEmptyRows,
};
use vbc_layout::invariants::{Invariant, Layout};

/// The seed every search in these tests starts from, chosen so each planted defect is found.
const SEED: Seed = Seed::new(0x7669_6D62_6563_6F64);

/// A second seed, used to check that a search follows the seed it is given.
const ALTERNATE_SEED: Seed = Seed::new(0x6C61_796F_7574_0001);

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

#[test]
fn reference_layout_satisfies_every_invariant() {
    for seed in 0..8 {
        if let Err(failure) = search(&Wrapped, Seed::new(seed), DEFAULT_CASES) {
            panic!("the reference layout broke an invariant:\n{failure}");
        }
    }
}

#[test]
fn row_width_violation_is_caught() {
    assert_violates(&expect_failure(&NeverWraps, SEED), Invariant::RowWidth);
}

#[test]
fn grapheme_conservation_violation_is_caught() {
    assert_violates(
        &expect_failure(&DropsLastGrapheme, SEED),
        Invariant::GraphemeConservation,
    );
}

#[test]
fn no_empty_rows_violation_is_caught() {
    assert_violates(
        &expect_failure(&PadsWithEmptyRows, SEED),
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
    let failure = expect_failure(&NeverWraps, SEED);
    let alternate = expect_failure(&NeverWraps, ALTERNATE_SEED);

    assert_ne!(
        failure.original, alternate.original,
        "both seeds searched the same cases, so a reported seed replays nothing"
    );
}

#[test]
fn shrinking_reduces_the_failing_case() {
    let failure = expect_failure(&NeverWraps, SEED);

    assert!(
        failure.minimal.size() < failure.original.size(),
        "shrinking did not reduce the case:\n{failure}"
    );
}
