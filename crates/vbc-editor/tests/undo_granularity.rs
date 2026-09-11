//! Cross-checks what one `u` takes back against the vim the undo was copied from.
//!
//! An operator the seam answers is not one action reaching the text. A delete over a display
//! motion is re-issued against a mark the shim wrote; a shift over one is spliced into the buffer
//! a line at a time, each splice an insert and a delete of its own; a `.` repeats the keys of a
//! change rather than the change itself. Every one of those is a keystroke a reader typed once, so
//! vim takes the whole of it back with one `u`, and an engine that filed a checkpoint in the
//! middle of one would leave `u` taking back half an edit and the reader typing `u` again to find
//! the rest.
//!
//! Nothing about that granularity is visible in a cursor or in a register: an undo that took back
//! half a shift leaves a text that is neither the text before the keystroke nor the text after it,
//! and only the text says so. So every case here is replayed against a real vim in the window the
//! case declares, and compared in the record `vim_engine.rs` shares -- the text, the cursor, the
//! mode and every register with the type a put would reinsert it with -- rather than against an
//! expectation written out by hand.
//!
//! Three things are asserted of each change beyond agreeing with vim, because agreement alone
//! would also be reached by an engine that did nothing at all:
//!
//! * The change left the text other than it found it, so an undo of it has something to restore.
//! * One `u` restores the text the change was typed against, exactly.
//! * One `C-r` puts back what that `u` took, exactly.
//!
//! The changes the seam answers are compared beside the ones it hands straight to modalkit --
//! `dw`, `d$`, `dj` and `dd` -- which are the control group: their undo was never in question, and
//! a change to the seam that broke them would break this file too.
//!
//! One dimension of the record is compared for a change and not for the undo behind it, and is
//! left out in writing rather than quietly. **Where an undo leaves the cursor is modalkit's answer
//! rather than vim's**: vim records the extent of each change and puts the cursor at the start of
//! the one it took back, where modalkit reconstructs an older rope and carries every cursor
//! through the difference between the two, which leaves it at the far end of the text that came
//! back. `dgj` then `u` restores the same twenty characters in both and rests on column 20 here
//! and on column 0 in vim. It is the same answer for the changes the seam never touches -- `dw`,
//! `dj` and `dd` all diverge the same way -- so it is a difference about modalkit's undo rather
//! than about this seam, and `the_cursor_an_undo_leaves_is_modalkits_rather_than_vims` pins it
//! with both numbers so that an engine which starts placing it where vim does fails this file.
//!
//! One shape of change is out of this file's reach rather than out of its interest, and is named
//! here so that it is not mistaken for covered. A shift reaching more than one logical line -- a
//! `>5gj` out of a line that wraps -- is undone correctly and then panics on the `C-r` behind the
//! `u`, inside the rope diff modalkit reconstructs an older buffer with. It is not the seam's:
//! `2>>`, `>j` and `Vj>` over the same text panic in exactly the same place, and none of the three
//! reaches the shim at all. Nothing here is tuned around it; the sample holds the shifts that
//! reach one line until the crash below it is fixed.
//!
//! Two changes are repeated by `.` in a way that already disagrees with vim before any undo is
//! typed -- a `d$` over a line the first one emptied, and a `3dd` over a text with fewer than
//! three lines left -- so an undo after either would be reporting the repeat rather than the undo.
//! They are left out of the repeated cases and asserted as already disagreeing, so that a repeat
//! which starts agreeing fails this file rather than quietly staying out of the sample.

mod notation;
mod outcome;

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroUsize;

use vbc_editor::engine::Engine;
use vbc_editor::screen::Geometry;
use vbc_oracle::corpus;
use vbc_oracle::state::{Mode, Register};
use vbc_oracle::vim::VimDriver;

use crate::notation::keys;
use crate::outcome::Outcome;

/// The text every case is replayed against: five logical lines, the first and third of which wrap
/// into more than one row of the window below, so a display motion out of either takes part of a
/// line where a logical motion takes the whole of it.
const WRAPPED: &str = concat!(
    "the quick brown fox jumps over the lazy dog\n",
    "second line here\n",
    "a third line that also wraps around this window\n",
    "fourth\n",
    "fifth line here\n",
);

/// A text of lines that begin with no indent at all, on which a shift moves every line it reaches
/// and none of them is already indented as far as the shift would carry it.
const FLUSH: &str = concat!(
    "one line that is long enough to wrap around this narrow window\n",
    "two\n",
    "three\n",
    "four\n",
);

/// The texts every change is replayed against, each named for what makes it worth replaying.
const TEXTS: [(&str, &str); 2] = [("wrapped", WRAPPED), ("flush", FLUSH)];

/// The cells the window every case is laid out in is wide.
const COLUMNS: u16 = 20;

/// The screen lines the window every case is laid out in is tall.
const ROWS: u16 = 10;

/// The keys of one change apiece that the seam answers rather than handing on: an operator over a
/// display motion, an operator over a counted one, an operator over one that walks the other way,
/// the change that leaves an inserting mode behind it, and the shift the seam runs itself a splice
/// at a time. `3dd` is here for the count rather than for the seam: it is one keystroke reaching
/// three lines, which is the other shape of change one `u` has to take the whole of back.
const SHIMMED: [(&str, &str); 8] = [
    ("delete down a display line", "dgj"),
    ("delete down two display lines", "d2gj"),
    ("delete to the end of a display line", "dg$"),
    ("delete up a display line into the line above", "jjdgk"),
    ("change down a display line", "cgjtyped<Esc>"),
    ("shift down a display line", ">gj"),
    ("shift down three display lines", ">3gj"),
    ("delete three whole lines", "3dd"),
];

/// The keys of one change apiece that reach modalkit exactly as they did before the seam existed,
/// which is the control group: their undo was never in question and a change to the seam that
/// broke it would break this file too.
const LOGICAL: [(&str, &str); 4] = [
    ("delete a word", "dw"),
    ("delete to the end of a line", "d$"),
    ("delete down a logical line", "dj"),
    ("delete a whole line", "dd"),
];

/// The changes whose `.` already disagrees with vim before any undo is typed, each with the reason,
/// so that a repeat which starts agreeing fails this file rather than quietly joining the sample.
/// What `.` repeats is `dot_repeat.rs`'s to hold to vim; an undo typed behind one of these would
/// be reporting the repeat rather than the undo.
const REPEATS_THAT_ALREADY_DISAGREE: [(&str, &str); 2] = [
    (
        "d$",
        "the first `d$` empties the line, and the repeat over an empty line leaves vim's registers \
         holding what they held",
    ),
    (
        "3dd",
        "the repeat asks for three lines of a text with one line left, which vim refuses rather \
         than clamping",
    ),
];

/// The change whose second keystroke reaches a line the first one had already emptied, so typing
/// it twice leaves the text where typing it once left it and an undo of the second would say
/// nothing about granularity. It is named here, and asserted to still be that change, rather than
/// left out of the doubled cases without a word.
const UNCHANGED_BY_A_SECOND_KEYSTROKE: &str = "d$";

/// One pinned case: the keys, the line and column this engine rests on, and the line and column
/// vim rests on.
type Pinned = (&'static str, (u64, u64), (u64, u64));

/// The undos pinned with the cursor both engines leave, which is what says the divergence written
/// down at the top of this file is still the divergence there is.
const PINNED_CURSORS: [Pinned; 2] = [("dgju", (0, 20), (0, 0)), ("jjdgku", (2, 0), (1, 0))];

/// What a change and the undo behind it leave that this engine and vim agree on, which is the
/// whole of the record but the cursor. Where an undo leaves the cursor is pinned as a divergence
/// of its own, with the reason, by `the_cursor_an_undo_leaves_is_modalkits_rather_than_vims`.
#[derive(Debug, Eq, PartialEq)]
struct Restored {
    text: String,
    mode: Mode,
    registers: BTreeMap<char, Register>,
}

impl From<Outcome> for Restored {
    fn from(outcome: Outcome) -> Self {
        Self {
            text: outcome.text,
            mode: outcome.mode,
            registers: outcome.registers,
        }
    }
}

#[test]
fn every_change_leaves_the_engine_where_vim_leaves_it() -> anyhow::Result<()> {
    let vim = VimDriver::new()?;

    for (name, text) in TEXTS {
        for (id, change) in SHIMMED.into_iter().chain(LOGICAL) {
            assert_eq!(
                vim_outcome(&vim, text, change)?,
                replayed(text, change)?,
                "`{id}` left the engine somewhere other than where vim left it, on the {name} text"
            );
        }
    }

    Ok(())
}

#[test]
fn every_undo_and_redo_restores_what_vim_restores() -> anyhow::Result<()> {
    let vim = VimDriver::new()?;

    for (name, text) in TEXTS {
        for (id, change) in SHIMMED.into_iter().chain(LOGICAL) {
            for keys in [format!("{change}u"), format!("{change}u<C-r>")] {
                assert_eq!(
                    Restored::from(vim_outcome(&vim, text, &keys)?),
                    Restored::from(replayed(text, &keys)?),
                    "`{keys}` left a text, a mode or a register other than vim's, on the {name} \
                     text where the change is `{id}`"
                );
            }
        }
    }

    Ok(())
}

#[test]
fn one_undo_takes_back_the_whole_of_one_change() -> anyhow::Result<()> {
    for (name, text) in TEXTS {
        for (id, change) in SHIMMED.into_iter().chain(LOGICAL) {
            assert_ne!(
                text,
                replayed(text, change)?.text,
                "`{id}` left the {name} text as it found it, so an undo of it restores nothing"
            );
            assert_eq!(
                text,
                replayed(text, &format!("{change}u"))?.text,
                "one `u` after `{id}` left the {name} text neither where the change found it nor \
                 where the change left it"
            );
        }
    }

    Ok(())
}

#[test]
fn one_undo_takes_back_one_change_and_not_the_one_in_front_of_it() -> anyhow::Result<()> {
    for (name, text) in TEXTS {
        for (id, change) in SHIMMED.into_iter().chain(LOGICAL) {
            let once = replayed(text, change)?.text;
            let twice = replayed(text, &format!("{change}{change}"))?.text;
            if UNCHANGED_BY_A_SECOND_KEYSTROKE == change {
                assert_eq!(
                    once, twice,
                    "`{id}` typed twice now leaves the {name} text somewhere typing it once does \
                     not, so it belongs among the doubled cases rather than out of them"
                );

                continue;
            }
            assert_ne!(
                once, twice,
                "`{id}` typed twice left the {name} text where typing it once left it, so an undo \
                 of the second says nothing"
            );
            assert_eq!(
                once,
                replayed(text, &format!("{change}{change}u"))?.text,
                "one `u` after `{id}` typed twice took back more or less than the second of them, \
                 on the {name} text"
            );
        }
    }

    Ok(())
}

#[test]
fn a_redo_puts_back_exactly_what_the_undo_took() -> anyhow::Result<()> {
    for (name, text) in TEXTS {
        for (id, change) in SHIMMED.into_iter().chain(LOGICAL) {
            assert_eq!(
                replayed(text, change)?.text,
                replayed(text, &format!("{change}u<C-r>"))?.text,
                "the `C-r` behind the `u` that took `{id}` back left the {name} text somewhere \
                 the change itself had not"
            );
        }
    }

    Ok(())
}

#[test]
fn an_undo_after_a_repeat_takes_back_the_repeat_alone() -> anyhow::Result<()> {
    let vim = VimDriver::new()?;

    for (name, text) in TEXTS {
        for (id, change) in repeated() {
            let repeated = format!("{change}.");
            let undone = format!("{change}.u");
            assert_eq!(
                Restored::from(vim_outcome(&vim, text, &repeated)?),
                Restored::from(replayed(text, &repeated)?),
                "the repeat of `{id}` disagrees with vim on the {name} text, so an undo behind it \
                 would report the repeat rather than the undo"
            );
            assert_eq!(
                Restored::from(vim_outcome(&vim, text, &undone)?),
                Restored::from(replayed(text, &undone)?),
                "an undo after a repeat of `{id}` restored something other than what vim restored, \
                 on the {name} text"
            );
            assert_eq!(
                replayed(text, change)?.text,
                replayed(text, &undone)?.text,
                "one `u` after a repeat of `{id}` took back more or less than the repeat, on the \
                 {name} text"
            );
        }
    }

    Ok(())
}

#[test]
fn a_repeat_this_file_leaves_out_is_a_repeat_that_already_disagrees() -> anyhow::Result<()> {
    let vim = VimDriver::new()?;

    for (change, reason) in REPEATS_THAT_ALREADY_DISAGREE {
        let repeated = format!("{change}.");
        let mut disagreed = false;
        for (_name, text) in TEXTS {
            disagreed |= Restored::from(vim_outcome(&vim, text, &repeated)?)
                != Restored::from(replayed(text, &repeated)?);
        }

        assert!(
            disagreed,
            "`{repeated}` agrees with vim on every text here, so leaving it out of the repeated \
             cases is no longer explained by `{reason}`"
        );
    }

    Ok(())
}

#[test]
fn the_cursor_an_undo_leaves_is_modalkits_rather_than_vims() -> anyhow::Result<()> {
    let vim = VimDriver::new()?;

    for (keys, ours, theirs) in PINNED_CURSORS {
        let left = replayed(WRAPPED, keys)?;
        let right = vim_outcome(&vim, WRAPPED, keys)?;

        assert_eq!(
            (ours, theirs),
            ((left.line, left.column), (right.line, right.column)),
            "`{keys}` no longer leaves the cursor where this divergence says it does, so the \
             reason written down for it no longer describes what happens"
        );
        assert_eq!(
            left.text, right.text,
            "`{keys}` restored a text other than vim's, which is more than a cursor apart"
        );
    }

    Ok(())
}

/// # Returns
///
/// The changes whose repeat is worth an undo, which is every change but the two whose `.` already
/// disagrees with vim.
fn repeated() -> Vec<(&'static str, &'static str)> {
    SHIMMED
        .into_iter()
        .chain(LOGICAL)
        .filter(|(_id, change)| {
            !REPEATS_THAT_ALREADY_DISAGREE
                .iter()
                .any(|(left_out, _reason)| left_out == change)
        })
        .collect()
}

/// # Returns
///
/// What the engine was left holding after `keys` were typed at `text`, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`Engine::press_all`]'s return values on failure.
fn replayed(text: &str, keys: &str) -> anyhow::Result<Outcome> {
    let mut engine = Engine::laid_out_in(text, geometry());
    engine.press_all(self::keys(keys))?;

    Ok(Outcome::of(&mut engine))
}

/// # Returns
///
/// What vim was left holding after the same keys were typed at the same text in the same window,
/// on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`VimDriver::run_case`]'s return values on failure.
fn vim_outcome(vim: &VimDriver, text: &str, keys: &str) -> anyhow::Result<Outcome> {
    let case = corpus::Case {
        id: "undo-granularity".to_owned(),
        description: "One `u` behind one change.".to_owned(),
        buffer: text.to_owned(),
        keys: keys.to_owned(),
        viewport_width: COLUMNS,
        viewport_height: ROWS,
        tags: BTreeSet::new(),
        options: corpus::Options::default(),
    };

    Ok(Outcome::from(vim.run_case(&case)?))
}

/// # Returns
///
/// The window the cases here are laid out in, on both sides of the comparison.
///
/// # Panics
///
/// Panics if the window is zero columns wide or zero rows tall, which it is not.
fn geometry() -> Geometry {
    let columns =
        NonZeroUsize::new(usize::from(COLUMNS)).expect("the window is not zero columns wide");
    let rows = NonZeroUsize::new(usize::from(ROWS)).expect("the window is not zero rows tall");

    Geometry::new(columns, rows)
}
