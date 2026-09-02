//! Cross-checks the anchor-relative mapping against the cell vim itself drew the cursor in.
//!
//! The corpus baseline records the screen row and column vim left the cursor at for every case, so
//! mapping the same case's cursor from an anchor at the top of the window says whether this crate
//! puts a position where vim puts it. The corpus is where the awkward text lives -- CJK, joined
//! emoji, flags, combining marks, tabs -- and every case is mapped, wrapped rows and the cursor
//! `A` leaves past the end of a line included.
//!
//! A case vim draws by a rule this crate does not carry is named below together with the reason,
//! so that a case which starts or stops agreeing fails this file rather than quietly leaving the
//! sample.

use std::collections::BTreeSet;
use std::num::NonZeroUsize;

use vbc_layout::anchor::{visual_offset_from_anchor, VisualOffset, Wrapping};
use vbc_layout::invariants::LogicalPosition;
use vbc_layout::line::Options;
use vbc_layout::width::{grapheme_indices, graphemes, AmbiWidth, Metrics};
use vbc_oracle::baseline::{self, Baseline};
use vbc_oracle::corpus::{self, AmbiWidth as CaseAmbiWidth, Case, Corpus};
use vbc_oracle::state::EditorState;

/// The number of cases whose cursor cell this mapping reproduces, which is the sample the
/// cross-check is worth. A case added to the corpus lands in the sample or in one of the lists
/// below, and either way this number moves.
const CURSORS_ANCHORED: usize = 55;

/// The cases whose window scrolls sideways rather than wrapping, which an anchor at the top left
/// of the window does not describe.
const NOT_WRAPPED: [&str; 2] = ["nowrap-w20-cjk", "nowrap-w40-horizontal-scroll"];

/// The cases whose cursor vim leaves inside a grapheme cluster, which is not a position this
/// crate's coordinates can name.
const CURSOR_INSIDE_A_CLUSTER: [&str; 6] = [
    "emoji-zwj-family-delete-cluster",
    "flag-odd-regional-indicator-run",
    "word-b-zwj-family",
    "word-big-e-zwj-family",
    "word-e-zwj-family",
    "word-w-zwj-family",
];

/// The cases whose cursor vim draws in another cell than the one this mapping puts it in, with the
/// reason.
const CURSOR_DIVERGENCES: [(&str, &str); 4] = [
    (
        "combining-hangul-jamo-decomposed",
        "vim draws a decomposed jamo syllable four columns wide, a cluster two",
    ),
    ("flag-wrap-narrow-viewport", REGIONAL_INDICATOR),
    (
        "tab-leading-indent-ts8",
        "vim draws the cursor on a tab's last cell rather than at the column the tab starts at",
    ),
    ("word-big-w-zwj-family", ZERO_WIDTH_JOINER),
];

/// The number of cases whose every position this mapping draws where vim drew it.
const SCREENS_ANCHORED: usize = 54;

/// The cases whose recorded screen holds something other than the graphemes this crate lays out,
/// with the reason.
const SCREEN_DIVERGENCES: [(&str, &str); 11] = [
    (
        "cjk-mixed-latin-delete-char",
        "the recorded screen repeats every double-width character that starts at an odd column, \
         which is what vim's own capture reports after the deletion redrew part of the row",
    ),
    ("emoji-skin-tone-modifier", ZERO_WIDTH_JOINER),
    ("emoji-zwj-family-delete-cluster", ZERO_WIDTH_JOINER),
    ("emoji-zwj-family-wrap-edge", ZERO_WIDTH_JOINER),
    ("flag-wrap-narrow-viewport", REGIONAL_INDICATOR),
    ("word-b-zwj-family", ZERO_WIDTH_JOINER),
    ("word-big-b-zwj-family", ZERO_WIDTH_JOINER),
    ("word-big-e-zwj-family", ZERO_WIDTH_JOINER),
    ("word-big-w-zwj-family", ZERO_WIDTH_JOINER),
    ("word-e-zwj-family", ZERO_WIDTH_JOINER),
    ("word-w-zwj-family", ZERO_WIDTH_JOINER),
];

/// The reason vim draws a joined emoji cluster in cells this crate does not account for.
const ZERO_WIDTH_JOINER: &str =
    "vim spells a zero-width joiner as the escape `<200d>`, six columns this crate measures as \
     none";

/// The reason vim draws a flag in cells this crate does not account for.
const REGIONAL_INDICATOR: &str =
    "vim draws each regional indicator two columns wide, a flag cluster two together";

#[test]
fn cursor_cells_match_the_cell_vim_drew() {
    let corpus = Corpus::load_dir(&corpus::default_dir()).expect("the corpus loads");
    let baseline = Baseline::read(&baseline::default_path()).expect("the baseline is readable");

    let mut anchored = BTreeSet::new();
    let mut diverged = BTreeSet::new();
    let mut not_wrapped = BTreeSet::new();
    let mut inside_a_cluster = BTreeSet::new();
    for case in corpus.cases() {
        if !case.options.wrap {
            not_wrapped.insert(case.id.as_str());
            continue;
        }

        let state = state_of(&baseline, case);
        let Some(drawn) = drawn_at(case, state) else {
            inside_a_cluster.insert(case.id.as_str());
            continue;
        };
        if drawn == recorded(state) {
            anchored.insert(case.id.as_str());
        } else {
            diverged.insert(case.id.as_str());
        }
    }

    assert_eq!(BTreeSet::from(NOT_WRAPPED), not_wrapped);
    assert_eq!(BTreeSet::from(CURSOR_INSIDE_A_CLUSTER), inside_a_cluster);
    assert_eq!(ids(&CURSOR_DIVERGENCES), diverged);
    assert_eq!(CURSORS_ANCHORED, anchored.len());
}

#[test]
fn every_grapheme_is_drawn_in_the_cell_vim_drew_it_in() {
    let corpus = Corpus::load_dir(&corpus::default_dir()).expect("the corpus loads");
    let baseline = Baseline::read(&baseline::default_path()).expect("the baseline is readable");

    let mut anchored = BTreeSet::new();
    let mut diverged = BTreeSet::new();
    for case in corpus.cases() {
        if !case.options.wrap {
            continue;
        }

        let state = state_of(&baseline, case);
        let wrong_cells = misdrawn(case, state);
        if wrong_cells.is_empty() {
            anchored.insert(case.id.as_str());
            continue;
        }

        assert!(
            ids(&SCREEN_DIVERGENCES).contains(case.id.as_str()),
            "case `{}` draws:\n{}",
            case.id,
            wrong_cells.join("\n")
        );
        diverged.insert(case.id.as_str());
    }

    assert_eq!(ids(&SCREEN_DIVERGENCES), diverged);
    assert_eq!(SCREENS_ANCHORED, anchored.len());
}

/// Maps every position of a case's buffer and looks up the cell vim drew it in.
///
/// # Returns
///
/// Every grapheme the mapping puts somewhere other than where vim drew it, each with the cell the
/// mapping chose and what vim drew there.
///
/// # Panics
///
/// Panics if a position of the buffer is drawn outside the window.
fn misdrawn(case: &Case, state: &EditorState) -> Vec<String> {
    let lines: Vec<String> = lines_of(&state.buffer).map(str::to_owned).collect();
    let metrics = wrapping_of(case).metrics();
    let anchor = LogicalPosition {
        line: 0,
        grapheme: 0,
    };

    let mut misdrawn = Vec::new();
    for (line, text) in lines.iter().enumerate() {
        for (grapheme, drawn) in graphemes(text).enumerate() {
            let position = LogicalPosition { line, grapheme };
            let offset = visual_offset_from_anchor(
                &lines,
                anchor,
                position,
                &wrapping_of(case),
                usize::from(case.viewport_height),
            )
            .unwrap_or_else(|error| {
                panic!("`{}` draws {position} in the window: {error}", case.id)
            });
            let row = u64::try_from(offset.rows).expect("a screen row is not above the window");
            let cell = state
                .screen_text
                .row(row)
                .and_then(|text| cell_at(text, offset.column, metrics));
            let expected = if "\t" == drawn { " " } else { drawn };
            if cell.unwrap_or(" ") != expected {
                misdrawn.push(format!(
                    "{position} is drawn at {offset}, where vim drew {cell:?} rather than \
                     {expected:?}"
                ));
            }
        }
    }

    misdrawn
}

/// # Returns
///
/// The grapheme a screen row draws starting at `column`, or `None` where the row draws nothing
/// there because it ends before the column or because another grapheme covers it.
fn cell_at(row: &str, column: usize, metrics: Metrics) -> Option<&str> {
    let mut drawn = 0;
    for grapheme in graphemes(row) {
        if drawn == column {
            return Some(grapheme);
        }
        drawn += metrics.grapheme_width(grapheme, drawn);
    }

    None
}

/// # Returns
///
/// The identifiers of the cases `divergences` names.
fn ids<'divergences>(divergences: &[(&'divergences str, &str)]) -> BTreeSet<&'divergences str> {
    divergences.iter().map(|&(id, _)| id).collect()
}

/// # Returns
///
/// The state the baseline records vim ending `case` in.
fn state_of<'baseline>(baseline: &'baseline Baseline, case: &Case) -> &'baseline EditorState {
    baseline
        .cases
        .get(&case.id)
        .unwrap_or_else(|| panic!("the baseline holds the case `{}`", case.id))
}

/// # Returns
///
/// The cell vim recorded its cursor in, counted from the top left of the window.
fn recorded(state: &EditorState) -> (isize, usize) {
    (
        isize::try_from(state.display_position.row).expect("a screen row fits in an `isize`"),
        usize::try_from(state.display_position.column).expect("a screen column fits in a `usize`"),
    )
}

/// Maps a case's cursor from an anchor at the top left of its window, which is where the window
/// starts for a case whose buffer fits in it.
///
/// # Returns
///
/// * The cell the mapping draws the cursor in, counted from the top left of the window.
/// * `None` if vim left the cursor inside a grapheme cluster.
///
/// # Panics
///
/// Panics if the cursor is drawn outside the window.
fn drawn_at(case: &Case, state: &EditorState) -> Option<(isize, usize)> {
    let lines: Vec<String> = lines_of(&state.buffer).map(str::to_owned).collect();
    let line = usize::try_from(state.cursor.line).expect("a cursor line fits in a `usize`");
    let offset = usize::try_from(state.cursor.column).expect("a cursor column fits in a `usize`");
    let cursor = LogicalPosition {
        line,
        grapheme: grapheme_offset(&lines[line], offset)?,
    };
    let anchor = LogicalPosition {
        line: 0,
        grapheme: 0,
    };
    let VisualOffset { rows, column } = visual_offset_from_anchor(
        &lines,
        anchor,
        cursor,
        &wrapping_of(case),
        usize::from(case.viewport_height),
    )
    .unwrap_or_else(|error| {
        panic!(
            "the cursor of `{}` is drawn in the window: {error}",
            case.id
        )
    });

    Some((rows, column))
}

/// # Returns
///
/// The logical lines of a buffer's text, with the newline closing the last line excluded.
fn lines_of(buffer: &str) -> impl Iterator<Item = &str> {
    buffer.strip_suffix('\n').unwrap_or(buffer).split('\n')
}

/// # Returns
///
/// The number of graphemes of `line` before the byte at `offset`, or `None` if the offset does not
/// start a grapheme.
fn grapheme_offset(line: &str, offset: usize) -> Option<usize> {
    if line.len() == offset {
        return Some(grapheme_indices(line).count());
    }

    grapheme_indices(line).position(|(start, _)| start == offset)
}

/// # Returns
///
/// The way `case` draws its buffer.
fn wrapping_of(case: &Case) -> Wrapping {
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
    let width = NonZeroUsize::new(usize::from(case.viewport_width))
        .expect("a case's viewport width is at least twelve cells");

    Wrapping::new(width, Metrics::new(ambiwidth, tab_stop), options)
}
