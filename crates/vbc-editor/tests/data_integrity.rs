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
//! of a mark or a register, and the key after `Z`, `z`, `[`, `]` and `CTRL-W` as the rest of a
//! command, rather than as a command in its own right. This editor keeps no marks, no macros, no
//! folds and no windows, so that further key fell through to normal mode and ran there: `ma` and
//! `za` alike opened insert mode, and everything typed after went into the file. Nothing said so,
//! because the notice the unbound `m` left was wiped by the very keystroke it was warning about.
//! So the cases here type the whole gesture a reader would type -- `majjd'a`, not `m` on its own
//! -- and read the file back.
//!
//! One case is not about a gesture at all but about the table. A key vim reads a further key
//! after is a key somebody may bind later, and binding it without that further key puts the leak
//! back exactly where it was. So every such key is enumerated and each is required to be read one
//! of three ways: bound only as the beginning of a longer sequence, read by the machine ahead of
//! the table as a register prefix is, or bound nowhere and therefore consumed. A binding that
//! answers the key on its own is a fourth way, and it fails here.
//!
//! The write is checked in bytes rather than in lines. vim writes back the file it read, and a
//! file whose last line ends in no line ending is written back without one; an editor that adds
//! one has changed a byte of somebody's file that nobody asked it to change, which no assertion
//! about the text it holds can see.

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use modalkit::env::vim::VimMode;
use modalkit::key::TerminalKey;
use ratatui::layout::Rect;
use tempfile::TempDir;
use vbc_editor::app::{App, Outcome};
use vbc_editor::engine::typed;
use vbc_editor::keys::{named, Argument, Bindings, ARGUMENTS};
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

/// The modes the application reads a key it may have to take a further key for in, which are
/// every mode but the inserting one, where a key is text.
const READ_IN: [VimMode; 2] = [VimMode::Normal, VimMode::Visual];

/// The keys vim reads a further key after that this editor implements nothing for, in either of
/// the modes it reads one in, listed in the order [`ARGUMENTS`] names them.
///
/// `m` sets a mark, `'` and `` ` `` jump to one, and `q` and `@` record and run a macro; this
/// editor keeps neither marks nor macros. `Z` writes and leaves, `z` folds, `[` and `]` jump by
/// structure and `CTRL-W` moves between windows; this editor has no windows, folds no file and
/// leaves by the ex commands alone. The character searches, the replace and the register prefix
/// are the keys of the list it does implement.
const UNIMPLEMENTED: [&str; 10] = ["m", "q", "@", "'", "`", "Z", "z", "[", "]", "<C-W>"];

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
        Some("`m` takes a key after it that this editor does not implement"),
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
                Some(unimplemented(&recorded.to_string()).as_str()),
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

/// Validation 3: every key vim reads a further key after is read one of the three ways this
/// editor has for reading one, and each key nothing binds takes that further key rather than
/// passing it on.
#[test]
fn no_key_that_takes_an_argument_lets_its_argument_through() {
    let bindings = Bindings::vim();
    for mode in READ_IN {
        let read: Vec<&str> = ARGUMENTS
            .into_iter()
            .filter(|spelled| {
                Some(Argument::Unimplemented) == bindings.argument(mode, named(spelled))
            })
            .collect();

        assert_eq!(
            UNIMPLEMENTED.as_slice(),
            read,
            "{mode:?} reads a different set of arguments than the one this file drives"
        );

        for spelled in ARGUMENTS {
            assert_ne!(
                Some(Argument::Leaked),
                bindings.argument(mode, named(spelled)),
                "`{spelled}` is bound in {mode:?} without the key vim reads after it, so that key \
                 runs as a command of its own"
            );
        }
        for spelled in UNIMPLEMENTED {
            let mut app = holding(FIXTURE);
            typing(&mut app, if VimMode::Visual == mode { "v" } else { "" });
            app.press(area(), pressed(spelled));

            assert_eq!(
                Some(unimplemented(spelled).as_str()),
                app.notice(),
                "`{spelled}` said nothing about the key it was given in {mode:?}"
            );

            typing(&mut app, "x");

            assert_eq!(
                FIXTURE,
                app.text().text(),
                "the key after `{spelled}` reached the text in {mode:?}"
            );
            assert_eq!(
                Some(unimplemented(spelled).as_str()),
                app.notice(),
                "the notice `{spelled}` left was wiped by the very key it was warning about"
            );
        }
    }
}

/// Validation 3: the gestures a reader types at a fold, a window and vim's own way out leave the
/// file alone, which is the half of the leak that is spelled with a prefix rather than a name.
#[test]
fn a_prefix_command_and_the_key_that_completes_it_leave_the_text_as_it_was() {
    for keys in ["za", "zo", "zR", "ZZ", "[p", "]s"] {
        let mut app = holding(FIXTURE);
        typing(&mut app, keys);

        assert_eq!(
            FIXTURE,
            app.text().text(),
            "`{keys}` changed a file this editor folds, splits and leaves by other keys"
        );
        assert_eq!(
            VimMode::Normal,
            app.mode(),
            "a key of `{keys}` reached normal mode and opened an inserting one"
        );
    }

    let mut window = holding(FIXTURE);
    window.press(
        area(),
        KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL),
    );
    typing(&mut window, "s");

    assert_eq!(FIXTURE, window.text().text(), "`<C-W>s` split a line");
    assert_eq!(VimMode::Normal, window.mode());
}

/// Validation 3: an inserting mode reads these keys as the text they are there rather than taking
/// the key behind them, which is the one mode the application takes nothing in.
///
/// `CTRL-R` is the divergence this leaves standing. vim pastes a register by it in insert mode and
/// this editor binds it nowhere, so the register name behind it is typed into the file as the
/// character it is. That is a key the reader typed landing where they typed it rather than a
/// command running unasked, so it is stated here rather than taken.
#[test]
fn an_inserting_mode_reads_the_keys_that_take_one_as_the_text_they_are() {
    for spelled in UNIMPLEMENTED {
        let Some(character) = one(spelled) else {
            continue;
        };
        let mut app = holding(FIXTURE);
        typing(&mut app, "i");
        app.press(area(), pressed(spelled));
        typing(&mut app, "x");

        assert_eq!(
            format!("{character}xthe first line"),
            first(&app),
            "`{spelled}` and the key after it are not the text an inserting mode reads them as"
        );
    }
}

/// Validation 3: the interrupt abandons a key waiting to be taken, so that the keystroke after a
/// refusal is the reader's own rather than one the abandoned command eats.
#[test]
fn the_interrupt_abandons_a_key_that_was_waiting_to_be_taken() {
    let mut app = holding(FIXTURE);
    typing(&mut app, "dd");

    let edited = app.text().text();
    app.press(area(), typed('m'));

    assert_eq!(Outcome::Continues, app.press(area(), interrupt()));
    assert_eq!(Some(UNWRITTEN), app.notice());

    typing(&mut app, "x");

    assert_ne!(
        edited,
        app.text().text(),
        "the keystroke after the refusal was eaten by the `m` the interrupt abandoned"
    );
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
/// The key event a terminal reports when the key `spelled` names is typed.
///
/// What the event is built from is checked against the key the table reads that spelling as, so a
/// spelling this file types the wrong keystroke for is a failure here rather than a case that
/// quietly drives something else.
///
/// # Panics
///
/// Panics if `spelled` names no key, or names one this function builds a different key event for.
fn pressed(spelled: &str) -> KeyEvent {
    let held = spelled
        .strip_prefix("<C-")
        .and_then(|held| held.strip_suffix('>'));
    let event = match held {
        Some(held) => {
            let character = held
                .chars()
                .next()
                .expect("a control key names a character");

            KeyEvent::new(KeyCode::Char(character), KeyModifiers::CONTROL)
        }
        None => typed(spelled.chars().next().expect("a key names a character")),
    };

    assert_eq!(
        named(spelled),
        TerminalKey::from(event),
        "the key event this file types for `{spelled}` is not the key the table reads it as"
    );

    event
}

/// # Returns
///
/// The one character `spelled` is written as, and [`None`] where it is written as a name in angle
/// brackets rather than as a character a reader types into a file.
fn one(spelled: &str) -> Option<char> {
    let mut characters = spelled.chars();
    let character = characters.next()?;

    characters.next().is_none().then_some(character)
}

/// # Returns
///
/// What the status line says about the key `spelled` names, whose further key was taken rather
/// than run.
fn unimplemented(spelled: &str) -> String {
    format!("`{spelled}` takes a key after it that this editor does not implement")
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
