//! The transcript panel held to the one thing that separates it from the editor: it does not
//! write.
//!
//! A transcript is a record of an exchange that already happened, so the panel showing one shares
//! the editor's keybinding table, engine, motions and registers and shares none of its permission
//! to change the text. What that is worth depends entirely on the two halves of it being checked
//! together. A panel that answered every mutating key with a message and quietly edited anyway
//! would pass a test held to the message; a panel that dropped every key on the floor would pass
//! one held to the text. So every mutating keystroke below is required to leave the transcript
//! **byte-identical** after every single key of it *and* to have said something about at least one
//! of those keys, and neither assertion is made without the other.
//!
//! Nothing here is worth anything either unless the keystrokes it types are keystrokes that would
//! have written. A case with a typo in it, or one aimed at text it happens not to change -- `guw`
//! over a word already in lower case, `<<` over a line with no indent -- leaves the transcript
//! byte-identical against a panel with no policy at all, and would pass this file against an
//! editor that had never heard of one. So every case is replayed under [`Policy::Unrestricted`],
//! which is the same panel with the policy taken out, and is required to leave the transcript
//! different from the way it arrived. That is the fourth validation and it is also what stops the
//! first from being vacuous.
//!
//! The keys that write nothing by themselves are held to the same standard in the terms that
//! apply to them. `i` deletes nothing, inserts nothing and asks only for a mode, and a panel that
//! read actions alone would grant it and then sit in insert mode refusing every character typed
//! into it. Those keys are required to be refused where they are typed and to leave the panel in
//! a mode it can read from, and the panel with the policy taken out is required to reach insert
//! mode on the very same keys, so that the refusal is again the policy's doing.
//!
//! What a reader came for is checked against the editor rather than against a list written down
//! here: every motion, every character search, every yank and every visual selection is typed at a
//! panel and at a bare [`Engine`] laid out in the same window, and the four things an engine is
//! the authority on -- the text, the cursor, the mode and the registers -- are required to match.
//! Each is also required to leave the panel somewhere other than where it started, so that a case
//! whose keys did nothing at all cannot pass. The motions counted in screen lines are among them,
//! which is why every case is laid out in a window narrow enough for the transcript's lines to
//! wrap.
//!
//! Undo and redo get their own file space because they are the way a refusal could be undone
//! after the fact: an edit refused at the keystroke leaves nothing in the buffer's history, and an
//! engine that had run it and rolled it back would leave `u` holding it. `u` and `<C-R>` are
//! themselves keystrokes that write, so they are refused like the rest, and the transcript is
//! required to be byte-identical after each of them too.

use std::collections::BTreeMap;
use std::num::NonZeroUsize;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use modalkit::env::vim::VimMode;
use vbc_editor::chat::policy::{Panel, Policy, REFUSAL};
use vbc_editor::engine::{typed, Engine, Held, Position};
use vbc_editor::screen::Geometry;

/// One keystroke typed at a panel, named by what it is meant to do to the transcript.
struct Keystroke {
    id: &'static str,
    keys: &'static str,
}

/// The transcript every case below is typed at. Its first word is in mixed case so that the case
/// operators have something to change, its third line is indented so that an outdent has
/// something to take away, and its lines are longer than the window is wide so that the motions
/// counted in screen lines are counted over rows a logical motion cannot reach.
const TRANSCRIPT: &str =
    "User: make it compile\nclaude: one line to add\n    todo!();\nclaude: Done\n";

/// The cells the window every case is laid out in is wide.
const COLUMNS: usize = 20;

/// The screen lines the window every case is laid out in is tall.
const ROWS: usize = 10;

/// The keystrokes that would leave the transcript different from the way it arrived, which is
/// every operator, every command and every put that vim spells with them.
const MUTATIONS: [Keystroke; 38] = [
    Keystroke {
        id: "delete the character under the cursor",
        keys: "x",
    },
    Keystroke {
        id: "delete three characters under a count",
        keys: "3x",
    },
    Keystroke {
        id: "delete the character before the cursor",
        keys: "lX",
    },
    Keystroke {
        id: "delete a line",
        keys: "dd",
    },
    Keystroke {
        id: "delete two lines under a count",
        keys: "2dd",
    },
    Keystroke {
        id: "delete a line into a named register",
        keys: "\"add",
    },
    Keystroke {
        id: "delete a word",
        keys: "dw",
    },
    Keystroke {
        id: "delete to the end of the line",
        keys: "D",
    },
    Keystroke {
        id: "delete a line downwards",
        keys: "dj",
    },
    Keystroke {
        id: "delete over a motion counted in screen lines",
        keys: "dgj",
    },
    Keystroke {
        id: "delete a word object",
        keys: "daw",
    },
    Keystroke {
        id: "change a word",
        keys: "cw",
    },
    Keystroke {
        id: "change a line",
        keys: "cc",
    },
    Keystroke {
        id: "change to the end of the line",
        keys: "C",
    },
    Keystroke {
        id: "substitute the character under the cursor",
        keys: "s",
    },
    Keystroke {
        id: "substitute a line",
        keys: "S",
    },
    Keystroke {
        id: "replace the character under the cursor",
        keys: "rZ",
    },
    Keystroke {
        id: "put a yanked line after the cursor",
        keys: "yyp",
    },
    Keystroke {
        id: "put a yanked line before the cursor",
        keys: "yyP",
    },
    Keystroke {
        id: "open a line below the cursor",
        keys: "o",
    },
    Keystroke {
        id: "open a line above the cursor",
        keys: "O",
    },
    Keystroke {
        id: "join two lines with a space",
        keys: "J",
    },
    Keystroke {
        id: "join two lines without one",
        keys: "gJ",
    },
    Keystroke {
        id: "toggle the case of a character",
        keys: "~",
    },
    Keystroke {
        id: "lower the case of a word",
        keys: "guw",
    },
    Keystroke {
        id: "raise the case of a word",
        keys: "gUw",
    },
    Keystroke {
        id: "toggle the case of a word",
        keys: "g~w",
    },
    Keystroke {
        id: "indent a line",
        keys: ">>",
    },
    Keystroke {
        id: "outdent an indented line",
        keys: "jj<<",
    },
    Keystroke {
        id: "indent over a motion counted in screen lines",
        keys: ">gj",
    },
    Keystroke {
        id: "delete a characterwise selection",
        keys: "vd",
    },
    Keystroke {
        id: "delete a characterwise selection with x",
        keys: "vx",
    },
    Keystroke {
        id: "delete a linewise selection",
        keys: "Vd",
    },
    Keystroke {
        id: "change a characterwise selection",
        keys: "vc",
    },
    Keystroke {
        id: "put a yanked line over a selection",
        keys: "yyvp",
    },
    Keystroke {
        id: "indent a selection",
        keys: "v>",
    },
    Keystroke {
        id: "join the lines a selection spans",
        keys: "vjJ",
    },
    Keystroke {
        id: "raise the case of a selection",
        keys: "vlU",
    },
];

/// The keystrokes that write nothing themselves and ask only to stand somewhere every following
/// key would write from.
const INSERTIONS: [Keystroke; 7] = [
    Keystroke {
        id: "insert at the cursor",
        keys: "i",
    },
    Keystroke {
        id: "insert after the cursor",
        keys: "a",
    },
    Keystroke {
        id: "insert at the first non-blank of the line",
        keys: "I",
    },
    Keystroke {
        id: "insert at the end of the line",
        keys: "A",
    },
    Keystroke {
        id: "replace from the cursor",
        keys: "R",
    },
    Keystroke {
        id: "insert at the start of a selection",
        keys: "vI",
    },
    Keystroke {
        id: "insert at the end of a selection",
        keys: "vA",
    },
];

/// The keystrokes a reader came for, which write nothing and must reach the engine untouched.
const READINGS: [Keystroke; 34] = [
    Keystroke {
        id: "down a line",
        keys: "j",
    },
    Keystroke {
        id: "up a line",
        keys: "jjk",
    },
    Keystroke {
        id: "right a character",
        keys: "l",
    },
    Keystroke {
        id: "left a character",
        keys: "llh",
    },
    Keystroke {
        id: "forward a word",
        keys: "w",
    },
    Keystroke {
        id: "back a word",
        keys: "wwb",
    },
    Keystroke {
        id: "to the end of a word",
        keys: "e",
    },
    Keystroke {
        id: "to the first column",
        keys: "j$0",
    },
    Keystroke {
        id: "to the end of the line",
        keys: "$",
    },
    Keystroke {
        id: "to the first non-blank of an indented line",
        keys: "jj^",
    },
    Keystroke {
        id: "to the last line",
        keys: "G",
    },
    Keystroke {
        id: "to a numbered line",
        keys: "3gg",
    },
    Keystroke {
        id: "down three lines under a count",
        keys: "2j",
    },
    Keystroke {
        id: "forward two words under a count",
        keys: "2w",
    },
    Keystroke {
        id: "onto a searched character",
        keys: "fm",
    },
    Keystroke {
        id: "up to a searched character",
        keys: "tm",
    },
    Keystroke {
        id: "onto the next of a searched character",
        keys: "fm;",
    },
    Keystroke {
        id: "back onto a searched character",
        keys: "fm;,",
    },
    Keystroke {
        id: "back before a searched character",
        keys: "$Fm",
    },
    Keystroke {
        id: "down two screen lines",
        keys: "gjgj",
    },
    Keystroke {
        id: "up a screen line",
        keys: "gjgjgk",
    },
    Keystroke {
        id: "to the start of a screen line",
        keys: "jg$g0",
    },
    Keystroke {
        id: "to the end of a screen line",
        keys: "g$",
    },
    Keystroke {
        id: "yank a line",
        keys: "yy",
    },
    Keystroke {
        id: "yank a line with Y",
        keys: "Y",
    },
    Keystroke {
        id: "yank a word",
        keys: "yw",
    },
    Keystroke {
        id: "yank to the end of the line",
        keys: "y$",
    },
    Keystroke {
        id: "yank a line downwards",
        keys: "yj",
    },
    Keystroke {
        id: "yank over a motion counted in screen lines",
        keys: "ygj",
    },
    Keystroke {
        id: "yank a word object",
        keys: "yaw",
    },
    Keystroke {
        id: "yank into a named register",
        keys: "\"ayy",
    },
    Keystroke {
        id: "start a characterwise selection",
        keys: "v",
    },
    Keystroke {
        id: "yank a characterwise selection",
        keys: "vjy",
    },
    Keystroke {
        id: "yank a linewise selection",
        keys: "Vy",
    },
];

/// Everything an engine is the authority on, which is what a panel and an engine are compared in.
#[derive(Debug, Eq, PartialEq)]
struct Reading {
    text: String,
    cursor: Position,
    mode: VimMode,
    registers: BTreeMap<char, Held>,
}

impl Reading {
    /// # Returns
    ///
    /// What the keys typed so far left `panel` holding.
    fn of_panel(panel: &mut Panel) -> Self {
        Self {
            text: panel.text(),
            cursor: panel.cursor(),
            mode: panel.mode(),
            registers: panel.registers(),
        }
    }

    /// # Returns
    ///
    /// What the keys typed so far left `engine` holding.
    fn of_engine(engine: &mut Engine) -> Self {
        Self {
            text: engine.text(),
            cursor: engine.cursor(),
            mode: engine.mode(),
            registers: engine.registers(),
        }
    }
}

#[test]
fn every_mutating_keystroke_leaves_the_transcript_byte_identical_and_says_so() -> Result<()> {
    for case in MUTATIONS {
        let mut panel = panel(Policy::ReadOnly);
        let mut said = Vec::new();
        for key in keys(case.keys) {
            panel.press(key)?;
            assert_eq!(
                TRANSCRIPT,
                panel.text(),
                "`{}`, which would {}, changed the transcript",
                case.keys,
                case.id
            );
            if let Some(refusal) = panel.refusal() {
                said.push(refusal.to_string());
            }
        }

        assert_ne!(
            Vec::<String>::new(),
            said,
            "`{}`, which would {}, was dropped without a word",
            case.keys,
            case.id
        );
        for message in said {
            assert!(
                message.starts_with(REFUSAL),
                "`{}` was refused with `{message}`, which does not say the transcript is read-only",
                case.keys
            );
        }
    }

    Ok(())
}

#[test]
fn a_refusal_names_the_keys_that_were_refused() -> Result<()> {
    let mut panel = panel(Policy::ReadOnly);
    panel.press_all(keys("dw"))?;
    let refusal = panel.refusal().expect("`dw` is refused").clone();

    assert_eq!("dw", refusal.keys());
    assert_eq!(
        "the transcript is read-only: `dw` would change what was said",
        refusal.to_string()
    );

    Ok(())
}

#[test]
fn a_refusal_is_taken_off_the_status_line_by_the_next_keystroke_that_runs() -> Result<()> {
    let mut panel = panel(Policy::ReadOnly);
    panel.press_all(keys("dd"))?;

    assert!(panel.refusal().is_some());

    panel.press_all(keys("j"))?;

    assert_eq!(None, panel.refusal());

    Ok(())
}

#[test]
fn every_mutating_keystroke_changes_the_transcript_once_the_policy_is_taken_out() -> Result<()> {
    for case in MUTATIONS {
        let mut panel = panel(Policy::Unrestricted);
        panel.press_all(keys(case.keys))?;

        assert_ne!(
            TRANSCRIPT,
            panel.text(),
            "`{}`, which would {}, changes nothing even with the policy taken out, so refusing \
             it says nothing",
            case.keys,
            case.id
        );
        assert_eq!(
            None,
            panel.refusal(),
            "`{}` was refused by a panel with no policy",
            case.keys
        );
    }

    Ok(())
}

#[test]
fn a_keystroke_that_would_leave_the_panel_writing_is_refused_where_it_is_typed() -> Result<()> {
    for case in INSERTIONS {
        let mut panel = panel(Policy::ReadOnly);
        let mut said = Vec::new();
        for key in keys(case.keys) {
            panel.press(key)?;
            assert_eq!(
                TRANSCRIPT,
                panel.text(),
                "`{}`, which would {}, changed the transcript",
                case.keys,
                case.id
            );
            if let Some(refusal) = panel.refusal() {
                said.push(refusal.to_string());
            }
        }

        assert_ne!(
            Vec::<String>::new(),
            said,
            "`{}`, which would {}, was dropped without a word",
            case.keys,
            case.id
        );
        assert_ne!(
            VimMode::Insert,
            panel.mode(),
            "`{}` left the panel standing where every following key would write",
            case.keys
        );
    }

    Ok(())
}

#[test]
fn a_keystroke_that_would_leave_the_panel_writing_reaches_insert_mode_without_the_policy(
) -> Result<()> {
    for case in INSERTIONS {
        let mut panel = panel(Policy::Unrestricted);
        panel.press_all(keys(case.keys))?;

        assert_eq!(
            VimMode::Insert,
            panel.mode(),
            "`{}`, which would {}, does not reach insert mode even with the policy taken out, so \
             refusing it says nothing",
            case.keys,
            case.id
        );
    }

    Ok(())
}

#[test]
fn nothing_typed_after_a_refused_way_into_insert_mode_reaches_the_transcript() -> Result<()> {
    let mut panel = panel(Policy::ReadOnly);
    for key in keys("ihello") {
        panel.press(key)?;

        assert_eq!(TRANSCRIPT, panel.text());
        assert_ne!(VimMode::Insert, panel.mode());
    }

    Ok(())
}

#[test]
fn every_motion_search_and_yank_reaches_the_transcript_as_it_reaches_an_editor() -> Result<()> {
    let untouched = Reading::of_panel(&mut panel(Policy::ReadOnly));
    for case in READINGS {
        let mut panel = panel(Policy::ReadOnly);
        for key in keys(case.keys) {
            panel.press(key)?;
            assert_eq!(
                None,
                panel.refusal(),
                "`{}`, which would {}, was refused",
                case.keys,
                case.id
            );
        }
        let mut engine = Engine::laid_out_in(TRANSCRIPT, window());
        engine.press_all(keys(case.keys))?;
        let read = Reading::of_panel(&mut panel);

        assert_ne!(
            untouched, read,
            "`{}`, which would {}, left the panel exactly where it started, so agreeing with the \
             editor says nothing",
            case.keys, case.id
        );
        assert_eq!(
            Reading::of_engine(&mut engine),
            read,
            "`{}`, which would {}, answered differently from the editor",
            case.keys,
            case.id
        );
    }

    Ok(())
}

#[test]
fn a_panel_with_the_policy_taken_out_answers_every_keystroke_as_the_editor_does() -> Result<()> {
    for case in MUTATIONS.iter().chain(&INSERTIONS).chain(&READINGS) {
        let mut panel = panel(Policy::Unrestricted);
        panel.press_all(keys(case.keys))?;
        let mut engine = Engine::laid_out_in(TRANSCRIPT, window());
        engine.press_all(keys(case.keys))?;

        assert_eq!(
            Reading::of_engine(&mut engine),
            Reading::of_panel(&mut panel),
            "`{}`, which would {}, answered differently from the editor with the policy taken out",
            case.keys,
            case.id
        );
    }

    Ok(())
}

#[test]
fn undo_and_redo_cannot_resurrect_an_edit_that_was_refused() -> Result<()> {
    let undo = typed('u');
    let redo = KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL);
    for case in MUTATIONS {
        let mut panel = panel(Policy::ReadOnly);
        panel.press_all(keys(case.keys))?;
        panel.press(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))?;
        for key in [undo, redo, undo, redo] {
            panel.press(key)?;

            assert_eq!(
                TRANSCRIPT,
                panel.text(),
                "the transcript came back after `{}` was refused",
                case.keys
            );
            assert!(
                panel
                    .refusal()
                    .is_some_and(|refusal| refusal.to_string().starts_with(REFUSAL)),
                "undoing after `{}` was not refused",
                case.keys
            );
        }
    }

    Ok(())
}

#[test]
fn undo_takes_a_mutating_keystroke_back_once_the_policy_is_taken_out() -> Result<()> {
    let mut panel = panel(Policy::Unrestricted);
    panel.press_all(keys("dd"))?;

    assert_ne!(TRANSCRIPT, panel.text());

    panel.press(typed('u'))?;

    assert_eq!(TRANSCRIPT, panel.text());

    Ok(())
}

/// # Returns
///
/// A newly created panel showing [`TRANSCRIPT`] under `policy`, laid out in a window narrow
/// enough for its lines to wrap.
fn panel(policy: Policy) -> Panel {
    Panel::laid_out_in(TRANSCRIPT, window()).governed_by(policy)
}

/// # Returns
///
/// The window every case is laid out in.
///
/// # Panics
///
/// Panics if [`COLUMNS`] or [`ROWS`] is zero, which neither is.
fn window() -> Geometry {
    Geometry::new(
        NonZeroUsize::new(COLUMNS).expect("the window is not zero columns wide"),
        NonZeroUsize::new(ROWS).expect("the window is not zero rows tall"),
    )
}

/// # Returns
///
/// The key events a terminal reports when `keys` is typed at it, one per character.
fn keys(keys: &str) -> Vec<KeyEvent> {
    keys.chars().map(typed).collect()
}
