//! Runs the layout fuzz harness across the scroll commands, and searches for a text and a scroll
//! on which the viewport itself ends somewhere it may not.
//!
//! The harness owns the invariants a layout must keep and is already known to catch each of them
//! being broken, so scrolling a viewport and then mapping every position through it says whether a
//! scroll leaves the screen consistent with the text: an anchor pointing at a row its line does
//! not hold, or a vertical offset counted in the wrong space, shows up as a position that does not
//! round-trip.
//!
//! The harness cannot see the two things a scroll owes on its own, because it knows nothing about
//! a window's height: that the viewport never scrolls past the last row of the text, and that the
//! cursor is left on a row the window draws. Those are searched for here instead, over the same
//! shapes of text and across sequences of commands rather than single ones, since a viewport is
//! only ever as sound as the state the scroll before it left behind.

use std::cell::RefCell;
use std::num::NonZeroUsize;

use proptest::prelude::*;
use vbc_layout::anchor::{
    char_idx_at_visual_offset, visual_offset_from_anchor, VisualOffset, Wrapping,
};
use vbc_layout::invariants::{
    DisplayPosition, Document, Layout, LogicalPosition, Row, Screen, Viewport as Area,
};
use vbc_layout::line::{self, Options};
use vbc_layout::viewport::{Command, Scrolled, Viewport, Window};
use vbc_layout::width::Metrics;

#[path = "fuzz/harness.rs"]
mod harness;

use harness::{search, Seed, DEFAULT_CASES};

/// The number of seeds each search is repeated from.
const SEEDS: u64 = 4;

/// The rows a mapping is allowed to walk, which every generated case fits inside.
const MAX_ROWS: usize = 1024;

/// The window every search scrolls inside, short enough that a generated text reaches past it.
const HEIGHT: usize = 3;

/// The rows the searched windows keep beside the cursor.
const SCROLLOFF: usize = 1;

/// The scrolls the harness searches over, each one starting from the top of the text.
///
/// A single command is not enough: a viewport is scrolled from wherever the scroll before it left
/// it, so the sequences run the commands into one another and into both ends of the text.
const SCROLLS: [&[Command]; 6] = [
    &[Command::HalfPageDown],
    &[Command::PageDown, Command::PageDown, Command::HalfPageUp],
    &[Command::RowDown, Command::RowDown, Command::RowUp],
    &[Command::PageDown, Command::CenterCursor],
    &[
        Command::HalfPageDown,
        Command::CursorToTop,
        Command::RowDown,
    ],
    &[Command::PageDown, Command::CursorToBottom, Command::PageUp],
];

/// Every command a generated scroll is drawn from.
const EVERY_COMMAND: [Command; 9] = [
    Command::HalfPageDown,
    Command::HalfPageUp,
    Command::PageDown,
    Command::PageUp,
    Command::RowDown,
    Command::RowUp,
    Command::CenterCursor,
    Command::CursorToTop,
    Command::CursorToBottom,
];

/// The graphemes generated lines are drawn from, covering plain, combining, and double-width text
/// as well as the spaces a row may end on.
const ALPHABET: [&str; 5] = ["a", "b", " ", "e\u{0301}", "漢"];

/// The number of cases each search of the scroll invariants runs.
const CASES: u32 = 256;

/// The bounds a generated case is drawn from.
const MAX_LINES: usize = 4;
const MAX_LINE_LEN: usize = 24;
const MIN_WIDTH: usize = 2;
const MAX_WIDTH: usize = 8;
const MAX_HEIGHT: usize = 5;
const MAX_SCROLLOFF: usize = 3;
const MAX_COMMANDS: usize = 6;

/// A viewport that has been scrolled, put behind the trait the harness checks.
///
/// Laying the whole document out is the harness's business rather than the viewport's: the checker
/// compares every row of the screen, and only a test can afford to build one.
struct AfterScrolling {
    commands: &'static [Command],
    scrolled: RefCell<Option<(Document, Area, LogicalPosition, usize)>>,
}

impl AfterScrolling {
    /// # Returns
    ///
    /// The viewport the commands leave, scrolled from the top of the document.
    ///
    /// # Panics
    ///
    /// Panics if a scroll failed, which none of a generated case's does.
    fn viewport(&self, document: &Document, area: Area) -> Viewport {
        let window =
            Window::new(NonZeroUsize::new(HEIGHT).expect("the searched height is not zero"))
                .with_scrolloff(SCROLLOFF);
        let mut state = Scrolled {
            viewport: Viewport::new(),
            cursor: LogicalPosition {
                line: 0,
                grapheme: 0,
            },
        };
        for &command in self.commands {
            state = state
                .viewport
                .scroll(
                    document.lines(),
                    &wrapping(area),
                    window,
                    state.cursor,
                    command,
                )
                .expect("a searched scroll is taken");
        }

        state.viewport
    }

    /// # Returns
    ///
    /// * The position drawn at the start of the scrolled viewport's top row.
    /// * The row of the whole screen that row is.
    ///
    /// # Panics
    ///
    /// Panics if the viewport's top row is not drawn, which a scrolled one always is.
    fn top(&self, document: &Document, area: Area) -> (LogicalPosition, usize) {
        if let Some((cached, drawn, position, row)) = self.scrolled.borrow().as_ref() {
            if cached == document && *drawn == area {
                return (*position, *row);
            }
        }

        let viewport = self.viewport(document, area);
        let position = viewport
            .top_position(document.lines(), &wrapping(area))
            .expect("a scrolled viewport is drawn");
        let above: usize = document.lines()[..viewport.anchor()]
            .iter()
            .map(|text| rows_of(text, area).len())
            .sum();
        let row = above + viewport.vertical_offset();
        self.scrolled
            .replace(Some((document.clone(), area, position, row)));

        (position, row)
    }
}

impl Layout for AfterScrolling {
    fn lay_out(&self, document: &Document, area: Area) -> Screen {
        Screen {
            rows: document
                .lines()
                .iter()
                .enumerate()
                .flat_map(|(line, text)| {
                    rows_of(text, area).into_iter().map(move |row| Row {
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
        area: Area,
        position: LogicalPosition,
    ) -> Option<DisplayPosition> {
        let (top, origin) = self.top(document, area);
        let offset =
            visual_offset_from_anchor(document.lines(), top, position, &wrapping(area), MAX_ROWS)
                .ok()?;
        let row = signed(origin) + offset.rows;

        Some(DisplayPosition {
            row: usize::try_from(row).ok()?,
            column: offset.column,
        })
    }

    fn logical_position(
        &self,
        document: &Document,
        area: Area,
        position: DisplayPosition,
    ) -> Option<LogicalPosition> {
        let (top, origin) = self.top(document, area);
        let offset = VisualOffset {
            rows: signed(position.row) - signed(origin),
            column: position.column,
        };
        let landing =
            char_idx_at_visual_offset(document.lines(), top, offset, &wrapping(area)).ok()?;

        (offset == landing.offset).then_some(landing.position)
    }
}

/// One generated case: the text scrolled, how it is drawn, the window it is scrolled inside, and
/// the commands it is scrolled by.
#[derive(Clone, Debug)]
struct ScrollInput {
    lines: Vec<String>,
    width: NonZeroUsize,
    window: Window,
    commands: Vec<Command>,
}

#[test]
fn scrolling_leaves_a_layout_that_satisfies_every_invariant() {
    for commands in SCROLLS {
        for seed in 0..SEEDS {
            let layout = AfterScrolling {
                commands,
                scrolled: RefCell::new(None),
            };
            if let Err(failure) = search(&layout, Seed::new(seed), DEFAULT_CASES) {
                panic!("scrolling by {commands:?} broke an invariant:\n{failure}");
            }
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: CASES,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    /// Every scroll of every generated text leaves a viewport that names a row its own line holds.
    #[test]
    fn a_scrolled_viewport_names_a_row_of_the_text(input in scroll_input()) {
        let wrapping = wrapping_of(input.width);
        let mut state = Scrolled {
            viewport: Viewport::new(),
            cursor: LogicalPosition { line: 0, grapheme: 0 },
        };
        for command in input.commands {
            state = state
                .viewport
                .scroll(&input.lines, &wrapping, input.window, state.cursor, command)
                .expect("a generated scroll is taken");
            let viewport = state.viewport;

            prop_assert!(
                viewport.anchor() < input.lines.len(),
                "{command:?} anchored the viewport to line {} of a text holding {} lines",
                viewport.anchor(),
                input.lines.len()
            );
            let rows = line::lay_out(
                &input.lines[viewport.anchor()],
                input.width,
                Metrics::default(),
                &Options::new(),
            );
            prop_assert!(
                viewport.vertical_offset() < rows.len(),
                "{command:?} hid {} rows of a line drawn on {}",
                viewport.vertical_offset(),
                rows.len()
            );
        }
    }

    /// Every scroll of every generated text leaves the cursor on a row the window draws.
    #[test]
    fn a_scroll_leaves_the_cursor_inside_the_window(input in scroll_input()) {
        let wrapping = wrapping_of(input.width);
        let rows = screen_rows(&input.lines, input.width);
        let height = input.window.height().get();
        let mut state = Scrolled {
            viewport: Viewport::new(),
            cursor: LogicalPosition { line: 0, grapheme: 0 },
        };
        for command in input.commands {
            state = state
                .viewport
                .scroll(&input.lines, &wrapping, input.window, state.cursor, command)
                .expect("a generated scroll is taken");
            let top = row_of(&rows, state.viewport.anchor(), state.viewport.vertical_offset());
            let cursor = row_holding(&rows, &input.lines, state.cursor);

            prop_assert!(
                top < rows.len(),
                "{command:?} scrolled the top past the {} rows the text is drawn on",
                rows.len()
            );
            prop_assert!(
                top <= cursor && cursor < top + height,
                "{command:?} left the cursor on row {cursor} of a window drawing rows {top} to {}",
                top + height - 1
            );
        }
    }
}

/// # Returns
///
/// A strategy generating the texts, widths, windows, and scrolls a viewport is searched over.
fn scroll_input() -> impl Strategy<Value = ScrollInput> {
    let line = proptest::collection::vec(
        proptest::sample::select(ALPHABET.as_slice()),
        0..=MAX_LINE_LEN,
    )
    .prop_map(|graphemes| graphemes.concat());
    (
        proptest::collection::vec(line, 1..=MAX_LINES),
        MIN_WIDTH..=MAX_WIDTH,
        1..=MAX_HEIGHT,
        0..=MAX_SCROLLOFF,
        proptest::collection::vec(
            proptest::sample::select(EVERY_COMMAND.as_slice()),
            1..=MAX_COMMANDS,
        ),
    )
        .prop_map(|(lines, width, height, scrolloff, commands)| ScrollInput {
            lines,
            width: NonZeroUsize::new(width).expect("the generated width is at least one"),
            window: Window::new(
                NonZeroUsize::new(height).expect("the generated height is at least one"),
            )
            .with_scrolloff(scrolloff),
            commands,
        })
}

/// # Returns
///
/// The way an area's text is drawn, which is the way the invariants measure it.
fn wrapping(area: Area) -> Wrapping {
    wrapping_of(area.width)
}

/// # Returns
///
/// The way text is drawn in `width` columns, under vim's own defaults.
fn wrapping_of(width: NonZeroUsize) -> Wrapping {
    Wrapping::new(width, Metrics::default(), Options::new())
}

/// # Returns
///
/// The rows rendering `line` in `area`.
fn rows_of(line: &str, area: Area) -> Vec<line::DisplayRow> {
    line::lay_out(line, area.width, Metrics::default(), &Options::new())
}

/// # Returns
///
/// The line and the grapheme each row of the whole text starts at, top to bottom.
fn screen_rows(lines: &[String], width: NonZeroUsize) -> Vec<(usize, usize)> {
    lines
        .iter()
        .enumerate()
        .flat_map(|(line, text)| {
            line::lay_out(text, width, Metrics::default(), &Options::new())
                .into_iter()
                .map(move |row| (line, row.start()))
        })
        .collect()
}

/// # Returns
///
/// The row of the whole screen that is the row at `offset` of the line at `line`.
///
/// # Panics
///
/// Panics if the text draws no such row.
fn row_of(rows: &[(usize, usize)], line: usize, offset: usize) -> usize {
    rows.iter()
        .position(|&(index, _)| index == line)
        .expect("the text draws the anchored line")
        + offset
}

/// # Returns
///
/// The row of the whole screen that draws `position`, which is its line's last row for a position
/// past the end of that line.
///
/// # Panics
///
/// Panics if the text draws no row of the position's line.
fn row_holding(rows: &[(usize, usize)], lines: &[String], position: LogicalPosition) -> usize {
    rows.iter()
        .enumerate()
        .filter(|(_, &(line, start))| line == position.line && start <= position.grapheme)
        .map(|(row, _)| row)
        .next_back()
        .unwrap_or_else(|| panic!("the text of {} lines draws {position}", lines.len()))
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
