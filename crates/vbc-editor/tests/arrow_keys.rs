//! The arrow keys held to the letters they stand for, and to the vim both were copied from.
//!
//! A reader who has not learned `hjkl` reaches for the arrows, and an editor that answers them
//! with nothing tells that reader nothing either. What is bound for them is not a second set of
//! motions but the first set reached by another key, so the whole of what is checked here is
//! sameness: `<Down>` is required to leave the engine exactly where `j` leaves it, count for
//! count, operator for operator and selection for selection.
//!
//! Sameness against the editor alone would be a claim about the table checked against itself, so
//! every pairing is put to a real vim three ways. vim is required to answer the arrow where it
//! answers the letter, which is what says the pairing is vim's rather than one written down here;
//! the engine is required to answer the arrow where vim answers it; and the outcome is required to
//! differ from the one an engine handed no keys at all reports, so that a case whose keys moved
//! nothing cannot pass by leaving both sides untouched.
//!
//! An arrow over a wrapped line is the one place a reader could reasonably expect something else.
//! This editor counts `j` in lines of the text rather than rows of the screen, and the arrow is
//! bound to `j`, so it counts the same way -- which is checked against the vim that decided it,
//! and against `gj` in the same window, so a case laid out too wide to wrap cannot pass as
//! agreement.
//!
//! In an inserting mode the letters are text and the arrows are all that is left to move by. Both
//! halves of that are required together: the text after an arrow is required to be byte-identical
//! to the text before it, and the cursor is required to have moved. A test holding only the first
//! would pass against a key that did nothing at all, and one holding only the second would pass
//! against a key that moved the cursor by typing.
//!
//! A transcript is read through the same table, so the arrows are read there too, and the panel is
//! required to answer a bare arrow exactly as the editor does and to refuse an operator over one
//! exactly as it refuses the operator over the letter -- with the transcript byte-identical after
//! it, which is the promise the panel exists to keep.
//!
//! The last of these is the control group, and it is measured rather than argued. The table is a
//! list of entries, and the entries the arrows added can be taken back out of it: a table with the
//! six of them unbound is the table as it stood before they were bound. Every sequence reaching
//! every entry of that stripped table is typed at both, and the actions, the contexts and the mode
//! are required to be identical, so a binding that buried an existing key or changed the mode a
//! key lands in fails here rather than being noticed later. The stripping itself is checked to
//! bite: against the stripped table an arrow is a key that reaches nothing.
//!
//! A sweep over a stripped table cannot see a key the six were bound *over*, because stripping
//! would take that key away too and the sweep would never type it. So the two things that would
//! make such a collision possible are held closed instead: the table is required to have grown by
//! exactly as many entries as the six were bound in modes, and each of the six is required to be a
//! key no character types, which is what puts them out of reach of every letter this table already
//! answers.

mod notation;
mod outcome;

use std::collections::BTreeSet;
use std::num::NonZeroUsize;

use anyhow::Result;
use modalkit::env::vim::VimMode;
use modalkit::key::TerminalKey;
use modalkit::keybindings::InputKey;
use vbc_editor::chat::block::{Block, Kind, Role};
use vbc_editor::chat::policy::{Panel, Policy};
use vbc_editor::chat::transcript::Transcript;
use vbc_editor::engine::Engine;
use vbc_editor::keys::{Bindings, Edge, Keys, Step, CURSOR_KEYS};
use vbc_editor::screen::Geometry;
use vbc_oracle::corpus::{Case, Options};
use vbc_oracle::vim::VimDriver;

use crate::notation::keys;
use crate::outcome::Outcome;

/// One pairing under test: an arrow key sequence and the sequence of letters it stands for.
struct Paired {
    arrow: &'static str,
    letter: &'static str,
}

/// One pairing measured where a line of the text and a row of the screen part company.
struct Counted {
    arrow: &'static str,
    logical: &'static str,
    by_row: &'static str,
}

/// The text every case is typed at. Its first line is longer than the window is wide so that it
/// wraps, its lines are of differing lengths so that the two keys that name an end of a line name
/// a different column on each, and it holds enough lines for a count to walk down.
const PROSE: &str = "abcdefghijklmnopqrstuvwxyz0123456789\nsecond line here\nthird\nfourth line \
                     is longer\nfifth\n";

/// The cells every case is laid out in, narrow enough that the first line of [`PROSE`] wraps.
const COLUMNS: u16 = 20;

/// The screen lines every case is laid out in.
const ROWS: u16 = 10;

/// The pairings the arrows are held to: every way the table lets a motion be reached -- bare,
/// under a count, as an operator's target, and inside a visual selection.
const PAIRED: [Paired; 18] = [
    Paired {
        arrow: "<Down><Down>",
        letter: "jj",
    },
    Paired {
        arrow: "<Down><Down><Up>",
        letter: "jjk",
    },
    Paired {
        arrow: "<Right><Right><Right>",
        letter: "lll",
    },
    Paired {
        arrow: "<Right><Right><Left>",
        letter: "llh",
    },
    Paired {
        arrow: "3<Right>",
        letter: "3l",
    },
    Paired {
        arrow: "2<Down>",
        letter: "2j",
    },
    Paired {
        arrow: "<End>",
        letter: "$",
    },
    Paired {
        arrow: "<Down><End>",
        letter: "j$",
    },
    Paired {
        arrow: "<End><Home>x",
        letter: "$0x",
    },
    Paired {
        arrow: "d<Down>",
        letter: "dj",
    },
    Paired {
        arrow: "d<Right>",
        letter: "dl",
    },
    Paired {
        arrow: "2d<Down>",
        letter: "2dj",
    },
    Paired {
        arrow: "<Down><Down>d<Up>",
        letter: "jjdk",
    },
    Paired {
        arrow: "d<End>",
        letter: "d$",
    },
    Paired {
        arrow: "<Down><End>d<Home>",
        letter: "j$d0",
    },
    Paired {
        arrow: "y<Down>",
        letter: "yj",
    },
    Paired {
        arrow: "v<Down>d",
        letter: "vjd",
    },
    Paired {
        arrow: "V<Down>d",
        letter: "Vjd",
    },
];

/// The pairings measured over the line of [`PROSE`] that wraps: the arrow, the motion counted in
/// lines of the text it is required to answer as, and the motion counted in rows of the screen it
/// is required to answer differently from.
const COUNTED: [Counted; 4] = [
    Counted {
        arrow: "<Down>",
        logical: "j",
        by_row: "gj",
    },
    Counted {
        arrow: "<Down><End>",
        logical: "j$",
        by_row: "gjg$",
    },
    Counted {
        arrow: "d<Down>",
        logical: "dj",
        by_row: "dgj",
    },
    Counted {
        arrow: "y<Down>",
        logical: "yj",
        by_row: "ygj",
    },
];

/// The keys that reach an inserting mode, each with the arrows typed once it is reached. The keys
/// that reach it are letters, because what is under test is the arrow rather than the way in.
const INSERTING: [(&str, &str); 7] = [
    ("i", "<Right>"),
    ("i", "<Right><Right><Right>"),
    ("i", "<Down>"),
    ("i", "<Down><Down><Up>"),
    ("i", "<End>"),
    ("jj$i", "<Home>"),
    ("A", "<Left><Left>"),
];

/// The sequences typed at a transcript panel, each with what the panel is required to make of it:
/// a bare arrow is read, and an operator over one is refused.
const READ_BY_PANEL: [&str; 6] = [
    "<Down>",
    "<Down><Down><Up>",
    "3<Right>",
    "<End>",
    "<Down>v<Right>y",
    "<Down>y<Down>",
];

/// The sequences a transcript panel is required to refuse, which are the ones that would write.
const REFUSED_BY_PANEL: [&str; 4] = ["d<Down>", "d<Right>", "c<End>", "v<Down>d"];

/// The transcript the panel cases are read from. Its lines are longer than the window is wide, so
/// the arrows are read over a transcript that wraps.
const TRANSCRIPT: &str = "User: make the arrow keys work\nclaude: they are the letters now\n\
                          claude: Done\n";

/// The keys that reach each mode the table binds in, from a machine in normal mode.
const REACHED: [(VimMode, &str); 4] = [
    (VimMode::Normal, ""),
    (VimMode::Visual, "v"),
    (VimMode::Insert, "i"),
    (VimMode::OperationPending, "d"),
];

/// The change a repeat is reached behind, which is a change the table binds itself.
const CHANGED: &str = "x";

/// The modes each of [`CURSOR_KEYS`] is bound in, which are the three a motion is read in and the
/// inserting one where the letters are text.
const BOUND_IN: usize = 4;

#[test]
fn vim_answers_every_arrow_where_it_answers_the_letter_it_stands_for() -> Result<()> {
    let vim = VimDriver::new()?;
    let untouched = vim_outcome(&vim, "")?;

    for case in PAIRED {
        let answered = vim_outcome(&vim, case.arrow)?;

        assert_eq!(
            vim_outcome(&vim, case.letter)?,
            answered,
            "vim answers `{}` somewhere other than where it answers `{}`, so the pairing this \
             table binds is not vim's",
            case.arrow,
            case.letter
        );
        assert_ne!(
            untouched, answered,
            "vim left `{}` where it left a buffer handed no keys, so the case cannot tell a key \
             that ran from one that was dropped",
            case.arrow
        );
    }

    Ok(())
}

#[test]
fn every_arrow_leaves_the_engine_where_vim_leaves_it() -> Result<()> {
    let vim = VimDriver::new()?;

    for case in PAIRED {
        let typed = engine_outcome(case.arrow)?;

        assert_eq!(
            vim_outcome(&vim, case.arrow)?,
            typed,
            "`{}` left the engine somewhere other than where vim left it",
            case.arrow
        );
        assert_eq!(
            engine_outcome(case.letter)?,
            typed,
            "`{}` left the engine somewhere other than `{}` leaves it",
            case.arrow,
            case.letter
        );
    }

    Ok(())
}

#[test]
fn an_arrow_over_a_wrapped_line_walks_a_line_of_the_text_rather_than_a_row_of_the_screen(
) -> Result<()> {
    let vim = VimDriver::new()?;

    for case in COUNTED {
        let typed = engine_outcome(case.arrow)?;

        assert_eq!(
            vim_outcome(&vim, case.arrow)?,
            typed,
            "`{}` left the engine somewhere other than where vim left it in a window its first \
             line wraps in",
            case.arrow
        );
        assert_eq!(
            engine_outcome(case.logical)?,
            typed,
            "`{}` no longer walks a line of the text, as `{}` does",
            case.arrow,
            case.logical
        );
        assert_ne!(
            engine_outcome(case.by_row)?,
            typed,
            "`{}` leaves the engine where `{}` leaves it, so the case is laid out too wide to \
             tell a line of the text from a row of the screen",
            case.arrow,
            case.by_row
        );
    }

    Ok(())
}

#[test]
fn an_arrow_in_an_inserting_mode_moves_the_cursor_and_types_nothing() -> Result<()> {
    let vim = VimDriver::new()?;

    for (into, arrow) in INSERTING {
        let keys = format!("{into}{arrow}");
        let standing = engine_outcome(into)?;
        let moved = engine_outcome(&keys)?;

        assert_eq!(
            standing.text, moved.text,
            "`{arrow}` changed the text it was typed into"
        );
        assert_ne!(
            (standing.line, standing.column),
            (moved.line, moved.column),
            "`{arrow}` left the cursor where `{into}` left it, so the case cannot tell a key that \
             moved from one that was dropped"
        );
        assert_eq!(
            vim_outcome(&vim, &keys)?,
            moved,
            "`{keys}` left the engine somewhere other than where vim left it"
        );
    }

    Ok(())
}

#[test]
fn a_transcript_panel_reads_an_arrow_as_the_editor_does() -> Result<()> {
    for keys in READ_BY_PANEL {
        let mut panel = panel();
        let mut engine = Engine::laid_out_in(TRANSCRIPT, window());
        let standing = panel.cursor();
        panel.press_all(self::keys(keys))?;
        engine.press_all(self::keys(keys))?;

        assert_eq!(None, panel.refusal(), "`{keys}` was refused by the panel");
        assert_eq!(
            TRANSCRIPT,
            panel.text(),
            "`{keys}` changed the transcript the panel is over"
        );
        assert_eq!(
            (engine.cursor(), engine.mode(), engine.registers()),
            (panel.cursor(), panel.mode(), panel.registers()),
            "`{keys}` left the panel somewhere other than where it leaves the editor"
        );
        assert_ne!(
            standing,
            panel.cursor(),
            "`{keys}` left the panel's cursor where it started, so the case says nothing about \
             the arrow having been read"
        );
    }

    Ok(())
}

#[test]
fn a_transcript_panel_refuses_an_operator_over_an_arrow() -> Result<()> {
    for keys in REFUSED_BY_PANEL {
        let mut panel = panel();
        let mut said = Vec::new();
        for key in self::keys(keys) {
            panel.press(key)?;
            assert_eq!(
                TRANSCRIPT,
                panel.text(),
                "`{keys}` changed the transcript the panel is over"
            );
            if let Some(refusal) = panel.refusal() {
                said.push(refusal.to_string());
            }
        }

        assert_ne!(
            Vec::<String>::new(),
            said,
            "`{keys}` was dropped by the panel without a word"
        );

        let mut free = Panel::laid_out_in(said_by_claude(TRANSCRIPT), window())
            .governed_by(Policy::Unrestricted);
        free.press_all(self::keys(keys))?;

        assert_ne!(
            TRANSCRIPT,
            free.text(),
            "`{keys}` changes nothing even with the policy taken out, so refusing it says nothing"
        );
    }

    Ok(())
}

#[test]
fn binding_the_arrows_left_every_key_that_already_worked_answering_as_it_did() {
    let bound = Bindings::vim();
    let stripped = stripped();
    for (spelled, _move_type, _count) in CURSOR_KEYS {
        let typed = keys(spelled);
        let [only] = typed.as_slice() else {
            panic!("`{spelled}` is spelled by something other than one key");
        };

        assert_eq!(
            None,
            TerminalKey::from(*only).get_char(),
            "`{spelled}` is a key a character types, so binding it could have taken the binding \
             of a letter this table already answers"
        );
    }

    assert_eq!(
        stripped.entries().len() + CURSOR_KEYS.len() * BOUND_IN,
        bound.entries().len(),
        "the table grew by something other than the arrows, so one of them was bound over a key \
         that was already there"
    );

    let mut changed = Vec::new();
    for keys in reachable(&stripped) {
        let before = through(&stripped, &keys);
        let after = through(&bound, &keys);
        if before != after {
            changed.push(shown(&keys));
        }
    }

    assert_eq!(Vec::<String>::new(), changed);
}

#[test]
fn the_control_group_is_measured_against_a_table_the_arrows_are_really_out_of() {
    let stripped = stripped();
    let mut bound = Vec::new();
    for (spelled, _move_type, _count) in CURSOR_KEYS {
        let mut machine = Keys::new(stripped.clone());
        for key in keys(spelled) {
            machine.input_key(key.into());
        }
        if machine.pop().is_some() {
            bound.push(spelled.to_owned());
        }
    }

    assert_eq!(
        Vec::<String>::new(),
        bound,
        "the table the control group compares against still answers an arrow, so it is not the \
         table as it stood before they were bound"
    );
}

/// # Returns
///
/// The editor's own table with every one of [`CURSOR_KEYS`] taken back out of it in every mode,
/// which is the table as it stood before they were bound.
fn stripped() -> Bindings {
    let mut stripped = Bindings::vim();
    for (spelled, _move_type, _count) in CURSOR_KEYS {
        for (mode, _reached) in REACHED {
            stripped.unbind(mode, spelled);
        }
    }

    stripped
}

/// # Returns
///
/// A newly created panel showing `text` as the one thing an assistant said, laid out in the window
/// every case is measured in and read-only, which is what a transcript is.
fn panel() -> Panel {
    Panel::laid_out_in(said_by_claude(TRANSCRIPT), window()).governed_by(Policy::ReadOnly)
}

/// # Returns
///
/// A transcript of the one thing `text` was, whose single block holds the whole of it, so the text
/// a panel is laid out over is `text` byte for byte.
fn said_by_claude(text: &str) -> Transcript {
    [Block::new(Kind::Message(Role::Assistant), text.to_owned())]
        .into_iter()
        .collect()
}

/// # Returns
///
/// What vim was left holding after `keys` were typed at [`PROSE`] in the window every case is laid
/// out in, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`VimDriver::run_case`]'s return values on failure.
fn vim_outcome(vim: &VimDriver, keys: &str) -> Result<Outcome> {
    let state = vim.run_case(&Case {
        id: keys.to_owned(),
        description: keys.to_owned(),
        buffer: PROSE.to_owned(),
        keys: keys.to_owned(),
        viewport_width: COLUMNS,
        viewport_height: ROWS,
        tags: BTreeSet::new(),
        options: Options::default(),
    })?;

    Ok(state.into())
}

/// # Returns
///
/// What an engine over [`PROSE`], laid out in the window every case is measured in, was left
/// holding after `keys` were typed at it, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`Engine::press_all`]'s return values on failure.
fn engine_outcome(keys: &str) -> Result<Outcome> {
    let mut engine = Engine::laid_out_in(PROSE, window());
    engine.press_all(self::keys(keys))?;

    Ok(Outcome::of(&mut engine))
}

/// # Returns
///
/// The window every case is laid out in, narrow enough that the first line of [`PROSE`] wraps.
///
/// # Panics
///
/// Panics if that window is zero columns wide or zero rows tall, which it is not.
fn window() -> Geometry {
    let columns = NonZeroUsize::new(usize::from(COLUMNS)).expect("the columns are not zero");
    let rows = NonZeroUsize::new(usize::from(ROWS)).expect("the rows are not zero");

    Geometry::new(columns, rows)
}

/// # Returns
///
/// A sequence of keys reaching every entry `bindings` holds, each typed from normal mode, in the
/// terms `keybinding_table` reaches them by: an operator is reached twice, once with a motion
/// after it and once typed again, and a repeat is reached behind a change.
///
/// # Panics
///
/// Panics if the table binds keys in a mode no sequence reaches.
fn reachable(bindings: &Bindings) -> Vec<Vec<TerminalKey>> {
    let mut reachable = Vec::new();
    for binding in bindings.entries() {
        let Some((_mode, prelude)) = REACHED.iter().find(|(mode, _)| *mode == binding.mode) else {
            panic!("`{:?}` is a mode no sequence reaches", binding.mode);
        };
        let mut keys: Vec<TerminalKey> = match &binding.operator {
            Some(operator) => operator.clone(),
            None => prelude.chars().map(typed).collect(),
        };
        if let Step::Repeat = binding.step {
            keys.extend(CHANGED.chars().map(typed));
        }
        keys.extend(filled(&binding.keys));
        if let Step::Operator { .. } = binding.step {
            let mut doubled = keys.clone();
            doubled.extend(filled(&binding.keys));
            reachable.push(doubled);
            keys.push(typed('w'));
        }
        reachable.push(keys);
    }

    reachable
}

/// # Returns
///
/// The keys `edges` are typed by, with a key of any kind stood in for by one the table gives no
/// meaning of its own.
fn filled(edges: &[Edge]) -> Vec<TerminalKey> {
    edges
        .iter()
        .map(|edge| match edge {
            Edge::Key(bound) => *bound,
            Edge::Any => typed('x'),
        })
        .collect()
}

/// # Returns
///
/// What `bindings` produce for `keys`, together with the mode they leave the machine in.
fn through(bindings: &Bindings, keys: &[TerminalKey]) -> Vec<String> {
    let mut machine = Keys::new(bindings.clone());
    let mut produced = Vec::new();
    for key in keys {
        machine.input_key(*key);
        while let Some((action, context)) = machine.pop() {
            produced.push(format!("{action:?} @ {context:?}"));
        }
    }
    produced.push(format!("{:?}", machine.mode()));

    produced
}

/// # Returns
///
/// The key a terminal reports when `character` is typed with no modifier held.
fn typed(character: char) -> TerminalKey {
    vbc_editor::engine::typed(character).into()
}

/// # Returns
///
/// `keys` spelled the way vim's manual spells them.
fn shown(keys: &[TerminalKey]) -> String {
    keys.iter().map(ToString::to_string).collect()
}
