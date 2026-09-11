//! The line a reader types at the status line: the ex commands that discard and leave, and the
//! search that finds.
//!
//! The refusals are checked as carefully as what they let through. `:q` over a draft nothing has
//! sent must not end the program, `:q!` must discard the draft rather than end the program, and
//! `:qa` must refuse to leave a draft that `:qa!` leaves anyway. A command that quietly threw a
//! reader's words away is worse than one that never worked, and only the text left behind can tell
//! the two apart. What `:wq` sends is `chat_screen.rs`'s to check, against a session that can be
//! sent to.
//!
//! The keys typed into the line are checked for not reaching the text. `:wq` holds a `w`, which is
//! a word motion, and a `q`, which is the key that ends the program; a command line that let
//! either of them through would move the cursor or stop the editor halfway through the command
//! being typed. So the text, the cursor and the mode are all read back after a line is typed and
//! abandoned.
//!
//! The search is over a text taller than the window it is typed at, so that finding a match is
//! also a matter of the window following it. What is asserted is not only where the cursor landed
//! but that the row it landed on is drawn, because a search that scrolls nothing leaves a reader
//! looking at the same screen and no way to tell it worked.

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use modalkit::env::vim::VimMode;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;
use vbc_editor::app::{App, Outcome};
use vbc_editor::engine::typed;
use vbc_layout::buffer::Buffer;

/// The window every case is driven in, which is narrower and shorter than the fixture so that a
/// search has a window to move.
const COLUMNS: u16 = 40;
const ROWS: u16 = 6;

/// The draft every case starts from, whose lines are told apart by the words in them.
const FIXTURE: &str = "the first line\nthe second line\nthe third line";

/// The text a search is run over, which is more lines than the window draws so that the match is
/// somewhere the window has to move to.
const SEARCHED: &str = "alpha\nbeta\ngamma\ndelta\nepsilon\nzeta\neta\ntheta\niota\nkappa\nneedle \
                        in the hay\nlambda\nmu\nneedle again\nnu";

/// The word a search looks for, and the lines the fixture holds it on.
const NEEDLE: &str = "needle";
const FIRST_MATCH: usize = 10;
const SECOND_MATCH: usize = 13;

/// Validation 1: `:q` refuses a draft nothing has sent, `:q!` discards it and leaves the program
/// running, and a `:q` over the empty draft that leaves ends the program.
#[test]
fn a_quit_command_refuses_an_unsent_draft_and_a_forced_one_discards_it() {
    let mut app = holding(FIXTURE);
    typing(&mut app, "x");

    assert_eq!(
        Outcome::Continues,
        typing(&mut app, ":q\r"),
        "`:q` ended the program over a draft nothing had sent"
    );
    assert!(
        app.status().contains("not sent"),
        "`:q` said {:?} rather than why it refused",
        app.status()
    );
    assert_eq!(
        "he first line\nthe second line\nthe third line",
        app.text().text(),
        "a refused `:q` changed the draft"
    );

    assert_eq!(
        Outcome::Continues,
        typing(&mut app, ":q!\r"),
        "`:q!` ended the program rather than discarding the draft"
    );
    assert_eq!("", app.text().text(), "`:q!` kept the draft it discards");
    assert_eq!(
        VimMode::Insert,
        app.mode(),
        "`:q!` left a prompt the next message cannot be typed straight into"
    );

    app.press(area(), key(KeyCode::Esc));

    assert_eq!(
        Outcome::Stops,
        typing(&mut app, ":q\r"),
        "`:q` over an empty draft did not end the program"
    );
}

/// Validation 1: `:qa` refuses a draft nothing has sent and `:qa!` leaves anyway.
#[test]
fn a_quit_all_command_refuses_an_unsent_draft_and_a_forced_one_leaves() {
    let mut app = holding(FIXTURE);

    assert_eq!(
        Outcome::Continues,
        typing(&mut app, ":qa\r"),
        "`:qa` ended the program over a draft nothing had sent"
    );
    assert_eq!(
        FIXTURE,
        app.text().text(),
        "a refused `:qa` changed the draft"
    );
    assert_eq!(
        Outcome::Stops,
        typing(&mut app, ":qa!\r"),
        "`:qa!` did not end the program"
    );
}

/// Validation 1: `:wq` with no session to send the draft to says so and keeps the draft.
#[test]
fn a_send_command_with_nothing_to_send_to_keeps_the_draft() {
    let mut app = holding(FIXTURE);

    assert_eq!(Outcome::Continues, typing(&mut app, ":wq\r"));
    assert_eq!(
        FIXTURE,
        app.text().text(),
        "`:wq` threw away a draft it sent nowhere"
    );
    assert_eq!("there is no session to say that to", app.status());
}

/// Validation 1: the keys typed into a command line reach the line rather than the text.
#[test]
fn the_keys_typed_into_a_command_line_never_reach_the_text() {
    let mut app = holding(FIXTURE);
    let before = app.cursor();
    typing(&mut app, ":wq");

    assert_eq!(":wq", app.status(), "the line being typed is not drawn");
    assert_eq!(
        before,
        app.cursor(),
        "the `w` of `:wq` moved the cursor as a word motion"
    );

    assert_eq!(
        Outcome::Continues,
        app.press(area(), key(KeyCode::Esc)),
        "the `q` of an abandoned `:wq` ended the program"
    );
    assert_eq!(FIXTURE, app.text().text());
    assert_eq!("", app.status(), "the abandoned line is still drawn");
}

/// Validation 2: `/` finds the pattern, `n` and `N` step between the matches, and the window
/// follows so that the match is on the screen.
#[test]
fn a_search_finds_the_pattern_and_the_window_follows_it() -> Result<()> {
    let mut app = holding(SEARCHED);
    typing(&mut app, "/needle\r");

    assert_eq!(FIRST_MATCH, app.cursor().line, "`/` found another line");
    assert_eq!(0, app.cursor().grapheme);
    assert!(
        drawn(&mut app)?.iter().any(|row| row.contains(NEEDLE)),
        "the window did not follow the search to the row the match is on"
    );

    typing(&mut app, "n");

    assert_eq!(SECOND_MATCH, app.cursor().line, "`n` found another line");

    typing(&mut app, "n");

    assert_eq!(
        FIRST_MATCH,
        app.cursor().line,
        "`n` did not wrap around the end of the text"
    );

    typing(&mut app, "N");

    assert_eq!(
        SECOND_MATCH,
        app.cursor().line,
        "`N` did not step back to the match above"
    );

    Ok(())
}

/// Validation 2: a search that finds nothing says so and moves nothing, and a backward search runs
/// backwards.
#[test]
fn a_search_says_what_it_could_not_find_and_runs_the_way_it_was_started() {
    let mut app = holding(SEARCHED);
    typing(&mut app, "/haystack\r");

    assert_eq!(0, app.cursor().line, "a search that found nothing moved");
    assert!(
        app.status().contains("not found"),
        "the search said {:?} rather than that it found nothing",
        app.status()
    );

    typing(&mut app, "?needle\r");

    assert_eq!(
        SECOND_MATCH,
        app.cursor().line,
        "`?` did not wrap backwards to the last match of the text"
    );

    typing(&mut app, "n");

    assert_eq!(
        FIRST_MATCH,
        app.cursor().line,
        "`n` after a `?` did not go on searching backwards"
    );

    typing(&mut app, "N");

    assert_eq!(
        SECOND_MATCH,
        app.cursor().line,
        "`N` after a `?` did not turn the search around"
    );
}

/// # Returns
///
/// An application over a draft of `text`, with nothing typed at it yet.
fn holding(text: &str) -> App {
    App::new(Buffer::from_text(text)).with_status(true)
}

/// Types the characters of `keys` at `app`, one at a time, a carriage return standing for the
/// return key that enters a line typed at the status line.
///
/// # Returns
///
/// What the last of them left the application asking for.
fn typing(app: &mut App, keys: &str) -> Outcome {
    let mut outcome = Outcome::Continues;
    for character in keys.chars() {
        outcome = match character {
            '\r' => app.press(area(), key(KeyCode::Enter)),
            character => app.press(area(), typed(character)),
        };
    }

    outcome
}

/// # Returns
///
/// The rows a frame of `app` is drawn in, trailing blanks left off.
///
/// # Errors
///
/// Returns an error if the frame could not be drawn.
fn drawn(app: &mut App) -> Result<Vec<String>> {
    let mut terminal = Terminal::new(TestBackend::new(COLUMNS, ROWS))?;
    terminal.draw(|frame| app.render(frame))?;
    let cells = terminal.backend().buffer().clone();

    Ok((0..ROWS)
        .map(|row| {
            let drawn: String = (0..COLUMNS)
                .map(|column| cells[(column, row)].symbol().to_owned())
                .collect();

            drawn.trim_end().to_owned()
        })
        .collect())
}

/// # Returns
///
/// The area every case is driven in.
fn area() -> Rect {
    Rect::new(0, 0, COLUMNS, ROWS)
}

/// # Returns
///
/// The key event a terminal reports when `code` is typed with no modifier held.
fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}
