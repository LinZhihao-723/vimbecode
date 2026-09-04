//! Cross-checks blockwise visual mode, extended by motions counted in screen lines, against the
//! vim it was copied from.
//!
//! `visual_display_motion.rs` says in writing that it types no `CTRL-V` and makes no claim about
//! blockwise mode. This file is that claim. Every case here starts with `CTRL-V`, extends the
//! block with a motion the seam answers, and runs one of the three edits a block is worth having
//! -- `d`, `I` and `A` -- with `y` and `c` beside them, over wrapped text, over a wrapped
//! paragraph and over characters two cells wide.
//!
//! A block is the one selection whose shape a register carries, so the register's type is
//! compared as well as its text: a blockwise delete fills a blockwise register, which is the `b`
//! of vim's own `getregtype()`, and an engine that yanked the same characters charwise would put
//! them back as one run rather than as a column. The comparison is the record `vim_engine.rs`
//! shares -- text, cursor, mode and every register with the type a put would reinsert it with --
//! and the mode half of it now tells vim's three visual modes apart, so a `CTRL-V` that quietly
//! started a charwise selection fails this file rather than passing it.
//!
//! Every case is written so that the display motion and the logical motion spelled the same way
//! answer differently, and that is asserted of vim rather than assumed: a case where `gj` and `j`
//! draw the same block would pass against an engine that has never heard of a screen line.
//!
//! Two divergences are named here rather than left to be rediscovered, each asserted with the
//! numbers both engines leave so that a case which starts or stops agreeing fails this file.
//!
//! **Where a blockwise insert leaves the cursor is modalkit's answer rather than vim's.** `I` and
//! `A` write what vim writes on every line of the block -- the text and the registers agree
//! throughout -- and vim then returns the cursor to the corner the block started at where
//! modalkit leaves it where the leading cursor finished typing. It is the same answer for a block
//! a logical `j` drew, so it is a gap in blockwise insert rather than in this seam.
//!
//! **A block whose far edge a display motion could not reach is stepped back from somewhere else.**
//! A `gj` onto a row too short for the column it wanted leaves vim's block reaching the end of
//! that line, and an `h` behind it steps back from the column the motion wanted rather than from
//! where the cursor was left; the seam carries no wanted column into modalkit's selection, so its
//! block steps back from the cursor and stops one grapheme short of vim's. The mechanism -- which
//! column vim measures a block's far edge from once a motion has run out of row -- is not
//! established here, and nothing has been tuned to cancel it.

mod notation;
mod outcome;

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroUsize;

use vbc_editor::engine::Engine;
use vbc_editor::screen::Geometry;
use vbc_oracle::corpus;
use vbc_oracle::state::{Mode, Register, RegisterType};
use vbc_oracle::vim::VimDriver;

use crate::notation::keys;
use crate::outcome::Outcome;

/// One cross-check: the keys that draw a block with a display motion, and the keys that draw one
/// with the logical motion spelled the same way, which is what says the case turns on the
/// difference between them.
struct Block {
    id: &'static str,
    keys: &'static str,
    logical: &'static str,
}

/// One logical line long enough to wrap into two rows of the window below, followed by lines that
/// do not, so a block drawn down a row of the first stays inside it where a block drawn down a
/// line leaves it.
const WRAPPED: &str = "abcdefghijklmnopqrstuvwxyz0123456789\nsecond line here\nthird\n";

/// A paragraph whose first line wraps into three rows, on which a display motion and a logical
/// motion both move and land in different places.
const PARAGRAPH: &str =
    "the quick brown fox jumps over the lazy dog\nsecond line here\nthird line\nfourth\n";

/// Lines of characters drawn two cells wide apiece, which wrap where the layout engine says they
/// do and not where modalkit's own width math says they do.
const WIDE: &str = "你好世界一二三四五六七八九十\n第二行的中文文字在这里\n第三行\n";

/// The texts every case is replayed against, each named for what makes it worth replaying.
const TEXTS: [(&str, &str); 3] = [
    ("wrapped", WRAPPED),
    ("paragraph", PARAGRAPH),
    ("wide", WIDE),
];

/// The cells the window every case is laid out in is wide.
const COLUMNS: u16 = 20;

/// The screen lines the window every case is laid out in is tall.
const ROWS: u16 = 10;

/// The blocks a display motion draws and the edits run over them, each paired with the logical
/// motion the case must not behave as.
const BLOCKS: [Block; 11] = [
    Block {
        id: "a block down a row",
        keys: "<C-v>gj",
        logical: "<C-v>j",
    },
    Block {
        id: "a block down a row, deleted",
        keys: "<C-v>gjd",
        logical: "<C-v>jd",
    },
    Block {
        id: "a block down a row, yanked",
        keys: "<C-v>gjy",
        logical: "<C-v>jy",
    },
    Block {
        id: "a block two columns wide, yanked",
        keys: "<C-v>gjly",
        logical: "<C-v>jly",
    },
    Block {
        id: "a block two columns wide, deleted",
        keys: "<C-v>gjld",
        logical: "<C-v>jld",
    },
    Block {
        id: "a block down two rows at once",
        keys: "<C-v>2gjld",
        logical: "<C-v>2jld",
    },
    Block {
        id: "a block down two rows a step at a time",
        keys: "<C-v>gjgjd",
        logical: "<C-v>jjd",
    },
    Block {
        id: "a block to the end of a row",
        keys: "<C-v>g$d",
        logical: "<C-v>$d",
    },
    Block {
        id: "a block to the end of a row, yanked",
        keys: "<C-v>g$y",
        logical: "<C-v>$y",
    },
    Block {
        id: "a block down a row and out to the end of a line",
        keys: "<C-v>gj$d",
        logical: "<C-v>j$d",
    },
    Block {
        id: "a block down a row, changed",
        keys: "<C-v>gjlc??<Esc>",
        logical: "<C-v>jlc??<Esc>",
    },
];

/// The blocks an insert is run over, each paired with the logical motion the case must not behave
/// as. They are held to vim in the text and the registers alone: where a blockwise insert leaves
/// the cursor is the divergence pinned below.
const INSERTS: [Block; 4] = [
    Block {
        id: "a block down a row, inserted in front of",
        keys: "<C-v>gjlIXX<Esc>",
        logical: "<C-v>jlIXX<Esc>",
    },
    Block {
        id: "a block down a row, appended to",
        keys: "<C-v>gjlAXX<Esc>",
        logical: "<C-v>jlAXX<Esc>",
    },
    Block {
        id: "a block down two rows, inserted in front of",
        keys: "<C-v>2gjIXX<Esc>",
        logical: "<C-v>2jIXX<Esc>",
    },
    Block {
        id: "a block to the end of a row, appended to",
        keys: "<C-v>g$AXX<Esc>",
        logical: "<C-v>$AXX<Esc>",
    },
];

/// The blockwise inserts pinned with the cursor both engines leave: the keys, the line and column
/// this engine rests on, and the line and column vim rests on. Every one of them is replayed
/// against [`WRAPPED`].
const PINNED_INSERT_CURSORS: [Pinned; 2] = [
    ("<C-v>gjlIXX<Esc>", (0, 1), (0, 0)),
    ("<C-v>gjlAXX<Esc>", (0, 23), (0, 0)),
];

/// The blocks whose far edge was set by a display motion that ran out of the row it landed on and
/// then stepped back from, pinned with what each engine deletes: the text, the keys, what this
/// engine leaves and what vim leaves.
const PINNED_EDGES: [(&str, &str, &str, &str); 2] = [
    (
        "wide",
        "5l<C-v>gjhd",
        "你好世界一十\n第二行的中文文字在这里\n第三行\n",
        "你好世界一\n第二行的中文文字在这里\n第三行\n",
    ),
    (
        "paragraph",
        "j5l<C-v>gkhd",
        "the qg\nsecon\nthird line\nfourth\n",
        "the q\nsecon\nthird line\nfourth\n",
    ),
];

/// One pinned case: the keys, the line and column this engine rests on, and the line and column
/// vim rests on.
type Pinned = (&'static str, (u64, u64), (u64, u64));

/// What a blockwise insert leaves that this engine and vim agree on, which is the whole of the
/// record but the cursor. Where a blockwise insert leaves the cursor is pinned as a divergence of
/// its own by `the_cursor_a_blockwise_insert_leaves_is_modalkits_rather_than_vims`.
#[derive(Debug, Eq, PartialEq)]
struct Written {
    text: String,
    mode: Mode,
    registers: BTreeMap<char, Register>,
}

impl From<Outcome> for Written {
    fn from(outcome: Outcome) -> Self {
        Self {
            text: outcome.text,
            mode: outcome.mode,
            registers: outcome.registers,
        }
    }
}

#[test]
fn a_control_v_starts_a_blockwise_selection() -> anyhow::Result<()> {
    let vim = VimDriver::new()?;

    for (name, text) in TEXTS {
        assert_eq!(
            Mode::VisualBlock,
            replayed(text, "<C-v>gj")?.mode,
            "`C-v` left the engine in a mode other than blockwise visual, on the {name} text"
        );
        assert_eq!(
            Mode::VisualBlock,
            vim_outcome(&vim, text, "<C-v>gj")?.mode,
            "vim itself was not left in blockwise visual mode, on the {name} text"
        );
    }

    Ok(())
}

#[test]
fn every_blockwise_display_motion_ends_where_vim_ends_it() -> anyhow::Result<()> {
    let vim = VimDriver::new()?;

    for (name, text) in TEXTS {
        for block in &BLOCKS {
            assert_eq!(
                vim_outcome(&vim, text, block.keys)?,
                replayed(text, block.keys)?,
                "`{}` left the engine somewhere other than where vim left it, on the {name} text",
                block.id
            );
        }
    }

    Ok(())
}

#[test]
fn every_case_answers_a_display_motion_differently_from_its_logical_twin() -> anyhow::Result<()> {
    let vim = VimDriver::new()?;

    for block in BLOCKS.iter().chain(&INSERTS) {
        let mut differed = false;
        for (_name, text) in TEXTS {
            differed |=
                vim_outcome(&vim, text, block.logical)? != vim_outcome(&vim, text, block.keys)?;
        }

        assert!(
            differed,
            "`{}` is a case vim answers the same way on every text here whether it counts rows or \
             lines",
            block.id
        );
    }

    Ok(())
}

#[test]
fn a_blockwise_delete_fills_a_blockwise_register() -> anyhow::Result<()> {
    let vim = VimDriver::new()?;
    let taken = Register {
        text: "ab\nse".to_owned(),
        register_type: RegisterType::Blockwise,
    };
    let ours = replayed(WRAPPED, "<C-v>2gjld")?;

    assert_eq!(
        Some(&taken),
        ours.registers.get(&'"'),
        "the two columns a block took out of two logical lines were not put in the unnamed \
         register as a block"
    );
    assert_eq!(
        "cdefghijklmnopqrstuvwxyz0123456789\ncond line here\nthird\n", ours.text,
        "a block walked down two rows of a wrapped line took a run of the text rather than a \
         column of it"
    );
    assert_eq!(
        Some(&taken),
        vim_outcome(&vim, WRAPPED, "<C-v>2gjld")?
            .registers
            .get(&'"'),
        "vim itself did not put those characters in the unnamed register as a block"
    );

    Ok(())
}

#[test]
fn a_blockwise_yank_over_wide_characters_takes_the_column_vim_takes() -> anyhow::Result<()> {
    let vim = VimDriver::new()?;
    let ours = replayed(WIDE, "<C-v>gjly")?;

    assert_eq!(
        vim_outcome(&vim, WIDE, "<C-v>gjly")?.registers.get(&'"'),
        ours.registers.get(&'"'),
        "a block drawn down a row of characters two cells wide holds something other than what \
         vim's holds"
    );
    assert_eq!(
        Some(RegisterType::Blockwise),
        ours.registers.get(&'"').map(|held| held.register_type),
        "a block over wide characters was not held as a block"
    );

    Ok(())
}

#[test]
fn every_blockwise_insert_writes_what_vim_writes() -> anyhow::Result<()> {
    let vim = VimDriver::new()?;

    for (name, text) in TEXTS {
        for block in &INSERTS {
            assert_eq!(
                Written::from(vim_outcome(&vim, text, block.keys)?),
                Written::from(replayed(text, block.keys)?),
                "`{}` wrote a text, a mode or a register other than vim's, on the {name} text",
                block.id
            );
        }
    }

    Ok(())
}

#[test]
fn the_cursor_a_blockwise_insert_leaves_is_modalkits_rather_than_vims() -> anyhow::Result<()> {
    let vim = VimDriver::new()?;

    for (keys, ours, theirs) in PINNED_INSERT_CURSORS {
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
            "`{keys}` wrote a text other than vim's, which is more than a cursor apart"
        );
    }

    Ok(())
}

#[test]
fn a_block_stepped_back_from_a_motion_that_ran_out_of_row_is_a_known_divergence(
) -> anyhow::Result<()> {
    let vim = VimDriver::new()?;

    for (name, keys, ours, theirs) in PINNED_EDGES {
        let text = TEXTS
            .into_iter()
            .find(|(named, _text)| *named == name)
            .expect("the pinned case names a text this file holds")
            .1;

        assert_eq!(
            ours,
            replayed(text, keys)?.text,
            "`{keys}` on the {name} text no longer takes what this divergence says it takes"
        );
        assert_eq!(
            theirs,
            vim_outcome(&vim, text, keys)?.text,
            "`{keys}` on the {name} text no longer takes what vim was measured taking, so the \
             reason written down for the divergence no longer describes it"
        );
    }

    Ok(())
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
        id: "blockwise-display-motion".to_owned(),
        description: "A blockwise selection extended by a display motion.".to_owned(),
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
