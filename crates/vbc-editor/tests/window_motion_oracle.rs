//! Cross-checks the motions counted against a window -- `H`, `M` and `L` -- against the vim they
//! were copied from, with the window scrolled somewhere other than the top of the text.
//!
//! These three name a line of the window rather than a line of the text, so an engine that has
//! never been told where the window is answers all three against the top of the file. That is what
//! a headless harness types at: it has a text and a cursor and no viewport, and every claim about
//! `H`, `M` and `L` made from one is a claim about a window resting where it started. So every case
//! here is typed at the whole application -- the thing that owns the viewport, scrolls it to follow
//! the cursor, and hands it to the engine before each keystroke -- and the window is scrolled by
//! walking the cursor down the text before the motion is typed.
//!
//! A comparison against vim is worth nothing unless the two are looking at the same window, and the
//! two do not scroll alike in general: vim without `'smoothscroll'` anchors a window to a whole
//! line, where this viewport can anchor part-way down a wrapped one. So the keys that scroll are
//! replayed on their own first and the two windows required to be the same window -- the row drawn
//! at the top of it, which names the line it is anchored to and how far down that line the anchor
//! sits, and the row of the window the cursor is drawn on -- before any motion is compared. A text
//! whose windows drift apart fails as a divergence rather than passing as an agreement about a
//! window vim was never looking at.
//!
//! Where each window is left once the motion has run is deliberately not compared. An operator
//! over one of these motions deletes the lines the window was drawing, and what a viewport does
//! with an anchor an edit took the text out from under is this application's own policy rather
//! than the seam's: it takes the window back to the top of the text where vim clamps the anchor to
//! the last line there is. The window the keystroke was typed at is what these motions are counted
//! against, and that is the window compared.
//!
//! What the three motions count is display rows rather than lines, which is why they are the
//! layout engine's: `M` is the line halfway down the window's rows, and a window drawing one
//! wrapped line and five short ones has its half in a different place from a window drawing six
//! short ones. `'scrolloff'` is counted in rows too -- vim pulls the cursor off the edge of a
//! scrolled window by the rows it would otherwise land within -- so each case is replayed with it
//! set and unset, and vim is told about it in the same words.
//!
//! Which rows those are is not the same as which rows the window holds. A window whose next line
//! takes more rows than it has left draws none of that line and leaves the rest of its rows to the
//! marker that says so, and vim counts those rows as rows no line is drawn in -- so the half `M`
//! is counted against is a half of the rows the window drew. A window ten rows tall drawing five
//! answers `M` two lines above where a half of ten would put it, which is what
//! `the_rows_a_line_too_tall_to_draw_leaves_over_are_rows_no_line_is_drawn_in` replays.
//!
//! Every case is required to disagree with an engine that was handed no window, which is what says
//! the case turns on the viewport having been wired through rather than on the answer being the
//! same either way.
//!
//! An operator over one of the three is linewise in vim, and `'scrolloff'` does not move the line
//! it names: the cursor correction is for a cursor move, and an operator waiting on the motion
//! takes the line the window's own edge names. `dH`, `dM` and `dL` are replayed for both halves of
//! that, and what each of them took is compared line for line with what vim took.
//!
//! Where such a delete comes to rest is not always where vim rests, and the cases are named with
//! their reasons in [`CURSOR_DIVERGENCES`] rather than left to be rediscovered. Neither reason is
//! the seam's: a `dd` and a `dG` rest in the same two places, and this file asserts that they do.

mod notation;

use std::collections::BTreeSet;
use std::num::NonZeroUsize;

use ratatui::buffer::{Buffer as Cells, CellWidth};
use ratatui::layout::Rect;
use vbc_editor::app::App;
use vbc_editor::engine::Engine;
use vbc_editor::gutter::Options as GutterOptions;
use vbc_editor::screen::Geometry;
use vbc_layout::buffer::Buffer;
use vbc_layout::width::graphemes;
use vbc_oracle::corpus;
use vbc_oracle::vim::VimDriver;

use crate::notation::keys;

/// A text of short lines with two long ones among them, so the window below holds fewer lines than
/// it has rows and the line halfway down its rows is not the line halfway down its lines. Walking
/// the cursor to its tenth line scrolls the window to a place where every one of the three motions
/// names a different line.
const MIXED: &str = concat!(
    "aa one\n",
    "bb two\n",
    "cc three is a line that wraps here\n",
    "dd four\n",
    "ee five\n",
    "ff six\n",
    "gg seven is a line that wraps too\n",
    "hh eight\n",
    "ii nine\n",
    "    jj ten\n",
    "kk eleven\n",
    "ll twelve\n",
    "mm thirteen\n",
    "nn fourteen\n",
    "oo fifteen\n",
);

/// The same shape of text written in characters two cells wide, on which a line's rows are twice
/// what a count of its characters divided by the window's width says they are.
const WIDE: &str = concat!(
    "一行\n",
    "二行\n",
    "这是一行很长的中文文本会换行\n",
    "四行\n",
    "五行\n",
    "六行\n",
    "这是另外一行很长的文本也换行\n",
    "八行\n",
    "九行\n",
    "    十行\n",
    "十一行\n",
    "十二行\n",
    "十三行\n",
    "十四行\n",
    "十五行\n",
);

/// A text whose sixth line takes more rows than the window below has left for it. vim draws the
/// five lines above it and fills the rest of its rows with the marker that says a line was left
/// undrawn, and it counts those rows as empty ones -- so `M` measures its half against the five
/// rows the window drew rather than against the ten it holds. A window that called those rows
/// drawn answers `M` two lines further down.
const BRIMMING: &str = concat!(
    "aa one\n",
    "bb two\n",
    "cc three\n",
    "dd four\n",
    "ee five\n",
    "ff six is a line long enough that this window is left with no room in it at all to draw a \
     single row of it\n",
    "gg seven\n",
    "hh eight\n",
    "ii nine\n",
    "    jj ten\n",
    "kk eleven\n",
    "ll twelve\n",
    "mm thirteen\n",
    "nn fourteen\n",
    "oo fifteen\n",
);

/// The texts every case is replayed against, each named for what makes it worth replaying.
///
/// [`BRIMMING`] is not among them. The two engines do not scroll it alike -- its sixth line is
/// taller than the rows a window has left for it, and this viewport will anchor part-way down such
/// a line where vim will not -- so it is replayed by a test of its own, at the top of the text,
/// where the two are looking at the same window.
const TEXTS: [(&str, &str); 2] = [("mixed", MIXED), ("wide", WIDE)];

/// The line vim's `M` names on [`BRIMMING`] with the window at the top of it, counted from zero.
const BRIMMING_MIDDLE: u64 = 2;

/// The last line the window draws of [`BRIMMING`] resting at the top of it, counted from zero,
/// which is what `L` names there. A half counted against the ten rows the window holds rather than
/// against the five it drew names this line instead of [`BRIMMING_MIDDLE`], which is what makes
/// the text worth replaying.
const BRIMMING_BOTTOM: u64 = 4;

/// The cells the window every case is drawn into is wide.
const COLUMNS: u16 = 20;

/// The screen lines the window every case is drawn into is tall.
const ROWS: u16 = 10;

/// The keys that scroll the window before a motion counted against it is typed. Each of them walks
/// the cursor a line at a time, which is the walk both engines scroll minimally under; a jump
/// would leave vim centring the window and this application following the cursor by the fewest
/// rows that draw it, which are two different windows.
///
/// They are also the walks at which the two windows come to rest in the same place, which the
/// first test here asserts of every one of them. The two do not anchor alike everywhere: vim goes
/// on keeping `'scrolloff'` rows below a cursor there is no text below, scrolling the end of the
/// file up the window, where this viewport stops scrolling at the text, and this viewport will
/// anchor part-way down a wrapped line where vim will not. Both are differences about what a
/// window may show rather than about what a motion counted against one names, so they belong to
/// the viewport rather than here; a walk is worth using here exactly when it leaves both engines
/// looking at the same window, and the assertion says which walks those are rather than a comment.
const SCROLLS: [(&str, &str); 4] = [
    ("at the top", ""),
    ("eight lines down", "jjjjjjjj"),
    ("nine lines down", "jjjjjjjjj"),
    ("eleven lines down", "jjjjjjjjjjj"),
];

/// The rows kept between the cursor and an edge of the window, which is vim's `'scrolloff'`.
const SCROLLOFFS: [usize; 2] = [0, 3];

/// The motions counted against the window, and the operators run over them.
const MOTIONS: [&str; 8] = ["H", "M", "L", "3H", "2L", "dH", "dL", "dM"];

/// The cases whose cursor vim leaves somewhere this engine does not, each with the reason, so that
/// a case which starts or stops agreeing fails this file rather than quietly leaving the sample.
///
/// Every one of them is where a linewise delete comes to rest rather than which line it named, and
/// neither reason is this seam's. vim ends a linewise delete on the first non-blank of the line it
/// left behind where this engine rests in that line's first column, and where the delete reached
/// the last line of the text this engine rests at the far end of the line left behind instead.
/// A `dd` and a `dG` -- neither of which the seam ever touches -- rest in exactly the same two
/// places, and `a_linewise_delete_rests_where_modalkit_rests_whatever_named_its_lines` asserts
/// that, so a modalkit which starts placing them where vim does fails this file.
const CURSOR_DIVERGENCES: [(&str, &str); 12] = [
    (
        "mixed so0 eight lines down dH",
        "the delete leaves an indented line behind and rests in its first column",
    ),
    (
        "mixed so0 eight lines down dL",
        "the delete leaves an indented line behind and rests in its first column",
    ),
    (
        "mixed so0 eight lines down dM",
        "the delete leaves an indented line behind and rests in its first column",
    ),
    (
        "mixed so3 eight lines down dH",
        "the delete leaves an indented line behind and rests in its first column",
    ),
    (
        "mixed so3 eight lines down dM",
        "the delete leaves an indented line behind and rests in its first column",
    ),
    (
        "mixed so3 eleven lines down dL",
        "the delete reaches the last line of the text and rests at the far end of the line left \
         behind",
    ),
    (
        "wide so0 eight lines down dH",
        "the delete leaves an indented line behind and rests in its first column",
    ),
    (
        "wide so0 eight lines down dL",
        "the delete leaves an indented line behind and rests in its first column",
    ),
    (
        "wide so0 eight lines down dM",
        "the delete leaves an indented line behind and rests in its first column",
    ),
    (
        "wide so3 eight lines down dH",
        "the delete leaves an indented line behind and rests in its first column",
    ),
    (
        "wide so3 eight lines down dM",
        "the delete leaves an indented line behind and rests in its first column",
    ),
    (
        "wide so3 eleven lines down dL",
        "the delete reaches the last line of the text and rests at the far end of the line left \
         behind",
    ),
];

/// The keys that delete linewise without a motion this seam answers, one landing on an indented
/// line and one reaching the last line of the text, which is what says the divergence named above
/// is modalkit's rather than the seam's.
const LINEWISE_DELETES_THAT_REST_ELSEWHERE: [&str; 3] = ["8jdd", "8jVd", "6jdG"];

#[test]
fn every_window_left_where_a_scroll_leaves_it_is_the_window_vim_is_looking_at() -> anyhow::Result<()>
{
    let vim = VimDriver::new()?;

    for (name, text) in TEXTS {
        for (_where_from, scroll) in SCROLLS {
            for scrolloff in SCROLLOFFS {
                let scrolled = vim.run_case(&case(text, scroll, scrolloff))?;
                let mut standing = typed(text, scroll, scrolloff);

                assert_eq!(
                    (
                        scrolled.screen_text.row(0).unwrap_or_default().trim_end(),
                        Some(scrolled.display_position.row)
                    ),
                    (top_row(&mut standing).as_str(), cursor_row(&mut standing)),
                    "the window `{scroll}` left this application on is not the window it left vim \
                     on, on the {name} text with `scrolloff` at {scrolloff}, so a motion counted \
                     against it would be compared against a window vim was never looking at"
                );
            }
        }
    }

    Ok(())
}

#[test]
fn every_window_motion_takes_and_lands_where_vim_does() -> anyhow::Result<()> {
    let vim = VimDriver::new()?;
    let mut diverged = BTreeSet::new();
    for (name, text) in TEXTS {
        for (where_from, scroll) in SCROLLS {
            for scrolloff in SCROLLOFFS {
                for motion in MOTIONS {
                    let keys = format!("{scroll}{motion}");
                    let state = vim.run_case(&case(text, &keys, scrolloff))?;
                    let ours = typed(text, &keys, scrolloff);

                    assert_eq!(
                        state.buffer,
                        written(&ours),
                        "`{motion}` {where_from} took something other than what vim took, on the \
                         {name} text with `scrolloff` at {scrolloff}"
                    );
                    if (state.cursor.line, state.cursor.column) != where_it_left_the_cursor(&ours) {
                        diverged.insert(format!("{name} so{scrolloff} {where_from} {motion}"));
                    }
                }
            }
        }
    }

    assert_eq!(
        CURSOR_DIVERGENCES
            .into_iter()
            .map(|(case, _reason)| case.to_owned())
            .collect::<BTreeSet<String>>(),
        diverged,
        "the cases that leave the cursor somewhere vim does not are not the ones named as leaving \
         it there"
    );

    Ok(())
}

#[test]
fn a_linewise_delete_rests_where_modalkit_rests_whatever_named_its_lines() -> anyhow::Result<()> {
    let vim = VimDriver::new()?;

    for keys in LINEWISE_DELETES_THAT_REST_ELSEWHERE {
        let state = vim.run_case(&case(MIXED, keys, 0))?;
        let ours = typed(MIXED, keys, 0);

        assert_eq!(
            state.buffer,
            written(&ours),
            "`{keys}` took something other than what vim took, which is more than a cursor apart"
        );
        assert_ne!(
            (state.cursor.line, state.cursor.column),
            where_it_left_the_cursor(&ours),
            "`{keys}` deletes whole lines without a motion this seam answers and now rests where \
             vim rests, so the reason written down for the divergence above no longer describes it"
        );
    }

    Ok(())
}

#[test]
fn the_rows_a_line_too_tall_to_draw_leaves_over_are_rows_no_line_is_drawn_in() -> anyhow::Result<()>
{
    let vim = VimDriver::new()?;
    let mut standing = typed(BRIMMING, "", 0);
    let scrolled = vim.run_case(&case(BRIMMING, "", 0))?;

    assert_eq!(
        (
            scrolled.screen_text.row(0).unwrap_or_default().trim_end(),
            Some(scrolled.display_position.row)
        ),
        (top_row(&mut standing).as_str(), cursor_row(&mut standing)),
        "the window this application rests at the top of the brimming text on is not the window \
         vim rests on, so nothing counted against it says anything about vim"
    );
    let named = vim.run_case(&case(BRIMMING, "M", 0))?.cursor.line;
    let last = vim.run_case(&case(BRIMMING, "L", 0))?.cursor.line;

    assert_eq!(
        (BRIMMING_MIDDLE, BRIMMING_BOTTOM),
        (named, last),
        "vim no longer answers `M` with line {BRIMMING_MIDDLE} of a window whose last drawn line \
         is line {BRIMMING_BOTTOM}, so the text no longer tells a half of the rows the window drew \
         from a half of the rows it holds"
    );
    for motion in MOTIONS {
        let state = vim.run_case(&case(BRIMMING, motion, 0))?;
        let ours = typed(BRIMMING, motion, 0);

        assert_eq!(
            state.buffer,
            written(&ours),
            "`{motion}` took something other than what vim took on the brimming text, whose \
             window leaves half its rows to the marker that says a line was too tall to draw"
        );
    }

    Ok(())
}

#[test]
fn every_scrolled_case_disagrees_with_an_engine_that_was_handed_no_window() -> anyhow::Result<()> {
    for (name, text) in TEXTS {
        for (where_from, scroll) in SCROLLS.into_iter().skip(1) {
            for scrolloff in SCROLLOFFS {
                for motion in MOTIONS {
                    let keys = format!("{scroll}{motion}");
                    let app = typed(text, &keys, scrolloff);
                    let mut engine = Engine::laid_out_in(text, geometry(scrolloff));
                    engine.press_all(self::keys(&keys))?;
                    let alone = engine.cursor();

                    assert_ne!(
                        (alone.line as u64, alone.column as u64),
                        where_it_left_the_cursor(&app),
                        "`{motion}` {where_from} lands where an engine that was never told where \
                         the window is lands it, on the {name} text with `scrolloff` at \
                         {scrolloff}, so the case says nothing about the window"
                    );
                }
            }
        }
    }

    Ok(())
}

#[test]
fn scrolloff_moves_a_cursor_and_leaves_the_lines_an_operator_takes_alone() -> anyhow::Result<()> {
    for (name, text) in TEXTS {
        let scroll = SCROLLS[1].1;
        for scrolloff in SCROLLOFFS {
            let standing = typed(text, scroll, scrolloff).cursor().line;
            let reached = typed(text, &format!("{scroll}H"), scrolloff).cursor().line;
            let taken = typed(text, &format!("{scroll}dH"), scrolloff);
            let spanned = standing - reached + 1;
            let removed = text.lines().count() - taken.text().line_count();

            if 0 == scrolloff {
                assert_eq!(
                    spanned, removed,
                    "with no rows to keep, `dH` took something other than the lines a bare `H` \
                     reaches back over, on the {name} text"
                );
            } else {
                assert!(
                    spanned < removed,
                    "`dH` took the {spanned} lines a bare `H` reaches back over rather than the \
                     lines back to the top of the window, on the {name} text with `scrolloff` at \
                     {scrolloff}, so the cursor correction is being applied to an operator"
                );
            }
        }
    }

    Ok(())
}

/// # Returns
///
/// The application `text` is drawn in, with `scrolloff` rows kept between the cursor and an edge,
/// once `keys` have been typed at it.
///
/// # Panics
///
/// Panics if the keys name one this notation does not hold.
fn typed(text: &str, keys: &str, scrolloff: usize) -> App {
    let mut app = App::new(Buffer::from_text(text.strip_suffix('\n').unwrap_or(text)))
        .with_gutter(GutterOptions::new())
        .with_scrolloff(scrolloff);
    for key in self::keys(keys) {
        app.press(area(), key);
    }

    app
}

/// # Returns
///
/// The row the application drew at the top of the window, trailing blanks trimmed, in the terms
/// vim reports a screen row in: a cell a wider grapheme beside it claimed is passed over rather
/// than read as a blank of its own.
fn top_row(app: &mut App) -> String {
    let mut cells = Cells::empty(area());
    app.draw(&mut cells, area());
    let mut row = String::new();
    let mut column = 0;
    while column < cells.area.width {
        let cell = &cells[(column, 0)];
        row.push_str(cell.symbol());
        column += cell.cell_width().max(1);
    }

    row.trim_end().to_owned()
}

/// # Returns
///
/// The row of the window the application draws the cursor on, and [`None`] where the window does
/// not draw the cursor at all.
fn cursor_row(app: &mut App) -> Option<u64> {
    let mut cells = Cells::empty(area());

    app.draw(&mut cells, area())
        .map(|position| u64::from(position.y))
}

/// # Returns
///
/// Where the application left the cursor, as the line and the byte within it, which is how vim
/// reports one.
fn where_it_left_the_cursor(app: &App) -> (u64, u64) {
    let at = app.cursor();
    let line = app.text().line(at.line).unwrap_or_default();
    let column: usize = graphemes(line).take(at.grapheme).map(str::len).sum();

    (at.line as u64, column as u64)
}

/// # Returns
///
/// The text the application holds, ending in a newline as the text vim reports does.
fn written(app: &App) -> String {
    format!("{}\n", app.text().text())
}

/// # Returns
///
/// The area every case is drawn into.
fn area() -> Rect {
    Rect::new(0, 0, COLUMNS, ROWS)
}

/// # Returns
///
/// The window an engine handed no viewport is laid out in, which is the window the application
/// draws into [`area`].
///
/// # Panics
///
/// Panics if the window is zero columns wide or zero rows tall, which it is not.
fn geometry(scrolloff: usize) -> Geometry {
    let columns =
        NonZeroUsize::new(usize::from(COLUMNS)).expect("the window is not zero columns wide");
    let rows = NonZeroUsize::new(usize::from(ROWS)).expect("the window is not zero rows tall");

    Geometry::new(columns, rows).with_scrolloff(scrolloff)
}

/// # Returns
///
/// A case replaying `keys` against `text` in the window every case here is drawn in, with vim told
/// about `scrolloff` in the words it reads them in.
fn case(text: &str, keys: &str, scrolloff: usize) -> corpus::Case {
    corpus::Case {
        id: "window-motion".to_owned(),
        description: "A motion counted against a scrolled window.".to_owned(),
        buffer: text.to_owned(),
        keys: format!(":set scrolloff={scrolloff}<CR>{keys}"),
        viewport_width: COLUMNS,
        viewport_height: ROWS,
        tags: BTreeSet::new(),
        options: corpus::Options::default(),
    }
}
