//! The two ways a keystroke used to take a reader's work away from them, and the bytes a write
//! puts on disk.
//!
//! The interrupt used to end the program from wherever it was typed. `q` and `:q` had always
//! refused to leave a text nothing had written and said why, and the one key that reached the
//! application ahead of everything else walked past both of them, so the fastest way to lose an
//! afternoon's editing was the key a terminal sends when a reader means "stop". What is checked
//! here is the refusal and the two cases it must not swallow: a text that was written, and a text
//! nothing ever changed.
//!
//! The second way is quieter. vim reads the key after `m`, `q`, `@`, `'` and `` ` `` as the name
//! of a mark or a register rather than as a command, and this editor keeps neither, so the name
//! fell through to normal mode and ran there: `ma` opened insert mode, and everything typed after
//! it went into the file. Nothing said so, because the notice the unbound `m` left was wiped by
//! the very keystroke it was warning about. So the cases here type the whole gesture a reader
//! would type -- `majjd'a`, not `m` on its own -- and read the file back.
//!
//! One case is not about a gesture at all but about the table. A key that takes an argument is a
//! key somebody may bind later, and binding it without the argument it takes puts the leak back
//! exactly where it was. So every key vim gives an argument to is enumerated and each is required
//! to be read one of three ways: bound together with its argument, read by the machine ahead of
//! the table as a register prefix is, or bound nowhere and therefore consumed. A binding that
//! reads the key and not its argument is a fourth way, and it fails here.
//!
//! The write is checked in bytes rather than in lines. vim writes back the file it read, and a
//! file whose last line ends in no line ending is written back without one; an editor that adds
//! one has changed a byte of somebody's file that nobody asked it to change, which no assertion
//! about the text it holds can see.

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use modalkit::env::vim::VimMode;
use ratatui::layout::Rect;
use tempfile::TempDir;
use vbc_editor::app::{App, Outcome};
use vbc_editor::engine::typed;
use vbc_editor::keys::{Argument, Bindings, ARGUMENTS};
use vbc_layout::buffer::Buffer;

/// The area every case is typed into.
const COLUMNS: u16 = 40;
const ROWS: u16 = 6;

/// The text every case is typed at, whose lines are told apart by the words in them.
const FIXTURE: &str = "the first line\nthe second line\nthe third line";

/// The name the file a write is read back off is held under.
const FILE: &str = "draft.txt";

/// What the status line says about a text nothing has written, which is vim's own wording.
const UNWRITTEN: &str = "no write since the last change (add `!` to override)";

/// The modes the application reads a key it may have to take an argument for in, which are every
/// mode but the inserting one, where a key is text.
const READ_IN: [VimMode; 2] = [VimMode::Normal, VimMode::Visual];

/// The keys vim gives an argument to that this editor implements nothing for, in either of the
/// modes it reads one in, listed in the order [`ARGUMENTS`] names them.
///
/// `m` sets a mark, `'` and `` ` `` jump to one, and `q` and `@` record and run a macro. This
/// editor keeps neither marks nor macros, and the character searches, the replace and the register
/// prefix are the argument keys it does keep.
const UNIMPLEMENTED: [char; 5] = ['m', 'q', '@', '\'', '`'];

/// Validation 1: the interrupt refuses a text nothing has written, says so, and takes nothing
/// away.
#[test]
fn the_interrupt_refuses_an_unwritten_text() {
    let mut app = holding(FIXTURE);
    typing(&mut app, "dd");

    assert_ne!(
        FIXTURE,
        app.text().text(),
        "the edit never reached the text"
    );

    let edited = app.text().text();

    assert_eq!(
        Outcome::Continues,
        app.press(area(), interrupt()),
        "the interrupt threw away a text nothing had written"
    );
    assert_eq!(Some(UNWRITTEN), app.notice());
    assert_eq!(edited, app.text().text());
    assert!(app.modified());
}

/// Validation 1: the interrupt leaves a text nothing has changed, and one a `:w` has written.
#[test]
fn the_interrupt_leaves_a_text_nothing_is_owed() -> Result<()> {
    let mut untouched = holding(FIXTURE);

    assert_eq!(Outcome::Stops, untouched.press(area(), interrupt()));

    let held = TempDir::new()?;
    let (mut written, _path) = opened(&held, &format!("{FIXTURE}\n"))?;
    typing(&mut written, "dd:w\r");

    assert!(!written.modified(), "`:w` left the text modified");
    assert_eq!(Outcome::Stops, written.press(area(), interrupt()));

    Ok(())
}

/// Validation 2: the gesture that sets a mark and deletes to it leaves the file as it was and says
/// what it could not do.
#[test]
fn a_mark_and_the_keys_that_follow_it_leave_the_text_as_it_was() {
    let mut app = holding(FIXTURE);
    typing(&mut app, "m");

    assert_eq!(
        Some("`m` takes an argument this editor does not implement"),
        app.notice()
    );

    typing(&mut app, "ajjd'a");

    assert_eq!(FIXTURE, app.text().text());
    assert_eq!(
        VimMode::Normal,
        app.mode(),
        "a key of the gesture reached normal mode and opened an inserting one"
    );
}

/// Validation 2: the keys a macro is recorded and run by take their register name with them, in
/// normal mode and over a selection.
#[test]
fn a_macro_key_and_its_register_name_leave_the_text_as_it_was() {
    for opened in ["", "v"] {
        for recorded in ['q', '@'] {
            let mut app = holding(FIXTURE);
            typing(&mut app, opened);
            app.press(area(), typed(recorded));

            assert_eq!(
                Some(
                    format!("`{recorded}` takes an argument this editor does not implement")
                        .as_str()
                ),
                app.notice(),
                "`{recorded}` said nothing about the register name it was given"
            );

            typing(&mut app, "x");

            assert_eq!(
                FIXTURE,
                app.text().text(),
                "the register name after `{opened}{recorded}` reached the text"
            );
        }
    }
}

/// Validation 3: every key vim gives an argument to is read one of the three ways this editor has
/// for reading one, and each key nothing binds takes its argument rather than passing it on.
#[test]
fn no_key_that_takes_an_argument_lets_its_argument_through() {
    let bindings = Bindings::vim();
    for mode in READ_IN {
        let read: Vec<char> = ARGUMENTS
            .into_iter()
            .filter(|character| {
                Some(Argument::Unimplemented) == bindings.argument(mode, *character)
            })
            .collect();

        assert_eq!(
            UNIMPLEMENTED.as_slice(),
            read,
            "{mode:?} reads a different set of arguments than the one this file drives"
        );

        for character in ARGUMENTS {
            assert_ne!(
                Some(Argument::Leaked),
                bindings.argument(mode, character),
                "`{character}` is bound in {mode:?} without the argument vim reads after it, so \
                 the key after it runs as a command of its own"
            );
        }
        for character in UNIMPLEMENTED {
            let mut app = holding(FIXTURE);
            typing(&mut app, if VimMode::Visual == mode { "v" } else { "" });
            app.press(area(), typed(character));

            assert_eq!(
                Some(
                    format!("`{character}` takes an argument this editor does not implement")
                        .as_str()
                ),
                app.notice()
            );

            typing(&mut app, "x");

            assert_eq!(
                FIXTURE,
                app.text().text(),
                "the argument of `{character}` reached the text in {mode:?}"
            );
        }
    }
}

/// Validation 4: a `:w` writes back the bytes it read, whether or not the file ended in a line
/// ending.
#[test]
fn a_write_keeps_the_last_line_ending_the_file_was_read_with() -> Result<()> {
    for read in [FIXTURE.to_owned(), format!("{FIXTURE}\n")] {
        let held = TempDir::new()?;
        let (mut app, path) = opened(&held, &read)?;
        typing(&mut app, ":w\r");

        assert_eq!(
            read.as_bytes(),
            std::fs::read(&path)?,
            "`:w` wrote bytes the file it read never held"
        );
        assert!(!app.modified(), "the text is still modified after `:w`");

        typing(&mut app, "x:w\r");

        assert_eq!(
            read.replacen("the", "he", 1).as_bytes(),
            std::fs::read(&path)?,
            "an edited text was written with a different last line ending than it was read with"
        );
    }

    Ok(())
}

/// Validation 5: the keys that do read an argument still read it.
#[test]
fn the_argument_keys_the_editor_implements_still_read_their_argument() {
    let mut replaced = holding(FIXTURE);
    typing(&mut replaced, "rZ");

    assert_eq!(None, replaced.notice());
    assert_eq!("Zhe first line", first(&replaced));

    let mut searched = holding(FIXTURE);
    typing(&mut searched, "dfs");

    assert_eq!("t line", first(&searched));

    let mut stopped = holding(FIXTURE);
    typing(&mut stopped, "dts");

    assert_eq!("st line", first(&stopped));

    let mut held = holding(FIXTURE);
    typing(&mut held, "\"adw\"aP");

    assert_eq!(
        FIXTURE,
        held.text().text(),
        "the register the word was yanked into is not the one it was pasted back out of"
    );
}

/// # Returns
///
/// An application over `text`, with nothing typed at it yet.
fn holding(text: &str) -> App {
    App::new(Buffer::from_text(text)).with_status(true)
}

/// # Returns
///
/// An application over a file in `held` holding exactly `read`, and the file it writes back to.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`std::fs::write`]'s return values on failure.
/// * Forwards [`App::opened`]'s return values on failure.
fn opened(held: &TempDir, read: &str) -> Result<(App, std::path::PathBuf)> {
    let path = held.path().join(FILE);
    std::fs::write(&path, read)?;
    let app = App::opened(path.clone())?.with_status(true);

    Ok((app, path))
}

/// Types the characters of `keys` at `app`, one at a time, a carriage return standing for the
/// return key that enters a line typed at the status line.
fn typing(app: &mut App, keys: &str) {
    for character in keys.chars() {
        match character {
            '\r' => app.press(area(), KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            character => app.press(area(), typed(character)),
        };
    }
}

/// # Returns
///
/// The first line of what `app` holds.
fn first(app: &App) -> String {
    app.text().lines()[0].clone()
}

/// # Returns
///
/// The key event a terminal reports for the interrupt.
fn interrupt() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
}

/// # Returns
///
/// The area every case is typed into.
fn area() -> Rect {
    Rect::new(0, 0, COLUMNS, ROWS)
}
