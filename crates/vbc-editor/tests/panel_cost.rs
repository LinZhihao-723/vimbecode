//! Checks that the frame the application draws the transcript panel into costs the screen rather
//! than the scroll.
//!
//! The window a block is drawn in was bounded by where it begins, and the panel went on asking for
//! it by ordinal. `App::draw_panel` reached the panel through a position that named a row number,
//! so every frame walked the block down to that row before it drew anything, and the bound the
//! anchored window promises was one the application never spent. Twenty rows of a hundred-thousand
//! line tab-indented message took 50 µs at the top of the message and 96 ms ninety-nine thousand
//! rows down, which is a panel a reader cannot scroll.
//!
//! So the panel's position names where a row begins rather than which row it is, and what is
//! measured here is the application rather than the block. Every case drives `App` from the keys a
//! reader types: `CTRL-T` reaches the panel, `CTRL-E` scrolls it a row at a time, and the frame is
//! the one `App::draw` writes. A measurement taken against `Block::render` proves the block and
//! says nothing about the program that draws it, which is how this defect survived being fixed
//! once already.
//!
//! Four things are required of it. A frame must cost the same at the top of a long message and
//! deep inside one, in time and in what it asks the allocator for. It must cost the same over
//! every content whose rows cannot be read off a length -- tab-indented lines, CJK, emoji and
//! box-drawing characters -- and under `'showbreak'`, `'breakindent'` and `'linebreak'`, which are
//! the configurations that hid this twice. The scroll itself must cost the same wherever the panel
//! stands, upward as well as downward -- a `CTRL-Y` is answered by a step that reads the line
//! above the one the panel stands on rather than the block around it -- and so must the follow
//! that runs after every other keystroke, which asks for a window of the screen and two hundred
//! and fifty-six rows more.
//!
//! The fifth is that none of that changed what is drawn. The same transcript is drawn twice, once
//! into a window tall enough to hold the whole of it and once into a screenful scrolled down it a
//! row at a time and then back up it again, and every frame of either scroll is required to be
//! the rows of the tall frame it stands over, cell for cell, at three widths, over five contents
//! and under five option sets. Both directions are swept because they are answered by different
//! steps: the way down reads the line the panel stands on, and the way up reads the line above it
//! and, at the top of an entry, reaches the last row of the entry above without laying out the
//! rows before it.
//!
//! What these assert is what they were seen to measure. In a debug build at eighty columns, a
//! frame of twenty rows of a hundred-thousand-line tab-indented message asks the allocator for
//! 141,161 bytes in 1,518 calls and takes 780 µs at rows 0, 1,000, 50,000 and 99,000 alike, to the
//! byte and to the call, against the 96 ms the frame at row 99,000 took while the panel asked for
//! its rows by ordinal. One `CTRL-E` asks for 9,607 bytes in 30 calls and takes 20 µs at every one
//! of those rows. The keystroke that follows the cursor asks for 6.7 MB in 51,401 calls and takes
//! 20 ms at row 1,000 and exactly the same at row 99,000: what it costs is the screenful and the
//! two hundred and fifty-six rows it walks, which is a great deal more than a frame and is the
//! same wherever the panel stands.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::hint::black_box;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer as Cells;
use ratatui::layout::Rect;
use vbc_editor::app::{App, Focus};
use vbc_editor::chat::block::{Block, Kind, Role};
use vbc_editor::chat::transcript::Transcript;
use vbc_layout::buffer::Buffer;
use vbc_layout::line::Options;

/// The window the panel is drawn in, which is a terminal a reader would use.
const COLUMNS: u16 = 80;
const ROWS: u16 = 20;

/// The lines of the message the headline measurements are taken over, which is the length a
/// `cargo` build of this workspace runs to several times over. An assistant message never folds,
/// so a message that long is drawn rather than summarized away.
const LONG: usize = 100_000;

/// The rows down that message a frame is drawn from. A panel that walked down to the row it starts
/// at cost 50 µs at the first of these and 96 ms at the last.
const TOPS: [usize; 4] = [0, 1_000, 50_000, 99_000];

/// The lines of the message every content and every option set is measured over, and the row down
/// it the deep frame is drawn from. A walk down to that row lays out ten thousand lines of it,
/// which is two orders of magnitude more than a frame of twenty rows.
const MATRIX_LINES: usize = 20_000;
const MATRIX_TOP: usize = 10_000;

/// The rows the panel stands at when the follow after a keystroke is measured. Both are further
/// down the message than the follow walks, so both ask for the window and the rows beyond it that
/// a follow asks for, and neither finds the cursor in them.
const FOLLOWED_TOPS: [usize; 2] = [1_000, 99_000];

/// The rows the panel stands at when the scroll upward is measured. A `CTRL-Y` typed at the first
/// row of the message moves nothing, so the top of it is not among them.
const RAISED_TOPS: [usize; 3] = [1_000, 50_000, 99_000];

/// The widths the drawn frames are compared at: one that wraps every line of every fixture, one
/// that wraps most of them, and the one a panel is read at.
const COMPARED_WIDTHS: [u16; 3] = [20, 40, COLUMNS];

/// The rows the window tall enough to hold the whole of the compared transcript is drawn in, which
/// is more rows than that transcript takes at the narrowest of those widths.
const TALL: u16 = 240;

/// The number of runs a timing takes the fastest of, which is what keeps a machine's own noise out
/// of a ratio.
const RUNS: usize = 9;

/// The factor by which a frame, a scroll or a follow drawn deep in a message may cost more in time
/// than the same one at its top. A panel that walks down to the row it draws from cost two
/// thousand times as much at the deepest of [`TOPS`], so only one that costs the screen passes.
const MARGIN: u32 = 8;

/// The allocator every measurement here is read through.
#[global_allocator]
static ALLOCATOR: Counting = Counting;

thread_local! {
    /// The bytes this thread has asked for since the last [`counted`] began, given back or not.
    static ASKED: Cell<usize> = const { Cell::new(0) };

    /// The number of times it asked for them.
    static CALLS: Cell<usize> = const { Cell::new(0) };
}

#[test]
fn a_frame_of_the_panel_deep_in_a_long_message_asks_for_what_one_at_its_top_asks_for() {
    let mut app = reading(Content::Tabbed, LONG, &Options::new(), area(ROWS));
    let mut cells = Cells::empty(area(ROWS));
    let mut at = 0;
    let mut counts = Vec::new();

    for top in TOPS {
        at = scrolled(&mut app, at, top);
        app.draw(&mut cells, area(ROWS));
        let (_, count) = counted(|| app.draw(&mut cells, area(ROWS)));
        counts.push((top, count));
    }

    let (_, off_the_top) = counts[0];
    for (top, count) in &counts {
        let (bytes, calls) = *count;
        let (top_bytes, top_calls) = off_the_top;

        assert_eq!(
            off_the_top, *count,
            "a frame of the panel at row {top} of a {LONG}-line message asked for {bytes} bytes \
             in {calls} calls, and the same frame at its top asked for {top_bytes} in \
             {top_calls}, so the frame follows the row it is drawn from"
        );
    }
}

#[test]
fn a_frame_of_the_panel_deep_in_a_long_message_costs_what_one_at_its_top_costs() {
    let mut app = reading(Content::Tabbed, LONG, &Options::new(), area(ROWS));
    let mut cells = Cells::empty(area(ROWS));
    let mut at = 0;
    let mut times = Vec::new();

    for top in TOPS {
        at = scrolled(&mut app, at, top);
        times.push((top, fastest(|| app.draw(&mut cells, area(ROWS)))));
    }

    let off_the_top = times[0].1;
    for (top, taken) in &times {
        assert!(
            *taken < off_the_top * MARGIN,
            "a frame of the panel at row {top} of a {LONG}-line message took {taken:?} and the \
             same frame at its top took {off_the_top:?}, so the frame follows the row it is drawn \
             from"
        );
    }
}

#[test]
fn a_frame_of_the_panel_costs_the_same_deep_in_a_message_of_every_content_and_under_every_option() {
    for content in Content::ALL {
        for options in option_sets() {
            let mut app = reading(content, MATRIX_LINES, &options, area(ROWS));
            let mut cells = Cells::empty(area(ROWS));

            app.draw(&mut cells, area(ROWS));
            let (_, off_the_top) = counted(|| app.draw(&mut cells, area(ROWS)));
            let at_the_top = fastest(|| app.draw(&mut cells, area(ROWS)));

            scrolled(&mut app, 0, MATRIX_TOP);
            app.draw(&mut cells, area(ROWS));
            let (_, deep) = counted(|| app.draw(&mut cells, area(ROWS)));
            let taken = fastest(|| app.draw(&mut cells, area(ROWS)));

            assert_eq!(
                off_the_top, deep,
                "a frame of the panel at row {MATRIX_TOP} of a {MATRIX_LINES}-line message of \
                 {content:?} under {options:?} asked for {deep:?}, and the same frame at its top \
                 asked for {off_the_top:?}"
            );
            assert!(
                taken < at_the_top * MARGIN,
                "a frame of the panel at row {MATRIX_TOP} of a {MATRIX_LINES}-line message of \
                 {content:?} under {options:?} took {taken:?}, and the same frame at its top took \
                 {at_the_top:?}"
            );
        }
    }
}

#[test]
fn scrolling_the_panel_by_a_row_costs_the_same_deep_in_a_long_message_as_at_its_top() {
    let mut app = reading(Content::Tabbed, LONG, &Options::new(), area(ROWS));
    let mut at = 0;
    let mut measurements = Vec::new();

    for top in TOPS {
        at = scrolled(&mut app, at, top);
        let (_, count) = counted(|| app.press(area(ROWS), control('e')));
        at += 1;
        let taken = fastest(|| app.press(area(ROWS), control('e')));
        at += RUNS;
        measurements.push((top, count, taken));
    }

    let (_, off_the_top, at_the_top) = measurements[0];
    for (top, count, taken) in &measurements {
        assert_eq!(
            off_the_top, *count,
            "one `CTRL-E` at row {top} of a {LONG}-line message asked for {count:?}, and one at \
             its top asked for {off_the_top:?}, so a scroll follows where the panel stands"
        );
        assert!(
            *taken < at_the_top * MARGIN,
            "one `CTRL-E` at row {top} of a {LONG}-line message took {taken:?}, and one at its \
             top took {at_the_top:?}, so a scroll follows where the panel stands"
        );
    }
}

#[test]
fn scrolling_the_panel_up_by_a_row_costs_the_same_deep_in_a_long_message_as_near_its_top() {
    let mut app = reading(Content::Tabbed, LONG, &Options::new(), area(ROWS));
    let mut at = 0;
    let mut measurements = Vec::new();

    for top in RAISED_TOPS {
        at = scrolled(&mut app, at, top);
        let (_, count) = counted(|| app.press(area(ROWS), control('y')));
        let taken = fastest(|| app.press(area(ROWS), control('y')));
        at -= 1 + RUNS;
        measurements.push((top, count, taken));
    }

    let (near_the_top, off_the_top, at_the_top) = measurements[0];
    for (top, count, taken) in &measurements {
        assert_eq!(
            off_the_top, *count,
            "one `CTRL-Y` at row {top} of a {LONG}-line message asked for {count:?}, and one at \
             row {near_the_top} asked for {off_the_top:?}, so a scroll upward follows where the \
             panel stands"
        );
        assert!(
            *taken < at_the_top * MARGIN,
            "one `CTRL-Y` at row {top} of a {LONG}-line message took {taken:?}, and one at row \
             {near_the_top} took {at_the_top:?}, so a scroll upward follows where the panel stands"
        );
    }
}

#[test]
fn the_follow_after_a_keystroke_costs_the_same_deep_in_a_long_message_as_near_its_top() {
    let mut app = reading(Content::Tabbed, LONG, &Options::new(), area(ROWS));
    let mut at = 0;
    let mut measurements = Vec::new();

    for top in FOLLOWED_TOPS {
        at = scrolled(&mut app, at, top);
        app.press(area(ROWS), typed('0'));
        let (_, count) = counted(|| app.press(area(ROWS), typed('0')));
        let taken = fastest(|| app.press(area(ROWS), typed('0')));
        measurements.push((top, count, taken));
    }

    let (near_the_top, off_the_top, at_the_top) = measurements[0];
    for (top, count, taken) in &measurements {
        assert_eq!(
            off_the_top, *count,
            "the keystroke at row {top} of a {LONG}-line message asked for {count:?}, and the one \
             at row {near_the_top} asked for {off_the_top:?}, so the follow after it walks the \
             message rather than the rows it asked for"
        );
        assert!(
            *taken < at_the_top * MARGIN,
            "the keystroke at row {top} of a {LONG}-line message took {taken:?}, and the one at \
             row {near_the_top} took {at_the_top:?}, so the follow after it walks the message \
             rather than the rows it asked for"
        );
    }
}

#[test]
fn scrolling_the_panel_draws_the_rows_the_panel_drawn_whole_draws_at_every_width_and_option() {
    let mut compared = 0;
    for content in Content::ALL {
        for width in COMPARED_WIDTHS {
            for options in option_sets() {
                let whole = area_of(width, TALL);
                let tall = reading_over(said(content), &options, whole);
                let mut cells = Cells::empty(whole);
                tall.draw(&mut cells, whole);
                let rows = frame(&cells, whole);
                let drawn = rows
                    .iter()
                    .rposition(|row| !row.is_empty())
                    .map_or(0, |at| at + 1);

                assert!(
                    usize::from(ROWS) < drawn && drawn < usize::from(TALL),
                    "the compared transcript of {content:?} at {width} columns under {options:?} \
                     is drawn in {drawn} rows, which is not more than the window of {ROWS} holds \
                     and fewer than the {TALL} the whole of it is drawn into"
                );

                let window = area_of(width, ROWS);
                let mut app = reading_over(said(content), &options, window);
                let mut cells = Cells::empty(window);
                for top in 0..=(drawn - usize::from(ROWS)) {
                    app.draw(&mut cells, window);

                    assert_eq!(
                        rows[top..top + usize::from(ROWS)],
                        frame(&cells, window)[..],
                        "the panel of {content:?} at {width} columns under {options:?} scrolled \
                         to row {top} drew something other than those rows of the whole of it"
                    );
                    app.press(window, control('e'));
                    compared += 1;
                }
            }
        }
    }

    assert!(
        1_000 < compared,
        "only {compared} frames were compared, so the sweep is not the sweep it says it is"
    );
}

#[test]
fn scrolling_the_panel_back_up_draws_the_rows_it_drew_on_the_way_down_at_every_width_and_option() {
    let mut compared = 0;
    for content in Content::ALL {
        for width in COMPARED_WIDTHS {
            for options in option_sets() {
                let whole = area_of(width, TALL);
                let tall = reading_over(said(content), &options, whole);
                let mut cells = Cells::empty(whole);
                tall.draw(&mut cells, whole);
                let rows = frame(&cells, whole);
                let drawn = rows
                    .iter()
                    .rposition(|row| !row.is_empty())
                    .map_or(0, |at| at + 1);

                assert!(
                    usize::from(ROWS) < drawn && drawn < usize::from(TALL),
                    "the compared transcript of {content:?} at {width} columns under {options:?} \
                     is drawn in {drawn} rows, which is not more than the window of {ROWS} holds \
                     and fewer than the {TALL} the whole of it is drawn into"
                );

                let window = area_of(width, ROWS);
                let deepest = drawn - usize::from(ROWS);
                let mut app = reading_over(said(content), &options, window);
                let mut cells = Cells::empty(window);
                for _ in 0..deepest {
                    app.press(window, control('e'));
                }
                for top in (0..=deepest).rev() {
                    app.draw(&mut cells, window);

                    assert_eq!(
                        rows[top..top + usize::from(ROWS)],
                        frame(&cells, window)[..],
                        "the panel of {content:?} at {width} columns under {options:?} scrolled \
                         back up to row {top} drew something other than those rows of the whole \
                         of it"
                    );
                    app.press(window, control('y'));
                    compared += 1;
                }

                app.draw(&mut cells, window);

                assert_eq!(
                    rows[..usize::from(ROWS)],
                    frame(&cells, window)[..],
                    "a `CTRL-Y` typed at the first row of the panel of {content:?} at {width} \
                     columns under {options:?} moved it off that row"
                );
            }
        }
    }

    assert!(
        1_000 < compared,
        "only {compared} frames were compared, so the sweep is not the sweep it says it is"
    );
}

/// What the lines of a measured message are written in, which is what says whether the rows of one
/// can be read off its length.
///
/// Every line of a content is written in the same number of bytes as every other, so a frame drawn
/// deep in a message of one draws the same bytes as a frame drawn at its top and there is nothing
/// but where it is drawn from left for a measurement to follow.
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
    /// Every content the panel is measured over.
    const ALL: [Self; 5] = [
        Self::Plain,
        Self::Tabbed,
        Self::Cjk,
        Self::Emoji,
        Self::Boxes,
    ];

    /// # Returns
    ///
    /// The line numbered `number` of this content, its separator included, long enough that it
    /// wraps at the width a panel is read at.
    fn line(self, number: usize) -> String {
        match self {
            Self::Plain => format!(
                "line {number:06} of a message whose lines are long enough to wrap once over the \
                 columns it is drawn in\n"
            ),
            Self::Tabbed => format!("\t{}", Self::Plain.line(number)),
            Self::Cjk => format!("{}\n", "\u{4e2d}\u{6587}\u{5b57}\u{7b26}".repeat(11)),
            Self::Emoji => format!(
                "{}\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}\n",
                "\u{1f680}".repeat(42)
            ),
            Self::Boxes => format!(
                "{}\n",
                "\u{251c}\u{2500}\u{253c}\u{2500}\u{2524}\u{2500}".repeat(14)
            ),
        }
    }

    /// # Returns
    ///
    /// A message of `lines` lines of this content.
    fn message(self, lines: usize) -> String {
        (0..lines).map(|number| self.line(number)).collect()
    }
}

/// The allocator that records what a measurement asks for.
struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = System.alloc(layout);
        if !pointer.is_null() {
            let _ = ASKED.try_with(|asked| asked.set(asked.get() + layout.size()));
            let _ = CALLS.try_with(|calls| calls.set(calls.get() + 1));
        }

        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        System.dealloc(pointer, layout);
    }
}

/// Runs `measure` and reads off everything it asked the allocator for.
///
/// The counters are the running thread's own, so a measurement is not disturbed by whatever the
/// other tests of this binary are allocating beside it.
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

/// Runs `measure` [`RUNS`] times, which is what keeps a machine's own noise out of a ratio.
///
/// # Returns
///
/// The fastest of those runs.
fn fastest<ValueType>(mut measure: impl FnMut() -> ValueType) -> Duration {
    let mut quickest = Duration::MAX;
    for _ in 0..RUNS {
        let started = Instant::now();
        let value = measure();
        quickest = quickest.min(started.elapsed());
        black_box(&value);
    }

    quickest
}

/// Scrolls the panel of `app` from its row `from` down to its row `to`, a `CTRL-E` at a time,
/// which is the only way a reader moves it.
///
/// # Returns
///
/// The row the panel now stands at.
fn scrolled(app: &mut App, from: usize, to: usize) -> usize {
    for _ in from..to {
        app.press(area(ROWS), control('e'));
    }

    to.max(from)
}

/// # Returns
///
/// The text of every row of the frame in `cells`, top to bottom, each trimmed of the blanks the
/// frame padded it to the width with.
fn frame(cells: &Cells, area: Rect) -> Vec<String> {
    (area.y..area.bottom())
        .map(|y| {
            let drawn: String = (area.x..area.right())
                .map(|x| cells[(x, y)].symbol().to_owned())
                .collect();

            drawn.trim_end().to_owned()
        })
        .collect()
}

/// # Returns
///
/// An application whose keys the transcript panel has, over one message of `lines` lines of
/// `content` laid out in `area` under `options`.
fn reading(content: Content, lines: usize, options: &Options, area: Rect) -> App {
    let said: Transcript = [Block::new(
        Kind::Message(Role::Assistant),
        content.message(lines),
    )]
    .into_iter()
    .collect();

    reading_over(said, options, area)
}

/// Opens the transcript panel of an application over `said` and hands it the geometry of `area`,
/// which is what the first keystroke typed at a panel does.
///
/// # Returns
///
/// The application, with the panel holding the keys and standing at its first row.
///
/// # Panics
///
/// Panics if `CTRL-T` did not reach the panel.
fn reading_over(said: Transcript, options: &Options, area: Rect) -> App {
    let mut app = App::new(Buffer::from_text("a file the reader left open"))
        .with_transcript(said)
        .with_options(options.clone());
    app.press(area, control('t'));
    app.press(area, typed('0'));

    assert_eq!(Focus::Transcript, app.focus(), "`CTRL-T` reached no panel");

    app
}

/// # Returns
///
/// The transcript the drawn frames are compared over: a question, an answer wrapped over several
/// rows, a call to a tool and what it answered, both of which fold away to a summary row, and the
/// answer that followed.
fn said(content: Content) -> Transcript {
    [
        Block::new(
            Kind::Message(Role::User),
            "why does it not build".to_owned(),
        ),
        Block::new(Kind::Message(Role::Assistant), content.message(6)),
        Block::new(
            Kind::ToolCall {
                name: "Bash".to_owned(),
            },
            content.message(3),
        ),
        Block::new(Kind::ToolResult, content.message(4)),
        Block::new(Kind::Message(Role::Assistant), content.message(6)),
    ]
    .into_iter()
    .collect()
}

/// # Returns
///
/// The wrapping options a frame is measured under: vim's own defaults, then each of the options
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
/// The area a frame `rows` rows tall is drawn into, at the width a panel is read at.
fn area(rows: u16) -> Rect {
    area_of(COLUMNS, rows)
}

/// # Returns
///
/// The area a frame `rows` rows tall and `columns` columns wide is drawn into.
fn area_of(columns: u16, rows: u16) -> Rect {
    Rect::new(0, 0, columns, rows)
}

/// # Returns
///
/// The key `character` typed with no modifier.
fn typed(character: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE)
}

/// # Returns
///
/// The key `character` typed with `CTRL` held.
fn control(character: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(character), KeyModifiers::CONTROL)
}
