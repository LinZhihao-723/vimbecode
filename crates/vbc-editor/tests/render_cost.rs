//! Checks that drawing a frame costs no anchor mapping.
//!
//! The tempting renderer asks the anchor where each of its rows begins and lays that row's line
//! out again to answer. A mapping costs tens of microseconds, so a viewport of rows costs the best
//! part of a millisecond, and a frame that redraws forty rows spends most of its budget finding
//! rows it was already holding. The renderer is handed the rows instead, so drawing one costs the
//! cells it fills and nothing else.
//!
//! The two are timed against each other here: drawing a whole viewport is required to cost a small
//! fraction of what mapping the rows of that viewport costs, which only a renderer that maps
//! nothing can manage. The measurement is a ratio of the fastest of several runs, so a busy
//! machine slows both sides alike, and the margin is wide enough that only a renderer whose cost
//! actually follows the mapping can break it.
//!
//! That a renderer names no part of the mapping in the first place is a property of the source
//! rather than of a run, and it is read off every source of the workspace that draws -- this one,
//! the gutter beside it, and whatever draws next -- by `vbc-ci`'s architecture guards.

use std::hint::black_box;
use std::num::NonZeroUsize;
use std::time::{Duration, Instant};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use vbc_editor::render::Renderer;
use vbc_layout::anchor::{
    char_idx_at_visual_offset, visual_offset_from_anchor, VisualOffset, Wrapping,
};
use vbc_layout::invariants::LogicalPosition;
use vbc_layout::line::{self, DisplayRow, Options};
use vbc_layout::width::Metrics;

/// The number of columns the timed viewport is drawn in.
const WIDTH: usize = 80;

/// The number of rows the timed viewport draws, which is the screenful a frame redraws.
const ROWS: usize = 40;

/// The number of runs a measurement takes the fastest of, which is what keeps a machine's own
/// noise out of the ratio.
const RUNS: usize = 9;

/// The factor by which mapping a viewport's rows has to cost more than drawing them. A renderer
/// that maps every row it draws costs about thirty times as much, so nothing but a renderer that
/// maps nothing passes.
const MARGIN: u32 = 8;

#[test]
fn drawing_a_whole_viewport_costs_a_fraction_of_mapping_its_rows() {
    let lines = text(4 * ROWS);
    let wrapping = Wrapping::new(
        NonZeroUsize::new(WIDTH).expect("the timed width is not zero"),
        Metrics::default(),
        Options::new(),
    );
    let rows: Vec<DisplayRow> = lines
        .iter()
        .enumerate()
        .map(|(line, text)| {
            line::lay_out(
                line,
                text,
                wrapping.width(),
                wrapping.metrics(),
                wrapping.options(),
            )
        })
        .map(|mut laid_out| laid_out.remove(0))
        .collect();

    drawing(&rows);
    mapping(&lines, &wrapping);
    let drawing = drawing(&rows);
    let mapping = mapping(&lines, &wrapping);

    assert!(
        drawing * MARGIN < mapping,
        "drawing {ROWS} rows costs {drawing:?} and mapping the same rows costs {mapping:?}, so \
         the renderer maps rows it was handed"
    );
}

/// Times a frame that draws a viewport of rows.
///
/// # Returns
///
/// The fastest of [`RUNS`] frames.
///
/// # Panics
///
/// Panics if the timed viewport does not fit in a `u16`.
fn drawing(rows: &[DisplayRow]) -> Duration {
    let width = u16::try_from(WIDTH).expect("the timed width fits in a `u16`");
    let height = u16::try_from(ROWS).expect("the timed height fits in a `u16`");
    let area = Rect::new(0, 0, width, height);
    let renderer = Renderer::new(Metrics::default());
    let mut buffer = Buffer::empty(area);

    let mut fastest = Duration::MAX;
    for _ in 0..RUNS {
        let started = Instant::now();
        for (index, row) in rows.iter().take(ROWS).enumerate() {
            let screen_row = u16::try_from(index).expect("a timed row fits in a `u16`");
            renderer.draw_row(&mut buffer, area, screen_row, row, None);
        }
        fastest = fastest.min(started.elapsed());
    }
    black_box(&buffer);

    fastest
}

/// Times the anchor mappings a renderer that asked where its rows begin would perform: one round
/// trip for every row of the viewport, anchored at its top left.
///
/// # Returns
///
/// The fastest of [`RUNS`] viewports.
///
/// # Panics
///
/// Panics if a row of the viewport cannot be mapped.
fn mapping(lines: &[String], wrapping: &Wrapping) -> Duration {
    let anchor = LogicalPosition {
        line: 0,
        grapheme: 0,
    };
    let mut fastest = Duration::MAX;
    for _ in 0..RUNS {
        let started = Instant::now();
        let mut reached = 0;
        for row in 0..ROWS {
            let landing = char_idx_at_visual_offset(
                lines,
                anchor,
                VisualOffset {
                    rows: isize::try_from(row).expect("a timed row fits in an `isize`"),
                    column: 0,
                },
                wrapping,
            )
            .expect("a row of the viewport is mapped");
            let offset = visual_offset_from_anchor(lines, anchor, landing.position, wrapping, ROWS)
                .expect("a row of the viewport is mapped back");
            reached += landing.position.grapheme + offset.column;
        }
        black_box(reached);
        fastest = fastest.min(started.elapsed());
    }

    fastest
}

/// # Returns
///
/// A text of `lines` lines, each of which fills one row of the timed viewport.
fn text(lines: usize) -> Vec<String> {
    (0..lines)
        .map(|index| format!("line {index} of a text drawn into a viewport of its own width"))
        .collect()
}
