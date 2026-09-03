//! The seam screen motions are taken out of modalkit's hands at, held to the two things a seam is
//! worth having only if both are true: that what it is for reaches it, and that everything else
//! reaches the text exactly as it did before it existed.
//!
//! The control group is the whole corpus. Every case is replayed twice -- once through an engine
//! with the shim installed and once through an engine built without one -- and the two are
//! required to end byte for byte in the same state, or to fail in the same way. A seam that
//! disturbed the logical path would show up here as a case whose text, cursor, mode or registers
//! moved, and there is nothing about a case counted in characters that this file exempts.
//!
//! That the seam is reached is asserted rather than inferred. A seam nothing ever enters would
//! pass every comparison in this file and be worth nothing at all, so the screen motions the
//! corpus asks for are named case by case, in the order each case asks for them, and the engine is
//! required to have handed exactly those to the shim. The cases that ask for none are named by
//! being everything else, and are required to hand it nothing.
//!
//! Both of those would still pass against a comparison that cannot fail, so the comparison is held
//! to each of the four dimensions it holds in turn: a key that moves the text, one that moves the
//! cursor, one that changes the mode and one that fills a register are each required to be
//! reported. And what the shim measures is checked where the answer separates the two engines: on
//! a line of CJK the cell the cursor is drawn in and the character it sits at are different
//! numbers, and the shim is required to be holding the first.
//!
//! The measurement is then held to the layout it was handed rather than to a number it could have
//! reached by any route: the same cursor is measured in two windows, under both `'ambiwidth'`
//! settings and with and without a `'showbreak'` marker, and each pair is required to answer
//! differently. A cursor modalkit left in the middle of a cluster is required to be measured at
//! the cell the whole cluster is drawn in, and a motion is required to reach the shim with the
//! count its keys asked for already resolved.

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroUsize;

use crossterm::event::KeyCode;
use editor_types::prelude::{MoveDir1D, MovePosition};
use modalkit::env::vim::VimMode;
use vbc_editor::engine::{typed, Engine, Error, Held};
use vbc_editor::event::KeyEvent;
use vbc_editor::screen::Geometry;
use vbc_editor::shim::ScreenMotion;
use vbc_layout::line::Options;
use vbc_layout::position::DisplayPosition;
use vbc_layout::width::{AmbiWidth, Metrics};
use vbc_oracle::corpus::{self, AmbiWidth as CaseAmbiWidth, Case, Corpus};

/// The screen motions each case of the corpus asks for, in the order it asks for them. A case the
/// corpus holds and this table does not asks for none.
const SCREENWISE: [(&str, &[ScreenMotion]); 18] = [
    (
        "anchor-walk-w40-breakindent-showbreak",
        &[ScreenMotion::LinePos(MovePosition::End)],
    ),
    (
        "cjk-ambiwidth-double",
        &[ScreenMotion::LinePos(MovePosition::End)],
    ),
    (
        "cjk-ambiwidth-single",
        &[ScreenMotion::LinePos(MovePosition::End)],
    ),
    (
        "cjk-wide-cell-straddles-edge",
        &[ScreenMotion::Line(MoveDir1D::Next)],
    ),
    (
        "emoji-zwj-family-wrap-edge",
        &[ScreenMotion::LinePos(MovePosition::End)],
    ),
    (
        "flag-wrap-narrow-viewport",
        &[ScreenMotion::LinePos(MovePosition::End)],
    ),
    (
        "nowrap-w40-horizontal-scroll",
        &[ScreenMotion::LinePos(MovePosition::Beginning)],
    ),
    (
        "tab-wrapped-with-breakindent",
        &[
            ScreenMotion::Line(MoveDir1D::Next),
            ScreenMotion::Line(MoveDir1D::Next),
        ],
    ),
    (
        "wrap-w20-boundary-exact",
        &[ScreenMotion::Line(MoveDir1D::Next)],
    ),
    (
        "wrap-w20-plain",
        &[
            ScreenMotion::Line(MoveDir1D::Next),
            ScreenMotion::Line(MoveDir1D::Next),
        ],
    ),
    (
        "wrap-w24-breakindent",
        &[
            ScreenMotion::Line(MoveDir1D::Next),
            ScreenMotion::Line(MoveDir1D::Next),
        ],
    ),
    (
        "wrap-w24-breakindent-showbreak",
        &[
            ScreenMotion::Line(MoveDir1D::Next),
            ScreenMotion::Line(MoveDir1D::Next),
        ],
    ),
    (
        "wrap-w24-plain",
        &[
            ScreenMotion::Line(MoveDir1D::Next),
            ScreenMotion::Line(MoveDir1D::Next),
        ],
    ),
    (
        "wrap-w24-showbreak",
        &[
            ScreenMotion::Line(MoveDir1D::Next),
            ScreenMotion::Line(MoveDir1D::Next),
        ],
    ),
    (
        "wrap-w40-breakindent-showbreak",
        &[
            ScreenMotion::Line(MoveDir1D::Next),
            ScreenMotion::LinePos(MovePosition::End),
        ],
    ),
    (
        "wrap-w40-plain",
        &[
            ScreenMotion::Line(MoveDir1D::Next),
            ScreenMotion::LinePos(MovePosition::End),
        ],
    ),
    ("wrap-w80-plain", &[ScreenMotion::Line(MoveDir1D::Next)]),
    ("wrap-w80-showbreak", &[ScreenMotion::Line(MoveDir1D::Next)]),
];

/// A line whose every character is drawn two cells wide, on which the column a cursor is drawn in
/// and the character it sits at are different numbers.
const WIDE: &str = "你好世界一二三四五六\n";

/// The columns [`WIDE`] is measured in.
const WIDE_COLUMNS: usize = 10;

/// The rows [`WIDE`] is measured in.
const WIDE_ROWS: usize = 5;

/// Keys counted in characters, which the shim has no business seeing.
const CHARACTERWISE: &str = "wwbee0$jkxdwyyp";

/// The text the keys named here are typed at.
const PROSE: &str = "the quick brown fox\njumps over it\nand lands\n";

/// Keys that move one of the dimensions a replay is compared in, paired with the dimension each
/// moves, which is what says the comparison is sensitive to every dimension it holds.
const MOVED: [(&str, &str); 4] = [
    ("x", "text"),
    ("l", "cursor"),
    ("i", "mode"),
    ("yy", "registers"),
];

/// A line long enough to wrap in a narrow window and not in a wide one, on which the row a cursor
/// is drawn in is not the same number in every window.
const RUNNING: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\nbbbb\n";

/// A line of characters whose width is the one `'ambiwidth'` names, on which the cell a cursor is
/// drawn in is not the same number under both settings.
const AMBIGUOUS: &str = "\u{b1}\u{b1}\u{b1}\u{b1}\u{b1}\u{b1}\u{b1}\u{b1}\nbbbb\n";

/// A line beginning with a family emoji joined by zero-width joiners, whose characters a screen
/// draws as one cluster.
const CLUSTERED: &str = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}ab\ncccc\n";

/// Keys that walk the cursor twelve characters along a line and then ask for a screen motion.
const WALKED_FAR_THEN_SCREENWISE: &str = "llllllllllllgj";

/// Keys that walk the cursor three characters along a line and then ask for a screen motion.
const WALKED_THEN_SCREENWISE: &str = "lllgj";

/// The keys a screen motion is asked for with, and the count it reaches the shim resolved to.
/// Each motion counts in its own terms: `gj` and `H` count the screen lines to move, `g$` counts
/// the screen lines below the cursor's own, and `g0` takes no count at all.
const COUNTED: [(&str, usize); 8] = [
    ("gj", 1),
    ("3gj", 3),
    ("12gj", 12),
    ("H", 1),
    ("3H", 3),
    ("g$", 0),
    ("2g$", 1),
    ("g0", 0),
];

/// The window a line is laid out in where a measurement does not turn on the window's width.
const ROOMY_COLUMNS: usize = 20;

/// The screen lines the measurements this file makes are laid out in.
const MEASURED_ROWS: usize = 5;

/// What a replay left an engine holding, which is everything the engine is the authority on.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Outcome {
    text: String,
    line: usize,
    column: usize,
    mode: VimMode,
    registers: BTreeMap<char, Held>,
}

#[test]
fn the_whole_corpus_ends_where_it_ended_before_the_seam_existed() {
    let corpus = Corpus::load_dir(&corpus::default_dir()).expect("the corpus loads");
    let screenwise: BTreeSet<&str> = SCREENWISE.iter().map(|(id, _)| *id).collect();

    let mut control = BTreeSet::new();
    for case in corpus.cases() {
        let group = if screenwise.contains(case.id.as_str()) {
            "screen motion"
        } else {
            control.insert(case.id.as_str());
            "control"
        };

        assert_eq!(
            replay(case, false).0,
            replay(case, true).0,
            "the {group} case `{}` ends somewhere else with the shim installed",
            case.id
        );
    }

    assert_eq!(
        corpus.cases().len(),
        control.len() + screenwise.len(),
        "the corpus holds a case that is neither in the control group nor named as screenwise"
    );
    assert!(
        !control.is_empty(),
        "the control group is empty, so the comparison above compared nothing"
    );
}

#[test]
fn the_comparison_reports_a_state_that_moved_in_any_dimension_it_holds() {
    let mut resting = Engine::new(PROSE);
    let resting = outcome(&mut resting);

    for (keys, dimension) in MOVED {
        let mut engine = Engine::new(PROSE);
        engine
            .press_all(keys.chars().map(typed))
            .expect("the keys run");

        assert!(
            moved(&resting, &outcome(&mut engine)).contains(&dimension),
            "typing `{keys}` moved the {dimension} and the comparison did not report it"
        );
    }
}

#[test]
fn every_screen_motion_the_corpus_asks_for_reaches_the_shim() {
    let corpus = Corpus::load_dir(&corpus::default_dir()).expect("the corpus loads");
    let expected: BTreeMap<&str, &[ScreenMotion]> = SCREENWISE.into_iter().collect();

    let mut asked = BTreeSet::new();
    for case in corpus.cases() {
        let taken: Vec<ScreenMotion> = replay(case, true)
            .1
            .into_iter()
            .map(|(motion, _count)| motion)
            .collect();
        if !taken.is_empty() {
            asked.insert(case.id.as_str());
        }

        assert_eq!(
            expected.get(case.id.as_str()).copied().unwrap_or_default(),
            taken.as_slice(),
            "case `{}` handed the shim motions other than the ones it asks for",
            case.id
        );
    }

    assert_eq!(
        expected.keys().copied().collect::<BTreeSet<&str>>(),
        asked,
        "the cases that reached the shim are not the ones named as screenwise"
    );
}

#[test]
fn a_motion_counted_in_characters_is_never_handed_to_the_shim() {
    let mut engine = Engine::new(PROSE);
    engine
        .press_all(CHARACTERWISE.chars().map(typed))
        .expect("the character-counted keys run");

    assert_eq!(
        Vec::<ScreenMotion>::new(),
        engine
            .shim()
            .expect("an engine built by `new` holds a shim")
            .intercepted()
            .iter()
            .map(|taken| taken.motion)
            .collect::<Vec<ScreenMotion>>()
    );
}

#[test]
fn an_engine_built_without_a_shim_has_nothing_for_a_screen_motion_to_reach() {
    let mut engine = Engine::bypassing_the_shim(PROSE, &laid_out(ROOMY_COLUMNS));
    engine
        .press_all("gjg$".chars().map(typed))
        .expect("the screen motions run");

    assert!(engine.shim().is_none());
}

#[test]
fn the_shim_holds_the_cell_the_cursor_is_drawn_in_rather_than_the_character_it_sits_at() {
    let columns = NonZeroUsize::new(WIDE_COLUMNS).expect("the columns are not zero");
    let rows = NonZeroUsize::new(WIDE_ROWS).expect("the rows are not zero");
    let mut engine = Engine::laid_out_in(WIDE, Geometry::new(columns, rows));
    engine
        .press_all("lllgj".chars().map(typed))
        .expect("the keys run");

    let taken = engine
        .shim()
        .expect("an engine built by `laid_out_in` holds a shim")
        .intercepted();

    assert_eq!(1, taken.len());
    assert_eq!(ScreenMotion::Line(MoveDir1D::Next), taken[0].motion);
    assert_eq!(1, taken[0].count);
    assert_eq!(
        6, taken[0].from.column,
        "the shim measured the cursor in characters rather than in cells"
    );
    assert_eq!(3, engine.cursor().column / "你".len());
}

#[test]
fn a_screen_motion_is_measured_in_the_window_the_engine_was_built_for() {
    for (columns, expected) in [
        (10, DisplayPosition { row: 1, column: 2 }),
        (80, DisplayPosition { row: 0, column: 12 }),
    ] {
        assert_eq!(
            expected,
            measured(RUNNING, laid_out(columns), WALKED_FAR_THEN_SCREENWISE),
            "the cursor was measured somewhere other than in a window {columns} columns wide"
        );
    }
}

#[test]
fn a_screen_motion_is_measured_with_the_metrics_the_geometry_carries() {
    let tab_stop = NonZeroUsize::new(8).expect("the tab stop is not zero");
    for (ambiwidth, expected) in [(AmbiWidth::Single, 3), (AmbiWidth::Double, 6)] {
        let geometry = laid_out(ROOMY_COLUMNS).with_metrics(Metrics::new(ambiwidth, tab_stop));

        assert_eq!(
            expected,
            measured(AMBIGUOUS, geometry, WALKED_THEN_SCREENWISE).column,
            "a character of ambiguous width was not measured as `{ambiwidth:?}` draws it"
        );
    }
}

#[test]
fn a_screen_motion_is_measured_with_the_wrapping_options_the_geometry_carries() {
    for (marker, expected) in [("", 2), (">>", 4)] {
        let geometry = laid_out(10).with_options(Options::new().with_show_break(marker.to_owned()));

        assert_eq!(
            expected,
            measured(RUNNING, geometry, WALKED_FAR_THEN_SCREENWISE).column,
            "a continuation row was not measured beside the marker `{marker}` it is drawn with"
        );
    }
}

#[test]
fn a_cursor_inside_a_cluster_is_measured_at_the_cell_the_whole_cluster_is_drawn_in() {
    assert_eq!(
        DisplayPosition { row: 0, column: 0 },
        measured(CLUSTERED, laid_out(ROOMY_COLUMNS), "llgj"),
        "a cursor modalkit left inside a cluster was measured past the cluster"
    );
    assert_eq!(
        DisplayPosition { row: 0, column: 2 },
        measured(CLUSTERED, laid_out(ROOMY_COLUMNS), "lllllgj"),
        "a cursor on the grapheme after a cluster was not measured past the cluster"
    );
}

#[test]
fn a_screen_motion_reaches_the_shim_with_the_count_the_keys_asked_for_resolved() {
    for (keys, expected) in COUNTED {
        let mut engine = Engine::laid_out_in(RUNNING, laid_out(ROOMY_COLUMNS));
        engine
            .press_all(keys.chars().map(typed))
            .expect("the keys run");

        assert_eq!(
            expected,
            engine
                .shim()
                .expect("an engine built by `laid_out_in` holds a shim")
                .intercepted()[0]
                .count,
            "`{keys}` reached the shim asking to be run a different number of times"
        );
    }
}

/// # Returns
///
/// Where the layout engine drew the cursor for the screen motion `keys` ends with, typed at
/// `text` through an engine measuring in `geometry`.
///
/// # Panics
///
/// Panics if the keys do not run, or if they ask for no screen motion.
fn measured(text: &str, geometry: Geometry, keys: &str) -> DisplayPosition {
    let mut engine = Engine::laid_out_in(text, geometry);
    engine
        .press_all(keys.chars().map(typed))
        .expect("the keys run");
    let taken = engine
        .shim()
        .expect("an engine built by `laid_out_in` holds a shim")
        .intercepted();
    assert_eq!(
        1,
        taken.len(),
        "`{keys}` asks for exactly one screen motion"
    );

    taken[0].from
}

/// # Returns
///
/// A window `columns` columns wide, in which the measurements this file makes are laid out.
///
/// # Panics
///
/// Panics if `columns` is zero.
fn laid_out(columns: usize) -> Geometry {
    Geometry::new(
        NonZeroUsize::new(columns).expect("the columns are not zero"),
        NonZeroUsize::new(MEASURED_ROWS).expect("the rows are not zero"),
    )
}

/// Replays a case's keys against its buffer, through an engine laid out as the case declares.
///
/// # Returns
///
/// * What the engine was left holding, or the error it failed with.
/// * The screen motions the shim was handed, empty where none was installed.
fn replay(case: &Case, with_shim: bool) -> (Result<Outcome, Error>, Vec<(ScreenMotion, usize)>) {
    let mut engine = if with_shim {
        Engine::laid_out_in(&case.buffer, geometry_of(case))
    } else {
        Engine::bypassing_the_shim(&case.buffer, &geometry_of(case))
    };
    let outcome = engine
        .press_all(keys(&case.keys))
        .map(|()| outcome(&mut engine));
    let taken = engine
        .shim()
        .map(|shim| {
            shim.intercepted()
                .iter()
                .map(|taken| (taken.motion, taken.count))
                .collect()
        })
        .unwrap_or_default();

    (outcome, taken)
}

/// # Returns
///
/// The dimensions in which `before` and `after` disagree, in the order an outcome holds them.
fn moved(before: &Outcome, after: &Outcome) -> Vec<&'static str> {
    let mut moved = Vec::new();
    if before.text != after.text {
        moved.push("text");
    }
    if (before.line, before.column) != (after.line, after.column) {
        moved.push("cursor");
    }
    if before.mode != after.mode {
        moved.push("mode");
    }
    if before.registers != after.registers {
        moved.push("registers");
    }

    moved
}

/// # Returns
///
/// Everything the engine is the authority on.
fn outcome(engine: &mut Engine) -> Outcome {
    let cursor = engine.cursor();

    Outcome {
        text: engine.text(),
        line: cursor.line,
        column: cursor.column,
        mode: engine.mode(),
        registers: engine.registers(),
    }
}

/// # Returns
///
/// The keys a corpus case's sequence stands for, in which `<Esc>` names the escape key and every
/// other character stands for itself.
///
/// # Panics
///
/// Panics if the sequence names a key this corpus does not hold.
fn keys(sequence: &str) -> Vec<KeyEvent> {
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

/// # Returns
///
/// The layout a case's screen motions are measured in.
///
/// # Panics
///
/// Panics if the case's viewport is zero columns wide or zero rows tall, which the corpus loader
/// rejects.
fn geometry_of(case: &Case) -> Geometry {
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
