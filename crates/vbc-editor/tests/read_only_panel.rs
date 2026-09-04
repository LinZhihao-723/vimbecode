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
//!
//! A list of keystrokes written down by hand is a list something can be missing from, so the list
//! is not the whole of what is checked. Every one- and two-key sequence, and every three-key
//! sequence of the keys a longer mutating keystroke begins with, is typed at a panel with the
//! policy taken out and at one with it in place: a sequence the first writes with, or reaches
//! insert mode with, the second is required to refuse and to leave the transcript byte-identical.
//! That holds a key the list forgot, and a key a later table binds, to the same promise as the
//! ones the list names.
//!
//! The keys those sequences are spelled with are read off the table rather than written down
//! here, because a sweep over the keys a terminal reports as printable characters is a sweep that
//! cannot type `<C-V>`, and `<C-V>` starts a selection every operator writes over. So the sweep's
//! alphabet is every printable key together with every key [`Bindings::vim`] names, and a key the
//! table grows that nothing here spells fails a case of its own rather than being passed over.
//!
//! Leaving the transcript byte-identical is required of every case the sweeps type, including the
//! ones the engine answers with an error, because a keystroke this editor cannot yet measure is
//! still a keystroke the panel promises writes nothing. Only the other half -- that a keystroke
//! which writes is one the panel says something about -- waits on the panel with the policy taken
//! out being able to show that it writes at all.
//!
//! The sweep runs the other way too: every pair of the keys a reader reaches for is required to be
//! neither refused nor answered any differently from a bare [`Engine`], so a policy that bought
//! its promise by refusing too much would be caught by the same machinery that catches one
//! refusing too little.
//!
//! Winding the keys back to where the engine stands after a refusal gets its own case, because the
//! transcript alone cannot show the difference: a panel wound back to a mode the engine is not in
//! leaves the transcript exactly as it was and reads every key after it in the wrong table.
//!
//! The window every case above is laid out in is narrow, but it is wrapped the one way a vim
//! manual wraps its examples, and a panel that answers a screen motion by counting characters
//! agrees with an editor doing the same. So the display motions are typed again under the settings
//! that decide where a screen line ends -- `'breakindent'`, `'showbreak'`, `'ambiwidth'` and the
//! tab stop -- at a transcript indented with tabs and holding characters whose width
//! `'ambiwidth'` decides. Each motion is required to answer as an editor laid out the same way
//! does, and to land somewhere different under at least two of those settings, so a wrapping that
//! wrapped nothing differently cannot pass.

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroUsize;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use modalkit::env::vim::VimMode;
use vbc_editor::chat::block::{Block, Kind, Role};
use vbc_editor::chat::policy::{Panel, Policy, REFUSAL};
use vbc_editor::chat::transcript::Transcript;
use vbc_editor::engine::{typed, Engine, Held, Position};
use vbc_editor::keys::{Bindings, Edge};
use vbc_editor::screen::Geometry;
use vbc_layout::line::Options;
use vbc_layout::width::{AmbiWidth, Metrics};

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
const READINGS: [Keystroke; 52] = [
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
    Keystroke {
        id: "to the top of the window",
        keys: "jjHl",
    },
    Keystroke {
        id: "to the middle of the window",
        keys: "M",
    },
    Keystroke {
        id: "to the bottom of the window",
        keys: "L",
    },
    Keystroke {
        id: "to the matching bracket",
        keys: "%",
    },
    Keystroke {
        id: "up to the first non-blank of a line",
        keys: "jj-",
    },
    Keystroke {
        id: "down to the first non-blank of a line",
        keys: "+",
    },
    Keystroke {
        id: "to the first non-blank with an underscore",
        keys: "jj_",
    },
    Keystroke {
        id: "forward a big word",
        keys: "W",
    },
    Keystroke {
        id: "back a big word",
        keys: "WWB",
    },
    Keystroke {
        id: "to the end of a big word",
        keys: "E",
    },
    Keystroke {
        id: "back to the end of a word",
        keys: "wwge",
    },
    Keystroke {
        id: "back to the end of a big word",
        keys: "WWgE",
    },
    Keystroke {
        id: "to a byte offset",
        keys: "30go",
    },
    Keystroke {
        id: "back after a searched character",
        keys: "$Tm",
    },
    Keystroke {
        id: "onto the second of a searched character",
        keys: "2fe",
    },
    Keystroke {
        id: "yank an inner word object",
        keys: "yiw",
    },
    Keystroke {
        id: "yank an inner big-word object",
        keys: "yiW",
    },
    Keystroke {
        id: "yank a bracketed object and its brackets",
        keys: "jj$hya(",
    },
];

/// The keys a three-key sweep starts a sequence with, which are the operators, the counts and the
/// register and prefix keys every longer mutating keystroke begins with.
const STARTERS: &str = "\"123dcy<>=gvVrzq@";

/// The keys a three-key sweep continues a sequence with.
const FOLLOWERS: &str = "\"2<>dcywgvViIaAoOpPuUxX~$0jkl";

/// The keys no terminal reports as a printable character that a three-key sweep starts and
/// continues a sequence with. `<C-R>` is not among them because redo takes no target, so what it
/// writes it writes on its own and the one- and two-key sweep already types it.
const NAMED_SEQUENCE_KEYS: [&str; 2] = ["<C-V>", "<Esc>"];

/// Every key a terminal reports as one printable character.
const PRINTABLE: &str = " !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcde\
fghijklmnopqrstuvwxyz{|}~";

/// The keys a reader reaches for, which a sweep pairs with one another. The keys that ask to write
/// are left out of it, because a `p` with nothing yanked, an `X` in the first column and a `u`
/// with nothing to take back all leave the transcript as it stands without being keys a reader
/// came for.
const READABLE: &str = "hjklwWbBeE0^$-+_G%HMLgvV\"123fFtT;,yY";

/// The transcript the display motions are typed at when the window is not laid out the way a vim
/// manual draws its examples. Its lines are indented with tabs so that a tab stop decides where
/// its text starts, one of them holds characters whose width `'ambiwidth'` decides, and all of
/// them are far longer than the window is wide so that every one of them wraps several times over.
const WRAPPED: &str = "User: why does \u{00a7}\u{00b1} render at two cells here and one there\n\
\tclaude: because 'ambiwidth' says so, and the wrap moves with it\n\
\t\tclaude: the continuation is indented too once 'breakindent' is set\n";

/// The ways the window every display-motion case is wrapped in, which are the settings that decide
/// where a screen line ends and where the next one starts.
const WRAPPINGS: [Wrapping; 5] = [
    Wrapping {
        id: "the settings a vim manual draws its examples with",
        break_indent: false,
        show_break: "",
        ambiwidth: AmbiWidth::Single,
        tab_stop: 8,
    },
    Wrapping {
        id: "`breakindent`",
        break_indent: true,
        show_break: "",
        ambiwidth: AmbiWidth::Single,
        tab_stop: 8,
    },
    Wrapping {
        id: "`showbreak`",
        break_indent: false,
        show_break: "+++ ",
        ambiwidth: AmbiWidth::Single,
        tab_stop: 8,
    },
    Wrapping {
        id: "`breakindent` and `showbreak` together",
        break_indent: true,
        show_break: "> ",
        ambiwidth: AmbiWidth::Single,
        tab_stop: 4,
    },
    Wrapping {
        id: "ambiguous characters two cells wide",
        break_indent: false,
        show_break: "",
        ambiwidth: AmbiWidth::Double,
        tab_stop: 2,
    },
];

/// The keystrokes counted in screen lines rather than in the transcript's own lines, which are the
/// ones the way a window wraps decides the answer to.
const DISPLAY_MOTIONS: [Keystroke; 6] = [
    Keystroke {
        id: "down a screen line",
        keys: "gj",
    },
    Keystroke {
        id: "down two screen lines and up one",
        keys: "gjgjgk",
    },
    Keystroke {
        id: "to the end of the screen line",
        keys: "g$",
    },
    Keystroke {
        id: "to the start of the screen line below",
        keys: "gjg0",
    },
    Keystroke {
        id: "to the end of the screen line below",
        keys: "gjg$",
    },
    Keystroke {
        id: "down a screen line from the tab-indented line",
        keys: "jgj",
    },
];

/// One way of wrapping the window a display motion is measured in.
struct Wrapping {
    id: &'static str,
    break_indent: bool,
    show_break: &'static str,
    ambiwidth: AmbiWidth,
    tab_stop: usize,
}

/// The keys a reader reaches for that no terminal reports as a printable character.
const NAMED_READABLE: [&str; 3] = ["<Enter>", "<Esc>", "<C-V>"];

/// One key of a sweep: what a terminal reports when it is typed, and the spelling a vim manual
/// names it by, which is what a sweep's failure is reported in.
#[derive(Clone, Debug)]
struct Typed {
    spelling: String,
    event: KeyEvent,
}

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

#[test]
fn every_key_the_table_names_is_one_a_sweep_can_type() {
    let mut untyped = Vec::new();
    for binding in Bindings::vim().entries() {
        for edge in &binding.keys {
            let Edge::Key(key) = edge else {
                continue;
            };
            let spelling = key.to_string();
            if event_of(&spelling).is_none() {
                untyped.push(spelling);
            }
        }
    }

    assert_eq!(Vec::<String>::new(), untyped);
}

#[test]
fn no_one_or_two_key_sequence_that_writes_goes_unrefused() {
    let alphabet = alphabet();
    let mut escaped = Vec::new();
    for first in &alphabet {
        if let Some(escape) = escape(std::slice::from_ref(first)) {
            escaped.push(escape);
        }
        for second in &alphabet {
            if let Some(escape) = escape(&[first.clone(), second.clone()]) {
                escaped.push(escape);
            }
        }
    }

    assert_eq!(Vec::<String>::new(), escaped);
}

#[test]
fn no_three_key_sequence_of_an_operator_and_its_target_that_writes_goes_unrefused() {
    let starters = sweep_keys(STARTERS, &NAMED_SEQUENCE_KEYS);
    let followers = sweep_keys(FOLLOWERS, &NAMED_SEQUENCE_KEYS);
    let mut escaped = Vec::new();
    for first in &starters {
        for second in &followers {
            for third in &followers {
                if let Some(escape) = escape(&[first.clone(), second.clone(), third.clone()]) {
                    escaped.push(escape);
                }
            }
        }
    }

    assert_eq!(Vec::<String>::new(), escaped);
}

#[test]
fn a_panel_answers_the_display_motions_as_an_editor_does_however_the_window_wraps() -> Result<()> {
    let mut answers = BTreeMap::new();
    for wrapping in &WRAPPINGS {
        let geometry = laid_out(wrapping);
        for case in DISPLAY_MOTIONS {
            let mut panel = Panel::laid_out_in(said(WRAPPED), geometry.clone());
            for key in keys(case.keys) {
                panel.press(key)?;
                assert_eq!(
                    None,
                    panel.refusal(),
                    "`{}`, which would {}, was refused with `{}` wrapping",
                    case.keys,
                    case.id,
                    wrapping.id
                );
            }
            let mut engine = Engine::laid_out_in(WRAPPED, geometry.clone());
            engine.press_all(keys(case.keys))?;
            let read = Reading::of_panel(&mut panel);

            assert_eq!(
                Reading::of_engine(&mut engine),
                read,
                "`{}`, which would {}, answered differently from the editor with `{}` wrapping",
                case.keys,
                case.id,
                wrapping.id
            );
            answers
                .entry(case.keys)
                .or_insert_with(BTreeSet::new)
                .insert(read.cursor);
        }
    }

    for case in DISPLAY_MOTIONS {
        assert_ne!(
            1,
            answers[case.keys].len(),
            "`{}`, which would {}, lands in the same place however the window wraps, so wrapping \
             it differently says nothing",
            case.keys,
            case.id
        );
    }

    Ok(())
}

#[test]
fn a_refusal_winds_the_panel_back_to_where_the_engine_stands() -> Result<()> {
    let mut panel = panel(Policy::ReadOnly);
    panel.press_all(keys("vjdjy"))?;
    let mut engine = Engine::laid_out_in(TRANSCRIPT, window());
    engine.press_all(keys("vjjy"))?;

    assert_eq!(TRANSCRIPT, panel.text());
    assert_eq!(
        Reading::of_engine(&mut engine),
        Reading::of_panel(&mut panel)
    );

    Ok(())
}

#[test]
fn no_pair_of_the_keys_a_reader_reaches_for_is_refused_or_answered_differently() {
    let readable = sweep_keys(READABLE, &NAMED_READABLE);
    let mut wrong = Vec::new();
    for first in &readable {
        if let Some(reason) = over_refusal(std::slice::from_ref(first)) {
            wrong.push(reason);
        }
        for second in &readable {
            if let Some(reason) = over_refusal(&[first.clone(), second.clone()]) {
                wrong.push(reason);
            }
        }
    }

    assert_eq!(Vec::<String>::new(), wrong);
}

/// # Returns
///
/// A newly created panel showing [`TRANSCRIPT`] under `policy`, laid out in a window narrow
/// enough for its lines to wrap.
fn panel(policy: Policy) -> Panel {
    Panel::laid_out_in(said(TRANSCRIPT), window()).governed_by(policy)
}

/// # Returns
///
/// A transcript of the one thing `said` was, which is what a panel is over now that it holds the
/// blocks that were said rather than a string of them. One message block holds the whole of the
/// fixture, so the text a panel is laid out over is the fixture byte for byte.
fn said(text: &str) -> Transcript {
    [Block::new(Kind::Message(Role::Assistant), text.to_owned())]
        .into_iter()
        .collect()
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
/// The window a display-motion case is measured in, wrapped as `wrapping` says.
///
/// # Panics
///
/// Panics if [`COLUMNS`] or [`ROWS`] is zero, or if a wrapping's tab stop is, none of which is.
fn laid_out(wrapping: &Wrapping) -> Geometry {
    let tab_stop = NonZeroUsize::new(wrapping.tab_stop).expect("a tab stop is not zero");
    let options = Options::new()
        .with_break_indent(wrapping.break_indent)
        .with_show_break(wrapping.show_break.to_owned());

    window()
        .with_metrics(Metrics::new(wrapping.ambiwidth, tab_stop))
        .with_options(options)
}

/// # Returns
///
/// The key events a terminal reports when `keys` is typed at it, one per character.
fn keys(keys: &str) -> Vec<KeyEvent> {
    keys.chars().map(typed).collect()
}

/// # Returns
///
/// What `case` did that a read-only panel promises nothing does, and [`None`] where it did none of
/// it.
///
/// Leaving the transcript byte-identical and standing in a mode a reader can read from are
/// required of every case, whatever the engine made of it, because a keystroke the engine answers
/// with an error is still a keystroke the panel promises writes nothing. Only the other half --
/// that a keystroke which writes is one the panel says something about -- waits on the panel with
/// the policy taken out being able to show that it writes at all.
fn escape(case: &[Typed]) -> Option<String> {
    let spelling = spelled(case);
    let mut locked = panel(Policy::ReadOnly);
    let mut said = false;
    for key in case {
        let ran = locked.press(key.event);
        if TRANSCRIPT != locked.text() {
            return Some(format!("`{spelling}` changed the transcript"));
        }
        said |= locked.refusal().is_some();
        if ran.is_err() {
            break;
        }
    }
    if VimMode::Insert == locked.mode() {
        return Some(format!(
            "`{spelling}` left the panel where every key would write"
        ));
    }
    if said {
        return None;
    }

    let mut free = panel(Policy::Unrestricted);
    free.press_all(case.iter().map(|key| key.event)).ok()?;
    if TRANSCRIPT == free.text() && VimMode::Insert != free.mode() {
        return None;
    }

    Some(format!("`{spelling}` was dropped without a word"))
}

/// # Returns
///
/// What a read-only panel took away from `case`, and [`None`] where it neither refused it nor
/// answered it any differently from an editor laid out in the same window. A case the engine
/// answers with an error is one this editor does not measure yet, and is passed over.
fn over_refusal(case: &[Typed]) -> Option<String> {
    let spelling = spelled(case);
    let mut locked = panel(Policy::ReadOnly);
    locked.press_all(case.iter().map(|key| key.event)).ok()?;
    if let Some(refusal) = locked.refusal() {
        return Some(format!("`{spelling}` was refused with `{refusal}`"));
    }
    let mut engine = Engine::laid_out_in(TRANSCRIPT, window());
    engine.press_all(case.iter().map(|key| key.event)).ok()?;
    let read = Reading::of_panel(&mut locked);
    if Reading::of_engine(&mut engine) != read {
        return Some(format!("`{spelling}` answered differently from the editor"));
    }

    None
}

/// # Returns
///
/// The keys the one- and two-key sweep types, which are every key a terminal reports as a
/// printable character together with every key the table names, so that the sweep is over the keys
/// this editor binds rather than over the ones a list here remembered.
///
/// # Panics
///
/// Panics if the table names a key nothing here spells, which is a key the sweep would otherwise
/// pass over in silence.
fn alphabet() -> Vec<Typed> {
    let mut alphabet: Vec<Typed> = PRINTABLE.chars().map(printable).collect();
    for binding in Bindings::vim().entries() {
        for edge in &binding.keys {
            let Edge::Key(key) = edge else {
                continue;
            };
            let spelling = key.to_string();
            let event = event_of(&spelling).unwrap_or_else(|| {
                panic!("`{spelling}` is a key the table names and nothing here types")
            });
            if alphabet.iter().any(|typed| event == typed.event) {
                continue;
            }
            alphabet.push(Typed { spelling, event });
        }
    }

    alphabet
}

/// # Returns
///
/// The keys a sweep runs over: every character of `characters`, and every key `spellings` names.
///
/// # Panics
///
/// Panics if `spellings` names a key nothing here spells.
fn sweep_keys(characters: &str, spellings: &[&str]) -> Vec<Typed> {
    characters
        .chars()
        .map(printable)
        .chain(spellings.iter().map(|spelling| {
            Typed {
                spelling: (*spelling).to_owned(),
                event: named(spelling)
                    .unwrap_or_else(|| panic!("`{spelling}` is a key nothing here types")),
            }
        }))
        .collect()
}

/// # Returns
///
/// The key a terminal reports when `character` is typed with no modifier held.
fn printable(character: char) -> Typed {
    Typed {
        spelling: character.to_string(),
        event: typed(character),
    }
}

/// # Returns
///
/// The key event a terminal reports for the key a vim manual spells `spelling`, and [`None`] where
/// nothing here spells it.
fn event_of(spelling: &str) -> Option<KeyEvent> {
    let mut characters = spelling.chars();
    let character = characters.next()?;
    if characters.next().is_none() {
        return Some(typed(character));
    }

    named(spelling)
}

/// # Returns
///
/// The key event a terminal reports for the key a vim manual spells `spelling` with a name in
/// angle brackets, and [`None`] where nothing here spells it.
fn named(spelling: &str) -> Option<KeyEvent> {
    let held = match spelling {
        "<lt>" => return Some(typed('<')),
        "<Esc>" => return Some(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        "<Enter>" => return Some(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        "<Tab>" => return Some(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        "<BS>" => return Some(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)),
        spelling => spelling.strip_prefix("<C-")?.strip_suffix('>')?,
    };
    let mut characters = held.chars();
    let character = characters.next()?;
    if characters.next().is_some() {
        return None;
    }

    Some(KeyEvent::new(
        KeyCode::Char(character.to_ascii_lowercase()),
        KeyModifiers::CONTROL,
    ))
}

/// # Returns
///
/// `case` spelled the way a vim manual spells a sequence of keystrokes.
fn spelled(case: &[Typed]) -> String {
    case.iter().map(|key| key.spelling.as_str()).collect()
}
