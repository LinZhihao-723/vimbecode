//! Runs the layout fuzz harness against the anchor-relative mapping.
//!
//! The harness owns the invariants and is already known to catch each of them being broken, so
//! putting the mapping behind its [`Layout`] trait says whether the mapping keeps them: that every
//! grapheme round-trips in both directions, and that the cursor -- the one past the end of a line
//! included -- is drawn inside the viewport.
//!
//! The mapping is run twice, once anchored above everything it is asked about and once anchored
//! below it, so that the harness searches walks in both directions rather than only the forward
//! walk an anchor at the top of the text would ever take.

use vbc_layout::anchor::{
    char_idx_at_visual_offset, visual_offset_from_anchor, VisualOffset, Wrapping,
};
use vbc_layout::invariants::{
    DisplayPosition, Document, Layout, LogicalPosition, Row, Screen, Viewport,
};
use vbc_layout::line::{self, Options};
use vbc_layout::width::Metrics;

#[path = "fuzz/harness.rs"]
mod harness;

use harness::{search, Seed, DEFAULT_CASES};

/// The number of seeds each search is repeated from.
const SEEDS: u64 = 8;

/// The rows a mapping is allowed to walk, which every generated case fits inside.
const MAX_ROWS: usize = 1024;

/// The mapping anchored at the first position of the text, which walks forwards to everything.
const FROM_THE_TOP: Anchored = Anchored { from_the_top: true };

/// The mapping anchored at the start of the last line, which walks backwards to almost everything.
const FROM_THE_BOTTOM: Anchored = Anchored {
    from_the_top: false,
};

/// The anchor-relative mapping, put behind the trait the harness checks.
///
/// Laying the whole document out is the harness's business rather than the mapping's: the checker
/// compares every row of the screen, and only a test can afford to build one.
struct Anchored {
    from_the_top: bool,
}

impl Anchored {
    /// # Returns
    ///
    /// The position every mapping of `document` is anchored at.
    fn anchor(&self, document: &Document) -> LogicalPosition {
        LogicalPosition {
            line: if self.from_the_top {
                0
            } else {
                document.lines().len() - 1
            },
            grapheme: 0,
        }
    }

    /// # Returns
    ///
    /// The screen row the anchor of `document` is drawn on.
    fn origin(&self, document: &Document, viewport: Viewport) -> usize {
        let anchor = self.anchor(document);

        document.lines()[..anchor.line]
            .iter()
            .map(|text| rows_of(text, viewport).len())
            .sum()
    }
}

impl Layout for Anchored {
    fn lay_out(&self, document: &Document, viewport: Viewport) -> Screen {
        Screen {
            rows: document
                .lines()
                .iter()
                .enumerate()
                .flat_map(|(line, text)| {
                    rows_of(text, viewport).into_iter().map(move |row| Row {
                        line,
                        start: row.start(),
                        text: row.text().to_owned(),
                    })
                })
                .collect(),
        }
    }

    fn display_position(
        &self,
        document: &Document,
        viewport: Viewport,
        position: LogicalPosition,
    ) -> Option<DisplayPosition> {
        let offset = visual_offset_from_anchor(
            document.lines(),
            self.anchor(document),
            position,
            &wrapping(viewport),
            MAX_ROWS,
        )
        .ok()?;
        let row = signed(self.origin(document, viewport)) + offset.rows;

        Some(DisplayPosition {
            row: usize::try_from(row).ok()?,
            column: offset.column,
        })
    }

    fn logical_position(
        &self,
        document: &Document,
        viewport: Viewport,
        position: DisplayPosition,
    ) -> Option<LogicalPosition> {
        let offset = VisualOffset {
            rows: signed(position.row) - signed(self.origin(document, viewport)),
            column: position.column,
        };
        let landing = char_idx_at_visual_offset(
            document.lines(),
            self.anchor(document),
            offset,
            &wrapping(viewport),
        )
        .ok()?;

        (offset == landing.offset).then_some(landing.position)
    }
}

#[test]
fn a_mapping_anchored_above_the_text_satisfies_every_invariant() {
    for seed in 0..SEEDS {
        if let Err(failure) = search(&FROM_THE_TOP, Seed::new(seed), DEFAULT_CASES) {
            panic!("a mapping anchored at the top broke an invariant:\n{failure}");
        }
    }
}

#[test]
fn a_mapping_anchored_below_the_text_satisfies_every_invariant() {
    for seed in 0..SEEDS {
        if let Err(failure) = search(&FROM_THE_BOTTOM, Seed::new(seed), DEFAULT_CASES) {
            panic!("a mapping anchored at the bottom broke an invariant:\n{failure}");
        }
    }
}

/// # Returns
///
/// The way a viewport's text is drawn, which is the way the invariants measure it.
fn wrapping(viewport: Viewport) -> Wrapping {
    Wrapping::new(viewport.width, Metrics::default(), Options::new())
}

/// # Returns
///
/// The rows rendering `line` in `viewport`.
fn rows_of(line: &str, viewport: Viewport) -> Vec<line::DisplayRow> {
    line::lay_out(line, viewport.width, Metrics::default(), &Options::new())
}

/// # Returns
///
/// `count` as a signed row count.
///
/// # Panics
///
/// Panics if `count` does not fit in an `isize`.
fn signed(count: usize) -> isize {
    isize::try_from(count).expect("a row count fits in an `isize`")
}
