//! The keys typed at the application, from a terminal event to the text they edit.
//!
//! Every piece below has been checked on its own before now: the engine runs vim's operators, the
//! table binds the keys that ask for them, the shim measures the display motions, the viewport
//! scrolls and the frame draws. What was never checked is that a key typed at the program reaches
//! any of it, and the only thing that can check it is the application: what is typed here is a
//! [`KeyEvent`] of the kind the terminal reader delivers, and what is read back is the text, the
//! cursor, the mode and the frame the program would show.
//!
//! The edits are the ones vim's own manual names, and each case asserts the text vim leaves rather
//! than a shape of it: `dw`, `x`, `dd`, `3dd`, `>>` and `gUgj`. That the engine behind them agrees
//! with a real vim is the differential harness's business and is checked in `vim_engine.rs`,
//! `indent_operators.rs` and `operator_display_motions.rs`; what is checked here is that a
//! keystroke arrives at it at all.
//!
//! The display motion is the case that says which window the keys are being typed at. `gj` walks
//! down a display row, and a display row is only a display row relative to a width, so the same
//! key at the same text is typed into two areas here: a narrow one, where it stops halfway along
//! the line, and a wide one, where the line does not wrap and it lands where a plain `j` would. An
//! application that handed the engine a window of its own choosing would answer both the same way.
//!
//! The scrolls are the keys the program had before it had an engine, and they are checked for
//! going through the same dispatch rather than beside it. A second, parallel key reader is exactly
//! what would still scroll on the `CTRL-D` that follows a `d`, or on the one typed in insert mode,
//! because a reader beside the engine cannot know that a sequence is in the middle of being typed.

use std::num::NonZeroUsize;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use modalkit::env::vim::VimMode;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;
use vbc_editor::app::{App, Outcome};
use vbc_editor::screen::Screen;
use vbc_layout::buffer::Buffer;
use vbc_layout::position::LogicalPosition;
use vbc_layout::width::{AmbiWidth, Metrics};

/// One edit: the keys typed, the columns the terminal they are typed at is wide, and the text they
/// leave behind.
struct Case {
    keys: &'static str,
    columns: u16,
    text: &'static str,
}

/// The prose the edits are typed at, which is more lines than a count reaches and long enough to
/// wrap in the narrow terminal.
const PROSE: &str = "the quick brown fox\njumps over it\nand lands again\nthen rests";

/// The lines the scrolls and the window's own following are typed at, which is more lines than the
/// window has rows.
const TALL: &str = "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\nten";

/// The prose whose cursor is counted differently in bytes and in graphemes: every accented
/// character is two bytes wide and one cell wide, so a cursor written down as a byte offset and
/// read as a grapheme offset lands on the wrong character without either count leaving the line.
const ACCENTED: &str = "h\u{e9}llo w\u{f6}rld";

/// The line a shift is measured against, whose indent is four columns of blanks already.
const INDENTED: &str = "    x";

/// The terminal the edits are typed at, wide enough that [`PROSE`] does not wrap in it.
const WIDE: u16 = 40;

/// The terminal the display motions are typed at: four columns of gutter and ten of text, so that
/// the first line of [`PROSE`] wraps after `the quick `.
const NARROW: u16 = 14;

/// The rows every terminal here holds, which is fewer than [`TALL`] has lines.
const ROWS: u16 = 5;

/// The lines the window stranded by an edit is typed at, which is several windows' worth so that a
/// window resting where the edit left it is a window nowhere near the row the cursor was left on.
const LINES: usize = 30;

/// The grapheme of the first line of [`PROSE`] that its second display row begins at, in a
/// terminal [`NARROW`] columns wide.
const SECOND_ROW: usize = 10;

/// The edits, each of them the text a real vim leaves behind the same keys.
const EDITS: [Case; 6] = [
    Case {
        keys: "dw",
        columns: WIDE,
        text: "quick brown fox\njumps over it\nand lands again\nthen rests",
    },
    Case {
        keys: "x",
        columns: WIDE,
        text: "he quick brown fox\njumps over it\nand lands again\nthen rests",
    },
    Case {
        keys: "dd",
        columns: WIDE,
        text: "jumps over it\nand lands again\nthen rests",
    },
    Case {
        keys: "3dd",
        columns: WIDE,
        text: "then rests",
    },
    Case {
        keys: ">>",
        columns: WIDE,
        text: "\tthe quick brown fox\njumps over it\nand lands again\nthen rests",
    },
    Case {
        keys: "gUgj",
        columns: NARROW,
        text: "THE QUICK brown fox\njumps over it\nand lands again\nthen rests",
    },
];

/// Validation 1: a key sequence typed at the program edits the text it draws, which is the whole
/// of what the binary could not do before it had an engine.
#[test]
fn typing_at_the_application_edits_the_text_as_vim_does() {
    for case in EDITS {
        let mut app = App::new(Buffer::from_text(PROSE));
        let area = area(case.columns);
        for key in typed(case.keys) {
            app.press(area, key);
        }

        assert_eq!(
            case.text,
            app.text().text(),
            "`{}` left the text it did not leave in vim",
            case.keys
        );
        assert_eq!(
            None,
            app.notice(),
            "`{}` was typed at the program and refused",
            case.keys
        );
    }
}

/// Validation 2: `gj` walks down a display row of the window the program is drawing, rather than
/// down a logical line or down a row of some window the engine chose for itself.
#[test]
fn a_display_motion_walks_the_rows_of_the_terminal_it_was_typed_at() {
    let mut wrapped = App::new(Buffer::from_text(PROSE));
    let mut unwrapped = App::new(Buffer::from_text(PROSE));
    let mut lines = App::new(Buffer::from_text(PROSE));
    for key in typed("gj") {
        wrapped.press(area(NARROW), key);
        unwrapped.press(area(WIDE), key);
    }
    for key in typed("j") {
        lines.press(area(NARROW), key);
    }

    assert_eq!(
        LogicalPosition {
            line: 0,
            grapheme: SECOND_ROW
        },
        wrapped.cursor(),
        "`gj` left the first line, so it was measured in a window this terminal is not"
    );
    assert_eq!(
        LogicalPosition {
            line: 1,
            grapheme: 0
        },
        unwrapped.cursor(),
        "`gj` walked a row of a line that does not wrap in this terminal"
    );
    assert_eq!(
        LogicalPosition {
            line: 1,
            grapheme: 0
        },
        lines.cursor(),
        "`j` is the motion `gj` is worth telling apart from"
    );
}

/// The cursor the engine counts in bytes is drawn at the grapheme those bytes fall in, which a
/// text of one-byte characters cannot tell from a cursor nothing converted at all.
#[test]
fn a_cursor_the_engine_counts_in_bytes_is_read_back_in_graphemes() {
    let area = area(WIDE);
    let mut app = App::new(Buffer::from_text(ACCENTED));
    for key in typed("w") {
        app.press(area, key);
    }

    assert_eq!(
        LogicalPosition {
            line: 0,
            grapheme: 6
        },
        app.cursor(),
        "`w` stopped a byte past the grapheme it landed on"
    );

    app.press(area, KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));

    assert_eq!("h\u{e9}llo \u{f6}rld", app.text().text());
}

/// A shift is laid down in the blanks the metrics the application measures its text by ask for,
/// rather than in the ones the engine would have chosen for a window it was never told about.
///
/// vim's `'shiftwidth'` carries the line eight columns either way; what the tab stop decides is
/// what those columns are written in, so the same twelve-column indent is three tabs against a
/// stop of four and one tab and four spaces against a stop of eight.
#[test]
fn a_shift_is_written_in_the_blanks_the_metrics_ask_for() {
    let area = area(WIDE);
    let mut narrow = App::new(Buffer::from_text(INDENTED)).with_metrics(Metrics::new(
        AmbiWidth::default(),
        NonZeroUsize::new(4).expect("four is not zero"),
    ));
    let mut default = App::new(Buffer::from_text(INDENTED));
    for key in typed(">>") {
        narrow.press(area, key);
        default.press(area, key);
    }

    assert_eq!(
        "\t\t\tx",
        narrow.text().text(),
        "the shift was laid down against a tab stop this application does not measure by"
    );
    assert_eq!("\t    x", default.text().text());
}

/// Validation 3: the mode the keys left the editor in is readable from outside it and is drawn
/// where a reader of the screen can see it.
#[test]
fn the_mode_is_readable_and_drawn() -> Result<()> {
    let mut app = App::new(Buffer::from_text(TALL)).with_status(true);
    let area = area(WIDE);

    assert_eq!(VimMode::Normal, app.mode());
    assert_eq!("", app.status());
    assert_eq!("", status(&app, area)?.trim());

    for key in typed("i") {
        app.press(area, key);
    }

    assert_eq!(VimMode::Insert, app.mode());
    assert_eq!("-- INSERT --", app.status());
    assert_eq!("-- INSERT --", status(&app, area)?.trim());

    app.press(area, KeyEvent::from(KeyCode::Esc));

    assert_eq!(VimMode::Normal, app.mode());
    assert_eq!("", status(&app, area)?.trim());

    for key in typed("v") {
        app.press(area, key);
    }

    assert_eq!(VimMode::Visual, app.mode());
    assert_eq!("-- VISUAL --", app.status());
    assert_eq!("-- VISUAL --", status(&app, area)?.trim());

    Ok(())
}

/// Validation 4: the scrolls the program had before it had an engine still scroll, and they reach
/// the window through the dispatch the edits go through rather than beside it.
///
/// A scroll carries the cursor, so the keystroke after one is asserted to edit the line the window
/// left the cursor on: an engine that was not told where the scroll put the cursor would edit the
/// line it was on before.
#[test]
fn the_scroll_keys_reach_the_window_through_the_dispatch_the_edits_go_through() {
    let area = area(WIDE);
    let mut app = App::new(Buffer::from_text(TALL));
    app.press(area, control('d'));

    assert_eq!(2, app.viewport().anchor(), "`CTRL-D` scrolled nothing");
    assert_eq!(
        LogicalPosition {
            line: 2,
            grapheme: 0
        },
        app.cursor()
    );

    app.press(area, KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));

    assert_eq!(
        "one\ntwo\nhree\nfour\nfive\nsix\nseven\neight\nnine\nten",
        app.text().text(),
        "the key after the scroll edited the line the cursor was on before it"
    );
}

/// Validation 4: the same key is the engine's wherever a sequence is in the middle of being typed,
/// which is what a key reader running beside the engine could not know.
#[test]
fn a_scroll_key_that_a_sequence_is_waiting_on_never_reaches_the_window() {
    let area = area(WIDE);
    let mut pending = App::new(Buffer::from_text(TALL));
    pending.press(area, KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
    pending.press(area, control('d'));

    assert_eq!(
        0,
        pending.viewport().anchor(),
        "`CTRL-D` scrolled the window while an operator was waiting for its motion"
    );
    assert_eq!(TALL, pending.text().text());
    assert_eq!(Some("`d<C-D>` is bound to nothing"), pending.notice());

    let mut inserting = App::new(Buffer::from_text(TALL));
    inserting.press(area, KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));
    inserting.press(area, control('d'));

    assert_eq!(
        0,
        inserting.viewport().anchor(),
        "`CTRL-D` scrolled the window while insert mode was reading text"
    );
    assert_eq!(VimMode::Insert, inserting.mode());
    assert_eq!(Some("`<C-D>` is bound to nothing"), inserting.notice());
}

/// Validation 5: a key nothing binds does nothing and says so, and a key something binds says
/// nothing however little it did.
#[test]
fn a_key_bound_to_nothing_says_so_and_a_bound_one_does_not() {
    let area = area(WIDE);
    let mut app = App::new(Buffer::from_text(TALL));
    let outcome = app.press(area, KeyEvent::new(KeyCode::Char('Z'), KeyModifiers::NONE));

    assert_eq!(Outcome::Continues, outcome);
    assert_eq!(TALL, app.text().text());
    assert_eq!(
        LogicalPosition {
            line: 0,
            grapheme: 0
        },
        app.cursor()
    );
    assert_eq!(Some("`Z` is bound to nothing"), app.notice());

    app.press(area, KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));

    assert_eq!(
        None,
        app.notice(),
        "`k` is bound, and a bound key that had nowhere to go is not a key nothing answered"
    );
}

/// The window follows the cursor the keys move, so a motion that walks off the bottom of the
/// window draws the row it landed on rather than leaving the cursor somewhere the frame does not
/// show.
#[test]
fn the_window_follows_the_cursor_off_its_own_edges() -> Result<()> {
    let area = area(WIDE);
    let mut app = App::new(Buffer::from_text(TALL));
    for key in typed("jjjjjjj") {
        app.press(area, key);
    }

    assert_eq!(
        LogicalPosition {
            line: 7,
            grapheme: 0
        },
        app.cursor()
    );
    assert_eq!(3, app.viewport().anchor());
    assert_eq!(
        Some(usize::from(ROWS) - 1),
        drawn_row(&app, area)?,
        "the cursor walked off the bottom of the window and the window did not follow"
    );

    for key in typed("kkkkkkk") {
        app.press(area, key);
    }

    assert_eq!(
        LogicalPosition {
            line: 0,
            grapheme: 0
        },
        app.cursor()
    );
    assert_eq!(0, app.viewport().anchor());
    assert_eq!(Some(0), drawn_row(&app, area)?);

    Ok(())
}

/// A window an edit left anchored past the end of the text comes back to the cursor rather than
/// resting on the one row the anchor can still be clamped onto.
///
/// A screen draws the last line of its text for a viewport anchored past it, so an anchor an edit
/// stranded is an anchor whose clamp can land on the cursor's own line -- and a window that only
/// asks whether the cursor is drawn sees no reason to move. What it would leave is a frame holding
/// one line at the top and blanks under it, and a viewport still naming a line the text no longer
/// holds for the next scroll to count from.
#[test]
fn a_window_stranded_past_the_end_of_a_shortened_text_comes_back_to_the_cursor() -> Result<()> {
    let window = usize::from(ROWS);
    let area = area(WIDE);
    let mut app = App::new(Buffer::from_text(&numbered(LINES)));
    for key in typed("Gkkkk") {
        app.press(area, key);
    }

    assert_eq!(
        LINES - window,
        app.viewport().anchor(),
        "the window is not resting on the rows the edit is about to take away"
    );

    for key in typed("dG") {
        app.press(area, key);
    }

    assert_eq!(LINES - window, app.text().line_count());
    assert_eq!(LINES - window - 1, app.cursor().line);
    assert_eq!(
        LINES - window - window,
        app.viewport().anchor(),
        "the window stayed where the edit stranded it, past the end of the text"
    );
    assert_eq!(Some(window - 1), drawn_row(&app, area)?);

    Ok(())
}

/// The program stops on the key it has always stopped on, and on the interrupt from a mode where
/// that key is text rather than a command.
#[test]
fn the_program_stops_on_its_own_key_and_on_the_interrupt() {
    let area = area(WIDE);
    let mut app = App::new(Buffer::from_text(TALL));

    assert_eq!(
        Outcome::Stops,
        app.press(area, KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE))
    );

    let mut inserting = App::new(Buffer::from_text(TALL));
    inserting.press(area, KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));

    assert_eq!(
        Outcome::Continues,
        inserting.press(area, KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)),
        "`q` is text in insert mode rather than the key that ends the program"
    );
    assert_eq!("qone\ntwo\nthree", inserting.text().lines()[..3].join("\n"));
    assert_eq!(Outcome::Stops, inserting.press(area, control('c')));
}

/// # Returns
///
/// The area a case is typed into, which is `columns` columns of a terminal [`ROWS`] rows tall.
fn area(columns: u16) -> Rect {
    Rect::new(0, 0, columns, ROWS)
}

/// # Returns
///
/// A text of `count` lines, each of them naming its own number so that a window drawing it says
/// which lines it drew.
fn numbered(count: usize) -> String {
    (1..=count)
        .map(|line| format!("line {line}"))
        .collect::<Vec<String>>()
        .join("\n")
}

/// # Returns
///
/// The key events a terminal reports when `keys` is typed at it, one per character.
fn typed(keys: &str) -> Vec<KeyEvent> {
    keys.chars()
        .map(|character| KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE))
        .collect()
}

/// # Returns
///
/// The key event a terminal reports when `character` is typed with control held.
fn control(character: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(character), KeyModifiers::CONTROL)
}

/// Draws a frame of an application and reads its status line back off the terminal.
///
/// # Returns
///
/// What the bottom row of the frame says on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`ratatui::Terminal::new`]'s return values on failure.
/// * Forwards [`ratatui::Terminal::draw`]'s return values on failure.
fn status(app: &App, area: Rect) -> Result<String> {
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height))?;
    terminal.draw(|frame| app.render(frame))?;
    let cells = terminal.backend().buffer();

    Ok((0..area.width)
        .map(|x| cells[(x, area.height - 1)].symbol())
        .collect())
}

/// # Returns
///
/// The row of the window the cursor is drawn on, and [`None`] where the window does not draw the
/// cursor's own row, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`anyhow::Error`] if the area is too small to draw a row of text in.
fn drawn_row(app: &App, area: Rect) -> Result<Option<usize>> {
    let geometry = app
        .geometry(area)
        .ok_or_else(|| anyhow::anyhow!("the area draws no text"))?;

    Ok(Screen::of(app.text(), &app.viewport(), app.cursor(), &geometry).cursor_row())
}
