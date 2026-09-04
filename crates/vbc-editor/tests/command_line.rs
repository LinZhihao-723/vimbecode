//! The line a reader types at the status line: the ex commands that write and leave, and the
//! search that finds.
//!
//! Everything the editor could do before this file was written was done to a text nobody could
//! keep. The binary read a file, let a reader edit it and ended on `q`, and the edit went nowhere:
//! there was no `fs::write` anywhere in the program. So the first thing checked here is the bytes
//! on disk, read back from a real file after keys were typed at a real application, because a save
//! asserted as "the editor thinks it saved" is the assertion that would have passed all along.
//!
//! The refusals are checked as carefully as the writes. `:q` over a text nothing has written must
//! not end the program, `:q!` must end it anyway and must leave the file as it was, and `:wq` must
//! write before it leaves. A `:q` that quietly discarded an edit is worse than a `:q` that never
//! worked, and only the file on disk can tell the two apart.
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

use std::path::PathBuf;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;
use tempfile::TempDir;
use vbc_editor::app::{App, Outcome};
use vbc_editor::engine::typed;
use vbc_layout::buffer::Buffer;

/// The window every case is driven in, which is narrower and shorter than the fixture so that a
/// search has a window to move.
const COLUMNS: u16 = 40;
const ROWS: u16 = 6;

/// The file every case starts from, whose lines are told apart by the words in them.
const FIXTURE: &str = "the first line\nthe second line\nthe third line";

/// The name the file is written under.
const FILE: &str = "draft.txt";

/// The text a search is run over, which is more lines than the window draws so that the match is
/// somewhere the window has to move to.
const SEARCHED: &str = "alpha\nbeta\ngamma\ndelta\nepsilon\nzeta\neta\ntheta\niota\nkappa\nneedle \
                        in the hay\nlambda\nmu\nneedle again\nnu";

/// The word a search looks for, and the lines the fixture holds it on.
const NEEDLE: &str = "needle";
const FIRST_MATCH: usize = 10;
const SECOND_MATCH: usize = 13;

/// Validation 1: `:w` puts the edited bytes on disk.
#[test]
fn a_write_command_puts_the_edited_bytes_on_disk() -> Result<()> {
    let held = TempDir::new()?;
    let (mut app, path) = opened(&held)?;
    typing(&mut app, "x");
    typing(&mut app, ":w\r");

    assert_eq!(
        "he first line\nthe second line\nthe third line\n",
        std::fs::read_to_string(&path)?,
        "`:w` wrote something other than the text the keystrokes left"
    );
    assert!(!app.modified(), "the text is still modified after `:w`");

    Ok(())
}

/// Validation 1: `:q` refuses a text nothing has written, `:q!` discards it, and neither touches
/// the file.
#[test]
fn a_quit_command_refuses_an_unwritten_text_and_a_forced_one_discards_it() -> Result<()> {
    let held = TempDir::new()?;
    let (mut app, path) = opened(&held)?;
    typing(&mut app, "x");

    assert_eq!(
        Outcome::Continues,
        typing(&mut app, ":q\r"),
        "`:q` ended the program over a text nothing had written"
    );
    assert!(
        app.status().contains("no write"),
        "`:q` said {:?} rather than why it refused",
        app.status()
    );
    assert_eq!(
        Outcome::Stops,
        typing(&mut app, ":q!\r"),
        "`:q!` did not end the program"
    );
    assert_eq!(
        format!("{FIXTURE}\n"),
        std::fs::read_to_string(&path)?,
        "a refused and then forced quit wrote the file anyway"
    );

    Ok(())
}

/// Validation 1: `:wq` writes and leaves, and a `:q` after a `:w` leaves without complaining.
#[test]
fn a_write_and_quit_command_writes_and_leaves() -> Result<()> {
    let held = TempDir::new()?;
    let (mut app, path) = opened(&held)?;
    typing(&mut app, "x");

    assert_eq!(
        Outcome::Stops,
        typing(&mut app, ":wq\r"),
        "`:wq` did not end the program"
    );
    assert_eq!(
        "he first line\nthe second line\nthe third line\n",
        std::fs::read_to_string(&path)?,
        "`:wq` did not write what the keystrokes left"
    );

    let (mut written, _) = opened(&held)?;
    typing(&mut written, "x");
    typing(&mut written, ":w\r");

    assert_eq!(
        Outcome::Stops,
        typing(&mut written, ":q\r"),
        "`:q` refused a text that had just been written"
    );

    Ok(())
}

/// Validation 1: a `:w` that was asked to change nothing writes back the bytes that were read,
/// empty last lines and all.
///
/// The read and the write are one round trip and only a round trip can check them. A read that
/// took every trailing line ending off what it read and a write that put one back each read a
/// file the other could not write: `one\n\n\n` came back as `one\n`, and two lines of somebody's
/// file went missing on the `:w` of a session that typed nothing.
#[test]
fn a_written_file_keeps_the_empty_lines_it_was_read_with() -> Result<()> {
    let held = TempDir::new()?;
    for (index, original) in ["one\ntwo\n", "one\n\n\n", "one\n\ntwo\n\n\n", "\n"]
        .into_iter()
        .enumerate()
    {
        let path = held.path().join(format!("kept{index}.txt"));
        std::fs::write(&path, original)?;
        let mut app = App::opened(path.clone())?.with_status(true);

        assert!(
            !app.modified(),
            "a file that was read and not touched is reported as modified"
        );

        typing(&mut app, ":w\r");

        assert_eq!(
            original,
            std::fs::read_to_string(&path)?,
            "`:w` over a text nothing had changed rewrote the file"
        );
    }

    Ok(())
}

/// Validation 1: an application with no file to write to says so rather than writing somewhere of
/// its own choosing, and `:w` naming a file writes there.
#[test]
fn a_write_command_names_the_file_it_writes_where_the_editor_was_given_none() -> Result<()> {
    let held = TempDir::new()?;
    let mut app = App::new(Buffer::from_text(FIXTURE)).with_status(true);
    typing(&mut app, ":w\r");

    assert_eq!(
        "no file name",
        app.status(),
        "`:w` over an unnamed text said {:?}",
        app.status()
    );

    let elsewhere = held.path().join("elsewhere.txt");
    typing(&mut app, &format!(":w {}\r", elsewhere.display()));

    assert_eq!(format!("{FIXTURE}\n"), std::fs::read_to_string(&elsewhere)?);

    Ok(())
}

/// Validation 1: the keys typed into a command line reach the line rather than the text.
#[test]
fn the_keys_typed_into_a_command_line_never_reach_the_text() -> Result<()> {
    let held = TempDir::new()?;
    let (mut app, _) = opened(&held)?;
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
    assert!(!app.modified());

    Ok(())
}

/// Validation 2: `/` finds the pattern, `n` and `N` step between the matches, and the window
/// follows so that the match is on the screen.
#[test]
fn a_search_finds_the_pattern_and_the_window_follows_it() -> Result<()> {
    let mut app = App::new(Buffer::from_text(SEARCHED)).with_status(true);
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
    let mut app = App::new(Buffer::from_text(SEARCHED)).with_status(true);
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
/// An application over a copy of [`FIXTURE`] written into `held`, and the file it writes to.
///
/// # Errors
///
/// Returns an error if the fixture could not be written or read back.
fn opened(held: &TempDir) -> Result<(App, PathBuf)> {
    let path = held.path().join(FILE);
    std::fs::write(&path, format!("{FIXTURE}\n"))?;
    let app = App::opened(path.clone())?.with_status(true);

    Ok((app, path))
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
