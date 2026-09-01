//! The property-test harness that searches for inputs on which a layout breaks an invariant.
//!
//! A search is driven entirely by its [`Seed`], so a failure prints a seed that replays the search
//! case for case. Failures are shrunk to a smaller input that still breaks the same invariant.

use std::cell::RefCell;
use std::fmt::{Display, Formatter, Result as FmtResult};
use std::num::{NonZeroUsize, ParseIntError};
use std::str::FromStr;

use proptest::prelude::*;
use proptest::test_runner::{Config, RngAlgorithm, TestCaseError, TestError, TestRng, TestRunner};
use vbc_layout::invariants::{
    check, graphemes, Document, Layout, LogicalPosition, Viewport, Violation,
};

/// The number of cases a search runs before it reports a layout as clean.
pub const DEFAULT_CASES: u32 = 512;

/// The graphemes lines are generated from, covering plain, accented, combining, and double-width
/// text as well as the spaces the grapheme count ignores.
const ALPHABET: [&str; 6] = ["a", "b", " ", "é", "e\u{0301}", "漢"];

/// The bounds a generated case is drawn from. The viewport is never narrower than the widest
/// grapheme in [`ALPHABET`], so that a correct layout can always fit one grapheme per row.
const MAX_LINES: usize = 5;
const MAX_LINE_LEN: usize = 16;
const MIN_WIDTH: usize = 2;
const MAX_WIDTH: usize = 8;

/// The generated cursor column that always clamps onto the position past a line's last grapheme,
/// so that end-of-line cursors are generated whatever the line's length.
const END_OF_LINE: usize = usize::MAX;

/// The seed a search's case generation is derived from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Seed(u64);

impl Seed {
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// # Returns
    ///
    /// The random number generator this seed drives a search with.
    fn rng(self) -> TestRng {
        const MIX: u64 = 0x9E37_79B9_7F4A_7C15;

        let mut bytes = [0_u8; 32];
        for (index, chunk) in bytes.chunks_exact_mut(8).enumerate() {
            chunk.copy_from_slice(&(self.0 ^ (index as u64).wrapping_mul(MIX)).to_le_bytes());
        }
        TestRng::from_seed(RngAlgorithm::ChaCha, &bytes)
    }
}

impl Display for Seed {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        write!(f, "{:#018x}", self.0)
    }
}

impl FromStr for Seed {
    type Err = ParseIntError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        u64::from_str_radix(text.strip_prefix("0x").unwrap_or(text), 16).map(Self)
    }
}

/// One generated case: the document laid out, the viewport it is laid out into, and where the
/// cursor rests.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LayoutInput {
    /// The text being laid out.
    pub document: Document,

    /// The area the text is laid out into.
    pub viewport: Viewport,

    /// The cursor's logical position, which may rest past the last grapheme of its line.
    pub cursor: LogicalPosition,
}

impl LayoutInput {
    /// # Returns
    ///
    /// How large the case is, which shrinking is expected to reduce.
    #[must_use]
    pub fn size(&self) -> usize {
        let grapheme_count: usize = self
            .document
            .lines()
            .iter()
            .map(|line| graphemes(line).count())
            .sum();
        grapheme_count + self.document.lines().len() + self.viewport.width.get()
    }
}

impl Display for LayoutInput {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        writeln!(f, "viewport width {}", self.viewport.width)?;
        writeln!(f, "cursor at {}", self.cursor)?;
        for (index, line) in self.document.lines().iter().enumerate() {
            writeln!(f, "line {index}: `{line}`")?;
        }

        Ok(())
    }
}

/// A layout defect a search found, holding both the case that first failed and the smallest case
/// the search could shrink it to.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FuzzFailure {
    /// The seed that replays the search which found this defect.
    pub seed: Seed,

    /// The first case the layout failed.
    pub original: LayoutInput,

    /// The smallest case the search could shrink the failure to.
    pub minimal: LayoutInput,

    /// The invariants the layout breaks on [`FuzzFailure::minimal`].
    pub violations: Vec<Violation>,
}

impl Display for FuzzFailure {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        for violation in &self.violations {
            writeln!(f, "{violation}")?;
        }
        writeln!(f, "shrunk from a case of size {}:", self.original.size())?;
        write!(f, "{}", self.minimal)?;
        write!(f, "replay this search with seed {}", self.seed)
    }
}

/// Searches for an input on which a layout breaks an invariant, shrinking whatever it finds.
///
/// # Type Parameters
///
/// * `LayoutType` - The layout under search.
///
/// # Returns
///
/// `Ok(())` if the layout survived `cases` generated inputs.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`FuzzFailure`] if an input broke an invariant, holding the shrunk case and the seed that
///   replays the search. It is boxed because a failure is far larger than the success it is
///   returned alongside.
///
/// # Panics
///
/// Panics if the case generator cannot produce enough cases to run the search.
pub fn search<LayoutType: Layout>(
    layout: &LayoutType,
    seed: Seed,
    cases: u32,
) -> Result<(), Box<FuzzFailure>> {
    let config = Config {
        cases,
        failure_persistence: None,
        ..Config::default()
    };
    let mut runner = TestRunner::new_with_rng(config, seed.rng());
    let original: RefCell<Option<LayoutInput>> = RefCell::new(None);

    let outcome = runner.run(&layout_input(), |input| {
        let violations = check(layout, &input.document, input.viewport, input.cursor);
        let Some(violation) = violations.first() else {
            return Ok(());
        };
        original.borrow_mut().get_or_insert_with(|| input.clone());
        Err(TestCaseError::fail(violation.to_string()))
    });

    match outcome {
        Ok(()) => Ok(()),
        Err(TestError::Fail(_, minimal)) => {
            let violations = check(layout, &minimal.document, minimal.viewport, minimal.cursor);
            let original = original
                .into_inner()
                .expect("the failing case is recorded before it is shrunk");
            Err(Box::new(FuzzFailure {
                seed,
                original,
                minimal,
                violations,
            }))
        }
        Err(TestError::Abort(reason)) => panic!("the layout search was aborted: {reason}"),
    }
}

/// # Returns
///
/// A strategy generating the documents, viewports, and cursor positions a layout is searched over.
fn layout_input() -> impl Strategy<Value = LayoutInput> {
    let line = proptest::collection::vec(
        proptest::sample::select(ALPHABET.as_slice()),
        0..=MAX_LINE_LEN,
    )
    .prop_map(|line_graphemes| line_graphemes.concat());
    (
        MIN_WIDTH..=MAX_WIDTH,
        proptest::collection::vec(line, 1..=MAX_LINES),
        0..MAX_LINES,
        prop_oneof![3 => 0..=MAX_LINE_LEN, 1 => Just(END_OF_LINE)],
    )
        .prop_map(|(width, lines, line_choice, column_choice)| {
            let document = Document::new(lines);
            let line = line_choice % document.lines().len();
            let cursor = document.clamp(LogicalPosition {
                line,
                grapheme: column_choice,
            });
            LayoutInput {
                document,
                viewport: Viewport {
                    width: NonZeroUsize::new(width).expect("the generated width is at least one"),
                },
                cursor,
            }
        })
}
