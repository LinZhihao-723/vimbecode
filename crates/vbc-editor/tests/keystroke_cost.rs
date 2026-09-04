//! Checks that a keystroke costs the window it is typed at rather than the file it is typed into.
//!
//! The frame was bounded and the reconciliation behind it was not. After every key, the
//! application read the engine's whole rope back as a string and laid every line of it out again,
//! so a `j` cost 17.8 µs over a hundred lines, 294 µs over ten thousand and 3.17 ms over a hundred
//! thousand. The anchor-relative layout was built so that a frame costs the screen; a keystroke
//! that costs the file spends that saving before the frame is ever drawn, and a hundred-thousand
//! line file is a file where holding `j` down stutters.
//!
//! So one `j` is timed at three lengths and required to cost the same at all of them, in time and
//! in what it asks the allocator for. Allocations are counted rather than held bytes alone,
//! because the reconciliation that grew with the file handed back everything it took: it held a
//! screenful at the end of the keystroke and had asked for the file in the middle of it, which a
//! peak-held measurement cannot see.
//!
//! The content is varied because the cost of laying a line out is not the cost of counting its
//! bytes. Plain ASCII lines can be wrapped by dividing a length; a tab has to be measured against
//! the column it starts at, a CJK line against the width of every grapheme, and an emoji against
//! the clusters its characters join into. A measurement taken over printable ASCII alone would
//! have passed against a reconciliation that re-laid out every line of the file, because the line
//! it re-laid out was the cheap one. The position is varied for the same reason: a `j` at the top
//! of a file walks a window that starts at the first line, and a `j` deep inside one walks a window
//! that starts wherever the reader scrolled to.
//!
//! What is not claimed is that an edit is free. `x` and `i` change the text, and the text is what
//! the layout is built from, so a keystroke that writes still lays the file out again. What the
//! engine reports is whether a keystroke could have changed the text at all, which is what a
//! motion, a scroll, a search and a selection answer no to -- and those are the keys a reader holds
//! down.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::hint::black_box;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use vbc_editor::app::App;
use vbc_editor::engine::typed;
use vbc_layout::buffer::Buffer;

/// The window a keystroke is typed at, which is a terminal a reader would use.
const COLUMNS: u16 = 80;
const ROWS: u16 = 24;

/// The lengths a keystroke is timed at, which are the three the reconciliation was measured at
/// when it still cost the file.
const SHORT: usize = 100;
const MIDDLE: usize = 10_000;
const LONG: usize = 100_000;

/// The number of runs a measurement takes the fastest of, which is what keeps a machine's own
/// noise out of the ratio.
const RUNS: usize = 9;

/// The factor by which a keystroke over the longest file may cost more than the same keystroke
/// over the shortest. The reconciliation that read the whole rope back cost a hundred and eighty
/// times as much at a hundred thousand lines as at a hundred, so only one bounded by the window
/// passes.
const MARGIN: u32 = 3;

/// Where down a file the deep measurement is taken, as a share of its lines.
const DEEP: usize = 3;
const OF: usize = 4;

/// The allocator every measurement here is read through.
#[global_allocator]
static ALLOCATOR: Counting = Counting;

thread_local! {
    /// The bytes this thread has asked for since the last [`counted`] began, given back or not.
    static ASKED_FOR: Cell<usize> = const { Cell::new(0) };

    /// The number of times it asked for them.
    static CALLS: Cell<usize> = const { Cell::new(0) };
}

/// What a line of a measured file is written in, which is what says whether the cost of laying one
/// out is the cost of counting its bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Content {
    /// Printable ASCII, which is the one content whose rows can be counted from a length.
    Plain,

    /// A leading tab, which is as many columns as it takes to reach the next tab stop.
    Tabbed,

    /// Han characters, each of which is drawn in two cells.
    Cjk,

    /// A family emoji, whose characters join into one grapheme cluster.
    Emoji,
}

impl Content {
    /// # Returns
    ///
    /// Every content a measurement is taken over.
    fn every() -> [Self; 4] {
        [Self::Plain, Self::Tabbed, Self::Cjk, Self::Emoji]
    }

    /// # Returns
    ///
    /// The line numbered `index` of a file written in this content, long enough that it wraps in a
    /// window [`COLUMNS`] columns wide.
    fn line(self, index: usize) -> String {
        match self {
            Self::Plain => format!("line {index} of a file a reader is holding `j` down inside of"),
            Self::Tabbed => format!("\tline {index}\tof a file indented with tabs\tand more tabs"),
            Self::Cjk => format!("第{index}行，这一行是中文的，宽度不是字符数，要一个个量过去。"),
            Self::Emoji => {
                format!("line {index} \u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467} e\u{301} x")
            }
        }
    }

    /// # Returns
    ///
    /// A run of characters found on the line numbered `index` of a file written in this content
    /// and on no line above it, which is what a search is given to reach that line.
    fn mark(self, index: usize) -> String {
        match self {
            Self::Cjk => format!("\u{7b2c}{index}\u{884c}"),
            _ => format!("line {index}"),
        }
    }
}

/// Validation 5: one `j` costs the same over a hundred lines, ten thousand and a hundred thousand.
#[test]
fn a_keystroke_costs_the_same_over_a_short_file_and_a_long_one() {
    for content in Content::every() {
        for deep in [false, true] {
            let of_short = timed(SHORT, content, deep);
            let of_middle = timed(MIDDLE, content, deep);
            let of_long = timed(LONG, content, deep);

            for (lines, cost) in [(MIDDLE, of_middle), (LONG, of_long)] {
                assert!(
                    cost < of_short * MARGIN,
                    "{content:?} `j` {} took {of_short:?} over {SHORT} lines and {cost:?} over \
                     {lines}, so the keystroke follows the file",
                    place(deep)
                );
            }
        }
    }
}

/// Validation 5: one `j` asks the allocator for the same over a hundred lines, ten thousand and a
/// hundred thousand.
#[test]
fn a_keystroke_asks_the_allocator_for_the_same_over_a_short_file_and_a_long_one() {
    for content in Content::every() {
        for deep in [false, true] {
            let of_short = counted(SHORT, content, deep);
            let of_middle = counted(MIDDLE, content, deep);
            let of_long = counted(LONG, content, deep);

            for (lines, cost) in [(MIDDLE, of_middle), (LONG, of_long)] {
                assert!(
                    cost.0 < of_short.0 * usize::try_from(MARGIN).expect("the margin is small"),
                    "{content:?} `j` {} asked for {} bytes over {SHORT} lines and {} over {lines}",
                    place(deep),
                    of_short.0,
                    cost.0
                );
                assert!(
                    cost.1 < of_short.1 * usize::try_from(MARGIN).expect("the margin is small"),
                    "{content:?} `j` {} asked {} times over {SHORT} lines and {} times over \
                     {lines}",
                    place(deep),
                    of_short.1,
                    cost.1
                );
            }
        }
    }
}

/// The keystrokes that change the text are the ones that lay it out again, and a `j` is not one of
/// them however the file it is typed into was written.
#[test]
fn a_motion_lays_out_nothing_a_frame_does_not_draw() {
    for content in Content::every() {
        let mut app = opened(SHORT, content, false);
        let text = app.text().text();
        let moved = counted_at(&mut app, 'j');
        let edited = counted_at(&mut app, 'x');

        assert!(
            moved.0 < edited.0,
            "{content:?} a `j` asked for {} bytes and an `x` for {}, so nothing tells them apart",
            moved.0,
            edited.0
        );
        assert_ne!(text, app.text().text(), "the `x` edited nothing");
    }
}

/// Times one `j` typed at a file of `lines` lines written in `content`.
///
/// # Returns
///
/// The fastest of [`RUNS`] keystrokes.
fn timed(lines: usize, content: Content, deep: bool) -> Duration {
    let mut app = opened(lines, content, deep);

    let mut fastest = Duration::MAX;
    for _ in 0..RUNS {
        let started = Instant::now();
        let outcome = app.press(area(), typed('j'));
        fastest = fastest.min(started.elapsed());
        black_box(outcome);
    }
    black_box(app.cursor());

    fastest
}

/// Counts what one `j` typed at a file of `lines` lines written in `content` asks the allocator
/// for.
///
/// # Returns
///
/// The bytes it asked for and the number of times it asked.
fn counted(lines: usize, content: Content, deep: bool) -> (usize, usize) {
    let mut app = opened(lines, content, deep);
    counted_at(&mut app, 'j');

    counted_at(&mut app, 'j')
}

/// Counts what typing `key` at `app` asks the allocator for.
///
/// The counters are the running thread's own, so a measurement is not disturbed by whatever the
/// other tests of this binary are allocating beside it.
///
/// # Returns
///
/// The bytes it asked for and the number of times it asked.
fn counted_at(app: &mut App, key: char) -> (usize, usize) {
    let bytes = ASKED_FOR.with(Cell::get);
    let calls = CALLS.with(Cell::get);
    let outcome = app.press(area(), typed(key));
    let asked = (
        ASKED_FOR.with(Cell::get) - bytes,
        CALLS.with(Cell::get) - calls,
    );
    black_box(outcome);

    asked
}

/// # Returns
///
/// An application over a file of `lines` lines written in `content`, with the cursor left at the
/// top of it or three quarters of the way down it, and a window already followed to wherever that
/// is.
///
/// The deep cursor is put there by a search rather than by a walk, because a walk of ten thousand
/// keystrokes measures the walk.
fn opened(lines: usize, content: Content, deep: bool) -> App {
    let text: Vec<String> = (0..lines).map(|index| content.line(index)).collect();
    let mut app = App::new(Buffer::from_lines(text)).with_status(true);
    if deep {
        let wanted = lines * DEEP / OF;
        for character in format!("/{}", content.mark(wanted)).chars() {
            app.press(area(), typed(character));
        }
        app.press(area(), KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert_eq!(
            wanted,
            app.cursor().line,
            "the search meant to reach line {wanted} of a {lines}-line file landed elsewhere"
        );
    }
    app.press(area(), typed('j'));

    app
}

/// # Returns
///
/// What a message calls the place a keystroke was typed at.
fn place(deep: bool) -> &'static str {
    if deep {
        "deep inside the file"
    } else {
        "at the top of the file"
    }
}

/// # Returns
///
/// The window a keystroke is typed at.
fn area() -> Rect {
    Rect::new(0, 0, COLUMNS, ROWS)
}

/// The allocator that records what a measurement asks for.
struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = System.alloc(layout);
        if !pointer.is_null() {
            let _ = ASKED_FOR.try_with(|asked| asked.set(asked.get() + layout.size()));
            let _ = CALLS.try_with(|calls| calls.set(calls.get() + 1));
        }

        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        System.dealloc(pointer, layout);
    }
}
