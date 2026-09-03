//! The shift operators cross-checked against the vim they are taken from.
//!
//! `>` and `<` are the one family of operators modalkit ships as a stub: `EditBuffer::indent`
//! returns without touching the text, so every one of `>>`, `<<`, `>j` and `>gj` used to leave the
//! buffer byte for byte as it was. That is the exact shape of failure this workspace keeps hitting
//! -- a keystroke that quietly does nothing passes any assertion held to the text alone -- so the
//! first thing asserted here is not that the engine agrees with vim but that vim itself moves.
//! Every case meant to shift something is required to leave vim's text different from the text it
//! started with, which is what stops the comparison from passing against a stub on both sides.
//!
//! What a shift writes out depends on more than the keys. The same `>>` lays down four spaces
//! where `'shiftwidth'` is four and a tab where it is eight, and lays down spaces rather than tabs
//! where `'expandtab'` is set, so every case is replayed under three spellings of the same indent:
//! a shift narrower than a tab stop, one that reaches a tab stop and is written in tabs, and one
//! written in spaces alone. A test that replayed one spelling could not see an engine that ignored
//! either option, so the spellings are required to disagree with each other as well as to agree
//! with vim.
//!
//! A shift is linewise however it was reached, which is what `>gj` is here to hold. A display
//! motion stops on a screen row, and a wrapped line has several; shifting the row rather than the
//! logical line would move a fragment of the text and leave the rest of the line where it was.
//! The wrapped cases are laid out in a window narrow enough that their first line takes three rows,
//! so an engine shifting rows rather than lines writes a different buffer than vim does.
//!
//! A shift also has to be a change that can be taken back. `u` is not a stub the way
//! `EditBuffer::indent` is: the keybindings issue a checkpoint after every edit and the buffer
//! runs it, so an engine that writes a shift out by replacing the whole text -- which
//! reinitializes the buffer's undo history -- leaves `u` with nothing to undo and takes the edits
//! made before the shift out of reach with it. Nothing in a comparison of the text a shift leaves
//! behind can see that, so the undo cases below type the `u` themselves.
//!
//! The rest is the arithmetic vim does at the edges: a count that reaches past the last line, a
//! motion that starts on it, an outdent already at column zero, an indent far past any width a
//! terminal could draw, a line of nothing but blanks and a line of nothing at all. Vim shifts a
//! line of blanks and leaves an empty line alone, which is the off-by-one an implementation
//! reading "the first non-blank" too literally gets wrong in both directions.

mod outcome;

use std::collections::BTreeSet;
use std::num::NonZeroUsize;

use crossterm::event::{KeyCode, KeyModifiers};
use vbc_editor::engine::{typed, Engine, Error};
use vbc_editor::event::KeyEvent;
use vbc_editor::indent::Shift;
use vbc_editor::screen::Geometry;
use vbc_layout::width::{AmbiWidth, Metrics};
use vbc_oracle::corpus::{Case as CorpusCase, Options as CaseOptions};
use vbc_oracle::vim::VimDriver;

use crate::outcome::Outcome;

/// One cross-check: a starting text, the window it is laid out in, and the keys typed at it.
struct Case {
    id: &'static str,
    text: &'static str,
    columns: u16,
    keys: &'static str,
}

/// One way of spelling the indent a shift lays down: how far a step carries a line, how wide a tab
/// carries it, and whether an indent is written in tabs at all.
#[derive(Clone, Copy)]
struct Spelling {
    id: &'static str,
    shiftwidth: u16,
    tabstop: u16,
    expandtab: bool,
}

/// How vim's notation names the keys these cases do not spell with the character they type. A `<`
/// that begins none of these is the outdent operator and stands for itself, which is what these
/// cases spell an outdent with.
const NAMED: [(&str, KeyCode, KeyModifiers); 2] = [
    ("<Esc>", KeyCode::Esc, KeyModifiers::NONE),
    ("<C-v>", KeyCode::Char('v'), KeyModifiers::CONTROL),
];

/// The window the wrapped cases are laid out in, narrow enough that their first line takes three
/// rows, so a shift over a display motion has rows to be wrong about.
const COLUMNS: u16 = 20;

/// The window the unwrapped cases are laid out in, wide enough that no line of theirs wraps.
const WIDE: u16 = 80;

/// The screen lines every case is laid out in, more than any of them fills.
const ROWS: u16 = 24;

/// The four spellings every case is replayed under. The first writes an indent no tab reaches, the
/// second writes one in tabs, the third writes one in spaces however wide it is, and the fourth
/// draws its tabs to a stop no other spelling uses, so an engine ignoring any of `'shiftwidth'`,
/// `'expandtab'` and `'tabstop'` disagrees with vim under at least one of them. A tab is worth the
/// columns it takes to reach the next stop rather than a fixed number of them, so a spelling that
/// never moved the stop could not see an engine measuring every tab as eight columns.
const SPELLINGS: [Spelling; 4] = [
    Spelling {
        id: "sw4-ts8-noexpandtab",
        shiftwidth: 4,
        tabstop: 8,
        expandtab: false,
    },
    Spelling {
        id: "sw8-ts8-noexpandtab",
        shiftwidth: 8,
        tabstop: 8,
        expandtab: false,
    },
    Spelling {
        id: "sw4-ts8-expandtab",
        shiftwidth: 4,
        tabstop: 8,
        expandtab: true,
    },
    Spelling {
        id: "sw2-ts4-noexpandtab",
        shiftwidth: 2,
        tabstop: 4,
        expandtab: false,
    },
];

/// Three plain lines, none of them wide enough to wrap.
const PROSE: &str = "alpha\nbeta\ngamma\n";

/// The same lines already carrying an indent of a tab apiece, which is what an outdent needs to
/// have something to take away.
const INDENTED: &str = "\talpha\n\tbeta\n\tgamma\n";

/// A line of nothing but blanks and a line of nothing at all, which vim treats differently: the
/// blanks are shifted with everything else and the empty line is left exactly as it is.
const BLANKS: &str = "alpha\n   \n\ngamma\n";

/// A first line long enough to take three rows in a twenty-column window, so that a display motion
/// out of it can stop on a row that is not the row it started on.
const WRAPPED: &str = "abcdefghijklmnopqrstuvwxyz0123456789abcdefg\nsecond\nthird\n";

/// The same wrapped text already carrying an indent, which is what an outdent over a display
/// motion needs to have something to take away.
const INDENTED_WRAPPED: &str = "\tabcdefghijklmnopqrstuvwxyz0123456789abcdefg\n\tsecond\n\tthird\n";

/// A display motion that stays inside the wrapped line it started in: one screen row down is still
/// the first logical line, so the shift covers that line and no other.
const WITHIN: Case = Case {
    id: "indent over a display motion staying inside a wrapped line",
    text: WRAPPED,
    columns: COLUMNS,
    keys: ">gj",
};

/// A display motion that leaves the wrapped line it started in: four screen rows down is the first
/// row of the third logical line, and vim's rule for an exclusive motion stopping in the first
/// column of a line leaves the shift covering the two lines above it.
const ACROSS: Case = Case {
    id: "indent over a display motion crossing out of a wrapped line",
    text: WRAPPED,
    columns: COLUMNS,
    keys: ">4gj",
};

/// The case the three spellings are held apart by: an indent already reaching a tab stop, so that
/// a step of four columns, a step of eight and a step written in spaces alone each write the line
/// out differently. An indent narrower than a tab stop is spelled in spaces whatever `'expandtab'`
/// says, which is why a case shifting an unindented line cannot tell the three apart.
const SPELLED: Case = Case {
    id: "indent a line already indented to a tab stop",
    text: INDENTED,
    columns: WIDE,
    keys: ">>",
};

/// The indenting commands this seam does not run: vim's automatic reindent, which works an indent
/// out from the text around it rather than from a count, a shift over a target this seam does not
/// turn into whole lines, and a shift over a blockwise selection, which vim lays down at the
/// block's own left column rather than at the start of the lines the block spans. All are refused
/// rather than run over a guess, because an editor that answers them by editing nothing -- or, in
/// the blockwise case, by editing the wrong columns -- is one whose tests pass against a keystroke
/// that did something else entirely.
const REFUSED: [&str; 4] = ["==", ">w", ">}", "ll<C-v>j>"];

/// The blockwise shift the seam refuses, and the linewise one it runs. vim answers the two
/// differently, which is the whole reason the blockwise one cannot be run as though it named the
/// lines its block spans.
const BLOCKWISE: Case = Case {
    id: "indent the columns of a blockwise selection",
    text: PROSE,
    columns: WIDE,
    keys: "ll<C-v>j>",
};

/// The linewise shift covering the same lines, which is the answer a seam that read a blockwise
/// selection as the lines it spans would give.
const LINEWISE: Case = Case {
    id: "indent the lines a blockwise selection spans",
    text: PROSE,
    columns: WIDE,
    keys: "Vj>",
};

/// The cases whose keys leave vim's text different from the text they started with, which is every
/// case that is not asserting an edge vim answers by editing nothing.
const SHIFTED: [Case; 28] = [
    Case {
        id: "indent one line",
        text: PROSE,
        columns: WIDE,
        keys: ">>",
    },
    Case {
        id: "indent one line twice over",
        text: PROSE,
        columns: WIDE,
        keys: ">>>>",
    },
    Case {
        id: "outdent one line",
        text: INDENTED,
        columns: WIDE,
        keys: "<<",
    },
    Case {
        id: "indent three lines by a count",
        text: PROSE,
        columns: WIDE,
        keys: "3>>",
    },
    Case {
        id: "outdent three lines by a count",
        text: INDENTED,
        columns: WIDE,
        keys: "3<<",
    },
    Case {
        id: "indent a count of lines that reaches past the last one",
        text: PROSE,
        columns: WIDE,
        keys: "9>>",
    },
    Case {
        id: "indent over a line motion down",
        text: PROSE,
        columns: WIDE,
        keys: ">j",
    },
    Case {
        id: "indent over a counted line motion down",
        text: PROSE,
        columns: WIDE,
        keys: ">2j",
    },
    Case {
        id: "indent over a line motion down that reaches past the last line",
        text: PROSE,
        columns: WIDE,
        keys: ">5j",
    },
    Case {
        id: "outdent over a line motion down",
        text: INDENTED,
        columns: WIDE,
        keys: "<j",
    },
    Case {
        id: "indent over a line motion up",
        text: PROSE,
        columns: WIDE,
        keys: "G>k",
    },
    Case {
        id: "indent over a counted line motion up",
        text: PROSE,
        columns: WIDE,
        keys: "G>2k",
    },
    Case {
        id: "indent the lines of a linewise selection",
        text: PROSE,
        columns: WIDE,
        keys: "Vj>",
    },
    Case {
        id: "indent a linewise selection by a count",
        text: PROSE,
        columns: WIDE,
        keys: "V3>",
    },
    Case {
        id: "indent the lines a charwise selection touches",
        text: PROSE,
        columns: WIDE,
        keys: "vj>",
    },
    Case {
        id: "outdent a selection reaching upwards",
        text: INDENTED,
        columns: WIDE,
        keys: "GVk<",
    },
    Case {
        id: "indent a line of blanks and leave the empty line alone",
        text: BLANKS,
        columns: WIDE,
        keys: "4>>",
    },
    Case {
        id: "indent the line of blanks on its own",
        text: BLANKS,
        columns: WIDE,
        keys: "j>>",
    },
    Case {
        id: "outdent the line of blanks on its own",
        text: BLANKS,
        columns: WIDE,
        keys: "j<<",
    },
    Case {
        id: "indent past any width a terminal could draw",
        text: PROSE,
        columns: WIDE,
        keys: "V400>",
    },
    Case {
        id: "outdent further than the indent reaches",
        text: INDENTED,
        columns: WIDE,
        keys: "V400<",
    },
    Case {
        id: "indent to the end of the buffer",
        text: PROSE,
        columns: WIDE,
        keys: ">G",
    },
    Case {
        id: "indent back to the start of the buffer",
        text: PROSE,
        columns: WIDE,
        keys: "G>gg",
    },
    Case {
        id: "indent to a numbered line",
        text: PROSE,
        columns: WIDE,
        keys: ">2G",
    },
    Case {
        id: "indent to a numbered line past the last one",
        text: PROSE,
        columns: WIDE,
        keys: ">9G",
    },
    WITHIN,
    ACROSS,
    Case {
        id: "outdent over a display motion down out of a wrapped line",
        text: INDENTED_WRAPPED,
        columns: COLUMNS,
        keys: "<4gj",
    },
];

/// The cases vim answers by editing nothing: a shift that ran out of text, an outdent with no
/// indent left to take, and the empty line vim refuses to indent.
const UNSHIFTED: [Case; 6] = [
    Case {
        id: "outdent a line already at column zero",
        text: PROSE,
        columns: WIDE,
        keys: "<<",
    },
    Case {
        id: "indent the empty line",
        text: BLANKS,
        columns: WIDE,
        keys: "jj>>",
    },
    Case {
        id: "indent a count of lines from the last line",
        text: PROSE,
        columns: WIDE,
        keys: "G3>>",
    },
    Case {
        id: "indent over a line motion down from the last line",
        text: PROSE,
        columns: WIDE,
        keys: "G>j",
    },
    Case {
        id: "indent over a line motion up from the first line",
        text: PROSE,
        columns: WIDE,
        keys: ">k",
    },
    Case {
        id: "indent over a display motion up from the first row",
        text: WRAPPED,
        columns: COLUMNS,
        keys: ">gk",
    },
];

/// The cases that shift something and then take it back. A shift has to be one undoable change,
/// and it has to leave the changes made before it undoable too: `u` is not a stub the way
/// `EditBuffer::indent` is, so an engine that writes a shift out by replacing the whole text --
/// which reinitializes the buffer's undo history -- answers every one of these by leaving the
/// shift standing, and the last of them by losing the edit that came before it as well.
const UNDONE: [Case; 5] = [
    Case {
        id: "undo an indent",
        text: PROSE,
        columns: WIDE,
        keys: ">>u",
    },
    Case {
        id: "undo a counted indent",
        text: PROSE,
        columns: WIDE,
        keys: "3>>u",
    },
    Case {
        id: "undo an outdent",
        text: INDENTED,
        columns: WIDE,
        keys: "<<u",
    },
    Case {
        id: "undo an indent of a selection",
        text: PROSE,
        columns: WIDE,
        keys: "Vj>u",
    },
    Case {
        id: "undo an edit made before an indent",
        text: PROSE,
        columns: WIDE,
        keys: "x3>>uu",
    },
];

/// A delete taken back, which has no shift anywhere in it. It is here to name whose the one thing
/// the undo cases do not compare is: modalkit carries the cursor an undo was standing at by the
/// change it takes back, where vim puts it at the start of the line it changed, and the two have
/// disagreed about that since long before `>` did anything at all.
const TAKEN_BACK: Case = Case {
    id: "undo a delete",
    text: INDENTED,
    columns: WIDE,
    keys: "xu",
};

#[test]
fn a_shift_is_one_undoable_change_that_leaves_the_edits_before_it_undoable() -> anyhow::Result<()> {
    let vim = VimDriver::new()?;

    for case in &UNDONE {
        for spelling in &SPELLINGS {
            let expected = vim_outcome(&vim, case, spelling)?;
            assert_eq!(
                case.text, expected.text,
                "`{}` under {} does not take vim's own text back to where it started, so the \
                 comparison below is not holding an undo to anything",
                case.id, spelling.id
            );

            let held = engine_outcome(case, spelling);
            assert_eq!(
                expected.text, held.text,
                "`{}` under {} left the engine holding text vim does not hold, so the shift is \
                 not a change `u` takes back",
                case.id, spelling.id
            );
            assert_eq!(
                expected.mode, held.mode,
                "`{}` under {} left the engine in a mode vim is not in",
                case.id, spelling.id
            );
            assert_eq!(
                expected.registers, held.registers,
                "`{}` under {} left the engine holding registers vim does not hold",
                case.id, spelling.id
            );
        }
    }

    Ok(())
}

#[test]
fn the_place_an_undo_leaves_the_cursor_is_modalkits_whatever_it_takes_back() -> anyhow::Result<()> {
    let vim = VimDriver::new()?;

    for spelling in &SPELLINGS {
        let expected = vim_outcome(&vim, &TAKEN_BACK, spelling)?;
        let held = engine_outcome(&TAKEN_BACK, spelling);
        assert_eq!(
            expected.text, held.text,
            "`{}` under {} did not take the delete back",
            TAKEN_BACK.id, spelling.id
        );

        assert_ne!(
            (expected.line, expected.column),
            (held.line, held.column),
            "`{}` under {} now leaves the cursor where vim leaves it, so modalkit's undo has \
             stopped diverging and the shift cases above should be comparing the whole outcome \
             rather than the text, the mode and the registers alone",
            TAKEN_BACK.id,
            spelling.id
        );
    }

    Ok(())
}

#[test]
fn every_case_meant_to_shift_something_moves_vims_own_text() -> anyhow::Result<()> {
    let vim = VimDriver::new()?;

    for case in &SHIFTED {
        for spelling in &SPELLINGS {
            let expected = vim_outcome(&vim, case, spelling)?;

            assert_ne!(
                case.text, expected.text,
                "`{}` under {} leaves vim's own text as it was, so the comparison below would \
                 pass against an engine whose shift operators do nothing at all",
                case.id, spelling.id
            );
        }
    }

    Ok(())
}

#[test]
fn a_shift_ends_where_vim_ends() -> anyhow::Result<()> {
    let vim = VimDriver::new()?;

    for case in SHIFTED.iter().chain(UNSHIFTED.iter()) {
        for spelling in &SPELLINGS {
            assert_eq!(
                vim_outcome(&vim, case, spelling)?,
                engine_outcome(case, spelling),
                "`{}` under {} left the engine somewhere other than where vim left it",
                case.id,
                spelling.id
            );
        }
    }

    Ok(())
}

#[test]
fn a_case_vim_answers_by_editing_nothing_leaves_the_engines_text_alone() -> anyhow::Result<()> {
    let vim = VimDriver::new()?;

    for case in &UNSHIFTED {
        for spelling in &SPELLINGS {
            assert_eq!(
                case.text,
                vim_outcome(&vim, case, spelling)?.text,
                "`{}` under {} is meant to leave vim's own text untouched and does not, so it \
                 belongs among the cases that shift something",
                case.id,
                spelling.id
            );
        }
    }

    Ok(())
}

#[test]
fn the_spellings_write_the_same_shift_out_differently() -> anyhow::Result<()> {
    let vim = VimDriver::new()?;

    let written: BTreeSet<String> = SPELLINGS
        .iter()
        .map(|spelling| vim_outcome(&vim, &SPELLED, spelling).map(|outcome| outcome.text))
        .collect::<anyhow::Result<_>>()?;

    assert_eq!(
        SPELLINGS.len(),
        written.len(),
        "two of the spellings write the same indent, so replaying a case under all three would \
         not see an engine ignoring `shiftwidth` or `expandtab`"
    );

    Ok(())
}

#[test]
fn an_engine_spelling_an_indent_the_other_way_diverges_from_vim() -> anyhow::Result<()> {
    let vim = VimDriver::new()?;

    for spelling in &SPELLINGS {
        let expected = vim_outcome(&vim, &SPELLED, spelling)?;
        for other in SPELLINGS.iter().filter(|other| other.id != spelling.id) {
            assert_ne!(
                expected,
                engine_outcome(&SPELLED, other),
                "`{}` agreed with the vim of {}, so the engine is not reading the options the \
                 case declares",
                other.id,
                spelling.id
            );
        }
    }

    Ok(())
}

#[test]
fn a_shift_over_a_display_motion_carries_whole_logical_lines() -> anyhow::Result<()> {
    let vim = VimDriver::new()?;
    let one_line = Case {
        id: "indent one line of the wrapped text",
        text: WRAPPED,
        columns: COLUMNS,
        keys: ">>",
    };
    let two_lines = Case {
        id: "indent two lines of the wrapped text",
        text: WRAPPED,
        columns: COLUMNS,
        keys: ">j",
    };

    for spelling in &SPELLINGS {
        let held = vim_outcome(&vim, &one_line, spelling)?;
        let crossed = vim_outcome(&vim, &two_lines, spelling)?;
        assert_ne!(
            held.text, crossed.text,
            "one whole logical line and two of them leave vim's text the same under {}, so the \
             two comparisons below say the same thing",
            spelling.id
        );

        assert_eq!(
            held.text,
            vim_outcome(&vim, &WITHIN, spelling)?.text,
            "vim shifts something other than the whole logical line for `{}` under {}",
            WITHIN.id,
            spelling.id
        );
        assert_eq!(
            crossed.text,
            vim_outcome(&vim, &ACROSS, spelling)?.text,
            "vim shifts something other than the whole logical lines for `{}` under {}",
            ACROSS.id,
            spelling.id
        );
        assert_eq!(
            held.text,
            engine_outcome(&WITHIN, spelling).text,
            "`{}` under {} shifted something other than the whole logical line its motion stayed \
             inside",
            WITHIN.id,
            spelling.id
        );
        assert_eq!(
            crossed.text,
            engine_outcome(&ACROSS, spelling).text,
            "`{}` under {} shifted something other than the whole logical lines its motion \
             crossed",
            ACROSS.id,
            spelling.id
        );
    }

    Ok(())
}

#[test]
fn an_indenting_command_whose_lines_the_seam_cannot_work_out_is_refused() {
    for keys in REFUSED {
        let mut engine = Engine::new(PROSE);
        let error = engine
            .press_all(typed_keys(keys))
            .expect_err("an indenting command the seam does not run is refused");

        assert!(
            matches!(error, Error::Unindentable { .. }),
            "`{keys}` stopped as `{error:?}` rather than being refused as an indenting command \
             this seam does not run"
        );
        assert_eq!(
            PROSE,
            engine.text(),
            "`{keys}` was refused after editing part of the text"
        );
    }
}

#[test]
fn a_blockwise_shift_is_not_the_linewise_one_over_the_lines_it_spans() -> anyhow::Result<()> {
    let vim = VimDriver::new()?;

    for spelling in &SPELLINGS {
        assert_ne!(
            vim_outcome(&vim, &BLOCKWISE, spelling)?.text,
            vim_outcome(&vim, &LINEWISE, spelling)?.text,
            "vim writes `{}` and `{}` out the same under {}, so refusing the blockwise shift \
             rather than running it over the lines it spans is refusing nothing",
            BLOCKWISE.id,
            LINEWISE.id,
            spelling.id
        );
    }

    Ok(())
}

#[test]
fn a_mark_set_before_a_shift_still_names_the_line_it_was_set_on() {
    let mut engine = Engine::new(PROSE);
    engine
        .press_all(typed_keys("jmagg3>>`a"))
        .expect("the keys run against the engine");

    assert_eq!(
        1,
        engine.cursor().line,
        "the mark set before the shift no longer names the line it was set on, so writing the \
         shifted text out took the buffer's marks with it"
    );
}

/// # Returns
///
/// What the engine was left holding after the case's keys were typed at its text, laid out in its
/// window and spelling its indent the way `spelling` says.
///
/// # Panics
///
/// Panics if the case's window is zero cells wide, or if the keys do not run.
fn engine_outcome(case: &Case, spelling: &Spelling) -> Outcome {
    let columns = NonZeroUsize::new(usize::from(case.columns)).expect("the columns are not zero");
    let rows = NonZeroUsize::new(usize::from(ROWS)).expect("the rows are not zero");
    let tab_stop =
        NonZeroUsize::new(usize::from(spelling.tabstop)).expect("the tab stop is not zero");
    let geometry =
        Geometry::new(columns, rows).with_metrics(Metrics::new(AmbiWidth::Single, tab_stop));
    let shift = Shift::new(
        usize::from(spelling.shiftwidth),
        tab_stop,
        spelling.expandtab,
    );
    let mut engine = Engine::laid_out_in(case.text, geometry).indenting_by(shift);
    engine
        .press_all(typed_keys(case.keys))
        .expect("the keys run against the engine");

    Outcome::of(&mut engine)
}

/// # Returns
///
/// What vim was left holding after the case's keys were typed at its text, laid out in its window
/// and spelling its indent the way `spelling` says, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`VimDriver::run_case`]'s return values on failure.
fn vim_outcome(vim: &VimDriver, case: &Case, spelling: &Spelling) -> anyhow::Result<Outcome> {
    let state = vim.run_case(&CorpusCase {
        id: case.id.to_owned(),
        description: case.id.to_owned(),
        buffer: case.text.to_owned(),
        keys: case.keys.to_owned(),
        viewport_width: case.columns,
        viewport_height: ROWS,
        tags: BTreeSet::new(),
        options: CaseOptions {
            tabstop: spelling.tabstop,
            shiftwidth: spelling.shiftwidth,
            expandtab: spelling.expandtab,
            ..CaseOptions::default()
        },
    })?;

    Ok(state.into())
}

/// # Returns
///
/// The key events `keys` stands for, in which the spellings of [`NAMED`] stand for the keys they
/// name and every other character stands for itself.
fn typed_keys(keys: &str) -> Vec<KeyEvent> {
    let mut typed_keys = Vec::new();
    let mut rest = keys;
    while let Some((index, name, code, modifiers)) = NAMED
        .iter()
        .filter_map(|(name, code, modifiers)| {
            rest.find(name)
                .map(|index| (index, *name, *code, *modifiers))
        })
        .min_by_key(|(index, ..)| *index)
    {
        typed_keys.extend(rest[..index].chars().map(typed));
        typed_keys.push(KeyEvent::new(code, modifiers));
        rest = &rest[index + name.len()..];
    }
    typed_keys.extend(rest.chars().map(typed));

    typed_keys
}
