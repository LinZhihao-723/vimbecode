//! Checks the claim the anchor-relative mapping is built on: that a mapping costs what the rows
//! around the anchor cost and nothing at all for the text those rows sit in.
//!
//! The claim is what pays for the design. A row table over the whole buffer costs milliseconds at
//! the sizes a transcript reaches, a large part of a frame, and it has to be invalidated on every
//! edit. A mapping that never looks past the rows it was asked about needs neither. That is a
//! measurement rather than an argument, so it is measured here: the same mapping is timed over a
//! hundred lines and over fifty thousand, and the two are required to cost the same.
//!
//! The measurement is a ratio of the fastest run of several rather than an absolute time, so a
//! busy machine slows both sides alike, and the tolerance is wide enough that only a mapping whose
//! cost actually follows the text can break it -- laying the whole buffer out would be hundreds of
//! times slower on the larger text rather than a few times.
//!
//! The other half of the claim -- that nothing keeps a cache of what it laid out, which is what a
//! mapping this cheap has no need of -- is a property of the source rather than of a run, and it
//! is read off every crate of the workspace by `vbc-ci`'s architecture guards.

use std::hint::black_box;
use std::num::NonZeroUsize;
use std::time::{Duration, Instant};

use vbc_layout::anchor::{
    char_idx_at_visual_offset, visual_offset_from_anchor, VisualOffset, Wrapping,
};
use vbc_layout::invariants::LogicalPosition;
use vbc_layout::line::Options;
use vbc_layout::width::Metrics;

/// The two texts the same mapping is timed over.
const SMALL_TEXT: usize = 100;
const LARGE_TEXT: usize = 50_000;

/// The number of columns the timed text is drawn in.
const WIDTH: usize = 60;

/// The number of mappings one timed run performs.
const MAPPINGS: usize = 256;

/// The number of runs a measurement takes the fastest of, which is what keeps a machine's own
/// noise out of the ratio.
const RUNS: usize = 9;

/// The rows a timed mapping reaches away from its anchor, in both directions, which is the
/// screenful a renderer asks about.
const REACH: usize = 12;

/// The factor by which the larger text is allowed to cost more than the smaller one, and the
/// smaller more than the larger. A mapping that lays the whole buffer out is five hundred times
/// slower over the larger text, so nothing but a mapping whose cost follows the text passes.
const TOLERANCE: u32 = 4;

#[test]
fn a_mapping_costs_the_same_over_a_short_text_and_a_long_one() {
    let wrapping = Wrapping::new(
        NonZeroUsize::new(WIDTH).expect("the timed width is not zero"),
        Metrics::default(),
        Options::new(),
    );
    let small = text(SMALL_TEXT);
    let large = text(LARGE_TEXT);

    // The first run of each pays for the pages the text was just written to.
    cost(&small, &wrapping);
    cost(&large, &wrapping);
    let small_cost = cost(&small, &wrapping);
    let large_cost = cost(&large, &wrapping);

    assert!(
        large_cost <= small_cost * TOLERANCE,
        "{MAPPINGS} mappings cost {large_cost:?} over {LARGE_TEXT} lines and {small_cost:?} over \
         {SMALL_TEXT}, so the cost follows the text"
    );
    assert!(
        small_cost <= large_cost * TOLERANCE,
        "{MAPPINGS} mappings cost {small_cost:?} over {SMALL_TEXT} lines and {large_cost:?} over \
         {LARGE_TEXT}, so the measurement is not comparing the same work"
    );
}

/// # Returns
///
/// A text of `lines` lines, each long enough to wrap onto a second row.
fn text(lines: usize) -> Vec<String> {
    (0..lines)
        .map(|index| format!("line {index} of a text that is long enough to wrap once or twice"))
        .collect()
}

/// Times a run of mappings anchored in the middle of a text, reaching the same number of rows
/// either side of the anchor whatever the text is.
///
/// # Returns
///
/// The fastest of [`RUNS`] runs of [`MAPPINGS`] mappings.
///
/// # Panics
///
/// Panics if a mapping within reach of the anchor is refused.
fn cost(lines: &[String], wrapping: &Wrapping) -> Duration {
    let anchor = LogicalPosition {
        line: lines.len() / 2,
        grapheme: 0,
    };
    let mut fastest = Duration::MAX;
    for _ in 0..RUNS {
        let started = Instant::now();
        let mut reached = 0;
        for step in 0..MAPPINGS {
            let away = step % (2 * REACH + 1);
            let position = LogicalPosition {
                line: anchor.line + away - REACH,
                grapheme: away % 8,
            };
            let offset =
                visual_offset_from_anchor(lines, anchor, position, wrapping, 2 * REACH + 1)
                    .expect("a position within reach of the anchor is mapped");
            let landing = char_idx_at_visual_offset(
                lines,
                anchor,
                VisualOffset {
                    rows: offset.rows,
                    column: offset.column,
                },
                wrapping,
            )
            .expect("an offset within reach of the anchor is mapped back");
            reached += landing.position.grapheme + offset.column;
        }
        black_box(reached);
        fastest = fastest.min(started.elapsed());
    }

    fastest
}
