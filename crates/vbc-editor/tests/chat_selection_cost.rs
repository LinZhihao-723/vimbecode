//! Checks that painting a selection costs the rows it is painted into rather than the selection.
//!
//! A selection over a transcript is the one thing a panel derives every frame. Yanking one costs
//! what it takes, which is the whole of it and unavoidable; painting one costs only the screenful
//! being looked at, and it must, because `ggVG` over what a tool answered selects whatever `cargo`
//! wrote and the panel then has to scroll through it. A highlight that read every line it covered
//! to paint twenty rows would spend a frame's budget on lines nobody is looking at, growing with
//! the block rather than with the screen.
//!
//! So a linewise, charwise and blockwise selection of the whole of a hundred-line block and of the
//! whole of a hundred-thousand-line one are painted into the same twenty rows here, off the top of
//! the block and again deep inside it, and the two are required to cost the same. The measurement
//! is the one `chat_cost.rs` makes of the render beneath it: allocations, which are exact, and
//! time, as the fastest of several runs so that a busy machine slows both sides alike.
//!
//! What these assert is what they were seen to measure. In release, painting twenty rows of a
//! selection of the whole block takes 1,920 bytes and 13 to 25 µs at a hundred lines and at a
//! hundred thousand alike, and 960 bytes for the blockwise half of them. A highlight that read the
//! whole selection to paint the same twenty rows took 3,968 bytes and 26 µs over the short block
//! and 3,145,728 bytes and 4.0 ms over the long one, which is the shape this rules out.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::hint::black_box;
use std::num::NonZeroUsize;
use std::ops::Range;
use std::time::Duration;
use std::time::Instant;

use vbc_editor::chat::block::{Block, Kind, Rendered, Role, RowWindow};
use vbc_editor::chat::selection::{Mode, Motion, Selection, Source};
use vbc_layout::anchor::Wrapping;
use vbc_layout::line::Options;
use vbc_layout::width::Metrics;

/// The columns a block is drawn in, which is the width a chat panel is about.
const COLUMNS: usize = 80;

/// The rows a window draws, which is a screenful of a panel.
const WINDOW: usize = 20;

/// The lines of the block that is short enough to walk whole without noticing.
const SHORT: usize = 100;

/// The lines of the block that is not, which is the length a `cargo` build of this workspace runs
/// to several times over.
const LONG: usize = 100_000;

/// The number of runs a timing takes the fastest of, which is what keeps a machine's own noise out
/// of the ratio.
const RUNS: usize = 9;

/// The factor by which painting a window of the selection over the long block may cost more than
/// painting the same window of the selection over the short one. A highlight that walks what it
/// covers costs about a thousand times as much, so only one bounded by its window passes.
const MARGIN: u32 = 4;

/// The columns a blockwise selection is taken at, which every line of the fixture is drawn past
/// on the first of the rows it is drawn in.
const BLOCK: Range<usize> = 4..12;

/// The columns a line of the fixture is drawn in, which is more than one row of the panel holds
/// and less than two, so that a window of it draws whole lines and half again of none.
const LINE: usize = 3 * COLUMNS / 2;

/// The rows a line of the fixture is drawn in, which the fixture is checked to be drawn in.
const ROWS_PER_LINE: usize = 2;

/// The row a window is drawn from to reach lines the short block ends not far past, so that a
/// window deep inside the block is measured as well as one off the top of it.
const DEEP: usize = ROWS_PER_LINE * SHORT - WINDOW;

/// The allocator every measurement here is read through.
#[global_allocator]
static ALLOCATOR: Counting = Counting;

thread_local! {
    /// The bytes this thread has been handed and has not given back.
    static HELD: Cell<usize> = const { Cell::new(0) };

    /// The most bytes this thread held at once since the last [`measured`] began.
    static PEAK: Cell<usize> = const { Cell::new(0) };
}

#[test]
fn a_selection_of_a_long_block_covers_it_whole_so_the_measurement_has_something_to_walk() {
    for lines in [SHORT, LONG] {
        let text = text(lines);
        let source = Source::new(&text, Metrics::default());
        let selection = over(source, Mode::Linewise, lines);

        assert_eq!(
            lines,
            selection.lines(source),
            "the selection does not cover the whole of a {lines}-line block"
        );
        assert_eq!(
            lines,
            selection.segments(source).len(),
            "the selection does not name a segment per line of a {lines}-line block"
        );

        for start in [0, DEEP] {
            let rendered = drawn(&text, RowWindow::new(start, WINDOW));

            assert_eq!(
                WINDOW,
                rendered.rows().len(),
                "the window at row {start} of a {lines}-line block did not fill"
            );
            assert_eq!(
                WINDOW / ROWS_PER_LINE,
                rendered
                    .rows()
                    .iter()
                    .filter(|row| 0 == row.styled().row().start())
                    .count(),
                "a line of a {lines}-line block is not drawn in {ROWS_PER_LINE} rows at row \
                 {start}, so a blockwise window paints something else"
            );
        }
    }
}

#[test]
fn painting_a_window_of_a_selection_of_a_long_block_allocates_what_a_short_one_does() {
    for mode in [Mode::Charwise, Mode::Linewise, Mode::Blockwise] {
        for start in [0, DEEP] {
            let window = RowWindow::new(start, WINDOW);
            let (painted, of_short) = measured_over(SHORT, mode, window);
            let (same, of_long) = measured_over(LONG, mode, window);
            let wanted = match mode {
                Mode::Blockwise => WINDOW / ROWS_PER_LINE,
                _ => WINDOW,
            };

            assert_eq!(
                wanted,
                painted.len(),
                "{mode:?} painted {} rows of the window at row {start} rather than {wanted}, so \
                 the measurement is not of the screenful it says",
                painted.len()
            );
            assert_eq!(
                painted, same,
                "{mode:?} painted the two blocks differently at row {start}, so the measurement \
                 compares two things"
            );
            assert_eq!(
                of_short, of_long,
                "{mode:?} painting {WINDOW} rows at row {start} of a selection of a {SHORT}-line \
                 block took {of_short} bytes and of a {LONG}-line block took {of_long}, so the \
                 highlight follows the selection"
            );
        }
    }
}

#[test]
fn painting_a_window_of_a_selection_of_a_long_block_costs_what_a_short_one_costs() {
    for mode in [Mode::Charwise, Mode::Linewise, Mode::Blockwise] {
        for start in [0, DEEP] {
            let window = RowWindow::new(start, WINDOW);
            let of_short = timed_over(SHORT, mode, window);
            let of_long = timed_over(LONG, mode, window);

            assert!(
                of_long < of_short * MARGIN,
                "{mode:?} painting {WINDOW} rows at row {start} of a selection of a {SHORT}-line \
                 block took {of_short:?} and of a {LONG}-line block took {of_long:?}, so the \
                 highlight follows the selection"
            );
        }
    }
}

/// Paints `window` of a selection of the whole of a `lines`-line block and reads off what it asked
/// the allocator for.
///
/// # Returns
///
/// The columns painted in each row the selection reached, and the most bytes the painting held at
/// once.
fn measured_over(lines: usize, mode: Mode, window: RowWindow) -> (Vec<Range<usize>>, usize) {
    let text = text(lines);
    let source = Source::new(&text, Metrics::default());
    let selection = over(source, mode, lines);
    let rendered = drawn(&text, window);

    measured(|| {
        selection
            .highlight(source, &rendered)
            .iter()
            .map(|highlight| highlight.columns().clone())
            .collect()
    })
}

/// Times `window` of a selection of the whole of a `lines`-line block being painted.
///
/// # Returns
///
/// The fastest of [`RUNS`] highlights.
fn timed_over(lines: usize, mode: Mode, window: RowWindow) -> Duration {
    let text = text(lines);
    let source = Source::new(&text, Metrics::default());
    let selection = over(source, mode, lines);
    let rendered = drawn(&text, window);

    let mut fastest = Duration::MAX;
    for _ in 0..RUNS {
        let started = Instant::now();
        let painted = selection.highlight(source, &rendered);
        fastest = fastest.min(started.elapsed());
        black_box(&painted);
    }

    fastest
}

/// Runs `measure` and reads off what it asked the allocator for.
///
/// The counters are the running thread's own, so a measurement is not disturbed by whatever the
/// other tests of this binary are allocating beside it.
///
/// # Returns
///
/// What it returned, and the most bytes it held at once beyond what was held before it ran.
fn measured<ValueType>(measure: impl FnOnce() -> ValueType) -> (ValueType, usize) {
    let before = HELD.with(Cell::get);
    PEAK.with(|peak| peak.set(before));
    let value = measure();
    let peak = PEAK.with(Cell::get);

    (value, peak.saturating_sub(before))
}

/// # Returns
///
/// A selection of `mode` over the whole of a source of `lines` logical lines, taken at [`BLOCK`]
/// where it is blockwise and carried to the end of the last line where it is not.
fn over(source: Source<'_>, mode: Mode, lines: usize) -> Selection {
    let mut selection = Selection::new(mode, source, BLOCK.start);
    selection.extend(source, Motion::Down(lines));
    match mode {
        Mode::Blockwise => selection.extend(source, Motion::Right(BLOCK.end - BLOCK.start - 1)),
        _ => selection.extend(source, Motion::LineEnd),
    }

    selection
}

/// # Returns
///
/// The rows of `window` of `text` drawn as one block in a panel [`COLUMNS`] columns wide.
fn drawn(text: &str, window: RowWindow) -> Rendered {
    let block = Block::new(Kind::Message(Role::Assistant), text.to_owned());
    let wrapping = Wrapping::new(
        NonZeroUsize::new(COLUMNS).expect("a panel is drawn in at least one column"),
        Metrics::default(),
        Options::new(),
    );

    block.render(window, &wrapping)
}

/// # Returns
///
/// `lines` logical lines, each of them long enough to wrap once at [`COLUMNS`] columns, and each
/// the same line whatever the block around it is long.
fn text(lines: usize) -> String {
    (0..lines)
        .map(|index| {
            let said = format!("line {index} of what a tool answered, said at some length");
            format!("{said:.<LINE$}")
        })
        .collect::<Vec<String>>()
        .join("\n")
}

/// The allocator that records what a measurement asks for.
struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = System.alloc(layout);
        if !pointer.is_null() {
            let _ = HELD.try_with(|held| {
                let bytes = held.get() + layout.size();
                held.set(bytes);
                let _ = PEAK.try_with(|peak| peak.set(peak.get().max(bytes)));
            });
        }

        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        let _ = HELD.try_with(|held| held.set(held.get().saturating_sub(layout.size())));
        System.dealloc(pointer, layout);
    }
}
