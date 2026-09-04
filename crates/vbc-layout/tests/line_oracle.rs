//! Cross-checks the line layout against the screen vim itself drew.
//!
//! The corpus baseline records the text vim drew on every row of every case's viewport, so laying
//! the same buffer out here and drawing it the way vim draws a wrapped buffer says whether this
//! module wraps where vim wraps. Every row is compared, not only the cursor's, so a wrapping
//! difference on any line of any case fails this file.
//!
//! Every case of the corpus is compared. A case vim lays out by a rule this module does not carry,
//! and a case whose recorded screen is not what vim draws for its own buffer, are named below
//! together with the reason, so that a case which starts or stops agreeing fails this file rather
//! than quietly leaving the sample.

use std::collections::BTreeSet;
use std::num::NonZeroUsize;

use vbc_layout::line::{self, DisplayRow, Options};
use vbc_layout::width::{graphemes, AmbiWidth, Metrics};
use vbc_oracle::baseline::{self, Baseline};
use vbc_oracle::corpus::{self, AmbiWidth as CaseAmbiWidth, Case, Corpus};
use vbc_oracle::state::{EditorState, ScreenText};

/// The number of cases whose whole screen this module reproduces, which is the sample the
/// cross-check is worth. A case added to the corpus lands in the sample or in one of the lists
/// below, and either way this number moves.
const SCREENS_ANCHORED: usize = 141;

/// The cases whose window scrolls sideways rather than wrapping, which a viewport decides and a
/// line layout therefore says nothing about.
const NOT_WRAPPED: [&str; 10] = [
    "matrix-tab-w20-ts8-nowrap",
    "matrix-tab-w24-ts8-nowrap",
    "matrix-tab-w40-ts8-nowrap",
    "matrix-w12-nowrap",
    "matrix-w20-nowrap",
    "matrix-w24-nowrap",
    "matrix-w40-nowrap",
    "matrix-w80-nowrap",
    "nowrap-w20-cjk",
    "nowrap-w40-horizontal-scroll",
];

/// The cases whose recorded screen holds something other than the graphemes this module lays out,
/// with the reason.
const SCREEN_DIVERGENCES: [(&str, &str); 12] = [
    ("emoji-skin-tone-modifier", ZERO_WIDTH_JOINER),
    ("emoji-zwj-family-delete-cluster", ZERO_WIDTH_JOINER),
    ("emoji-zwj-family-wrap-edge", ZERO_WIDTH_JOINER),
    (
        "flag-wrap-narrow-viewport",
        "vim draws each regional indicator two columns wide, a flag cluster two together",
    ),
    ("matrix-tab-w24-ts8-linebreak", LINE_BREAK_BESIDE_A_TAB),
    ("matrix-tab-w40-ts8-linebreak", LINE_BREAK_BESIDE_A_TAB),
    ("word-b-zwj-family", ZERO_WIDTH_JOINER),
    ("word-big-b-zwj-family", ZERO_WIDTH_JOINER),
    ("word-big-e-zwj-family", ZERO_WIDTH_JOINER),
    ("word-big-w-zwj-family", ZERO_WIDTH_JOINER),
    ("word-e-zwj-family", ZERO_WIDTH_JOINER),
    ("word-w-zwj-family", ZERO_WIDTH_JOINER),
];

/// The reason vim fits a word onto a word-wrapped row that this module carries to the next one,
/// which is the `'linebreak'` gap `vbc_layout::line`'s own documentation states.
const LINE_BREAK_BESIDE_A_TAB: &str =
    "vim measures a word from the column the break character in front of it starts at, so a tab \
     separating two words does not count against the word behind it";

/// The reason vim draws a joined emoji cluster in cells this module does not account for.
const ZERO_WIDTH_JOINER: &str =
    "vim spells a zero-width joiner as the escape `<200d>`, six columns this module measures as \
     none";

/// The marker vim leaves in the cells of a wrapped row that cannot hold the double-width character
/// coming next.
const WIDE_CHARACTER_MARKER: char = '>';

/// The character vim draws on a screen row that is past the end of the buffer.
const FILLER_ROW: &str = "~";

#[test]
fn wrapped_screens_match_the_screen_vim_drew() {
    let corpus = Corpus::load_dir(&corpus::default_dir()).expect("the corpus loads");
    let baseline = Baseline::read(&baseline::default_path()).expect("the baseline is readable");

    let mut anchored = BTreeSet::new();
    let mut diverged = BTreeSet::new();
    let mut not_wrapped = BTreeSet::new();
    for case in corpus.cases() {
        if !case.options.wrap {
            not_wrapped.insert(case.id.as_str());
            continue;
        }

        let state = state_of(&baseline, case);
        let drawn = screen(case, state);
        if drawn.rows() == state.screen_text.rows() {
            anchored.insert(case.id.as_str());
        } else {
            diverged.insert(case.id.as_str());
        }
    }

    assert_eq!(BTreeSet::from(NOT_WRAPPED), not_wrapped);
    assert_eq!(ids(&SCREEN_DIVERGENCES), diverged);
    assert_eq!(SCREENS_ANCHORED, anchored.len());
}

#[test]
fn every_anchored_case_is_reported_row_by_row() {
    let corpus = Corpus::load_dir(&corpus::default_dir()).expect("the corpus loads");
    let baseline = Baseline::read(&baseline::default_path()).expect("the baseline is readable");
    let skipped: BTreeSet<&str> = BTreeSet::from(NOT_WRAPPED)
        .union(&ids(&SCREEN_DIVERGENCES))
        .copied()
        .collect();

    for case in corpus.cases() {
        if skipped.contains(case.id.as_str()) {
            continue;
        }

        let state = state_of(&baseline, case);
        let drawn = screen(case, state);
        for (row, (expected, laid_out)) in state
            .screen_text
            .rows()
            .iter()
            .zip(drawn.rows().iter())
            .enumerate()
        {
            assert_eq!(
                expected, laid_out,
                "case `{}` draws row {row} differently",
                case.id
            );
        }
        assert_eq!(
            state.screen_text.rows().len(),
            drawn.rows().len(),
            "case `{}` draws a different number of rows",
            case.id
        );
    }
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

/// Lays a case's ending buffer out and draws it the way vim draws a wrapped window: every logical
/// line in turn, then the filler rows vim puts past the end of the buffer.
///
/// # Returns
///
/// The screen text the layout says vim drew.
///
/// # Panics
///
/// Panics if the case's buffer needs more rows than its viewport holds, which a line layout cannot
/// answer for because scrolling is the viewport's to decide.
fn screen(case: &Case, state: &EditorState) -> ScreenText {
    let width = usize::from(case.viewport_width);
    let viewport =
        NonZeroUsize::new(width).expect("a case's viewport width is at least twelve cells");
    let metrics = metrics_of(case);
    let options = options_of(case);

    let mut rows = Vec::new();
    for (line_index, line) in lines_of(&state.buffer).enumerate() {
        let laid_out = line::lay_out(line_index, line, viewport, metrics, &options);
        for (index, row) in laid_out.iter().enumerate() {
            rows.push(draw(row, laid_out.get(index + 1), width, metrics));
        }
    }

    let height = usize::from(case.viewport_height);
    assert!(
        rows.len() <= height,
        "case `{}` fills {} rows of a viewport {height} tall",
        case.id,
        rows.len()
    );
    rows.resize(height, FILLER_ROW.to_owned());

    ScreenText::new(rows)
}

/// Draws one display row into the cells vim fills for it.
///
/// vim leaves the cells a row has left over marked with [`WIDE_CHARACTER_MARKER`] when what comes
/// next is a double-width character that could not fit in them. A row that ends for any other
/// reason -- the line ended, a tab was carried to the next row, a word was kept whole -- leaves
/// them blank, and blanks at the end of a row are not part of what a screen capture reports.
///
/// # Returns
///
/// The text drawn on the row.
fn draw(row: &DisplayRow, next: Option<&DisplayRow>, width: usize, metrics: Metrics) -> String {
    let mut cells = row.cells().to_owned();
    let leftover = width.saturating_sub(row.width());
    let carried = next
        .and_then(|next| graphemes(next.text()).next())
        .filter(|&grapheme| "\t" != grapheme)
        .map_or(0, |grapheme| metrics.grapheme_width(grapheme, row.width()));
    if 0 < leftover && leftover < carried {
        cells.extend(std::iter::repeat_n(WIDE_CHARACTER_MARKER, leftover));
    }

    cells.trim_end_matches(' ').to_owned()
}

/// # Returns
///
/// The logical lines of a buffer's text, with the newline closing the last line excluded.
fn lines_of(buffer: &str) -> impl Iterator<Item = &str> {
    buffer.strip_suffix('\n').unwrap_or(buffer).split('\n')
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
/// The wrapping options `case` is laid out under.
fn options_of(case: &Case) -> Options {
    Options::new()
        .with_break_indent(case.options.breakindent)
        .with_show_break(case.options.showbreak.clone())
        .with_line_break(case.options.linebreak)
}
