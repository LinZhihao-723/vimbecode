//! The bare display motions held to the vim they are copied from.
//!
//! `gj`, `gk`, `g0`, `g^` and `g$` are the motions a window's own cells decide the answer to and
//! the shim answers, and it exists so that the layout engine decides them rather than modalkit's
//! character arithmetic. `gm` is the same shape and the display-motion audit puts it out of scope,
//! so it is refused rather than answered and is not compared here. Whether the engine decides the
//! rest the way vim does is not something this workspace can settle by itself, so every case here
//! is replayed against a real vim in the window the case declares, and the two cursors are
//! required to rest on the same byte of the same line.
//!
//! Which text a case is replayed against is the point of the exercise rather than a detail of it.
//! On plain ASCII a character column and a display column are the same number, so a comparison
//! made only there would pass against the very arithmetic the shim was written to replace. The
//! same motions are therefore replayed against CJK, where every character is two cells wide, and
//! against a text indented with tabs, where one character is eight; and each is laid out in more
//! than one window, because a motion answered by dividing by the wrong width is right in the
//! window whose width happens to divide evenly. A fourth text is tabs all the way along, so that a
//! row ends part-way through one: a column carried onto such a row lands in the middle of a
//! grapheme, which vim steps back off rather than being carried a row further along.
//!
//! A comparison is worth what it would catch, so every motion is replayed through an engine built
//! without the shim -- the engine modalkit answers a screen motion in by itself -- and that engine
//! is required to disagree with vim. A motion both engines answer alike is a motion whose
//! agreement with vim says nothing about the seam, and this file names none.
//!
//! Beyond the plain motions, four things a layout can get wrong on its own are pinned here. A
//! motion at the first display row of a logical line and one at its last cross into the line above
//! or below rather than stopping where their own line's rows run out, and the cases that cross are
//! required to have crossed. A chain of `gj` down a ragged buffer returns to the column it started
//! from rather than to the column the shortest row it passed cut it back to, and is compared after
//! every step of the walk rather than only at the end. A count multiplies the rows a motion steps,
//! and a count larger than the text stops where the text does. And a `$` in front of a chain
//! leaves it wanting the end of every row it lands on, which is a column no screen motion asked
//! for.
//!
//! What a chain wants outlives some of the keys typed in the middle of it and not others. vim's
//! `curswant` is left alone by a bare `j` or `k` and set by every other cursor move, so a `$` two
//! motions back is still wanting the end of a row where a column typed in between is not. Both
//! halves are replayed here, and each case that ends a chain is required to land somewhere the
//! same chain carried on would not, so that agreeing with vim there says which of the two happened
//! rather than only that the cursor arrived somewhere plausible. The one chain a `g$` starts is
//! not among them: it wants a column rather than an end, so the seam ends it at a `j` and vim does
//! not, and that is pinned as a divergence with the reason rather than answered here. A chain a
//! numbered column started is pinned beside it for the same reason, on the texts the `j` in front
//! of it is measured landing where vim lands it, so that the pin reports the column alone.
//!
//! Every case above is laid out in a window that draws no decoration in front of a continuation
//! row, and a window like that cannot tell a column measured against the whole window from a
//! column measured against the row's own text: the two are the same number only while nothing
//! stands in front of that text. So the same motions and the same walks are replayed with
//! `'showbreak'`, with `'breakindent'`, and with both at once, in three windows apiece, which is
//! where the difference between the two is a column rather than nothing. The walks that track a
//! column across more than one motion -- the ragged walk, the counted motions and the chains
//! above -- are replayed there too.
//!
//! What those decorated windows still disagree with vim about is a difference of coordinates
//! rather than of layout, and is named case by case below. vim decides whether to step back off a
//! grapheme a row split from `curswant`, which is a virtual column of the logical line that counts
//! no decoration at all, taken modulo the window's width; the seam carries the screen column a row
//! draws the cursor in, which counts every decoration cell drawn above it. On an undecorated row
//! the two are the same number and every case agrees. On a decorated one they are not, and no
//! threshold written in screen columns is the threshold vim wrote in virtual ones. The rows
//! themselves are not in question: every decorated layout here was compared with the screen vim
//! drew for it, cell for cell, and they are the same rows.
//!
//! The counted forms of `g$` are the one family left out of the decorated comparison, and are left
//! out in writing: they count the screen lines below the cursor's own rather than the rows a walk
//! steps, which is the fixed-column arithmetic named below, and the two vims this file has been
//! measured against answer them differently on a decorated CJK row.
//!
//! The corpus is compared too. Every case of it that asks for a screen motion is replayed against
//! vim in the window and under the display options the case declares, which is the sample this
//! seam was actually built against: the two cases vim answers somewhere else are named here with
//! the reason, so that a case which starts or stops agreeing fails this file rather than quietly
//! leaving the sample.
//!
//! One corner is left out of the widths the chain of ends is compared in, and is left out in
//! writing rather than quietly. Where a row is cut short because the double-width character coming
//! next did not fit in the cell it had left, vim measures the screen line below it as a fixed
//! number of columns from the one above and so reaches past the row it meant to: `g$gj` on the CJK
//! text in a window twenty-one columns wide lands, in vim, on the row after the one it was aiming
//! for. The rows here are walked rather than divided, so the shim lands on the row it was aiming
//! for and the two part company. The plain motions agree in that window, and are compared in it.

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroUsize;

use crossterm::event::KeyCode;
use vbc_editor::engine::{typed, Engine, Error};
use vbc_editor::event::KeyEvent;
use vbc_editor::screen::Geometry;
use vbc_layout::line::Options;
use vbc_layout::width::{AmbiWidth, Metrics};
use vbc_oracle::corpus::{
    self, AmbiWidth as CaseAmbiWidth, Case, Corpus, Options as CaseOptions, Tag,
};
use vbc_oracle::state::EditorState;
use vbc_oracle::vim::VimDriver;

/// Prose that wraps several times in every window below, over logical lines ragged enough that a
/// walk down the rows rarely lands on the row it left.
const WRAPPED: &str = concat!(
    "    the indented sentence that wraps a few times before it finally ends here\n",
    "short\n",
    "another fairly long line that also wraps around a couple of times over here\n",
    "\n",
    "tail\n",
);

/// The same shape of text written in characters two cells wide, on which the column a cursor is
/// drawn in and the character it sits at are different numbers.
const WIDE: &str = concat!(
    "这是一行很长的中文文本用来测试换行的边界情况\n",
    "短\n",
    "混合abc中文def结束done and some more text here\n",
);

/// A text whose lines begin with tabs, which are the one grapheme a screen draws as the blanks it
/// advances by and the one vim draws a cursor at the far end of.
const TABBED: &str = concat!(
    "\tindented with a tab and then a fairly long run of words to wrap it\n",
    "\t\tdeeper\n",
    "plain\n",
);

/// A text of tabs all the way along, in which a row of every window below ends part-way through
/// one. That is the grapheme a column carried down a screen line can land in the middle of, and
/// vim steps back off one rather than being carried a row further along than it asked for.
const STRADDLED: &str = concat!(
    "a\tb\tc\td\te\tf\tg\th\ti\tj\tk\tl\n",
    "0123456789012345678901234567890123456789\n",
    "m\tn\to\tp\tq\tr\ts\tt\tu\n",
);

/// The texts every motion is replayed against, each named for what makes it worth replaying.
const TEXTS: [(&str, &str); 4] = [
    ("ascii", WRAPPED),
    ("cjk", WIDE),
    ("tab", TABBED),
    ("straddled", STRADDLED),
];

/// The windows the texts are laid out in. A motion answered by dividing a character column by a
/// window's width is right wherever that width divides the text evenly, so one of these is an odd
/// number of columns, in which a double-width character cannot end a row.
const COLUMNS: [u16; 3] = [16, 21, 40];

/// The narrowest of those windows, which is the one the divergence pinned below is measured in.
const NARROWEST: u16 = COLUMNS[0];

/// The windows in which no row is cut short by a double-width character that did not fit, which
/// are the windows the chain of ends is compared in.
const WHOLE_COLUMNS: [u16; 2] = [16, 40];

/// The screen lines the windows below hold, which is more rows than any case's text draws.
const ROWS: u16 = 10;

/// What a continuation row is decorated with: the name the case is reported under, whether the
/// line's indent is repeated onto the row, and the marker drawn in front of it.
type Decoration = (&'static str, bool, &'static str);

/// The window that decorates nothing, which is the one every undecorated case is laid out in.
const PLAIN: Decoration = ("plain", false, "");

/// The decorations a continuation row is drawn with. Decoration is the whole of the difference
/// between a column measured against the window and a column measured against the row's own text,
/// so it is the one thing a case has to draw for the two to be told apart.
const DECORATIONS: [Decoration; 3] = [
    ("showbreak", false, "> "),
    ("breakindent", true, ""),
    ("both", true, "+++ "),
];

/// The windows the decorated cases are laid out in. A decoration takes the same cells from each of
/// them, so the fraction of a row it stands in front of is different in every one.
const DECORATED_COLUMNS: [u16; 3] = [15, 20, 31];

/// The keys that walk along the first row of a line and then down onto a decorated one, from the
/// columns at which a row's decoration moves its halfway mark past.
const DECORATED_WALKS: [&str; 5] = [
    "gg0lllgj",
    "gg0lllllgj",
    "gg0lllllllgj",
    "gg0lllllllllgj",
    "gg0lllllgjgj",
];

/// The decorated cases vim answers somewhere this seam does not, each with the reason, so that a
/// case which starts or stops agreeing fails this file rather than quietly leaving the sample.
///
/// The disagreement is about which column is being halved rather than about where the rows are:
/// the layout is confirmed innocent, with 327 decorated rows compared cell for cell against the
/// screen vim draws — tabs split across a wrap boundary included — and no differences. Every case
/// below is a row that splits a tab, where one engine steps back off it and the other stays on it.
///
/// **The mechanism is not established.** One model was proposed and then falsified: that
/// `'showbreak'` is absent from `win_col_off2()`, so vim's arithmetic reduces to the undecorated
/// virtual column and decoration cannot move its answer. Running vim against vim — same text,
/// width and keys, `showbreak=""` against `showbreak="> "` — contradicts it in **74 of 300 cases**,
/// by up to three characters, which that model forbids outright.
///
/// What is established is narrower. Measuring the halfway mark against the row's own text rather
/// than against the window is the better of the two rules: it takes the divergence count from 55
/// to 32 over a 1200-case sweep, leaves every undecorated case untouched, and fixes 26 while
/// introducing 3. Why the rest remain is unknown and tracked as an issue; nothing here has been
/// tuned to cancel them, and this list is asserted by set equality so a case that starts or stops
/// agreeing fails the build.
const DECORATED_DIVERGENCES: [(&str, &str); 7] = [
    (
        "straddled w15 showbreak gg0lllllllllgj",
        "the seam steps back off the tab the row split and vim stays on it",
    ),
    (
        "straddled w20 both gg0lllgj",
        "the seam steps back off the tab the row split and vim stays on it",
    ),
    (
        "straddled w20 showbreak gg0lllllllgj",
        "vim steps back off the tab the row split and the seam stays on it",
    ),
    (
        "straddled w31 both gg0lllllgj",
        "the seam steps back off the tab the row split and vim stays on it",
    ),
    (
        "straddled w31 both gg0lllllgjgj",
        "the seam steps back off the tab the row split and vim stays on it",
    ),
    (
        "straddled w31 showbreak gg0lllllgj",
        "the seam steps back off the tab the row split and vim stays on it",
    ),
    (
        "straddled w31 showbreak gg0lllllgjgj",
        "the seam steps back off the tab the row split and vim stays on it",
    ),
];

/// The decorated walks down a ragged buffer that disagree with vim, each with the reason, so that
/// a case which starts or stops agreeing fails this file rather than quietly leaving the sample.
///
/// These are the disagreement [`DECORATED_DIVERGENCES`] names, reached by different keys: a row
/// that splits a tab, where one engine steps back off the grapheme the row split and the other
/// stays on it, with no established mechanism for which of the two vim picks. Nothing here is
/// tuned to cancel them and the list is asserted by set equality.
const DECORATED_RAGGED_WALK_DIVERGENCES: [(&str, &str); 4] = [
    (
        "straddled w15 both llllllllllgjgj",
        "the seam steps back off the tab the row split and vim stays on it",
    ),
    (
        "straddled w15 both llllllllllgjgjgj",
        "the seam steps back off the tab the row split and vim stays on it",
    ),
    (
        "straddled w15 showbreak llllllllllgjgj",
        "the seam steps back off the tab the row split and vim stays on it",
    ),
    (
        "straddled w15 showbreak llllllllllgjgjgj",
        "the seam steps back off the tab the row split and vim stays on it",
    ),
];

/// The keys each motion is reached by.
const MOTIONS: [&str; 5] = ["gj", "gk", "g0", "g^", "g$"];

/// The keys typed in front of a motion to leave the cursor part-way along a continuation row
/// rather than on the first row of the text.
const ONTO_A_CONTINUATION_ROW: &str = "gjlll";

/// The keys that walk the cursor onto the first or the last display row of a logical line, paired
/// with the motion that then has to cross into the line above or below it. The column each walk
/// ends in is named by the walk itself rather than carried down from the line above, because what
/// modalkit carries down a line of tabs is not what vim carries down one and that is a difference
/// about `j` rather than about the seam.
const ACROSS_A_LINE_BOUNDARY: [(&str, &str); 4] =
    [("j0", "gk"), ("jj0", "gk"), ("$", "gj"), ("j$", "gj")];

/// The keys that walk down a ragged buffer one screen line at a time, each step of which is
/// compared with vim rather than only the last.
const RAGGED_WALK: [&str; 6] = [
    "llllllllll",
    "llllllllllgj",
    "llllllllllgjgj",
    "llllllllllgjgjgj",
    "llllllllllgjgjgjgj",
    "llllllllllgjgjgjgjgj",
];

/// The motions counted in the screen lines a walk steps, including counts larger than the text has
/// rows for. A count no text here has the rows for is clamped by vim rather than refused, and the
/// cursor is left on the row the walk ran out on; the same count under an operator is a different
/// answer and belongs to `operator_display_motions.rs`.
const COUNTED: [&str; 8] = [
    "3gj", "5gj", "12gj", "999gj", "G4gk", "G12gk", "999gk", "2gjg0",
];

/// The counted forms of `g$`, which count the screen lines below the cursor's own rather than the
/// rows a walk steps, and which are compared undecorated only.
///
/// That is the corner this file already leaves out of the chain of ends: where a row is cut short
/// because the double-width character coming next did not fit in the cell it had left, vim reaches
/// the row below it by a fixed number of columns rather than by walking, and lands past the row it
/// meant to. `2g$` and `3g$` on the CJK text in a decorated fifteen-column window are that corner,
/// and the two vims this file has been measured against do not agree with each other there: 8.2
/// lands where the seam lands and 9.1.697 lands elsewhere. Comparing them decorated would pin this
/// file to whichever vim happened to run it, so they are left out in writing.
const COUNTED_ENDS: [&str; 2] = ["2g$", "3g$"];

/// The keys that put a bare `j` or `k` in the middle of a chain a `$` left wanting the end of a
/// row, which is the one cursor move vim leaves `curswant` alone across and which the chain
/// therefore has to outlive.
const CARRIED_THROUGH_A_VERTICAL_MOTION: [&str; 6] =
    ["$jgj", "$jjgj", "$jgjgj", "$jkgj", "$kgj", "j$kgj"];

/// The keys that end a chain at a motion of their own, each paired with the keys the same chain
/// would have run had nothing ended it. vim sets `curswant` from a motion that names a column and
/// from the cursor an operator leaves behind, so the two are different places; requiring them to be
/// is what says a case here turns on the chain having ended rather than on where it was going
/// anyway.
///
/// An `H` is here because a motion the seam measures and then leaves to modalkit is a cursor move
/// like any other: vim sets `curswant` from it, so the chain has to end there even though nothing
/// here answered the motion. `M` and `L` are the same shape and are left out, because their own
/// landings disagree with vim before any chain is involved -- a bare `Mgj` was measured diverging
/// in 41 of these 80 windows and a bare `Lgj` in 22, where a bare `Hgj` diverges in none -- so a
/// case built on either would be reporting modalkit's viewport arithmetic rather than the chain.
///
/// A motion that fails is left out, because vim leaves `curswant` alone across one and nothing
/// here can see that a motion modalkit answered did not move: an `l` at the end of a line is a
/// chain vim carries on and this seam ends, which is a divergence of its own rather than one of
/// these rules.
const ENDED_BY_A_MOTION_OF_ITS_OWN: [(&str, &str); 5] = [
    ("$j0gj", "$jgj"),
    ("$wgj", "$gj"),
    ("$Hgj", "$gj"),
    ("llllllllllgjlgj", "llllllllllgjgj"),
    ("y$gj", "$gj"),
];

/// The keys that put a bare `j` between a `g$` and a screen motion, which vim answers somewhere
/// this seam does not in the narrowest window of every text here, pinned so that the divergence is
/// reported rather than rediscovered.
///
/// A `g$` leaves the chain wanting the column it landed in rather than the end of a row, which is
/// what makes the motion behind it exclusive and is asserted as such by
/// `operator_display_motions.rs`. A column is not carried across a vertical motion, so the seam
/// ends the chain at the `j` and vim does not. Carrying the `g$` end across it as a `$` end is
/// carried was measured agreeing with vim in more of these windows than it disagrees, and is not
/// adopted: the rule that would fix it is the rule the exclusive-ness of `g$` says is wrong, and
/// nothing reconciling the two has been measured.
const A_SCREEN_LINE_END_BEHIND_A_VERTICAL_MOTION: &str = "g$jgj";

/// The keys that put a bare `j` between two screen motions walking down a numbered column, which
/// vim answers somewhere this seam does not, pinned so that the divergence is reported rather than
/// rediscovered, paired with the keys in front of the second motion alone.
///
/// Only the end of a row is carried across a vertical motion, so the seam ends this chain at the
/// `j` and vim does not. Carrying the number too was measured against vim and moved forty-eight
/// cases off it that were on it, which is why it is not adopted; the reason is written out in
/// `shim`'s own docs.
///
/// The two texts below are the ones the divergence is the chain's alone on. A bare `gjj` was
/// measured landing where vim lands it on both of them in every window here, so what is left when
/// `gjgj` disagrees is the column and not the row it was measured from. On the tab-straddled text
/// the `j` itself already lands elsewhere, which would make a pin there report two things at once.
const A_NUMBERED_COLUMN_BEHIND_A_VERTICAL_MOTION: (&str, &str) = ("gjjgj", "gjj");

/// The texts the chain above is pinned on, which are the ones the `j` in front of it lands where
/// vim lands it on.
const A_NUMBERED_COLUMN_IS_THE_WHOLE_DIVERGENCE_ON: [&str; 2] = ["ascii", "cjk"];

/// The keys that put a chain of screen motions behind an end of line, which leaves the chain
/// wanting the end of every row it lands on rather than a column of its own.
const STICKING_TO_THE_END: [&str; 4] = ["$gj", "$gjgj", "g$gj", "g$gjgj"];

/// The number of cases the corpus holds that ask for a screen motion and reach it, which is the
/// sample the comparison against vim below is worth. A case added to the corpus that asks for one
/// moves this number.
const SCREENWISE_CASES: usize = 16;

/// The cases of the corpus whose keys reach a motion the display-motion audit puts out of scope,
/// paired with the keys vim's manual names that motion by. The engine refuses them before any
/// screen motion behind them is reached, so there is no cursor of ours to compare with vim's, and
/// naming them here is what keeps a case that starts or stops being refused from leaving the
/// sample unremarked.
const REFUSED_CASES: [(&str, &str); 4] = [
    ("tab-leading-indent-ts4", "|"),
    ("tab-leading-indent-ts8", "|"),
    ("wrap-w80-plain", "|"),
    ("wrap-w80-showbreak", "|"),
];

/// The corpus cases whose screen motion vim answers somewhere this seam does not, each with the
/// reason, so that a case which starts or stops agreeing fails this file rather than quietly
/// leaving the sample.
const CORPUS_DIVERGENCES: [(&str, &str); 2] = [
    (
        "flag-wrap-narrow-viewport",
        "vim draws each regional indicator two cells wide and a flag cluster two together, so the \
         row the motion is counted against is not the row this layout draws",
    ),
    (
        "nowrap-w40-horizontal-scroll",
        "a window that scrolls sideways counts a screen line from the column it is scrolled to, \
         which the viewport decides and the seam has not been handed",
    ),
];

#[test]
fn every_bare_display_motion_lands_where_vim_lands() -> anyhow::Result<()> {
    let vim = VimDriver::new()?;

    for motion in MOTIONS {
        agrees_with_vim(&vim, motion, &COLUMNS, PLAIN)?;
        agrees_with_vim(
            &vim,
            &format!("{ONTO_A_CONTINUATION_ROW}{motion}"),
            &COLUMNS,
            PLAIN,
        )?;
    }

    Ok(())
}

#[test]
fn every_bare_display_motion_on_a_decorated_row_lands_where_vim_lands() -> anyhow::Result<()> {
    let vim = VimDriver::new()?;

    for motion in MOTIONS {
        for decoration in DECORATIONS {
            agrees_with_vim(
                &vim,
                &format!("{ONTO_A_CONTINUATION_ROW}{motion}"),
                &DECORATED_COLUMNS,
                decoration,
            )?;
        }
    }

    Ok(())
}

#[test]
fn a_walk_onto_a_decorated_row_lands_where_vim_lands() -> anyhow::Result<()> {
    let vim = VimDriver::new()?;

    assert_eq!(
        DECORATED_DIVERGENCES
            .into_iter()
            .map(|(case, _reason)| case.to_owned())
            .collect::<BTreeSet<String>>(),
        diverging(&vim, DECORATED_WALKS, &DECORATED_COLUMNS)?,
        "the decorated cases that disagree with vim are not the ones named as disagreeing"
    );

    Ok(())
}

#[test]
fn a_motion_off_the_edge_of_a_logical_line_lands_where_vim_lands() -> anyhow::Result<()> {
    let vim = VimDriver::new()?;

    for (onto_the_edge, motion) in ACROSS_A_LINE_BOUNDARY {
        for (name, text) in TEXTS {
            for columns in COLUMNS {
                let edge = case(text, onto_the_edge, columns, PLAIN);
                let crossed = case(text, &format!("{onto_the_edge}{motion}"), columns, PLAIN);

                assert_ne!(
                    cursor(&edge, true).0,
                    cursor(&crossed, true).0,
                    "`{motion}` stayed on the logical line `{onto_the_edge}` walked to the edge \
                     of, on the {name} text in a window {columns} columns wide"
                );
                assert_eq!(
                    where_vim_left_it(&vim.run_case(&crossed)?),
                    cursor(&crossed, true),
                    "`{onto_the_edge}{motion}` on the {name} text in a window {columns} columns \
                     wide landed somewhere vim does not"
                );
            }
        }
    }

    Ok(())
}

#[test]
fn a_walk_down_a_ragged_buffer_tracks_the_column_vim_tracks() -> anyhow::Result<()> {
    let vim = VimDriver::new()?;

    for keys in RAGGED_WALK {
        agrees_with_vim(&vim, keys, &COLUMNS, PLAIN)?;
    }

    Ok(())
}

#[test]
fn a_counted_display_motion_lands_where_vim_lands() -> anyhow::Result<()> {
    let vim = VimDriver::new()?;

    for keys in COUNTED.into_iter().chain(COUNTED_ENDS) {
        agrees_with_vim(&vim, keys, &COLUMNS, PLAIN)?;
    }

    Ok(())
}

#[test]
fn a_chain_holds_the_column_vim_holds_across_a_motion_it_did_not_answer() -> anyhow::Result<()> {
    let vim = VimDriver::new()?;

    for keys in CARRIED_THROUGH_A_VERTICAL_MOTION
        .into_iter()
        .chain(ENDED_BY_A_MOTION_OF_ITS_OWN.map(|(keys, _carried)| keys))
    {
        agrees_with_vim(&vim, keys, &COLUMNS, PLAIN)?;
    }

    Ok(())
}

#[test]
fn a_chain_that_ends_lands_somewhere_the_same_chain_carried_on_does_not() -> anyhow::Result<()> {
    let vim = VimDriver::new()?;

    for (keys, carried) in ENDED_BY_A_MOTION_OF_ITS_OWN {
        let mut differed = false;
        for (_name, text) in TEXTS {
            for columns in COLUMNS {
                differed |= where_vim_left_it(&vim.run_case(&case(text, keys, columns, PLAIN))?)
                    != where_vim_left_it(&vim.run_case(&case(text, carried, columns, PLAIN))?);
            }
        }

        assert!(
            differed,
            "vim answers `{keys}` where it answers `{carried}`, so agreeing with it there says \
             nothing about whether the chain ended"
        );
    }

    Ok(())
}

#[test]
fn a_chain_a_screen_line_end_started_is_a_known_divergence() -> anyhow::Result<()> {
    let vim = VimDriver::new()?;

    let keys = A_SCREEN_LINE_END_BEHIND_A_VERTICAL_MOTION;
    for (name, text) in TEXTS {
        let case = case(text, keys, NARROWEST, PLAIN);

        assert_ne!(
            where_vim_left_it(&vim.run_case(&case)?),
            cursor(&case, true),
            "`{keys}` on the {name} text lands where vim lands it now, so the divergence this \
             file pins has been closed and the file should say so"
        );
    }

    Ok(())
}

#[test]
fn a_chain_a_numbered_column_started_is_a_known_divergence() -> anyhow::Result<()> {
    let vim = VimDriver::new()?;

    let (keys, stepped) = A_NUMBERED_COLUMN_BEHIND_A_VERTICAL_MOTION;
    for (name, text) in TEXTS {
        if !A_NUMBERED_COLUMN_IS_THE_WHOLE_DIVERGENCE_ON.contains(&name) {
            continue;
        }
        for columns in COLUMNS {
            let stopped = case(text, stepped, columns, PLAIN);

            assert_eq!(
                where_vim_left_it(&vim.run_case(&stopped)?),
                cursor(&stopped, true),
                "`{stepped}` on the {name} text no longer lands where vim lands it, so the \
                 divergence below is no longer the chain's alone"
            );

            let case = case(text, keys, columns, PLAIN);

            assert_ne!(
                where_vim_left_it(&vim.run_case(&case)?),
                cursor(&case, true),
                "`{keys}` on the {name} text lands where vim lands it now, so the divergence this \
                 file pins has been closed and the file should say so"
            );
        }
    }

    Ok(())
}

#[test]
fn a_walk_down_a_ragged_buffer_on_a_decorated_row_tracks_the_column_vim_tracks(
) -> anyhow::Result<()> {
    let vim = VimDriver::new()?;

    assert_eq!(
        DECORATED_RAGGED_WALK_DIVERGENCES
            .into_iter()
            .map(|(case, _reason)| case.to_owned())
            .collect::<BTreeSet<String>>(),
        diverging(&vim, RAGGED_WALK, &DECORATED_COLUMNS)?,
        "the decorated walks down a ragged buffer that disagree with vim are not the ones named \
         as disagreeing"
    );

    Ok(())
}

#[test]
fn a_counted_display_motion_on_a_decorated_row_lands_where_vim_lands() -> anyhow::Result<()> {
    let vim = VimDriver::new()?;

    assert_eq!(
        BTreeSet::<String>::new(),
        diverging(&vim, COUNTED, &DECORATED_COLUMNS)?,
        "a decorated counted motion disagrees with vim, and none is named as doing so"
    );

    Ok(())
}

#[test]
fn a_chain_on_a_decorated_row_holds_the_column_vim_holds() -> anyhow::Result<()> {
    let vim = VimDriver::new()?;
    let walks = CARRIED_THROUGH_A_VERTICAL_MOTION
        .into_iter()
        .chain(ENDED_BY_A_MOTION_OF_ITS_OWN.map(|(keys, _carried)| keys));

    assert_eq!(
        BTreeSet::<String>::new(),
        diverging(&vim, walks, &DECORATED_COLUMNS)?,
        "a decorated chain disagrees with vim, and none is named as doing so"
    );

    Ok(())
}

#[test]
fn an_end_of_line_behind_a_walk_sticks_to_the_ends_vim_sticks_to() -> anyhow::Result<()> {
    let vim = VimDriver::new()?;

    for keys in STICKING_TO_THE_END {
        agrees_with_vim(&vim, keys, &WHOLE_COLUMNS, PLAIN)?;
    }

    Ok(())
}

#[test]
fn the_engine_modalkit_answers_a_screen_motion_in_diverges_from_vim() -> anyhow::Result<()> {
    let vim = VimDriver::new()?;
    let mut answered_alike = BTreeSet::new();

    for motion in MOTIONS {
        let keys = format!("{ONTO_A_CONTINUATION_ROW}{motion}");
        let mut diverged = false;
        for (_name, text) in TEXTS {
            for columns in COLUMNS {
                let case = case(text, &keys, columns, PLAIN);
                diverged |= where_vim_left_it(&vim.run_case(&case)?) != cursor(&case, false);
            }
        }
        if !diverged {
            answered_alike.insert(motion);
        }
    }

    assert_eq!(
        BTreeSet::<&str>::new(),
        answered_alike,
        "these motions are answered alike with the shim and without it, so agreeing with vim on \
         them says nothing about the seam"
    );

    Ok(())
}

#[test]
fn every_corpus_case_that_asks_for_a_screen_motion_lands_where_vim_lands() -> anyhow::Result<()> {
    let vim = VimDriver::new()?;
    let corpus = Corpus::load_dir(&corpus::default_dir())?;

    let mut asked = BTreeSet::new();
    let mut diverged = BTreeSet::new();
    let mut refused = BTreeMap::new();
    for case in corpus.cases() {
        let mut engine = Engine::laid_out_in(&case.buffer, geometry(case));
        match engine.press_all(replayed(&case.keys)) {
            Ok(()) => {}
            Err(Error::OutOfScope { keys }) => {
                refused.insert(case.id.as_str(), keys);

                continue;
            }
            Err(error) => panic!("the keys of `{}` run: {error}", case.id),
        }
        if engine
            .shim()
            .expect("an engine laid out in a window holds a shim")
            .intercepted()
            .is_empty()
        {
            continue;
        }
        asked.insert(case.id.as_str());

        let at = engine.cursor();
        if where_vim_left_it(&vim.run_case(case)?) != (at.line as u64, at.column as u64) {
            diverged.insert(case.id.as_str());
        }
    }

    assert_eq!(
        REFUSED_CASES.into_iter().collect::<BTreeMap<&str, &str>>(),
        refused
            .iter()
            .map(|(id, keys)| (*id, keys.as_str()))
            .collect::<BTreeMap<&str, &str>>(),
        "the cases the audit refuses before they reach a screen motion are not the ones named"
    );
    assert_eq!(
        SCREENWISE_CASES,
        asked.len(),
        "the corpus asks for a screen motion in a different number of cases than it is read for"
    );
    assert_eq!(
        CORPUS_DIVERGENCES
            .into_iter()
            .collect::<BTreeMap<&str, &str>>()
            .into_keys()
            .collect::<BTreeSet<&str>>(),
        diverged,
        "the corpus cases that disagree with vim are not the ones named as disagreeing"
    );

    Ok(())
}

/// # Returns
///
/// The keys a corpus case's sequence stands for, in which `<Esc>` names the escape key and every
/// other character stands for itself.
///
/// # Panics
///
/// Panics if the sequence names a key this corpus was not written to hold.
fn replayed(sequence: &str) -> Vec<KeyEvent> {
    let mut keys = Vec::new();
    let mut rest = sequence;
    while let Some(index) = rest.find('<') {
        keys.extend(rest[..index].chars().map(typed));
        let named = &rest[index..];
        let end = named.find('>').expect("a named key is closed");
        assert_eq!(
            "<Esc>",
            &named[..=end],
            "`{sequence}` names a key this corpus was not written to hold"
        );
        keys.push(KeyEvent::from(KeyCode::Esc));
        rest = &named[end + 1..];
    }
    keys.extend(rest.chars().map(typed));

    keys
}

/// Replays `keys` against every text of [`TEXTS`] in every window of `widths`, whose continuation
/// rows are decorated as `decoration` says, through vim and through an engine whose screen motions
/// the shim answers, and requires the two to have left the cursor on the same byte of the same
/// line.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`VimDriver::run_case`]'s return values on failure.
///
/// # Panics
///
/// Panics if a replay left the two cursors somewhere different.
fn agrees_with_vim(
    vim: &VimDriver,
    keys: &str,
    widths: &[u16],
    decoration: Decoration,
) -> anyhow::Result<()> {
    for (name, text) in TEXTS {
        for columns in widths {
            let case = case(text, keys, *columns, decoration);

            assert_eq!(
                where_vim_left_it(&vim.run_case(&case)?),
                cursor(&case, true),
                "`{keys}` on the {name} text in a {} window {columns} columns wide landed \
                 somewhere vim does not",
                decoration.0
            );
        }
    }

    Ok(())
}

/// Replays every one of `walks` against every text of [`TEXTS`] in every window of `widths`, under
/// each of [`DECORATIONS`], through vim and through an engine whose screen motions the shim
/// answers.
///
/// # Type Parameters
///
/// * `WalksType` - The key sequences to replay.
///
/// # Returns
///
/// The cases the two left the cursor in different places in, named by text, width, decoration and
/// keys, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`VimDriver::run_case`]'s return values on failure.
fn diverging<WalksType: IntoIterator<Item = &'static str>>(
    vim: &VimDriver,
    walks: WalksType,
    widths: &[u16],
) -> anyhow::Result<BTreeSet<String>> {
    let mut diverged = BTreeSet::new();
    for keys in walks {
        for decoration in DECORATIONS {
            for (name, text) in TEXTS {
                for columns in widths {
                    let case = case(text, keys, *columns, decoration);
                    if where_vim_left_it(&vim.run_case(&case)?) != cursor(&case, true) {
                        diverged.insert(format!("{name} w{columns} {} {keys}", decoration.0));
                    }
                }
            }
        }
    }

    Ok(diverged)
}

/// # Returns
///
/// Where vim left the cursor, as the line and the byte within it.
fn where_vim_left_it(state: &EditorState) -> (u64, u64) {
    (state.cursor.line, state.cursor.column)
}

/// # Returns
///
/// Where an engine laid out as `case` declares leaves the cursor once the case's keys are typed at
/// it, counted the way vim counts a cursor. The engine's screen motions are answered by the shim
/// where `shimmed`, and by modalkit's own width arithmetic where it is not.
///
/// # Panics
///
/// Panics if the case's keys do not run.
fn cursor(case: &Case, shimmed: bool) -> (u64, u64) {
    let mut engine = if shimmed {
        Engine::laid_out_in(&case.buffer, geometry(case))
    } else {
        Engine::bypassing_the_shim(&case.buffer, &geometry(case))
    };
    engine
        .press_all(case.keys.chars().map(typed))
        .expect("the keys run");
    let at = engine.cursor();

    (at.line as u64, at.column as u64)
}

/// # Returns
///
/// A case replaying `keys` against `text` in a window `columns` columns wide whose continuation
/// rows are decorated as `decoration` says.
fn case(text: &str, keys: &str, columns: u16, decoration: Decoration) -> Case {
    let (name, break_indent, show_break) = decoration;

    Case {
        id: format!("display-motion-{name}-{columns}"),
        description: "A bare display motion held to vim.".to_owned(),
        buffer: text.to_owned(),
        keys: keys.to_owned(),
        viewport_width: columns,
        viewport_height: ROWS,
        tags: BTreeSet::from([Tag::Wrap]),
        options: CaseOptions {
            breakindent: break_indent,
            showbreak: show_break.to_owned(),
            ..CaseOptions::default()
        },
    }
}

/// # Returns
///
/// The layout a case's screen motions are measured in.
///
/// # Panics
///
/// Panics if the case's viewport is zero columns wide or zero rows tall, which none above is.
fn geometry(case: &Case) -> Geometry {
    let columns = NonZeroUsize::new(usize::from(case.viewport_width))
        .expect("a viewport is not zero columns wide");
    let rows = NonZeroUsize::new(usize::from(case.viewport_height))
        .expect("a viewport is not zero rows tall");
    let ambiwidth = match case.options.ambiwidth {
        CaseAmbiWidth::Single => AmbiWidth::Single,
        CaseAmbiWidth::Double => AmbiWidth::Double,
    };
    let tab_stop =
        NonZeroUsize::new(usize::from(case.options.tabstop)).expect("a tab stop is not zero");
    let options = Options::new()
        .with_break_indent(case.options.breakindent)
        .with_show_break(case.options.showbreak.clone())
        .with_line_break(case.options.linebreak);

    Geometry::new(columns, rows)
        .with_metrics(Metrics::new(ambiwidth, tab_stop))
        .with_options(options)
}
