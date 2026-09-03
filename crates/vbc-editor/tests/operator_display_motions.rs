//! Operators applied over the motions counted in cells, cross-checked against the vim they are
//! taken from.
//!
//! A bare screen motion has one answer to be right about, which is where the cursor ends. An
//! operator applied over one has four: the text it left behind, where the cursor rests in it, the
//! mode the keys ended in, and what the registers hold and how a put would reinsert it. All four
//! are compared here, because the way this seam fails is not that it moves the cursor to the wrong
//! cell -- it is that it answers the motion and drops the operator, which leaves the text byte for
//! byte as it was and the cursor exactly where a correct `gj` would have put it. A comparison held
//! to the cursor alone passes against that, and so does one held to the buffer alone, since a
//! dropped operator changes neither.
//!
//! What that failure is caught by is asserted rather than assumed. Every case naming an operator
//! is replayed a second time with the operator taken out of the keys, which is what an engine
//! answering the motion and dropping the operator does, and the comparison is required to report
//! the difference. The exception is the group vim itself leaves undone -- an operator whose motion
//! ran out of text is abandoned by vim too, and carrying the cursor and editing nothing is the
//! right answer there rather than the degraded one.
//!
//! The register's type is compared as well as its text, so a delete that ran over the wrong shape
//! -- vim turns `d2gj` into a linewise delete and `d3gj` from a column of its own into a charwise
//! one -- is reported rather than passing on the text it happens to share.
//!
//! The cases are the ones an operator over a display motion is worth having: `gj` and `gk` down
//! and up, across a wrapped row and out of the logical line into the next, `g$` to the end of a
//! screen line, counted forms of each, and the operators `d`, `y` and `c`. They are typed at
//! prose, at an indented line, at a line of characters two cells wide and at a line of tabs,
//! because every rule vim applies to such an operator turns on one of those: which cell a grapheme
//! is drawn from, whether the motion ended in the first column of a line, and whether the cursor
//! stood in the line's indent when it did.
//!
//! Two of the rules turn on what the keys in front of the operator left the chain wanting rather
//! than on the text. A `$` leaves it wanting a column it holds no number for, which is what makes
//! the `gj` or `gk` behind it take the grapheme it stops on, where a `g$` leaves it wanting the
//! column it landed in and the motion behind that one stops in front of its grapheme. And a delete
//! reaching from a line's indent to the end of a later line takes whole lines, which is a rule vim
//! applies to a delete and to no other operator.
//!
//! Every case is replayed in four windows rather than one: undecorated, with `'showbreak'`, with
//! `'breakindent'` and with both. An undecorated window cannot tell a column measured against the
//! whole window from a column measured against the row's own text, because the two are the same
//! number there, and every rule deciding what an operator takes is read off the column a screen
//! motion landed in; a comparison made only in such a window passes against a seam that has the
//! two confused. Two decorated cases disagree with vim and are named as disagreeing, both for a
//! reason the operator has no part in: they are a row of tabs whose bare motion already lands a
//! grapheme away from where vim lands it.
//!
//! A count larger than the text has rows for is here twice over, because what is in front of it
//! decides the answer: vim clamps a bare `999gj` to the row the walk ran out on and abandons
//! `d999gj` with the buffer untouched. The second belongs to the abandoned group, which is
//! required to have left vim's own text alone before the two states are compared at all.
//!
//! The control group is the operators counted in characters -- `dw`, `d$`, `dj` -- typed at the
//! same texts through the same seam. They are what says the seam left everything it is not for
//! alone.
//!
//! The corpus is replayed through the same comparison, so the screen motions the corpus already
//! held are answered against vim rather than against the engine that could not answer them. Two of
//! its cases are named as diverging, each for a reason that is not this seam's: one draws a flag
//! as the cluster a terminal draws and vim draws it as the escapes vim draws, and one asks for a
//! window that scrolls sideways rather than wrapping, which is a viewport this engine does not
//! model. Four more stop before their keys reach a screen motion at all, because they type `|` and
//! the display-motion audit puts that out of scope; they are named too, so that a case that stops
//! for some other reason is reported rather than passed over.

mod outcome;

use std::collections::BTreeSet;
use std::num::NonZeroUsize;

use crossterm::event::KeyCode;
use vbc_editor::engine::{typed, Engine, Error};
use vbc_editor::event::KeyEvent;
use vbc_editor::screen::Geometry;
use vbc_layout::line::Options;
use vbc_layout::width::{AmbiWidth, Metrics};
use vbc_oracle::corpus::{
    self, AmbiWidth as CaseAmbiWidth, Case as CorpusCase, Corpus, Options as CaseOptions,
};
use vbc_oracle::state::{Register, RegisterType};
use vbc_oracle::vim::VimDriver;

use crate::outcome::Outcome;

/// One cross-check: a starting text, the window it is laid out in, and the keys typed at it, split
/// so that the operator can be taken back out of them.
struct Case {
    id: &'static str,
    text: &'static str,
    columns: u16,
    walked: &'static str,
    operator: &'static str,
    motion: &'static str,
    typed: &'static str,
}

impl Case {
    /// # Returns
    ///
    /// The keys the case types.
    fn keys(&self) -> String {
        format!(
            "{}{}{}{}",
            self.walked, self.operator, self.motion, self.typed
        )
    }

    /// # Returns
    ///
    /// The keys an engine that answered the motion and dropped the operator would have run, which
    /// is the same keys with the operator and everything it was going to take out of them.
    fn degraded(&self) -> String {
        format!("{}{}", self.walked, self.motion)
    }
}

/// The prose the operators below are typed at, whose first line wraps into two rows in a window
/// twenty columns wide and whose second and third lines each fit in one.
const PROSE: &str = "abcdefghijklmnopqrstuvwxyz0123456789\nsecond line here\nthird\n";

/// A line whose indent is what decides the shape a motion out of it takes, since vim turns a
/// motion ending in the first column of a line into a linewise one exactly when the cursor stood
/// in the indent of the line it started from.
const INDENTED: &str = "    indented first line that wraps around\nnext\n";

/// A line whose indent is wider than the window, so that a continuation row of its own begins
/// inside the indent. That is the one place a screen motion can leave an operator standing at a
/// column that is neither the start of a line nor past what vim calls the line's indent.
const DEEPLY_INDENTED: &str = "                         deep indent line\nnext\n";

/// A line of characters two cells wide apiece, on which a character column and a display column
/// are not the same number and modalkit's own arithmetic is wrong by a factor of two.
const WIDE: &str = "你好世界一二三四五六\nb\n";

/// A line of tabs, on which the cell a cursor is carried down from is the last of the cells its
/// grapheme is drawn across rather than the first.
const TABS: &str = "a\tb\tc\td\te\tf\tg\th\ti\nzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz\n";

/// The columns the cases below are laid out in.
const COLUMNS: u16 = 20;

/// The screen lines the cases below are laid out in.
const ROWS: u16 = 10;

/// What a continuation row is decorated with: the name the case is reported under, whether the
/// line's indent is repeated onto the row, and the marker drawn in front of it.
type Decoration = (&'static str, bool, &'static str);

/// The decorations every case below is replayed under. A window that draws nothing in front of a
/// continuation row cannot tell a column measured against the whole window from a column measured
/// against the row's own text, because the two are the same number there; a decorated one can, and
/// the rules an operator's range is decided by are read off that column.
const DECORATIONS: [Decoration; 4] = [
    ("plain", false, ""),
    ("showbreak", false, "> "),
    ("breakindent", true, ""),
    ("both", true, "+++ "),
];

/// The cases an operator over a display motion leaves the engine and vim in different states in,
/// each named by the case and the decoration it is replayed under, with the reason.
///
/// Both are the same row of tabs drawn behind a continuation marker and a repeated indent, and
/// both are the bare motion's disagreement rather than the operator's: `lllgj` and `llllgj` typed
/// at that text in that window were measured landing on grapheme eight where vim lands on grapheme
/// nine, with no operator in front of them at all. The operator takes what the motion reached, so
/// the one grapheme shows up in the text and in the register rather than only in the cursor.
///
/// It is the disagreement `display_motion_oracle.rs` names for the bare motions, tracked as issue
/// #41, and no mechanism for it is established. Nothing here is tuned to cancel it, and the list
/// is asserted by set equality so a case that starts or stops agreeing fails the build.
const DECORATED_DIVERGENCES: [(&str, &str); 2] = [
    (
        "delete down a screen line from beside a tab both",
        "the seam steps back off the tab the row split and vim stays on it",
    ),
    (
        "delete down a screen line over tabs both",
        "the seam steps back off the tab the row split and vim stays on it",
    ),
];

/// The operators applied over a motion counted in cells, which is what this file is for.
const SCREENWISE: [Case; 38] = [
    Case {
        id: "delete down a screen line",
        text: PROSE,
        columns: COLUMNS,
        walked: "",
        operator: "d",
        motion: "gj",
        typed: "",
    },
    Case {
        id: "delete down a screen line from a column of its own",
        text: PROSE,
        columns: COLUMNS,
        walked: "llll",
        operator: "d",
        motion: "gj",
        typed: "",
    },
    Case {
        id: "delete down a screen line out of the logical line",
        text: PROSE,
        columns: COLUMNS,
        walked: "gjlll",
        operator: "d",
        motion: "gj",
        typed: "",
    },
    Case {
        id: "delete up a screen line",
        text: PROSE,
        columns: COLUMNS,
        walked: "gj",
        operator: "d",
        motion: "gk",
        typed: "",
    },
    Case {
        id: "delete up a screen line into the line above",
        text: PROSE,
        columns: COLUMNS,
        walked: "j",
        operator: "d",
        motion: "gk",
        typed: "",
    },
    Case {
        id: "delete up a screen line into the line above from a column of its own",
        text: PROSE,
        columns: COLUMNS,
        walked: "jllll",
        operator: "d",
        motion: "gk",
        typed: "",
    },
    Case {
        id: "delete to the end of a screen line",
        text: PROSE,
        columns: COLUMNS,
        walked: "llll",
        operator: "d",
        motion: "g$",
        typed: "",
    },
    Case {
        id: "delete to the end of a continuation row",
        text: PROSE,
        columns: COLUMNS,
        walked: "gj",
        operator: "d",
        motion: "g$",
        typed: "",
    },
    Case {
        id: "yank down a screen line",
        text: PROSE,
        columns: COLUMNS,
        walked: "llll",
        operator: "y",
        motion: "gj",
        typed: "",
    },
    Case {
        id: "yank to the end of a screen line",
        text: PROSE,
        columns: COLUMNS,
        walked: "",
        operator: "y",
        motion: "g$",
        typed: "",
    },
    Case {
        id: "change down a screen line",
        text: PROSE,
        columns: COLUMNS,
        walked: "llll",
        operator: "c",
        motion: "gj",
        typed: "ZZ",
    },
    Case {
        id: "delete down two screen lines",
        text: PROSE,
        columns: COLUMNS,
        walked: "",
        operator: "d",
        motion: "2gj",
        typed: "",
    },
    Case {
        id: "delete down three screen lines",
        text: PROSE,
        columns: COLUMNS,
        walked: "",
        operator: "d",
        motion: "3gj",
        typed: "",
    },
    Case {
        id: "delete down three screen lines from a column of its own",
        text: PROSE,
        columns: COLUMNS,
        walked: "lll",
        operator: "d",
        motion: "3gj",
        typed: "",
    },
    Case {
        id: "yank down two screen lines",
        text: PROSE,
        columns: COLUMNS,
        walked: "",
        operator: "y",
        motion: "2gj",
        typed: "",
    },
    Case {
        id: "delete to the end of the screen line below",
        text: PROSE,
        columns: COLUMNS,
        walked: "",
        operator: "d",
        motion: "2g$",
        typed: "",
    },
    Case {
        id: "delete down a screen line out of an indent",
        text: INDENTED,
        columns: COLUMNS,
        walked: "",
        operator: "d",
        motion: "gj",
        typed: "",
    },
    Case {
        id: "delete down two screen lines out of an indent",
        text: INDENTED,
        columns: COLUMNS,
        walked: "ll",
        operator: "d",
        motion: "2gj",
        typed: "",
    },
    Case {
        id: "delete down three screen lines out of an indent",
        text: INDENTED,
        columns: COLUMNS,
        walked: "",
        operator: "d",
        motion: "3gj",
        typed: "",
    },
    Case {
        id: "delete down three screen lines from inside an indent",
        text: INDENTED,
        columns: COLUMNS,
        walked: "ll",
        operator: "d",
        motion: "3gj",
        typed: "",
    },
    Case {
        id: "delete down three screen lines from past an indent",
        text: INDENTED,
        columns: COLUMNS,
        walked: "lllllll",
        operator: "d",
        motion: "3gj",
        typed: "",
    },
    Case {
        id: "delete down two screen lines out of a continuation row inside an indent",
        text: DEEPLY_INDENTED,
        columns: COLUMNS,
        walked: "gj",
        operator: "d",
        motion: "2gj",
        typed: "",
    },
    Case {
        id: "delete up a screen line into an indented line",
        text: INDENTED,
        columns: COLUMNS,
        walked: "j",
        operator: "d",
        motion: "gk",
        typed: "",
    },
    Case {
        id: "delete down a screen line over characters two cells wide",
        text: WIDE,
        columns: COLUMNS,
        walked: "l",
        operator: "d",
        motion: "gj",
        typed: "",
    },
    Case {
        id: "delete to the end of a screen line of characters two cells wide",
        text: WIDE,
        columns: COLUMNS,
        walked: "l",
        operator: "d",
        motion: "g$",
        typed: "",
    },
    Case {
        id: "delete down a screen line over tabs",
        text: TABS,
        columns: COLUMNS,
        walked: "lll",
        operator: "d",
        motion: "gj",
        typed: "",
    },
    Case {
        id: "delete down a screen line from beside a tab",
        text: TABS,
        columns: COLUMNS,
        walked: "llll",
        operator: "d",
        motion: "gj",
        typed: "",
    },
    Case {
        id: "delete down a screen line behind an end of line",
        text: PROSE,
        columns: COLUMNS,
        walked: "$",
        operator: "d",
        motion: "gj",
        typed: "",
    },
    Case {
        id: "delete up a screen line behind an end of line",
        text: PROSE,
        columns: COLUMNS,
        walked: "j$",
        operator: "d",
        motion: "gk",
        typed: "",
    },
    Case {
        id: "yank up a screen line behind an end of line",
        text: PROSE,
        columns: COLUMNS,
        walked: "$",
        operator: "y",
        motion: "gk",
        typed: "",
    },
    Case {
        id: "delete down a screen line behind the end of a screen line",
        text: PROSE,
        columns: COLUMNS,
        walked: "g$",
        operator: "d",
        motion: "gj",
        typed: "",
    },
    Case {
        id: "yank down two screen lines behind the end of a screen line",
        text: PROSE,
        columns: COLUMNS,
        walked: "g$",
        operator: "y",
        motion: "2gj",
        typed: "",
    },
    Case {
        id: "delete to the end of the screen line below out of a line of its own",
        text: WIDE,
        columns: COLUMNS,
        walked: "",
        operator: "d",
        motion: "2g$",
        typed: "",
    },
    Case {
        id: "yank to the end of a screen line taking the grapheme it lands on",
        text: PROSE,
        columns: COLUMNS,
        walked: "gjlll",
        operator: "y",
        motion: "g$",
        typed: "",
    },
    Case {
        id: "delete down a screen line behind an end of line carried across a line step",
        text: PROSE,
        columns: COLUMNS,
        walked: "$j",
        operator: "d",
        motion: "gj",
        typed: "",
    },
    Case {
        id: "delete up a screen line behind an end of line carried across a line step",
        text: PROSE,
        columns: COLUMNS,
        walked: "jj$k",
        operator: "d",
        motion: "gk",
        typed: "",
    },
    Case {
        id: "yank down a screen line behind an end of line carried across a line step",
        text: PROSE,
        columns: COLUMNS,
        walked: "$j",
        operator: "y",
        motion: "gj",
        typed: "",
    },
    Case {
        id: "delete down a screen line behind an end of line an operator already took",
        text: PROSE,
        columns: COLUMNS,
        walked: "llld$",
        operator: "d",
        motion: "gj",
        typed: "",
    },
];

/// The operators counted in characters, which the seam has no business touching and which are what
/// says it touched nothing it is not for.
const CHARACTERWISE: [Case; 8] = [
    Case {
        id: "delete a word",
        text: PROSE,
        columns: COLUMNS,
        walked: "",
        operator: "d",
        motion: "w",
        typed: "",
    },
    Case {
        id: "delete to the end of a line",
        text: PROSE,
        columns: COLUMNS,
        walked: "llll",
        operator: "d",
        motion: "$",
        typed: "",
    },
    Case {
        id: "delete a line downwards",
        text: PROSE,
        columns: COLUMNS,
        walked: "",
        operator: "d",
        motion: "j",
        typed: "",
    },
    Case {
        id: "delete a line downwards from a column of its own",
        text: PROSE,
        columns: COLUMNS,
        walked: "llll",
        operator: "d",
        motion: "j",
        typed: "",
    },
    Case {
        id: "yank a word",
        text: PROSE,
        columns: COLUMNS,
        walked: "",
        operator: "y",
        motion: "w",
        typed: "",
    },
    Case {
        id: "change a word",
        text: PROSE,
        columns: COLUMNS,
        walked: "",
        operator: "c",
        motion: "w",
        typed: "ZZ",
    },
    Case {
        id: "delete a word over characters two cells wide",
        text: WIDE,
        columns: COLUMNS,
        walked: "",
        operator: "d",
        motion: "w",
        typed: "",
    },
    Case {
        id: "delete to the end of a line of tabs",
        text: TABS,
        columns: COLUMNS,
        walked: "ll",
        operator: "d",
        motion: "$",
        typed: "",
    },
];

/// The operators vim itself leaves undone, which is what it does with an operator whose motion ran
/// out of text: the cursor is carried as far as the motion reached and nothing is edited.
///
/// They are held to vim like every other case, and they are the one group left out of the sweep
/// that drops the operator from the keys, because vim dropped it too and the two are supposed to
/// agree.
const ABANDONED: [Case; 8] = [
    Case {
        id: "delete down more screen lines than the text holds",
        text: PROSE,
        columns: COLUMNS,
        walked: "",
        operator: "d",
        motion: "99gj",
        typed: "",
    },
    Case {
        id: "yank down more screen lines than the text holds",
        text: PROSE,
        columns: COLUMNS,
        walked: "lll",
        operator: "y",
        motion: "9gj",
        typed: "",
    },
    Case {
        id: "delete up a screen line from the first row of the text",
        text: PROSE,
        columns: COLUMNS,
        walked: "",
        operator: "d",
        motion: "gk",
        typed: "",
    },
    Case {
        id: "delete to the end of a screen line further down than the text reaches",
        text: PROSE,
        columns: COLUMNS,
        walked: "",
        operator: "d",
        motion: "9g$",
        typed: "",
    },
    Case {
        id: "delete up more screen lines than the text holds",
        text: PROSE,
        columns: COLUMNS,
        walked: "j",
        operator: "d",
        motion: "9gk",
        typed: "",
    },
    Case {
        id: "delete down a count no text has the rows for",
        text: PROSE,
        columns: COLUMNS,
        walked: "",
        operator: "d",
        motion: "999gj",
        typed: "",
    },
    Case {
        id: "yank down a count no text has the rows for",
        text: INDENTED,
        columns: COLUMNS,
        walked: "ll",
        operator: "y",
        motion: "999gj",
        typed: "",
    },
    Case {
        id: "delete up a count no text has the rows for",
        text: PROSE,
        columns: COLUMNS,
        walked: "jj",
        operator: "d",
        motion: "999gk",
        typed: "",
    },
];

/// The corpus cases whose outcome the engine and vim do not share, each for a reason that is not
/// this seam's.
///
/// `flag-wrap-narrow-viewport` draws a regional-indicator pair as the one flag a terminal draws and
/// vim draws it as the escapes vim draws, so the two lay the line out into different rows before a
/// motion is counted in them at all. `nowrap-w40-horizontal-scroll` asks for a window that scrolls
/// sideways rather than wrapping, and where the leftmost drawn character is is a question about a
/// viewport this engine does not carry.
const DIVERGENT: [&str; 2] = ["flag-wrap-narrow-viewport", "nowrap-w40-horizontal-scroll"];

/// The corpus cases the engine refuses before its keys reach a screen motion, which are the ones
/// that type `|`. Where `|` lands is measured in cells and nothing here measures it, so the
/// display-motion audit puts it out of scope and the keys stop there rather than landing where
/// counting characters says. A case that stopped has no ending state to put to vim.
const REFUSED: [&str; 4] = [
    "tab-leading-indent-ts4",
    "tab-leading-indent-ts8",
    "wrap-w80-plain",
    "wrap-w80-showbreak",
];

#[test]
fn an_operator_over_a_display_motion_ends_where_vim_ends() -> anyhow::Result<()> {
    let vim = VimDriver::new()?;

    let mut diverged = BTreeSet::new();

    for case in &SCREENWISE {
        for decoration in DECORATIONS {
            if vim_outcome(&vim, case, &case.keys(), decoration)?
                != engine_outcome(case, &case.keys(), decoration)
            {
                diverged.insert(format!("{} {}", case.id, decoration.0));
            }
        }
    }

    assert_eq!(
        DECORATED_DIVERGENCES
            .into_iter()
            .map(|(case, _reason)| case.to_owned())
            .collect::<BTreeSet<String>>(),
        diverged,
        "the cases that leave the engine somewhere other than where vim leaves it are not the \
         ones named as doing so"
    );

    Ok(())
}

#[test]
fn an_operator_counted_in_characters_ends_where_vim_ends() -> anyhow::Result<()> {
    let vim = VimDriver::new()?;

    for case in &CHARACTERWISE {
        for decoration in DECORATIONS {
            assert_eq!(
                vim_outcome(&vim, case, &case.keys(), decoration)?,
                engine_outcome(case, &case.keys(), decoration),
                "`{}` left the engine somewhere other than where vim left it in a {} window",
                case.id,
                decoration.0
            );
        }
    }

    Ok(())
}

#[test]
fn an_operator_whose_motion_ran_out_of_text_ends_where_vim_ends() -> anyhow::Result<()> {
    let vim = VimDriver::new()?;

    for case in &ABANDONED {
        for decoration in DECORATIONS {
            let expected = vim_outcome(&vim, case, &case.keys(), decoration)?;

            assert_eq!(
                case.text, expected.text,
                "`{}` is meant to leave vim's own text untouched and does not in a {} window",
                case.id, decoration.0
            );
            assert_eq!(
                expected,
                engine_outcome(case, &case.keys(), decoration),
                "`{}` left the engine somewhere other than where vim left it in a {} window",
                case.id,
                decoration.0
            );
        }
    }

    Ok(())
}

#[test]
fn an_engine_that_answered_the_motion_and_dropped_the_operator_diverges_from_vim(
) -> anyhow::Result<()> {
    let vim = VimDriver::new()?;

    for case in SCREENWISE.iter().chain(CHARACTERWISE.iter()) {
        for decoration in DECORATIONS {
            assert_ne!(
                vim_outcome(&vim, case, &case.keys(), decoration)?,
                engine_outcome(case, &case.degraded(), decoration),
                "`{}` agreed with vim with its operator taken out of the keys in a {} window, so \
                 the comparison would pass against an engine that answered the motion and ran no \
                 operator at all",
                case.id,
                decoration.0
            );
        }
    }

    Ok(())
}

#[test]
fn a_register_holding_the_right_text_under_the_wrong_type_is_reported() -> anyhow::Result<()> {
    let vim = VimDriver::new()?;
    let mut shapes = Vec::new();

    for case in SCREENWISE.iter().chain(CHARACTERWISE.iter()) {
        for decoration in DECORATIONS {
            let expected = vim_outcome(&vim, case, &case.keys(), decoration)?;
            assert!(
                !expected.registers.is_empty(),
                "`{}` fills no register in a {} window, so reporting its registers under another \
                 type reports the same state and the comparison below would be comparing one with \
                 itself",
                case.id,
                decoration.0
            );
            for held in expected.registers.values() {
                shapes.push(held.register_type);
            }

            let retyped = Outcome {
                registers: expected
                    .registers
                    .iter()
                    .map(|(name, held)| (*name, retyped(held)))
                    .collect(),
                ..expected
            };
            assert_ne!(
                retyped,
                engine_outcome(case, &case.keys(), decoration),
                "`{}` agreed with a vim whose every register was reported under another type in a \
                 {} window, so the comparison is holding the register's text and not the shape a \
                 put reinserts it with",
                case.id,
                decoration.0
            );
        }
    }

    assert!(
        shapes.contains(&RegisterType::Charwise) && shapes.contains(&RegisterType::Linewise),
        "the cases fill no register of one of the two types, so the comparison above never saw \
         the type it would have to tell apart"
    );

    Ok(())
}

#[test]
fn the_corpus_ends_where_vim_ends_wherever_the_two_lay_a_line_out_alike() -> anyhow::Result<()> {
    let vim = VimDriver::new()?;
    let corpus = Corpus::load_dir(&corpus::default_dir()).expect("the corpus loads");
    let divergent: BTreeSet<&str> = DIVERGENT.into_iter().collect();

    let mut agreed = BTreeSet::new();
    let mut refused = BTreeSet::new();
    for case in corpus.cases() {
        let mut engine = Engine::laid_out_in(&case.buffer, geometry_of(case));
        if let Err(error) = engine.press_all(typed_keys(&case.keys)) {
            assert!(
                matches!(error, Error::OutOfScope { .. }),
                "`{}` stopped against the engine as `{error:?}` rather than being refused",
                case.id
            );
            refused.insert(case.id.as_str());
            continue;
        }

        let expected: Outcome = vim.run_case(case)?.into();
        let ends_where_vim_does = expected == Outcome::of(&mut engine);

        if divergent.contains(case.id.as_str()) {
            assert!(
                !ends_where_vim_does,
                "`{}` is named as diverging from vim and no longer does",
                case.id
            );
        } else if ends_where_vim_does {
            agreed.insert(case.id.as_str());
        }
    }

    assert_eq!(
        REFUSED.into_iter().collect::<BTreeSet<_>>(),
        refused,
        "the cases the engine refused are not the ones it is named as refusing"
    );
    assert!(
        agreed.contains("wrap-w20-plain") && agreed.contains("cjk-wide-cell-straddles-edge"),
        "the cases the corpus counts a screen motion in are not among the ones agreeing with vim"
    );

    Ok(())
}

/// # Returns
///
/// What the engine was left holding after `keys` were typed at `case`'s text.
///
/// # Panics
///
/// Panics if the keys do not run.
fn engine_outcome(case: &Case, keys: &str, decoration: Decoration) -> Outcome {
    let (_name, break_indent, show_break) = decoration;
    let columns = NonZeroUsize::new(usize::from(case.columns)).expect("the columns are not zero");
    let rows = NonZeroUsize::new(usize::from(ROWS)).expect("the rows are not zero");
    let geometry = Geometry::new(columns, rows).with_options(
        Options::new()
            .with_break_indent(break_indent)
            .with_show_break(show_break.to_owned()),
    );
    let mut engine = Engine::laid_out_in(case.text, geometry);
    engine
        .press_all(typed_keys(keys))
        .expect("the keys run against the engine");

    Outcome::of(&mut engine)
}

/// # Returns
///
/// What vim was left holding after `keys` were typed at `case`'s text in `case`'s window, on
/// success.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`VimDriver::run_case`]'s return values on failure.
fn vim_outcome(
    vim: &VimDriver,
    case: &Case,
    keys: &str,
    decoration: Decoration,
) -> anyhow::Result<Outcome> {
    let (_name, break_indent, show_break) = decoration;
    let state = vim.run_case(&CorpusCase {
        id: case.id.to_owned(),
        description: case.id.to_owned(),
        buffer: case.text.to_owned(),
        keys: keys.to_owned(),
        viewport_width: case.columns,
        viewport_height: ROWS,
        tags: BTreeSet::new(),
        options: CaseOptions {
            breakindent: break_indent,
            showbreak: show_break.to_owned(),
            ..CaseOptions::default()
        },
    })?;

    Ok(state.into())
}

/// # Returns
///
/// The same register under another type, which is the register a put would reinsert differently
/// while every character of it stayed where it was.
fn retyped(held: &Register) -> Register {
    Register {
        text: held.text.clone(),
        register_type: match held.register_type {
            RegisterType::Charwise => RegisterType::Linewise,
            RegisterType::Linewise | RegisterType::Blockwise => RegisterType::Charwise,
        },
    }
}

/// # Returns
///
/// The key events `keys` stands for, in which `<Esc>` names the escape key and every other
/// character stands for itself.
///
/// # Panics
///
/// Panics if the sequence names a key these cases were not written to hold.
fn typed_keys(keys: &str) -> Vec<KeyEvent> {
    let mut typed_keys = Vec::new();
    let mut rest = keys;
    while let Some(index) = rest.find('<') {
        typed_keys.extend(rest[..index].chars().map(typed));
        let named = &rest[index..];
        let end = named.find('>').expect("a named key is closed");
        assert_eq!(
            "<Esc>",
            &named[..=end],
            "`{keys}` names a key these cases were not written to hold"
        );
        typed_keys.push(KeyEvent::from(KeyCode::Esc));
        rest = &named[end + 1..];
    }
    typed_keys.extend(rest.chars().map(typed));

    typed_keys
}

/// # Returns
///
/// The layout a corpus case's screen motions are measured in.
///
/// # Panics
///
/// Panics if the case's viewport is zero columns wide or zero rows tall, which the corpus loader
/// rejects.
fn geometry_of(case: &CorpusCase) -> Geometry {
    let columns = NonZeroUsize::new(usize::from(case.viewport_width))
        .expect("a viewport is not zero columns wide");
    let rows = NonZeroUsize::new(usize::from(case.viewport_height))
        .expect("a viewport is not zero rows tall");
    let ambiwidth = match case.options.ambiwidth {
        CaseAmbiWidth::Single => AmbiWidth::Single,
        CaseAmbiWidth::Double => AmbiWidth::Double,
    };
    let tab_stop =
        NonZeroUsize::new(usize::from(case.options.tabstop)).expect("a case's tabstop is not zero");
    let options = Options::new()
        .with_break_indent(case.options.breakindent)
        .with_show_break(case.options.showbreak.clone())
        .with_line_break(case.options.linebreak);

    Geometry::new(columns, rows)
        .with_metrics(Metrics::new(ambiwidth, tab_stop))
        .with_options(options)
}
