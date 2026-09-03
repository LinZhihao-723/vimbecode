//! Checks that a block costs the window it is drawn in and that a diff costs bounded memory.
//!
//! These are the two places a transcript stops being small. What a tool answered is whatever
//! `cargo` wrote, and an edit is to whatever file was edited, so a renderer that laid a block out
//! whole would spend a frame's budget on rows nobody is looking at, and a diff that allocated a
//! cell per pair of lines would ask for gigabytes to show that one line changed. Both are
//! properties of a run rather than of the source, so both are measured here rather than argued in
//! a docstring.
//!
//! The render is measured twice over: in allocations, which are exact, and in time, as the fastest
//! of several runs so that a busy machine slows both sides alike. Drawing the same twenty rows off
//! the top of a hundred-line block and off the top of a hundred-thousand-line one is required to
//! cost the same either way, which only a walk that stops at the bottom of the window can manage.
//! The coloured pair is the same measurement over a block carrying a span per line, because the
//! spans are the other thing there are as many of as the block is long.
//!
//! The diff is measured in the memory it takes to align four thousand lines against four thousand,
//! which is where the table a full dynamic program allocates comes to 128 MB, and in the memory it
//! takes over a twenty-thousand-line file, where that table is 3.2 GB and the crash it causes is
//! the whole reason there is a bound at all.
//!
//! What these assert is what they were seen to measure. In release: twenty rows off the top of a
//! block cost 37 µs and 24,017 bytes at a hundred lines and at a hundred thousand alike, and 10 µs
//! and 13,726 bytes at a hundred coloured lines and at a hundred thousand alike; four thousand
//! lines diffed against four thousand with nothing in common cost 56 ms and 2.4 MB; twenty
//! thousand against twenty thousand cost 10 ms and 11 MB past the bound, and 3.6 ms with one line
//! inserted, which the common head and tail match off to a middle of one line either side.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::hint::black_box;
use std::num::NonZeroUsize;
use std::ops::Range;
use std::time::{Duration, Instant};

use vbc_editor::chat::block::{Block, Kind, Role, RowWindow};
use vbc_editor::chat::diff;
use vbc_layout::anchor::Wrapping;
use vbc_layout::line::Options;
use vbc_layout::width::Metrics;

/// The columns a block is drawn in, which is the width a chat panel is about.
const COLUMNS: usize = 80;

/// The rows a window draws, which is a screenful of a panel.
const WINDOW: usize = 20;

/// The lines of the block that is short enough to lay out whole without noticing.
const SHORT: usize = 100;

/// The lines of the block that is not, which is the length a `cargo` build of this workspace runs
/// to several times over.
const LONG: usize = 100_000;

/// The number of runs a timing takes the fastest of, which is what keeps a machine's own noise out
/// of the ratio.
const RUNS: usize = 9;

/// The factor by which drawing a window of the long block may cost more than drawing the same
/// window of the short one. A render that lays the whole block out costs about a thousand times
/// as much, so only a render bounded by its window passes.
const MARGIN: u32 = 4;

/// The lines each side of the diff that is measured, which is where a full table comes to 128 MB.
const DIFFED: usize = 4_000;

/// The memory that diff may take. A table of `(4000 + 1) * (4000 + 1)` `usize` is 128 MB, so
/// anything of that shape misses this by more than an order of magnitude.
const DIFF_MEMORY: usize = 8 << 20;

/// The lines of the file a routine `Edit` is to, which is longer than the bound reaches.
const EDITED: usize = 20_000;

/// The memory a diff of a file that long may take. A table of `(20000 + 1) * (20000 + 1)` `usize`
/// is 3.2 GB, which is the out-of-memory crash this bound exists against.
const BOUNDED_MEMORY: usize = 32 << 20;

/// The time that diff may take, which is generous enough for an unoptimized build on a shared
/// machine and still far under what a text large enough to be bounded instead would cost.
const DIFF_TIME: Duration = Duration::from_secs(20);

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
fn drawing_a_window_of_a_long_block_allocates_what_drawing_one_of_a_short_block_does() {
    let short = plain(SHORT);
    let long = plain(LONG);
    let wrapping = wrapping();
    let window = RowWindow::new(0, WINDOW);

    let (rows, of_short) = measured(|| short.render(window, &wrapping));
    let (same, of_long) = measured(|| long.render(window, &wrapping));

    assert_eq!(
        rows, same,
        "the two blocks did not draw the same rows, so the measurement compares two things"
    );
    assert_eq!(
        of_short, of_long,
        "drawing {WINDOW} rows of a {SHORT}-line block took {of_short} bytes and drawing the same \
         rows of a {LONG}-line block took {of_long}, so the render follows the block"
    );
}

#[test]
fn drawing_a_window_of_a_long_block_costs_what_drawing_one_of_a_short_block_costs() {
    let short = plain(SHORT);
    let long = plain(LONG);

    let of_short = timed(&short);
    let of_long = timed(&long);

    assert!(
        of_long < of_short * MARGIN,
        "drawing {WINDOW} rows of a {SHORT}-line block took {of_short:?} and drawing the same rows \
         of a {LONG}-line block took {of_long:?}, so the render follows the block"
    );
}

#[test]
fn drawing_a_window_of_a_long_coloured_block_costs_what_drawing_one_of_a_short_one_costs() {
    let short = coloured(SHORT);
    let long = coloured(LONG);
    let wrapping = wrapping();
    let window = RowWindow::new(0, WINDOW);

    assert_eq!(
        SHORT,
        short.spans().len(),
        "the short coloured block carries no span per line"
    );
    assert_eq!(
        LONG,
        long.spans().len(),
        "the long coloured block carries no span per line"
    );

    let (rows, of_short) = measured(|| short.render(window, &wrapping));
    let (same, of_long) = measured(|| long.render(window, &wrapping));
    assert_eq!(rows, same, "the two blocks did not draw the same rows");
    assert_eq!(
        of_short, of_long,
        "drawing {WINDOW} rows of a {SHORT}-line coloured block took {of_short} bytes and drawing \
         the same rows of a {LONG}-line one took {of_long}, so the render follows the spans"
    );

    let of_short = timed(&short);
    let of_long = timed(&long);
    assert!(
        of_long < of_short * MARGIN,
        "drawing {WINDOW} rows of a {SHORT}-line coloured block took {of_short:?} and drawing the \
         same rows of a {LONG}-line one took {of_long:?}"
    );
}

#[test]
fn a_window_deep_in_a_long_block_costs_what_the_same_window_of_a_short_one_costs() {
    let short = plain(SHORT);
    let long = plain(LONG);
    let wrapping = wrapping();
    let window = RowWindow::new(2 * SHORT - WINDOW, WINDOW);

    let (rows, of_short) = measured(|| short.render(window, &wrapping));
    let (same, of_long) = measured(|| long.render(window, &wrapping));

    assert_eq!(
        WINDOW,
        rows.rows().len(),
        "the window at the end of the short block did not fill"
    );
    assert_eq!(rows, same, "the two blocks did not draw the same rows");
    assert_eq!(
        of_short, of_long,
        "drawing the last {WINDOW} rows of a {SHORT}-line block took {of_short} bytes and drawing \
         the same rows of a {LONG}-line block took {of_long}"
    );
}

#[test]
fn diffing_four_thousand_lines_against_four_thousand_takes_bounded_memory() {
    let old = lines(0..DIFFED);
    let new = lines(DIFFED..2 * DIFFED);

    let started = Instant::now();
    let (block, memory) = measured(|| diff::compute(&old, &new));
    let elapsed = started.elapsed();

    assert_eq!(
        2 * DIFFED,
        block.source().lines().count(),
        "the diff did not mark every line of both texts"
    );
    assert!(
        memory < DIFF_MEMORY,
        "diffing {DIFFED} lines against {DIFFED} took {memory} bytes and {elapsed:?}"
    );
    assert!(
        elapsed < DIFF_TIME,
        "diffing {DIFFED} lines against {DIFFED} took {elapsed:?} and {memory} bytes"
    );
}

#[test]
fn an_edit_to_a_long_file_is_shown_whole_rather_than_allocated_for() {
    let old = lines(0..EDITED);
    let new = lines(EDITED..2 * EDITED);

    let started = Instant::now();
    let (block, memory) = measured(|| diff::compute(&old, &new));
    let elapsed = started.elapsed();

    assert_eq!(
        format!("{}{}", diff::BOUNDED, diff::BOUNDED_NOTE),
        block
            .source()
            .lines()
            .next()
            .expect("a bounded diff holds a line of its own"),
        "the diff past the bound did not say that it was"
    );
    assert_eq!(
        1 + 2 * EDITED,
        block.source().lines().count(),
        "the diff did not mark every line of both texts under that one"
    );
    assert!(
        memory < BOUNDED_MEMORY,
        "diffing {EDITED} lines against {EDITED} took {memory} bytes and {elapsed:?}"
    );
    assert!(
        elapsed < DIFF_TIME,
        "diffing {EDITED} lines against {EDITED} took {elapsed:?} and {memory} bytes"
    );
}

#[test]
fn an_edit_of_one_line_into_a_long_file_is_aligned_all_the_same() {
    let old = lines(0..EDITED);
    let new = lines(0..EDITED / 2) + &lines(EDITED..1 + EDITED) + &lines(EDITED / 2..EDITED);

    let started = Instant::now();
    let (block, memory) = measured(|| diff::compute(&old, &new));
    let elapsed = started.elapsed();

    assert_eq!(
        1 + EDITED,
        block.source().lines().count(),
        "the edit was not matched off to the one line it changed"
    );
    assert!(
        memory < BOUNDED_MEMORY,
        "diffing {EDITED} lines against {EDITED} took {memory} bytes and {elapsed:?}"
    );
    assert!(
        elapsed < DIFF_TIME,
        "diffing {EDITED} lines against {EDITED} took {elapsed:?} and {memory} bytes"
    );
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

/// Times a window of `block` drawn off its top.
///
/// # Returns
///
/// The fastest of [`RUNS`] renders.
fn timed(block: &Block) -> Duration {
    let wrapping = wrapping();
    let window = RowWindow::new(0, WINDOW);

    let mut fastest = Duration::MAX;
    for _ in 0..RUNS {
        let started = Instant::now();
        let rendered = block.render(window, &wrapping);
        fastest = fastest.min(started.elapsed());
        black_box(&rendered);
    }

    fastest
}

/// # Returns
///
/// A block of `count` lines, each of them long enough to wrap once.
fn plain(count: usize) -> Block {
    Block::new(Kind::Message(Role::Assistant), text(0..count))
}

/// # Returns
///
/// A block of `count` lines of tool output, each coloured by an escape of its own.
fn coloured(count: usize) -> Block {
    let raw: String = (0..count)
        .map(|number| format!("\u{1b}[31mline {number}\u{1b}[0m\n"))
        .collect();

    Block::from_ansi(Kind::ToolResult, &raw)
}

/// # Returns
///
/// The lines numbered by `range`, each of them long enough to wrap once at [`COLUMNS`] columns.
fn text(range: Range<usize>) -> String {
    range
        .map(|number| {
            format!(
                "line {number} of a block whose lines are long enough to wrap once over the \
                 columns it is drawn in\n"
            )
        })
        .collect()
}

/// # Returns
///
/// The lines numbered by `range`, short enough that what a diff of them takes is the alignment's
/// rather than the text's.
fn lines(range: Range<usize>) -> String {
    range.map(|number| format!("line {number}\n")).collect()
}

/// # Returns
///
/// A wrapping drawing rows [`COLUMNS`] columns wide under vim's own defaults.
fn wrapping() -> Wrapping {
    Wrapping::new(
        NonZeroUsize::new(COLUMNS).expect("the measured width is not zero"),
        Metrics::default(),
        Options::new(),
    )
}
