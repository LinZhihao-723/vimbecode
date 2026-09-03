//! The bare display motions held to the vim they are copied from.
//!
//! `gj`, `gk`, `g0`, `g^`, `g$` and `gm` are the motions a window's own cells decide the answer to,
//! and the shim exists so that the layout engine decides them rather than modalkit's character
//! arithmetic. Whether it decides them the way vim does is not something this workspace can settle
//! by itself, so every case here is replayed against a real vim in the window the case declares,
//! and the two cursors are required to rest on the same byte of the same line.
//!
//! Which text a case is replayed against is the point of the exercise rather than a detail of it.
//! On plain ASCII a character column and a display column are the same number, so a comparison
//! made only there would pass against the very arithmetic the shim was written to replace. The
//! same motions are therefore replayed against CJK, where every character is two cells wide, and
//! against a text indented with tabs, where one character is eight; and each is laid out in more
//! than one window, because a motion answered by dividing by the wrong width is right in the
//! window whose width happens to divide evenly.
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
use vbc_editor::engine::{typed, Engine};
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

/// The texts every motion is replayed against, each named for what makes it worth replaying.
const TEXTS: [(&str, &str); 3] = [("ascii", WRAPPED), ("cjk", WIDE), ("tab", TABBED)];

/// The windows the texts are laid out in. A motion answered by dividing a character column by a
/// window's width is right wherever that width divides the text evenly, so one of these is an odd
/// number of columns, in which a double-width character cannot end a row.
const COLUMNS: [u16; 3] = [16, 21, 40];

/// The windows in which no row is cut short by a double-width character that did not fit, which
/// are the windows the chain of ends is compared in.
const WHOLE_COLUMNS: [u16; 2] = [16, 40];

/// The screen lines the windows below hold, which is more rows than any case's text draws.
const ROWS: u16 = 10;

/// The keys each motion is reached by.
const MOTIONS: [&str; 6] = ["gj", "gk", "g0", "g^", "g$", "gm"];

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

/// The counted motions, including counts larger than the text has rows for.
const COUNTED: [&str; 8] = ["3gj", "5gj", "12gj", "G4gk", "G12gk", "2g$", "3g$", "2gjg0"];

/// The keys that put a chain of screen motions behind an end of line, which leaves the chain
/// wanting the end of every row it lands on rather than a column of its own.
const STICKING_TO_THE_END: [&str; 4] = ["$gj", "$gjgj", "g$gj", "g$gjgj"];

/// The number of cases the corpus holds that ask for a screen motion, which is the sample the
/// comparison against vim below is worth. A case added to the corpus that asks for one moves this
/// number.
const SCREENWISE_CASES: usize = 18;

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
        agrees_with_vim(&vim, motion, &COLUMNS)?;
        agrees_with_vim(
            &vim,
            &format!("{ONTO_A_CONTINUATION_ROW}{motion}"),
            &COLUMNS,
        )?;
    }

    Ok(())
}

#[test]
fn a_motion_off_the_edge_of_a_logical_line_lands_where_vim_lands() -> anyhow::Result<()> {
    let vim = VimDriver::new()?;

    for (onto_the_edge, motion) in ACROSS_A_LINE_BOUNDARY {
        for (name, text) in TEXTS {
            for columns in COLUMNS {
                let edge = case(text, onto_the_edge, columns);
                let crossed = case(text, &format!("{onto_the_edge}{motion}"), columns);

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
        agrees_with_vim(&vim, keys, &COLUMNS)?;
    }

    Ok(())
}

#[test]
fn a_counted_display_motion_lands_where_vim_lands() -> anyhow::Result<()> {
    let vim = VimDriver::new()?;

    for keys in COUNTED {
        agrees_with_vim(&vim, keys, &COLUMNS)?;
    }

    Ok(())
}

#[test]
fn an_end_of_line_behind_a_walk_sticks_to_the_ends_vim_sticks_to() -> anyhow::Result<()> {
    let vim = VimDriver::new()?;

    for keys in STICKING_TO_THE_END {
        agrees_with_vim(&vim, keys, &WHOLE_COLUMNS)?;
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
                let case = case(text, &keys, columns);
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
    for case in corpus.cases() {
        let mut engine = Engine::laid_out_in(&case.buffer, geometry(case));
        engine
            .press_all(replayed(&case.keys))
            .unwrap_or_else(|error| panic!("the keys of `{}` run: {error}", case.id));
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

/// Replays `keys` against every text of [`TEXTS`] in every window of `widths`, through vim and
/// through an engine whose screen motions the shim answers, and requires the two to have left the
/// cursor on the same byte of the same line.
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
fn agrees_with_vim(vim: &VimDriver, keys: &str, widths: &[u16]) -> anyhow::Result<()> {
    for (name, text) in TEXTS {
        for columns in widths {
            let case = case(text, keys, *columns);

            assert_eq!(
                where_vim_left_it(&vim.run_case(&case)?),
                cursor(&case, true),
                "`{keys}` on the {name} text in a window {columns} columns wide landed somewhere \
                 vim does not"
            );
        }
    }

    Ok(())
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
/// A case replaying `keys` against `text` in a window `columns` columns wide.
fn case(text: &str, keys: &str, columns: u16) -> Case {
    Case {
        id: format!("display-motion-{columns}"),
        description: "A bare display motion held to vim.".to_owned(),
        buffer: text.to_owned(),
        keys: keys.to_owned(),
        viewport_width: columns,
        viewport_height: ROWS,
        tags: BTreeSet::from([Tag::Wrap]),
        options: CaseOptions::default(),
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
