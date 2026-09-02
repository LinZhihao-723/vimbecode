//! Cross-checks the drawn screen against the rows it was drawn from and against the screen vim
//! itself drew.
//!
//! The layout is already held to vim's wrapping in `vbc-layout/tests/line_oracle.rs`; what is
//! checked here is that drawing those rows into a terminal buffer keeps them. Every case of the
//! corpus is laid out and drawn into a `TestBackend` the size of the case's viewport, and the
//! cells are then read two ways: against the columns the layout put each grapheme at, which holds
//! for every case, and against the screen the baseline records vim drawing for the same buffer,
//! which holds for the cases vim and the layout already agree on.
//!
//! The cases vim lays out by a rule the layout does not carry are the ones `line_oracle` names,
//! and they are named again here so that a case which starts or stops agreeing fails this file
//! rather than quietly leaving the sample. The two lists say the same thing about vim and are kept
//! in step by the count below, which is the same count `line_oracle` asserts.

mod screen;

use std::collections::BTreeSet;
use std::num::NonZeroUsize;

use ratatui::backend::TestBackend;
use ratatui::buffer::{Buffer, CellWidth};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::Terminal;
use vbc_editor::render::Renderer;
use vbc_layout::line::{self, DisplayRow, Options};
use vbc_layout::width::{graphemes, AmbiWidth, Metrics};
use vbc_oracle::baseline::{self, Baseline};
use vbc_oracle::corpus::{self, AmbiWidth as CaseAmbiWidth, Case, Corpus};
use vbc_oracle::state::EditorState;

use crate::screen::{broken_claims, BLANK};

/// The number of cases whose whole screen the renderer reproduces, which is the sample the
/// cross-check against vim is worth and is the count `line_oracle` asserts for the layout
/// underneath it.
const SCREENS_ANCHORED: usize = 62;

/// The cases whose window scrolls sideways rather than wrapping, which a viewport decides and a
/// row renderer therefore says nothing about.
const NOT_WRAPPED: [&str; 2] = ["nowrap-w20-cjk", "nowrap-w40-horizontal-scroll"];

/// The cases whose recorded screen holds something other than the graphemes the layout hands the
/// renderer, named in `line_oracle` together with the reason.
const SCREEN_DIVERGENCES: [&str; 11] = [
    "cjk-mixed-latin-delete-char",
    "emoji-skin-tone-modifier",
    "emoji-zwj-family-delete-cluster",
    "emoji-zwj-family-wrap-edge",
    "flag-wrap-narrow-viewport",
    "word-b-zwj-family",
    "word-big-b-zwj-family",
    "word-big-e-zwj-family",
    "word-big-w-zwj-family",
    "word-e-zwj-family",
    "word-w-zwj-family",
];

/// The character vim draws on a screen row that is past the end of the buffer.
const FILLER_ROW: &str = "~";

#[test]
fn every_drawn_cell_holds_the_grapheme_the_layout_put_there() {
    let corpus = Corpus::load_dir(&corpus::default_dir()).expect("the corpus loads");
    let baseline = Baseline::read(&baseline::default_path()).expect("the baseline is readable");

    let mut checked = 0;
    for case in wrapped(&corpus) {
        let (metrics, lines) = lay_out(case, state_of(&baseline, case));
        let buffer = frame(case, metrics, &lines);
        let mut y = 0;
        for row in lines.iter().flatten() {
            let columns = row.columns();
            for (index, grapheme) in graphemes(row.text()).enumerate() {
                let column = columns[index];
                let width = columns[index + 1] - column;
                if 0 == width || usize::from(case.viewport_width) < column + width {
                    continue;
                }

                let x = u16::try_from(column).expect("a drawn column fits in a `u16`");
                let claimed = u16::try_from(width).expect("a drawn width fits in a `u16`");
                let (drawn, claimed) = if "\t" == grapheme {
                    (BLANK, 1)
                } else {
                    (grapheme, claimed)
                };
                assert_eq!(
                    drawn,
                    buffer[(x, y)].symbol(),
                    "case `{}` draws row {y} column {column} from a grapheme of its own",
                    case.id
                );
                assert_eq!(
                    claimed,
                    buffer[(x, y)].cell_width(),
                    "case `{}` claims cells at row {y} column {column} the layout did not give it",
                    case.id
                );
                checked += 1;
            }
            y += 1;
        }
    }

    assert!(0 < checked, "the corpus drew no cells to check");
}

#[test]
fn drawn_screens_match_the_screen_vim_drew() {
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
        let (metrics, lines) = lay_out(case, state);
        let terminal = draw(case, metrics, &lines);
        if rows_of(terminal.backend().buffer()) == state.screen_text.rows() {
            anchored.insert(case.id.as_str());
        } else {
            diverged.insert(case.id.as_str());
        }
    }

    assert_eq!(BTreeSet::from(NOT_WRAPPED), not_wrapped);
    assert_eq!(BTreeSet::from(SCREEN_DIVERGENCES), diverged);
    assert_eq!(SCREENS_ANCHORED, anchored.len());
}

#[test]
fn every_anchored_case_is_drawn_row_by_row() {
    let corpus = Corpus::load_dir(&corpus::default_dir()).expect("the corpus loads");
    let baseline = Baseline::read(&baseline::default_path()).expect("the baseline is readable");
    let skipped = skipped();

    for case in wrapped(&corpus) {
        if skipped.contains(case.id.as_str()) {
            continue;
        }

        let state = state_of(&baseline, case);
        let (metrics, lines) = lay_out(case, state);
        let terminal = draw(case, metrics, &lines);
        let drawn = rows_of(terminal.backend().buffer());
        for (row, (expected, drawn)) in state
            .screen_text
            .rows()
            .iter()
            .zip(drawn.iter())
            .enumerate()
        {
            assert_eq!(
                expected, drawn,
                "case `{}` draws row {row} differently",
                case.id
            );
        }
        assert_eq!(
            state.screen_text.rows().len(),
            drawn.len(),
            "case `{}` draws a different number of rows",
            case.id
        );
    }
}

#[test]
fn no_case_draws_a_grapheme_across_the_right_edge_of_its_viewport() {
    let corpus = Corpus::load_dir(&corpus::default_dir()).expect("the corpus loads");
    let baseline = Baseline::read(&baseline::default_path()).expect("the baseline is readable");

    let mut checked = 0;
    for case in wrapped(&corpus) {
        let (metrics, lines) = lay_out(case, state_of(&baseline, case));
        checked += 1;
        assert_eq!(
            Vec::<String>::new(),
            broken_claims(&frame(case, metrics, &lines)),
            "case `{}` draws a grapheme into cells another one claimed",
            case.id
        );
    }

    assert_eq!(corpus.cases().len() - NOT_WRAPPED.len(), checked);
}

/// # Returns
///
/// The cases of `corpus` whose window wraps, which are the ones a row renderer draws whole.
fn wrapped(corpus: &Corpus) -> impl Iterator<Item = &Case> {
    corpus.cases().iter().filter(|case| case.options.wrap)
}

/// # Returns
///
/// The identifiers of the cases this file does not compare against vim.
fn skipped() -> BTreeSet<&'static str> {
    BTreeSet::from(NOT_WRAPPED)
        .union(&BTreeSet::from(SCREEN_DIVERGENCES))
        .copied()
        .collect()
}

/// # Returns
///
/// The state the baseline records vim ending `case` in.
///
/// # Panics
///
/// Panics if the baseline holds no such case.
fn state_of<'baseline>(baseline: &'baseline Baseline, case: &Case) -> &'baseline EditorState {
    baseline
        .cases
        .get(&case.id)
        .unwrap_or_else(|| panic!("the baseline holds the case `{}`", case.id))
}

/// Lays a case's ending buffer out under the display options the case declares.
///
/// # Returns
///
/// * The metrics the case is measured under.
/// * The rows rendering each of the buffer's logical lines, top to bottom.
///
/// # Panics
///
/// Panics if the case's viewport is zero columns wide, which the corpus loader rejects.
fn lay_out(case: &Case, state: &EditorState) -> (Metrics, Vec<Vec<DisplayRow>>) {
    let width =
        NonZeroUsize::new(usize::from(case.viewport_width)).expect("a viewport is not zero wide");
    let metrics = metrics_of(case);
    let options = options_of(case);
    let lines = lines_of(&state.buffer)
        .enumerate()
        .map(|(line, text)| line::lay_out(line, text, width, metrics, &options))
        .collect();

    (metrics, lines)
}

/// Draws a case's rows into a terminal, which is the screen a terminal would be sent.
///
/// # Returns
///
/// The terminal the case's screen was drawn into.
///
/// # Panics
///
/// Panics if the terminal cannot be built or drawn to.
fn draw(case: &Case, metrics: Metrics, lines: &[Vec<DisplayRow>]) -> Terminal<TestBackend> {
    let mut terminal = Terminal::new(TestBackend::new(case.viewport_width, case.viewport_height))
        .expect("a case's terminal is built");
    terminal
        .draw(|drawn| {
            let area = drawn.area();
            paint(case, metrics, lines, drawn.buffer_mut(), area);
        })
        .expect("a case's screen is drawn");

    terminal
}

/// Draws a case's rows into a buffer of its own, which is the buffer the renderer filled rather
/// than the one a terminal diff has already been through.
///
/// # Returns
///
/// The buffer the case's screen was drawn into.
fn frame(case: &Case, metrics: Metrics, lines: &[Vec<DisplayRow>]) -> Buffer {
    let area = Rect::new(0, 0, case.viewport_width, case.viewport_height);
    let mut buffer = Buffer::empty(area);
    paint(case, metrics, lines, &mut buffer, area);

    buffer
}

/// Draws a case's rows the way vim draws a wrapped window: every logical line in turn, then the
/// filler rows vim puts past the end of the buffer.
///
/// # Panics
///
/// Panics if the case's buffer needs more rows than its viewport holds, which scrolling would
/// decide and a row renderer cannot answer for.
fn paint(
    case: &Case,
    metrics: Metrics,
    lines: &[Vec<DisplayRow>],
    buffer: &mut Buffer,
    area: Rect,
) {
    let height = case.viewport_height;
    let drawn: usize = lines.iter().map(Vec::len).sum();
    assert!(
        drawn <= usize::from(height),
        "case `{}` fills {drawn} rows of a viewport {height} tall",
        case.id
    );

    let renderer = Renderer::new(metrics);
    let mut top = 0;
    for rows in lines {
        top += renderer.draw_line(buffer, area, top, rows);
    }
    for row in top..height {
        buffer.set_string(area.x, area.y + row, FILLER_ROW, Style::new());
    }
}

/// Reads a drawn screen back the way a terminal shows it: a cell a wider grapheme beside it has
/// claimed is passed over, and the blanks a row ends in are not part of what a screen capture
/// reports.
///
/// # Returns
///
/// The text drawn on each row of `buffer`, top to bottom.
///
/// # Panics
///
/// Panics if a column of the buffer does not fit in a `u16`, which no viewport is wide enough for.
fn rows_of(buffer: &Buffer) -> Vec<String> {
    let width = usize::from(buffer.area.width);
    (0..buffer.area.height)
        .map(|y| {
            let mut text = String::new();
            let mut column = 0;
            while column < width {
                let x = u16::try_from(column).expect("a column of a viewport fits in a `u16`");
                text.push_str(buffer[(x, y)].symbol());
                column += usize::from(buffer[(x, y)].cell_width().max(1));
            }

            text.trim_end_matches(' ').to_owned()
        })
        .collect()
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
///
/// # Panics
///
/// Panics if the case's tab stop is zero, which the corpus loader rejects.
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
