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
//! That is required of a *numbered* window over [`Content::Plain`], whose lines are the printable
//! ASCII whose rows a width divides out of a length. It is not what a numbered window over any
//! other content can promise: a tab's columns are not its bytes, so every line above such a window
//! is laid out to be counted, and the bytes that costs follow the row the window starts at. What
//! that walk must still do is throw each line away again, which is the difference between a frame
//! that holds a line of a block and one that holds all of it, so a numbered window over
//! [`Content::Tabbed`] is measured in what it holds at once rather than in what it asks for.
//!
//! An *anchored* window promises the same thing over every content and under every wrapping
//! option, because it is drawn from where it names rather than walked down to. That is measured
//! here over five contents -- printable ASCII, tab-indented lines, CJK, emoji and box-drawing
//! characters -- under vim's defaults and under each of `'showbreak'`, `'breakindent'` and
//! `'linebreak'`, in the bytes it asks for, the calls it asks in and the time it takes, at rows 0,
//! 50,000 and 99,000 of a hundred-thousand-line block. Measuring only printable ASCII under vim's
//! defaults is how the shape of the fast path went unstated: that is the one case in which a
//! line's rows are its length over the width, and the whole of the earlier bound rested on it.
//!
//! A window is bounded by where its line ends as well as by where the block does. A tool result
//! holding a minified document is one logical line of megabytes, and laying that line out to draw
//! twenty rows of it costs the line rather than the rows, so [`ENORMOUS`] and the line sixteen
//! times its length are drawn from and required to cost the same.
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
//! The same three windows of a hundred thousand tab-indented lines ask for 219,420 bytes, 236 MB
//! and 467 MB and take 59 µs, 48 ms and 102 ms, holding 24,952 bytes at once at every one of the
//! three against the 24,454 the plain fixture holds; counting that block asks for 942 MB in 2.7
//! million calls and takes 200 ms, and holds 5,193 bytes where counting the plain one holds none.
//! Four thousand lines diffed against four thousand with nothing in common cost 56 ms and 2.4 MB;
//! twenty thousand against twenty thousand cost 10 ms and 11 MB past the bound, and 3.6 ms with one
//! line inserted, which the common head and tail match off to a middle of one line either side.
//!
//! An anchored window of twenty rows asks for the same bytes in the same calls and takes the same
//! time at rows 0, 50,000 and 99,000 of each of the five contents: 121,800 bytes in 1,364 calls
//! and 42 µs of printable ASCII, 125,180 in 1,394 and 42 µs of tab-indented lines, 76,960 in 824
//! and 25 µs of CJK, 83,720 in 824 and 29 µs of emoji, and 131,040 in 1,264 and 41 µs of
//! box-drawing characters. The numbered window at row 99,000 of the same four blocks that are not
//! printable ASCII asks for 467 MB in 1.3 million calls and 93 ms, 245 MB in 1.1 million and
//! 59 ms, 258 MB in 1.1 million and 68 ms, and 466 MB in 1.2 million and 101 ms. Under
//! `'showbreak'`, `'breakindent'`, `'linebreak'` and all three at once the anchored window of the
//! tab-indented block asks for 125,340, 125,580, 125,180 and 125,940 bytes and takes 42 µs, 45 µs,
//! 56 µs and 57 µs, wherever it is anchored.
//!
//! Twenty rows off the top of a sixteen-megabyte logical line ask for 196,816 bytes in 2,063 calls
//! and take 65 µs, which is what twenty rows off the top of a one-megabyte one ask for and take;
//! at row 100 of either they ask for 1,142,384 bytes in 3,268 calls and take 200 µs. Laying those
//! two lines out whole asked for 160 MB in 1.3 million calls and 2.6 GB in 21 million, and took
//! 64 ms and 2.0 s.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::hint::black_box;
use std::num::NonZeroUsize;
use std::ops::Range;
use std::time::{Duration, Instant};

use vbc_editor::chat::block::{Block, Kind, Rendered, Role, RowAnchor, RowWindow};
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

/// The logical lines the rows of [`STARTS`] begin at, which is what an anchored window names them
/// by. That they are those rows is checked rather than assumed, both by the count of rows a line
/// of every fixture is drawn in and by walking a short block down to each of them.
const ANCHORED_LINES: [usize; 3] = [0, 25_000, 49_500];

/// The wide characters a line of [`Content::Cjk`] is filled with, and how many of them fill one.
const CJK_FILLER: &str = "\u{4e2d}\u{6587}\u{5b57}\u{7b26}";
const CJK_FILLERS: usize = 11;

/// The box-drawing characters a line of [`Content::Boxes`] is filled with, which is the shape a
/// tool draws a tree in, and how many of them fill one.
const BOX_FILLER: &str = "\u{251c}\u{2500}\u{253c}\u{2500}\u{2524}\u{2500}";
const BOX_FILLERS: usize = 14;

/// The emoji a line of [`Content::Emoji`] is filled with: one written in a single code point,
/// repeated, and one joined out of five, which is the cluster a width cannot be read off a length
/// for.
const EMOJI: &str = "\u{1f680}";
const EMOJIS: usize = 42;
const JOINED_EMOJI: &str = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}";

/// The bytes of the logical line that is longer than any window drawn into it, which is the shape
/// a minified document arrives in.
const ENORMOUS: usize = 1 << 20;

/// The factor by which the longer of the two enormous lines is longer than [`ENORMOUS`]. A render
/// that laid a line out to draw a window into it costs this many times as much on the longer.
const ENORMOUS_FACTOR: usize = 16;

/// The rows into an enormous logical line a window is drawn from. A window is anchored inside a
/// line as well as at the top of one, so the rows above it inside its own line are drawn from too.
const ENORMOUS_STARTS: [usize; 3] = [0, 1, 100];

/// The memory a window into an enormous logical line may ask for. Laying either line out whole
/// asks for tens of times the line, so a window that costs the rows it draws misses this by orders
/// of magnitude rather than by a margin.
const ENORMOUS_MEMORY: usize = 4 << 20;

/// The columns the rendered output is compared at. A width of one wraps every cluster onto a row of
/// its own, and one wider than any fixture wraps nothing at all.
const MATRIX_WIDTHS: [usize; 6] = [1, 2, 3, 7, 13, COLUMNS];

/// The rows of a fixture the comparison draws a window from. A short fixture is drawn from every
/// row of it and a long one from this many, spread evenly down it.
const MATRIX_STARTS: usize = 8;

/// The bytes of the one logical line the comparison holds, which is long enough that a window into
/// it is drawn from a prefix of it rather than from the whole.
const MATRIX_LINE: usize = 4_096;

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

/// The factor by which an anchored window drawn deep in a block may cost more in time than one
/// drawn off its top. An anchored window reads nothing above where it is anchored, so unlike
/// [`DEPTH_MARGIN`] this is a margin for a shared machine's own noise rather than for work.
const ANCHORED_MARGIN: u32 = 4;

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
fn a_line_of_every_fixture_is_drawn_in_the_rows_the_anchored_measurements_take_it_to_be() {
    for content in Content::ALL {
        let short = content.block(SHORT);
        for options in option_sets() {
            let wrapping = wrapping_under(&options);

            assert_eq!(
                ROWS_PER_LINE * SHORT + 1,
                short.row_count(&wrapping),
                "a line of the {content:?} fixture is not drawn in {ROWS_PER_LINE} rows under \
                 {options:?}, so the lines an anchor names are not the rows the measurements say"
            );
            for line in [0, 1, SHORT / 2, SHORT - 1] {
                assert_eq!(
                    short.anchor(ROWS_PER_LINE * line, &wrapping),
                    anchored(&short, line),
                    "line {line} of the {content:?} fixture is not row {} under {options:?}",
                    ROWS_PER_LINE * line
                );
            }
        }
    }
}

#[test]
fn an_anchored_window_deep_in_a_block_costs_what_one_anchored_at_its_top_costs() {
    for content in Content::ALL {
        let long = content.block(LONG);
        for options in option_sets() {
            let wrapping = wrapping_under(&options);
            let top = anchored(&long, ANCHORED_LINES[0]);
            let (_, off_the_top) = counted(|| long.render(RowWindow::at(top, WINDOW), &wrapping));
            let fastest = timed_at(&long, top, &wrapping);

            for line in ANCHORED_LINES {
                let anchor = anchored(&long, line);
                let (rendered, deep) =
                    counted(|| long.render(RowWindow::at(anchor, WINDOW), &wrapping));
                let elapsed = timed_at(&long, anchor, &wrapping);
                let (bytes, calls) = deep;
                let (top_bytes, top_calls) = off_the_top;

                assert_eq!(
                    WINDOW,
                    rendered.rows().len(),
                    "the window anchored at line {line} of the {content:?} fixture under \
                     {options:?} did not fill"
                );
                assert_eq!(
                    off_the_top,
                    deep,
                    "drawing {WINDOW} rows anchored at row {} of a {LONG}-line {content:?} block \
                     under {options:?} asked for {bytes} bytes in {calls} calls, and drawing the \
                     same rows off its top asked for {top_bytes} in {top_calls}, so an anchored \
                     render follows where it is anchored rather than the rows it was asked for",
                    ROWS_PER_LINE * line
                );
                assert!(
                    elapsed < fastest * ANCHORED_MARGIN,
                    "drawing {WINDOW} rows anchored at row {} of a {LONG}-line {content:?} block \
                     under {options:?} took {elapsed:?} and drawing the same rows off its top took \
                     {fastest:?}",
                    ROWS_PER_LINE * line
                );
            }
        }
    }
}

#[test]
fn an_anchored_window_of_a_long_block_costs_what_one_of_a_short_block_costs() {
    for content in Content::ALL {
        let short = content.block(SHORT);
        let long = content.block(LONG);
        for options in option_sets() {
            let wrapping = wrapping_under(&options);
            let anchor = RowAnchor::top();

            let (rows, of_short) =
                counted(|| short.render(RowWindow::at(anchor, WINDOW), &wrapping));
            let (same, of_long) = counted(|| long.render(RowWindow::at(anchor, WINDOW), &wrapping));
            let of_short_time = timed_at(&short, anchor, &wrapping);
            let of_long_time = timed_at(&long, anchor, &wrapping);

            assert_eq!(
                rows, same,
                "the {content:?} blocks did not draw the same rows under {options:?}, so the \
                 measurement compares two things"
            );
            assert_eq!(
                of_short, of_long,
                "drawing {WINDOW} rows of a {SHORT}-line {content:?} block under {options:?} asked \
                 for {of_short:?} and drawing the same rows of a {LONG}-line one asked for \
                 {of_long:?}, so the render follows the block"
            );
            assert!(
                of_long_time < of_short_time * MARGIN,
                "drawing {WINDOW} rows of a {SHORT}-line {content:?} block under {options:?} took \
                 {of_short_time:?} and drawing the same rows of a {LONG}-line one took \
                 {of_long_time:?}"
            );
        }
    }
}

#[test]
fn scrolling_an_anchored_window_by_a_row_costs_the_same_wherever_the_window_is() {
    for content in Content::ALL {
        let long = content.block(LONG);
        for options in option_sets() {
            let wrapping = wrapping_under(&options);
            let top = anchored(&long, ANCHORED_LINES[0]);
            let (_, off_the_top) = counted(|| scrolled(&long, top, &wrapping));
            let quickest = fastest(|| scrolled(&long, top, &wrapping));

            for line in ANCHORED_LINES {
                let anchor = anchored(&long, line);
                let (rendered, deep) = counted(|| scrolled(&long, anchor, &wrapping));
                let elapsed = fastest(|| scrolled(&long, anchor, &wrapping));

                assert!(
                    elapsed < quickest * ANCHORED_MARGIN,
                    "scrolling a {WINDOW}-row window by one row at row {} of a {LONG}-line \
                     {content:?} block under {options:?} took {elapsed:?}, and scrolling the same \
                     window off its top took {quickest:?}",
                    ROWS_PER_LINE * line
                );
                assert_eq!(
                    WINDOW,
                    rendered.rows().len(),
                    "the window scrolled a row below line {line} of the {content:?} fixture under \
                     {options:?} did not fill"
                );
                assert_eq!(
                    off_the_top,
                    deep,
                    "scrolling a {WINDOW}-row window by one row at row {} of a {LONG}-line \
                     {content:?} block under {options:?} asked for {deep:?}, and scrolling the \
                     same window off its top asked for {off_the_top:?}, so a keystroke costs the \
                     scroll rather than the screen",
                    ROWS_PER_LINE * line
                );
            }
        }
    }
}

#[test]
fn a_window_into_an_enormous_logical_line_costs_what_one_into_a_short_line_costs() {
    for content in Content::ALL {
        let short = content.one_line(ENORMOUS);
        let long = content.one_line(ENORMOUS_FACTOR * ENORMOUS);
        for options in option_sets() {
            let wrapping = wrapping_under(&options);

            for start in ENORMOUS_STARTS {
                let anchor = RowAnchor::new(0, 0, start);
                let (rows, of_short) =
                    counted(|| short.render(RowWindow::at(anchor, WINDOW), &wrapping));
                let (same, of_long) =
                    counted(|| long.render(RowWindow::at(anchor, WINDOW), &wrapping));
                let of_short_time = timed_at(&short, anchor, &wrapping);
                let of_long_time = timed_at(&long, anchor, &wrapping);
                let (bytes, _) = of_long;

                assert_eq!(
                    WINDOW,
                    rows.rows().len(),
                    "the window at row {start} of one {ENORMOUS}-byte {content:?} line under \
                     {options:?} did not fill"
                );
                assert_eq!(
                    rows, same,
                    "the two {content:?} lines did not draw the same rows at row {start} under \
                     {options:?}, so the measurement compares two things"
                );
                assert_eq!(
                    of_short, of_long,
                    "drawing {WINDOW} rows at row {start} of one {ENORMOUS}-byte {content:?} line \
                     under {options:?} asked for {of_short:?} and drawing the same rows of a line \
                     {ENORMOUS_FACTOR} times as long asked for {of_long:?}, so the render lays out \
                     the line rather than the window"
                );
                assert!(
                    of_long_time < of_short_time * ANCHORED_MARGIN,
                    "drawing {WINDOW} rows at row {start} of one {ENORMOUS}-byte {content:?} line \
                     under {options:?} took {of_short_time:?} and drawing the same rows of a line \
                     {ENORMOUS_FACTOR} times as long took {of_long_time:?}, so the render reads \
                     the line rather than the window"
                );
                assert!(
                    bytes < ENORMOUS_MEMORY,
                    "drawing {WINDOW} rows at row {start} of one {content:?} logical line of \
                     {} bytes under {options:?} asked for {bytes} bytes",
                    ENORMOUS_FACTOR * ENORMOUS
                );
            }
        }
    }
}

#[test]
fn an_anchored_window_draws_the_rows_the_whole_block_draws_there_at_every_width_and_option() {
    for source in matrix_sources() {
        let block = Block::new(Kind::ToolResult, source.clone());
        for width in MATRIX_WIDTHS {
            for options in option_sets() {
                let wrapping = Wrapping::new(
                    NonZeroUsize::new(width).expect("a fixture is drawn in at least one column"),
                    Metrics::default(),
                    options.clone(),
                );
                let whole = block.render(RowWindow::new(0, 2 * source.len() + 2), &wrapping);
                let all = whole.rows();
                let step = 1 + all.len() / MATRIX_STARTS;

                for start in (0..2 + all.len()).step_by(step) {
                    for wanted in [0, 1, 2, 3, WINDOW] {
                        let end = (start + wanted).min(all.len());
                        let expected = all.get(start.min(all.len())..end).unwrap_or_default();
                        let numbered = block.render(RowWindow::new(start, wanted), &wrapping);
                        let anchor = block.anchor(start, &wrapping);
                        let drawn = block.render(RowWindow::at(anchor, wanted), &wrapping);

                        assert_eq!(
                            expected,
                            numbered.rows(),
                            "a numbered window of {wanted} rows at row {start} of {source:?} at \
                             {width} columns under {options:?} drew something other than those \
                             rows of the whole of it"
                        );
                        assert_eq!(
                            expected,
                            drawn.rows(),
                            "a window of {wanted} rows anchored at {anchor:?} of {source:?} at \
                             {width} columns under {options:?} drew something other than those \
                             rows of the whole of it"
                        );
                    }
                }
            }
        }
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

/// What the lines of a measured block are written from.
///
/// The contents differ in the one thing that decides whether a line's rows can be read off its
/// length: [`Content::Plain`] is the printable ASCII a width divides out of a length, and every
/// other content has to be laid out to be counted. Every one of them is drawn in
/// [`ROWS_PER_LINE`] rows a line at [`COLUMNS`] columns under every option, and every line of one
/// is written in the same number of bytes as every other, so a window drawn deep in a block of
/// them draws the same bytes as one drawn off its top and there is nothing but where it is
/// anchored left for a measurement to follow.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Content {
    /// The printable ASCII whose rows are its length over the width.
    Plain,

    /// The same lines indented with a tab, which is what a build tool writes.
    Tabbed,

    /// CJK, whose clusters are two columns wide and three bytes long.
    Cjk,

    /// Emoji, one of them joined out of five code points.
    Emoji,

    /// The box-drawing characters a tool draws a tree with.
    Boxes,
}

impl Content {
    /// Every content a window is measured over.
    const ALL: [Self; 5] = [
        Self::Plain,
        Self::Tabbed,
        Self::Cjk,
        Self::Emoji,
        Self::Boxes,
    ];

    /// # Returns
    ///
    /// A block of `lines` lines of this content, each of them long enough to wrap once.
    fn block(self, lines: usize) -> Block {
        Block::new(
            Kind::Message(Role::Assistant),
            (0..lines).map(|number| self.line(number)).collect(),
        )
    }

    /// # Returns
    ///
    /// A block of one logical line of this content, at least `bytes` bytes long, which is the
    /// shape a minified document arrives in.
    fn one_line(self, bytes: usize) -> Block {
        let line = self.line(0);
        let text = line.trim_end_matches('\n');
        let repeated = bytes.div_ceil(text.len());

        Block::new(Kind::ToolResult, text.repeat(repeated))
    }

    /// # Returns
    ///
    /// The line numbered `number` of this content, its separator included.
    fn line(self, number: usize) -> String {
        match self {
            Self::Plain => format!(
                "line {number:06} of a block whose lines are long enough to wrap once over the \
                 columns it is drawn in\n"
            ),
            Self::Tabbed => format!("\t{}", Self::Plain.line(number)),
            Self::Cjk => format!("{}\n", CJK_FILLER.repeat(CJK_FILLERS)),
            Self::Emoji => format!("{}{JOINED_EMOJI}\n", EMOJI.repeat(EMOJIS)),
            Self::Boxes => format!("{}\n", BOX_FILLER.repeat(BOX_FILLERS)),
        }
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

/// Times an anchored window of `block`.
///
/// # Returns
///
/// The fastest of [`RUNS`] renders.
fn timed_at(block: &Block, anchor: RowAnchor, wrapping: &Wrapping) -> Duration {
    let window = RowWindow::at(anchor, WINDOW);

    fastest(|| block.render(window, wrapping))
}

/// Runs `render` [`RUNS`] times, which is what keeps a machine's own noise out of a ratio.
///
/// # Returns
///
/// The fastest of those runs.
fn fastest<ValueType>(mut render: impl FnMut() -> ValueType) -> Duration {
    let mut quickest = Duration::MAX;
    for _ in 0..RUNS {
        let started = Instant::now();
        let value = render();
        quickest = quickest.min(started.elapsed());
        black_box(&value);
    }

    quickest
}

/// Draws a window of `block` anchored at `anchor` and then the window one row below it, which is
/// what a reader pressing `C-e` asks for.
///
/// # Returns
///
/// The window a row below `anchor`.
fn scrolled(block: &Block, anchor: RowAnchor, wrapping: &Wrapping) -> Rendered {
    let drawn = block.render(RowWindow::at(anchor, WINDOW), wrapping);
    let below = block.render(RowWindow::at(anchor, 1), wrapping);
    let next = below.next().expect("a row below the window was drawn");
    black_box(&drawn);

    block.render(RowWindow::at(next, WINDOW), wrapping)
}

/// Finds where the logical line `line` of `block` begins by reading where the lines above it end,
/// which is what a caller holding a position in a transcript already has and is not what a window
/// anchored there is measured for.
///
/// # Returns
///
/// The anchor of the first display row of that line.
fn anchored(block: &Block, line: usize) -> RowAnchor {
    let offset = block
        .source()
        .split_inclusive('\n')
        .take(line)
        .map(str::len)
        .sum();

    RowAnchor::new(offset, line, 0)
}

/// # Returns
///
/// The wrapping options a window is measured under: vim's own defaults, then each of the options
/// that moves where a line breaks, then all of them together. A line's rows are its length over
/// the width under the first of these and under none of the others.
fn option_sets() -> Vec<Options> {
    vec![
        Options::new(),
        Options::new().with_show_break("> ".to_owned()),
        Options::new()
            .with_break_indent(true)
            .with_break_indent_min(1),
        Options::new().with_line_break(true),
        Options::new()
            .with_show_break("> ".to_owned())
            .with_break_indent(true)
            .with_break_indent_min(1)
            .with_line_break(true),
    ]
}

/// # Returns
///
/// The sources the rendered output is compared over: one of each content, the control characters
/// and escapes a tool writes, and one logical line longer than any window drawn into it.
fn matrix_sources() -> Vec<String> {
    let mut sources: Vec<String> = Content::ALL
        .iter()
        .map(|content| (0..4).map(|number| content.line(number)).collect())
        .collect();
    sources.push("a\tb\rc\u{7}d\u{7f}e\n\tindented\n\n  trailing  ".to_owned());
    sources.push(Content::Plain.one_line(MATRIX_LINE).source().to_owned());
    sources.push(Content::Cjk.one_line(MATRIX_LINE).source().to_owned());

    sources
}

/// # Returns
///
/// A block of `count` lines, each of them long enough to wrap once.
fn plain(count: usize) -> Block {
    Content::Plain.block(count)
}

/// # Returns
///
/// The lines of [`plain`], each indented with a tab. A tab is the commonest thing a build tool
/// writes and is what leaves a line's rows unreadable from its length, so the two fixtures differ
/// in the one thing that decides whether a walk over a block lays out the lines it steps over or
/// only reads where each of them ends.
fn tabbed(count: usize) -> Block {
    Content::Tabbed.block(count)
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
/// The lines numbered by `range`, short enough that what a diff of them takes is the alignment's
/// rather than the text's.
fn lines(range: Range<usize>) -> String {
    range.map(|number| format!("line {number}\n")).collect()
}

/// # Returns
///
/// A wrapping drawing rows [`COLUMNS`] columns wide under vim's own defaults.
fn wrapping() -> Wrapping {
    wrapping_under(&Options::new())
}

/// # Returns
///
/// A wrapping drawing rows [`COLUMNS`] columns wide under `options`.
fn wrapping_under(options: &Options) -> Wrapping {
    Wrapping::new(
        NonZeroUsize::new(COLUMNS).expect("the measured width is not zero"),
        Metrics::default(),
        options.clone(),
    )
}
