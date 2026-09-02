//! The property-test harness that searches for views on which a layout breaks an invariant.
//!
//! A search is driven entirely by its [`Seed`], so a failure prints a seed that replays the search
//! case for case. Failures are shrunk to a smaller view that still breaks the same invariant.
//!
//! The generated cases cover the text and the options a layout is hard on rather than the text it
//! is easy on: tabs, joined emoji, flags, combining marks, and ambiguous-width letters, drawn into
//! viewports of every shape with `'breakindent'`, `'showbreak'`, `'linebreak'`, `'tabstop'`, and
//! `'ambiwidth'` all in play. Two shapes are known to be where layouts break, so they are
//! generated deliberately rather than left to chance: a two-column cluster meeting a continuation
//! decoration with under two columns left beside it, and the cursor resting past the end of a line
//! whose last row is exactly full.
//!
//! A generated grapheme is never wider than the viewport and never zero columns wide. The first
//! would force a correct layout to overflow a row, and the second would draw two positions in one
//! cell, which no mapping can tell apart.

use std::cell::RefCell;
use std::fmt::{Display, Formatter, Result as FmtResult};
use std::num::{NonZeroUsize, ParseIntError};
use std::str::FromStr;

use proptest::prelude::*;
use proptest::test_runner::{Config, RngAlgorithm, TestCaseError, TestError, TestRng, TestRunner};
use vbc_layout::anchor::Wrapping;
use vbc_layout::invariants::{
    check, graphemes, Document, Layout, LogicalPosition, View, Viewport, Violation,
};
use vbc_layout::line::Options;
use vbc_layout::width::{AmbiWidth, Metrics};

/// The number of cases a search runs before it reports a layout as clean.
pub const DEFAULT_CASES: u32 = 512;

/// The ZWJ sequence for the family of a man, a woman, a girl, and a boy: seven code points and one
/// glyph.
pub const ZWJ_FAMILY: &str = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{200D}\u{1F466}";

/// The flag of Japan, a pair of regional indicators.
pub const FLAG: &str = "\u{1F1EF}\u{1F1F5}";

/// The graphemes lines are generated from, covering plain, break, tab, accented, combining,
/// ambiguous, double-width, joined, flag, and skin-toned text as well as the spaces grapheme
/// conservation ignores.
const ALPHABET: [&str; 13] = [
    "a",
    "b",
    " ",
    "-",
    "\t",
    "é",
    "e\u{0301}",
    "a\u{301}\u{302}\u{323}",
    "漢",
    "α",
    ZWJ_FAMILY,
    FLAG,
    "\u{1F44D}\u{1F3FD}",
];

/// The graphemes the squeezed cases are generated from, weighted towards the two-column clusters
/// that a continuation decoration leaves no room for.
const WIDE_ALPHABET: [&str; 4] = ["漢", ZWJ_FAMILY, FLAG, "a"];

/// The continuation markers a generated case is drawn with, the empty one for no marker.
const MARKERS: [&str; 6] = ["", ">", ">>", "…", "→ ", "#####"];

/// The bounds a generated case is drawn from.
const MAX_LINES: usize = 3;
const MAX_LINE_LEN: usize = 12;
const MIN_WIDTH: usize = 2;
const MAX_WIDTH: usize = 10;
const MAX_HEIGHT: usize = 5;
const MAX_BREAK_INDENT_MIN: usize = 8;

/// The narrowest viewport a squeezed case is drawn into, which needs a column for the decoration
/// and one beside it.
const MIN_SQUEEZED_WIDTH: usize = 3;

/// The rows a full line of an end-of-line case wraps into.
const MAX_FULL_ROWS: usize = 3;

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
    #[must_use]
    pub fn rng(self) -> TestRng {
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

/// One generated case: the document laid out, the window it is laid out into, and where the cursor
/// rests.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LayoutInput {
    /// The text being laid out.
    pub document: Document,

    /// The window the text is laid out into.
    pub viewport: Viewport,

    /// The cursor's logical position, which may rest past the last grapheme of its line.
    pub cursor: LogicalPosition,
}

impl LayoutInput {
    /// # Returns
    ///
    /// The view a layout is asked to draw for this case.
    #[must_use]
    pub fn view(&self) -> View<'_> {
        View {
            document: &self.document,
            viewport: &self.viewport,
            cursor: self.cursor,
        }
    }

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
        grapheme_count
            + self.document.lines().len()
            + self.viewport.width()
            + self.viewport.height.get()
    }
}

impl Display for LayoutInput {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        writeln!(f, "{}", self.viewport)?;
        writeln!(f, "cursor at {}", self.cursor)?;
        for (index, line) in self.document.lines().iter().enumerate() {
            writeln!(f, "line {index}: `{}`", line.escape_debug())?;
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

/// Searches for a view on which a layout breaks an invariant, shrinking whatever it finds.
///
/// # Type Parameters
///
/// * `LayoutType` - The layout under search.
///
/// # Returns
///
/// `Ok(())` if the layout survived `cases` generated views.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`FuzzFailure`] if a view broke an invariant, holding the shrunk case and the seed that
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
        let violations = check(layout, input.view());
        let Some(violation) = violations.first() else {
            return Ok(());
        };
        original.borrow_mut().get_or_insert_with(|| input.clone());
        Err(TestCaseError::fail(violation.to_string()))
    });

    match outcome {
        Ok(()) => Ok(()),
        Err(TestError::Fail(_, minimal)) => {
            let violations = check(layout, minimal.view());
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
/// A strategy generating the views a layout is searched over, mixing unconstrained cases with the
/// two shapes layouts are known to break on.
pub fn layout_input() -> impl Strategy<Value = LayoutInput> {
    prop_oneof![
        6 => any_case(),
        1 => squeezed_by_a_marker(),
        1 => squeezed_by_an_indent(),
        1 => end_of_line_case(),
    ]
}

/// The shape of the window a case is drawn into.
#[derive(Clone, Copy, Debug)]
struct Geometry {
    width: usize,
    tab_stop: usize,
    height: usize,
}

/// The display options a case is drawn with.
#[derive(Clone, Debug)]
struct GeneratedOptions {
    break_indent: bool,
    break_indent_min: usize,
    marker: String,
    line_break: bool,
    wide_ambiguous: bool,
}

/// # Returns
///
/// A strategy generating cases from the whole alphabet, in windows of every shape and under every
/// combination of the display options.
fn any_case() -> impl Strategy<Value = LayoutInput> {
    (
        lines(ALPHABET.as_slice()),
        geometry(),
        display_options(),
        cursor(),
    )
        .prop_map(|(lines, geometry, options, cursor)| case(lines, geometry, options, cursor))
}

/// # Returns
///
/// A strategy generating cases whose continuation marker is one column short of the viewport, so
/// that a two-column cluster starting a continuation row has under two columns left beside the
/// marker.
fn squeezed_by_a_marker() -> impl Strategy<Value = LayoutInput> {
    (
        lines(WIDE_ALPHABET.as_slice()),
        geometry(),
        display_options(),
        cursor(),
    )
        .prop_map(|(lines, geometry, options, cursor)| {
            let width = geometry.width.max(MIN_SQUEEZED_WIDTH);
            let options = GeneratedOptions {
                break_indent: false,
                marker: "#".repeat(width - 1),
                ..options
            };
            case(lines, Geometry { width, ..geometry }, options, cursor)
        })
}

/// # Returns
///
/// A strategy generating cases whose lines are indented to one column short of the viewport and
/// wrapped under `'breakindent'`, so that a two-column cluster starting a continuation row has
/// under two columns left beside the repeated indent.
fn squeezed_by_an_indent() -> impl Strategy<Value = LayoutInput> {
    (
        lines(WIDE_ALPHABET.as_slice()),
        geometry(),
        display_options(),
        cursor(),
    )
        .prop_map(|(lines, geometry, options, cursor)| {
            let width = geometry.width.max(MIN_SQUEEZED_WIDTH);
            let indent = " ".repeat(width - 1);
            let lines = lines
                .into_iter()
                .map(|line| format!("{indent}{line}"))
                .collect();
            let options = GeneratedOptions {
                break_indent: true,
                break_indent_min: 0,
                marker: String::new(),
                ..options
            };
            case(lines, Geometry { width, ..geometry }, options, cursor)
        })
}

/// # Returns
///
/// A strategy generating cases whose lines wrap into exactly full rows with the cursor resting
/// past the end of one, which is the cursor vim draws in the first cell of the row below.
fn end_of_line_case() -> impl Strategy<Value = LayoutInput> {
    (
        geometry(),
        1..=MAX_FULL_ROWS,
        1..=MAX_LINES,
        display_options(),
        0..MAX_LINES,
    )
        .prop_map(|(geometry, rows, line_count, options, line)| {
            let lines = vec!["a".repeat(geometry.width * rows); line_count];
            let options = GeneratedOptions {
                break_indent: false,
                marker: String::new(),
                ..options
            };
            case(lines, geometry, options, (line, END_OF_LINE))
        })
}

/// # Returns
///
/// A strategy generating the lines of a document from `alphabet`.
fn lines(alphabet: &'static [&'static str]) -> impl Strategy<Value = Vec<String>> {
    let line = proptest::collection::vec(proptest::sample::select(alphabet), 0..=MAX_LINE_LEN)
        .prop_map(|clusters| clusters.concat());

    proptest::collection::vec(line, 1..=MAX_LINES)
}

/// # Returns
///
/// A strategy generating the shape of a window, whose tab stop never exceeds its width so that no
/// tab is wider than a row.
fn geometry() -> impl Strategy<Value = Geometry> {
    (MIN_WIDTH..=MAX_WIDTH, 1..=MAX_HEIGHT).prop_flat_map(|(width, height)| {
        (1..=width).prop_map(move |tab_stop| Geometry {
            width,
            tab_stop,
            height,
        })
    })
}

/// # Returns
///
/// A strategy generating the display options a case is drawn with.
fn display_options() -> impl Strategy<Value = GeneratedOptions> {
    (
        any::<bool>(),
        0..=MAX_BREAK_INDENT_MIN,
        proptest::sample::select(MARKERS.as_slice()),
        any::<bool>(),
        any::<bool>(),
    )
        .prop_map(
            |(break_indent, break_indent_min, marker, line_break, wide_ambiguous)| {
                GeneratedOptions {
                    break_indent,
                    break_indent_min,
                    marker: marker.to_owned(),
                    line_break,
                    wide_ambiguous,
                }
            },
        )
}

/// # Returns
///
/// A strategy generating the line and the column a case's cursor rests at, which clamps into the
/// document it is generated for.
fn cursor() -> impl Strategy<Value = (usize, usize)> {
    (
        0..MAX_LINES,
        prop_oneof![3 => 0..=MAX_LINE_LEN, 1 => Just(END_OF_LINE)],
    )
}

/// # Returns
///
/// The case the generated parts describe, with its cursor clamped into its document.
fn case(
    lines: Vec<String>,
    geometry: Geometry,
    options: GeneratedOptions,
    cursor: (usize, usize),
) -> LayoutInput {
    let document = Document::new(lines);
    let ambiwidth = if options.wide_ambiguous {
        AmbiWidth::Double
    } else {
        AmbiWidth::Single
    };
    let metrics = Metrics::new(
        ambiwidth,
        NonZeroUsize::new(geometry.tab_stop).expect("a generated tab stop is at least one"),
    );
    let viewport = Viewport {
        wrapping: Wrapping::new(
            NonZeroUsize::new(geometry.width).expect("a generated width is at least two"),
            metrics,
            Options::new()
                .with_break_indent(options.break_indent)
                .with_break_indent_min(options.break_indent_min)
                .with_show_break(options.marker)
                .with_line_break(options.line_break),
        ),
        height: NonZeroUsize::new(geometry.height).expect("a generated height is at least one"),
    };
    let (line, column) = cursor;
    let cursor = document.clamp(LogicalPosition {
        line: line % document.lines().len(),
        grapheme: column,
    });

    LayoutInput {
        document,
        viewport,
        cursor,
    }
}
