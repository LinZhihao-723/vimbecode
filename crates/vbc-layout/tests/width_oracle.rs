//! Cross-checks the width primitives against the columns vim itself drew.
//!
//! The corpus baseline records where vim put the cursor and what vim drew on every screen row of
//! every case, so it says how wide vim measured that case's text to be. Measuring the same text
//! here and comparing anchors these primitives to the oracle rather than only to the crates they
//! are built on.
//!
//! Every case of the corpus is measured. A case vim draws by a rule of its own rather than by
//! grapheme cluster, and a case whose recorded position says nothing about a width, are named
//! below together with the reason, so that a case which starts or stops agreeing fails this file
//! rather than quietly leaving the sample.

use std::collections::BTreeSet;
use std::num::NonZeroUsize;

use vbc_layout::width::{graphemes, AmbiWidth, Metrics};
use vbc_oracle::baseline::{self, Baseline};
use vbc_oracle::corpus::{self, AmbiWidth as CaseAmbiWidth, Case, Corpus};
use vbc_oracle::state::EditorState;

/// The number of cases whose cursor column the widths here account for, which is the sample the
/// cross-check is worth. A case added to the corpus lands in the sample or in one of the lists
/// below, and either way this number moves.
const CURSOR_COLUMNS_ANCHORED: usize = 53;

/// The number of cases whose first screen row the widths here account for, which is the sample
/// that cross-check is worth.
const FIRST_ROWS_ANCHORED: usize = 61;

/// The cases whose cursor vim draws at another column than the width of the text before it, with
/// the reason.
const CURSOR_COLUMN_DIVERGENCES: [(&str, &str); 7] = [
    (
        "combining-hangul-jamo-decomposed",
        "vim draws a decomposed jamo syllable four columns wide, a cluster two",
    ),
    (
        "flag-odd-regional-indicator-run",
        "vim draws each regional indicator two columns wide, a flag cluster two together",
    ),
    (
        "flag-wrap-narrow-viewport",
        "vim draws each regional indicator two columns wide, a flag cluster two together",
    ),
    (
        "word-b-zwj-family",
        "vim spells a zero-width joiner as the escape `<200d>`",
    ),
    (
        "word-big-e-zwj-family",
        "vim spells a zero-width joiner as the escape `<200d>`",
    ),
    (
        "word-big-w-zwj-family",
        "vim spells a zero-width joiner as the escape `<200d>`",
    ),
    (
        "word-w-zwj-family",
        "vim spells a zero-width joiner as the escape `<200d>`",
    ),
];

/// The cases whose cursor rests on a tab, which vim draws on the tab's last cell rather than at
/// the column the tab starts at.
const CURSOR_ON_A_TAB: [&str; 1] = ["tab-leading-indent-ts8"];

/// The cases whose cursor is drawn past the first screen row of its line, where the column the
/// cursor is drawn in is the wrapping's to decide rather than the width of the line's text.
///
/// A case that does not wrap scrolls its window sideways instead, which puts its cursor here for
/// the same reason.
const CURSOR_PAST_THE_FIRST_ROW: [&str; 14] = [
    "cjk-wide-cell-straddles-edge",
    "nowrap-w20-cjk",
    "nowrap-w40-horizontal-scroll",
    "tab-wrapped-with-breakindent",
    "wrap-w20-boundary-exact",
    "wrap-w20-plain",
    "wrap-w24-breakindent",
    "wrap-w24-breakindent-showbreak",
    "wrap-w24-plain",
    "wrap-w24-showbreak",
    "wrap-w40-breakindent-showbreak",
    "wrap-w40-plain",
    "wrap-w80-plain",
    "wrap-w80-showbreak",
];

/// The cases whose first screen row vim fills with something other than the graphemes that fit in
/// it, with the reason.
const FIRST_ROW_DIVERGENCES: [(&str, &str); 12] = [
    (
        "cjk-mixed-latin-delete-char",
        "the recorded row repeats every double-width character that starts at an odd column",
    ),
    (
        "cjk-wide-cell-straddles-edge",
        "vim marks a row whose last cell cannot hold the next double-width character with a `>`",
    ),
    (
        "emoji-skin-tone-modifier",
        "vim spells a zero-width joiner as the escape `<200d>`",
    ),
    (
        "emoji-zwj-family-delete-cluster",
        "vim spells a zero-width joiner as the escape `<200d>`",
    ),
    (
        "emoji-zwj-family-wrap-edge",
        "vim spells a zero-width joiner as the escape `<200d>`",
    ),
    (
        "flag-wrap-narrow-viewport",
        "vim draws each regional indicator two columns wide, a flag cluster two together",
    ),
    (
        "word-b-zwj-family",
        "vim spells a zero-width joiner as the escape `<200d>`",
    ),
    (
        "word-big-b-zwj-family",
        "vim spells a zero-width joiner as the escape `<200d>`",
    ),
    (
        "word-big-e-zwj-family",
        "vim spells a zero-width joiner as the escape `<200d>`",
    ),
    (
        "word-big-w-zwj-family",
        "vim spells a zero-width joiner as the escape `<200d>`",
    ),
    (
        "word-e-zwj-family",
        "vim spells a zero-width joiner as the escape `<200d>`",
    ),
    (
        "word-w-zwj-family",
        "vim spells a zero-width joiner as the escape `<200d>`",
    ),
];

/// The cases whose first screen row does not start at the start of the line, since a window that
/// does not wrap scrolls sideways to keep the cursor in view.
const FIRST_ROW_SCROLLED_SIDEWAYS: [&str; 2] = ["nowrap-w20-cjk", "nowrap-w40-horizontal-scroll"];

#[test]
fn cursor_columns_match_the_columns_vim_drew() {
    let corpus = Corpus::load_dir(&corpus::default_dir()).expect("the corpus loads");
    let baseline = Baseline::read(&baseline::default_path()).expect("the baseline is readable");

    let mut anchored = BTreeSet::new();
    let mut diverged = BTreeSet::new();
    let mut on_a_tab = BTreeSet::new();
    let mut past_the_first_row = BTreeSet::new();
    for case in corpus.cases() {
        let state = state_of(&baseline, case);
        let metrics = metrics_of(case);
        let line = cursor_line(state);
        let offset = usize::try_from(state.cursor.column).expect("a cursor column fits in a usize");
        let prefix = line
            .get(..offset)
            .unwrap_or_else(|| panic!("the cursor of `{}` rests on a boundary", case.id));
        let width = metrics.text_width(prefix, 0);
        let cursor_grapheme = graphemes(&line[offset..]).next().unwrap_or("");

        if "\t" == cursor_grapheme {
            on_a_tab.insert(case.id.as_str());
            continue;
        }
        let cursor_width = metrics.grapheme_width(cursor_grapheme, width);
        if usize::from(case.viewport_width) < width + cursor_width {
            past_the_first_row.insert(case.id.as_str());
            continue;
        }

        let drawn =
            usize::try_from(state.display_position.column).expect("a column fits in a usize");
        if width == drawn {
            anchored.insert(case.id.as_str());
        } else {
            diverged.insert(case.id.as_str());
        }
    }

    assert_eq!(BTreeSet::from(CURSOR_ON_A_TAB), on_a_tab);
    assert_eq!(
        BTreeSet::from(CURSOR_PAST_THE_FIRST_ROW),
        past_the_first_row
    );
    assert_eq!(ids(&CURSOR_COLUMN_DIVERGENCES), diverged);
    assert_eq!(CURSOR_COLUMNS_ANCHORED, anchored.len());
}

#[test]
fn first_screen_rows_match_the_row_vim_drew() {
    let corpus = Corpus::load_dir(&corpus::default_dir()).expect("the corpus loads");
    let baseline = Baseline::read(&baseline::default_path()).expect("the baseline is readable");

    let mut anchored = BTreeSet::new();
    let mut diverged = BTreeSet::new();
    let mut scrolled_sideways = BTreeSet::new();
    for case in corpus.cases() {
        let state = state_of(&baseline, case);
        if !case.options.wrap {
            scrolled_sideways.insert(case.id.as_str());
            continue;
        }

        let first_line = state.buffer.split('\n').next().unwrap_or("");
        let predicted = first_row(
            &metrics_of(case),
            first_line,
            usize::from(case.viewport_width),
        );
        if predicted == state.screen_text.row(0).unwrap_or("") {
            anchored.insert(case.id.as_str());
        } else {
            diverged.insert(case.id.as_str());
        }
    }

    assert_eq!(
        BTreeSet::from(FIRST_ROW_SCROLLED_SIDEWAYS),
        scrolled_sideways
    );
    assert_eq!(ids(&FIRST_ROW_DIVERGENCES), diverged);
    assert_eq!(FIRST_ROWS_ANCHORED, anchored.len());
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
/// The metrics `case` is laid out under.
fn metrics_of(case: &Case) -> Metrics {
    let ambiwidth = match case.options.ambiwidth {
        CaseAmbiWidth::Single => AmbiWidth::Single,
        CaseAmbiWidth::Double => AmbiWidth::Double,
    };
    let tab_stop =
        NonZeroUsize::new(usize::from(case.options.tabstop)).expect("a case's tabstop is not zero");

    Metrics::new(ambiwidth, tab_stop)
}

/// # Returns
///
/// The line the cursor of `state` rests on.
fn cursor_line(state: &EditorState) -> &str {
    let line = usize::try_from(state.cursor.line).expect("a cursor line fits in a usize");

    state
        .buffer
        .split('\n')
        .nth(line)
        .expect("the cursor rests on a line the buffer holds")
}

/// Draws the first screen row of a line the way the widths say a wrapping editor draws it: the
/// graphemes that fit in the viewport, with every tab spelled as the blanks it advances by.
///
/// # Returns
///
/// The text the first screen row shows, with the trailing blanks a screen capture drops.
fn first_row(metrics: &Metrics, line: &str, viewport_width: usize) -> String {
    let mut row = String::new();
    let mut column = 0;
    for grapheme in graphemes(line) {
        let width = metrics.grapheme_width(grapheme, column);
        if viewport_width < column + width {
            break;
        }
        if "\t" == grapheme {
            for _ in 0..width {
                row.push(' ');
            }
        } else {
            row.push_str(grapheme);
        }
        column += width;
    }

    row.trim_end_matches(' ').to_owned()
}
