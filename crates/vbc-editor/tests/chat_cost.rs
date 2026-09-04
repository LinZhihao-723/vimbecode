//! Checks that a block costs the window it is drawn in and that a diff costs bounded memory.
//!
//! These are the two places a transcript stops being small. What a tool answered is whatever
//! `cargo` wrote, and an edit is to whatever file was edited, so a renderer that laid a block out
//! whole would spend a frame's budget on rows nobody is looking at, and a diff that allocated a
//! cell per pair of lines would ask for gigabytes to show that one line changed. Both are
//! properties of a run rather than of the source, so both are measured here rather than argued in
//! a docstring.
//!
//! A window is measured along both of the axes it could follow. It must not follow the length of
//! the block, which is what the short and the long fixture are for, and it must not follow the row
//! it starts at either, which is what [`STARTS`] is for: a panel scrolled to the bottom of what
//! `cargo` wrote asks for a window a hundred thousand rows down, and a render that laid out
//! everything above it would cost the scroll rather than the screen. Leaving that row unvaried is
//! how the second of those went unmeasured until it was found: the deep window used to be drawn at
//! row 180 of both fixtures, where the walk down to it is the same length in each and its growth
//! cancels.
//!
//! What is required of a window is exact rather than approximate. It must ask the allocator for
//! the same bytes in the same number of calls wherever it is drawn from, because those are the
//! rows it builds and it was asked for the same number of them, and the fixture's lines are all of
//! one length so that there is nothing else for the count to follow. Time is required only to stay
//! within [`DEPTH_MARGIN`], because a walk down to a deep window still reads the ends of the lines
//! it steps over, and reading them is memory bandwidth rather than layout: what it must not do is
//! lay them out.
//!
//! That is required of the window over [`plain`], whose lines are the printable ASCII whose rows a
//! width divides out of a length. It is not what a block of [`tabbed`] lines can promise: a tab's
//! columns are not its bytes, so every line above such a window is laid out to be counted, and the
//! bytes that costs follow the row the window starts at. What that walk must still do is throw each
//! line away again, which is the difference between a frame that holds a line of a block and one
//! that holds all of it, so the tabbed fixture is measured in what it holds at once rather than in
//! what it asks for. Leaving it unmeasured is how the shape of the fast path went unstated: every
//! fixture here was printable ASCII drawn under vim's own defaults, which is the one case in which
//! a line's rows are its length over the width.
//!
//! Counting the rows of an entry is measured the same way. A reader arriving at a block from below
//! has to know how many rows it is drawn in, and the count used to be taken by drawing the whole
//! block and asking how many rows came back, which holds every row of it at once.
//!
//! The diff is measured in the memory it takes to align four thousand lines against four thousand,
//! which is where the table a full dynamic program allocates comes to 128 MB, and in the memory it
//! takes over a twenty-thousand-line file, where that table is 3.2 GB and the crash it causes is
//! the whole reason there is a bound at all.
//!
//! What these assert is what they were seen to measure. In release at eighty columns, twenty rows
//! of a hundred-thousand-line block ask the allocator for 121,800 bytes in 1,364 calls wherever
//! they are drawn from, and take 42 µs off the top, 282 µs at row 50,000 and 526 µs at row 99,000;
//! the same three windows take 566 µs, 2.2 ms and 4.4 ms in debug. Before the lines above a window
//! were counted rather than laid out, the second of them asked for 228 MB in 626,364 calls and the
//! third for 452 MB in 1,238,864. Counting the rows of that block asks for nothing at all and takes
//! 1.0 ms, where drawing it to count them asked for 1.2 GB in 13.6 million calls and took 507 ms.
//! The same three windows of a hundred thousand tab-indented lines ask for 121 KB, 236 MB and
//! 467 MB and take 62 µs, 48 ms and 95 ms, holding 24,952 bytes at once at every one of the three
//! against the 24,454 the plain fixture holds; counting that block asks for 942 MB in 2.7 million
//! calls and takes 186 ms, and holds 5,193 bytes where counting the plain one holds none.
//! Four thousand lines diffed against four thousand with nothing in common cost 56 ms and 2.4 MB;
//! twenty thousand against twenty thousand cost 10 ms and 11 MB past the bound, and 3.6 ms with one
//! line inserted, which the common head and tail match off to a middle of one line either side.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::hint::black_box;
use std::num::NonZeroUsize;
use std::ops::Range;
use std::time::{Duration, Instant};

use vbc_editor::chat::block::{Block, Kind, Role, RowWindow};
use vbc_editor::chat::diff;
use vbc_editor::chat::fold::{Folds, View};
use vbc_editor::chat::transcript::Transcript;
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

/// The rows each line of the fixture is drawn in at [`COLUMNS`] columns, which is checked rather
/// than assumed because every row a window is drawn from is counted in it.
const ROWS_PER_LINE: usize = 2;

/// The rows the long block is drawn in: [`ROWS_PER_LINE`] for each of its lines, and one more for
/// the empty line its trailing separator leaves at its end.
const LONG_ROWS: usize = ROWS_PER_LINE * LONG + 1;

/// The rows a window is drawn from: the top of the long block, a quarter of the way down it, and
/// half of the way down. A render that laid out every line above its window asked the allocator for
/// 3,700 times as much at the third of these as at the first.
const STARTS: [usize; 3] = [0, 50_000, 99_000];

/// The number of runs a timing takes the fastest of, which is what keeps a machine's own noise out
/// of the ratio.
const RUNS: usize = 9;

/// The factor by which drawing a window of the long block may cost more than drawing the same
/// window of the short one. A render that lays the whole block out costs about a thousand times
/// as much, so only a render bounded by its window passes.
const MARGIN: u32 = 4;

/// The factor by which drawing a window deep in the long block may cost more in time than drawing
/// one off its top. A walk down to a deep window still reads the ends of the lines it steps over,
/// and a hundred thousand of them are nine megabytes, so the ratio is not one: it was 14 in release
/// and 8 in debug, against the 2,200 it was when those lines were laid out instead. What the walk
/// may not do is build anything, which is what the allocations either side of this are for.
const DEPTH_MARGIN: u32 = 32;

/// The memory counting the rows of the long block may ask for, which it was measured to take none
/// of at all, against the 1.2 GB drawing it to count them asked for.
const COUNTING_MEMORY: usize = 1 << 10;

/// The calls to the allocator counting the rows of the long block may make, measured at none,
/// against the 13.6 million drawing it to count them made.
const COUNTING_CALLS: usize = 8;

/// The bytes a walk over a block whose rows have to be laid out to be counted may hold at once
/// beyond the rows it was asked for. It is a line of the fixture's layout, which is what a walk
/// that throws each line away again holds, against the whole block's a walk that keeps them does.
const HELD_MEMORY: usize = 1 << 14;

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

    /// The bytes this thread has asked for since the last [`counted`] began, given back or not.
    static ASKED: Cell<usize> = const { Cell::new(0) };

    /// The number of times it asked for them.
    static CALLS: Cell<usize> = const { Cell::new(0) };
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

    let of_short = timed(&short, 0);
    let of_long = timed(&long, 0);

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

    let of_short = timed(&short, 0);
    let of_long = timed(&long, 0);
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
    let window = RowWindow::new(ROWS_PER_LINE * SHORT - WINDOW, WINDOW);

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
fn a_line_of_the_fixture_is_drawn_in_the_rows_the_measurements_take_it_to_be() {
    let long = plain(LONG);
    let wrapping = wrapping();

    assert_eq!(
        LONG_ROWS,
        long.row_count(&wrapping),
        "a line of the fixture is not drawn in {ROWS_PER_LINE} rows, so a window of it is not the \
         screenful the measurements say"
    );
    for start in STARTS {
        let rendered = long.render(RowWindow::new(start, WINDOW), &wrapping);

        assert_eq!(
            WINDOW,
            rendered.rows().len(),
            "the window at row {start} did not fill, so there is less there to measure than the \
             window off the top"
        );
    }
}

#[test]
fn a_window_deep_in_a_block_asks_the_allocator_for_what_one_off_its_top_asks_for() {
    let long = plain(LONG);
    let wrapping = wrapping();

    let (_, off_the_top) = counted(|| long.render(RowWindow::new(STARTS[0], WINDOW), &wrapping));
    for start in STARTS {
        let (_, deep) = counted(|| long.render(RowWindow::new(start, WINDOW), &wrapping));
        let (bytes, calls) = deep;
        let (top_bytes, top_calls) = off_the_top;

        assert_eq!(
            off_the_top, deep,
            "drawing {WINDOW} rows at row {start} of a {LONG}-line block asked for {bytes} bytes \
             in {calls} calls, and drawing the same rows off its top asked for {top_bytes} in \
             {top_calls}, so the render follows the row it starts at rather than the rows it was \
             asked for"
        );
    }
}

#[test]
fn a_window_deep_in_a_block_costs_what_one_off_its_top_costs() {
    let long = plain(LONG);

    let off_the_top = timed(&long, STARTS[0]);
    for start in STARTS {
        let deep = timed(&long, start);

        assert!(
            deep < off_the_top * DEPTH_MARGIN,
            "drawing {WINDOW} rows at row {start} of a {LONG}-line block took {deep:?} and drawing \
             the same rows off its top took {off_the_top:?}, so the render follows the row it \
             starts at rather than the rows it was asked for"
        );
    }
}

#[test]
fn counting_the_rows_of_an_open_block_does_not_draw_it() {
    let transcript: Transcript = [plain(LONG)].into_iter().collect();
    let folds = Folds::of(&transcript, &[]);
    let view = View::of(&folds, &transcript);
    let wrapping = wrapping();

    let (rows, (bytes, calls)) = counted(|| view.rows(0, &wrapping));

    assert_eq!(
        LONG_ROWS, rows,
        "the entry was counted as a different number of rows than the block is drawn in"
    );
    assert!(
        bytes < COUNTING_MEMORY && calls < COUNTING_CALLS,
        "counting the rows of a {LONG}-line block asked for {bytes} bytes in {calls} calls, so it \
         is drawing the block to count it"
    );
}

#[test]
fn counting_the_rows_of_an_open_block_of_tabs_holds_no_row_of_it() {
    let transcript: Transcript = [tabbed(LONG)].into_iter().collect();
    let folds = Folds::of(&transcript, &[]);
    let view = View::of(&folds, &transcript);
    let wrapping = wrapping();

    let (rows, held) = measured(|| view.rows(0, &wrapping));

    assert_eq!(
        LONG_ROWS, rows,
        "the tab-indented entry was counted as a different number of rows than it is drawn in"
    );
    assert!(
        held < HELD_MEMORY,
        "counting the rows of a {LONG}-line block whose rows have to be laid out to be counted \
         held {held} bytes at once, so it is keeping what it counted rather than a line of it"
    );
}

#[test]
fn a_window_deep_in_a_block_of_tabs_holds_a_line_of_it_at_a_time() {
    let tabbed = tabbed(LONG);
    let wrapping = wrapping();

    let (rendered, off_the_top) =
        measured(|| tabbed.render(RowWindow::new(STARTS[0], WINDOW), &wrapping));
    assert_eq!(
        WINDOW,
        rendered.rows().len(),
        "the window off the top of the tab-indented block did not fill"
    );

    for start in STARTS {
        let (rendered, held) = measured(|| tabbed.render(RowWindow::new(start, WINDOW), &wrapping));

        assert_eq!(
            WINDOW,
            rendered.rows().len(),
            "the window at row {start} of the tab-indented block did not fill"
        );
        assert!(
            held < off_the_top + HELD_MEMORY,
            "drawing {WINDOW} rows at row {start} of a {LONG}-line block whose rows have to be \
             laid out to be counted held {held} bytes at once, against the {off_the_top} the same \
             rows off its top held, so the walk down to it keeps the lines it steps over"
        );
    }
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
                let _ = ASKED.try_with(|asked| asked.set(asked.get() + layout.size()));
                let _ = CALLS.try_with(|calls| calls.set(calls.get() + 1));
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

/// Runs `measure` and reads off everything it asked the allocator for rather than what it held at
/// once.
///
/// A walk that builds a row for every line it steps over and throws each away again holds no more
/// at once than a walk that builds twenty rows, so the peak says nothing about it. What it asked
/// for does.
///
/// # Returns
///
/// What it returned, and the bytes it asked for paired with the number of calls it asked in.
fn counted<ValueType>(measure: impl FnOnce() -> ValueType) -> (ValueType, (usize, usize)) {
    ASKED.with(|asked| asked.set(0));
    CALLS.with(|calls| calls.set(0));
    let value = measure();

    (value, (ASKED.with(Cell::get), CALLS.with(Cell::get)))
}

/// Times a window of `block` drawn from its row `start`.
///
/// # Returns
///
/// The fastest of [`RUNS`] renders.
fn timed(block: &Block, start: usize) -> Duration {
    let wrapping = wrapping();
    let window = RowWindow::new(start, WINDOW);

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
/// The lines of [`plain`], each indented with a tab. A tab is the commonest thing a build tool
/// writes and is what leaves a line's rows unreadable from its length, so the two fixtures differ
/// in the one thing that decides whether a walk over a block lays out the lines it steps over or
/// only reads where each of them ends.
fn tabbed(count: usize) -> Block {
    Block::new(
        Kind::Message(Role::Assistant),
        text(0..count)
            .lines()
            .map(|line| format!("\t{line}\n"))
            .collect(),
    )
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
/// The lines numbered by `range`, each of them long enough to wrap once at [`COLUMNS`] columns and
/// all of them of one length, so that a window drawn deep in a block of them draws the same number
/// of bytes as one drawn off its top and there is nothing but the row it starts at left for a
/// measurement to follow.
fn text(range: Range<usize>) -> String {
    range
        .map(|number| {
            format!(
                "line {number:06} of a block whose lines are long enough to wrap once over the \
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
