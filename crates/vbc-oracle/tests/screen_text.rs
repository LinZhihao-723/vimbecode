//! The text vim draws in a case's viewport, captured and compared row by row.
//!
//! The other dimensions describe the cursor and the buffer behind it, so an engine that lays every
//! line out wrongly and still lands the cursor in the right cell agrees with vim in all of them.
//! Every test here pins what vim draws on screen rows that no other dimension mentions.

use std::collections::BTreeSet;

use vbc_oracle::baseline::{self, Baseline};
use vbc_oracle::corpus::{self, Case, Corpus, Options, Tag};
use vbc_oracle::runner::Dimension;
use vbc_oracle::state::{EditorState, Mode, ScreenText};
use vbc_oracle::vim::VimDriver;

/// The buffer of the case the oracle used to miss: a short first line the cursor sits on, and an
/// indented second line that three layouts draw three different ways.
const LAYOUT_BUFFER: &str = "alpha\n    beta gamma delta epsilon zeta\n";

/// The keys that park the cursor on the first line, leaving the second line to the layout alone.
const PARK_KEYS: &str = "gg";

/// The width the three layouts of [`LAYOUT_BUFFER`] are drawn in.
const LAYOUT_WIDTH: u16 = 24;

/// A viewport wide enough that its cells run into the thousands, which is more of them than vim
/// reads before it looks at its input again.
const WIDE_VIEWPORT_WIDTH: u16 = 400;

/// The screen row of [`LAYOUT_BUFFER`] the three layouts disagree on, which holds no part of the
/// cursor's own line.
const LAYOUT_ROW: u64 = 2;

/// A line of fifteen double-width characters, which fill twice their number of cells.
const CJK_BUFFER: &str = "中文测试行一二三四五六七八九十\nascii\n";

/// A line whose accents are combining marks rather than precomposed characters, and a heart
/// carrying a variation selector: cells vim draws as a character and the marks on top of it.
const COMBINING_BUFFER: &str = "cafe\u{301} nai\u{308}ve \u{2764}\u{fe0f}\n";

/// A line holding a joined emoji, a flag, and the letters around them.
const JOINED_EMOJI_BUFFER: &str = "a\u{1f469}\u{200d}\u{1f4bb}b\u{1f1ef}\u{1f1f5}c\n";

/// The same line with the joiner taken out, which vim draws in different cells.
const UNJOINED_EMOJI_BUFFER: &str = "a\u{1f469}\u{1f4bb}b\u{1f1ef}\u{1f1f5}c\n";

/// How many of the corpus's cases must draw a screen that the buffer behind them does not
/// describe, which is the part of the corpus the screen text is a dimension for.
const INFORMATIVE_CASES: usize = 41;

/// The keys that draw the whole screen again from an empty one, without moving the cursor or
/// leaving the mode the case ended in. A screen drawn this way holds nothing an earlier draw of
/// the same case left behind.
const REPAINT_KEYS: &str = "<Cmd>redraw!<CR>";

/// A line whose double-width characters sit one cell to the right of where deleting its first
/// character leaves them, so that each of them ends up over a cell the first draw wrote to.
const SHIFTED_WIDE_BUFFER: &str = "ax中文y\n";

/// The line [`SHIFTED_WIDE_BUFFER`] is drawn as once its first character is deleted, in which the
/// double-width characters start at the odd column 1 and the even column 3.
const SHIFTED_WIDE_ROW: &str = "x中文y";

/// The keys that delete the first character of [`SHIFTED_WIDE_BUFFER`] and put the cursor on the
/// first double-width character that the deletion moved.
const SHIFT_ONTO_AN_ODD_COLUMN_KEYS: &str = "xl";

/// The column the cursor of [`SHIFT_ONTO_AN_ODD_COLUMN_KEYS`] is drawn in, which is odd.
const ODD_COLUMN: u64 = 1;

/// A line whose double-width characters are drawn where they are however its last character is
/// edited, which is the even-column path that never doubled a character.
const UNSHIFTED_WIDE_BUFFER: &str = "中文xy\n";

/// The line [`UNSHIFTED_WIDE_BUFFER`] is drawn as once its last character is deleted.
const UNSHIFTED_WIDE_ROW: &str = "中文x";

/// The keys that delete the last character of [`UNSHIFTED_WIDE_BUFFER`] and put the cursor on the
/// second double-width character.
const DELETE_LAST_KEYS: &str = "$xh";

/// The column the cursor of [`DELETE_LAST_KEYS`] is drawn in, which is even.
const EVEN_COLUMN: u64 = 2;

/// The corpus case the screen capture used to record with its double-width characters doubled.
const CJK_MIXED_LATIN_CASE: &str = "cjk-mixed-latin-delete-char";

/// The line [`CJK_MIXED_LATIN_CASE`] draws, in which every character is drawn once.
const CJK_MIXED_LATIN_ROW: &str = "中文bc 英文def 结束done";

/// # Returns
///
/// The repository's corpus on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`Corpus::load_dir`]'s return values on failure.
fn repository_corpus() -> anyhow::Result<Corpus> {
    Ok(Corpus::load_dir(&corpus::default_dir())?)
}

/// # Returns
///
/// A case laying the given buffer out in the given viewport and under the given options.
fn case(id: &str, buffer: &str, viewport_width: u16, options: Options) -> Case {
    Case {
        id: id.to_owned(),
        description: "A case the tests build.".to_owned(),
        buffer: buffer.to_owned(),
        keys: PARK_KEYS.to_owned(),
        viewport_width,
        viewport_height: corpus::DEFAULT_VIEWPORT_HEIGHT,
        tags: BTreeSet::from([Tag::Ascii]),
        options,
    }
}

/// # Returns
///
/// The corpus case with the given identifier.
///
/// # Panics
///
/// Panics if the corpus holds no case with that identifier.
fn corpus_case<'corpus_lifetime>(
    corpus: &'corpus_lifetime Corpus,
    id: &str,
) -> &'corpus_lifetime Case {
    corpus
        .cases()
        .iter()
        .find(|case| case.id == id)
        .unwrap_or_else(|| panic!("the corpus must hold the case `{id}`"))
}

/// # Returns
///
/// The state with its screen text dropped, which is everything the oracle compared before the
/// screen text was one of its dimensions.
fn without_screen_text(state: &EditorState) -> EditorState {
    EditorState {
        screen_text: ScreenText::default(),
        ..state.clone()
    }
}

/// # Returns
///
/// The three layouts of [`LAYOUT_BUFFER`] the review replayed, each named as it is reported.
fn layout_variants() -> [Case; 3] {
    [
        case(
            "layout-plain-wrap",
            LAYOUT_BUFFER,
            LAYOUT_WIDTH,
            Options::default(),
        ),
        case(
            "layout-breakindent-showbreak",
            LAYOUT_BUFFER,
            LAYOUT_WIDTH,
            Options {
                breakindent: true,
                showbreak: "> ".to_owned(),
                ..Options::default()
            },
        ),
        case(
            "layout-nowrap",
            LAYOUT_BUFFER,
            LAYOUT_WIDTH,
            Options {
                wrap: false,
                ..Options::default()
            },
        ),
    ]
}

/// # Returns
///
/// Whether the case is one whose lines vim cannot draw one to a row: the cases that wrap a line,
/// scroll a viewport sideways, or expand a tab.
fn is_laid_out(id: &str) -> bool {
    ["anchor-walk-", "wrap-", "nowrap-", "tab-"]
        .iter()
        .any(|family| id.starts_with(family))
}

/// # Returns
///
/// The screen an engine would draw if it took a buffer's lines for the viewport's rows: as many
/// of them as the viewport is tall, each cut off at the viewport's width, with vim's filler rows
/// below the last line.
fn buffer_as_screen(buffer: &str, case: &Case) -> ScreenText {
    let width = usize::from(case.viewport_width);
    let height = usize::from(case.viewport_height);
    let mut rows: Vec<String> = buffer
        .lines()
        .map(|line| line.chars().take(width).collect())
        .collect();
    rows.resize(height, "~".to_owned());

    ScreenText::new(rows)
}

#[test]
fn three_layouts_of_one_buffer_are_told_apart_by_their_screen_text_alone() -> anyhow::Result<()> {
    let driver = VimDriver::new()?;
    let variants = layout_variants();

    let states: Vec<(&str, EditorState)> = variants
        .iter()
        .map(|case| Ok((case.id.as_str(), driver.run_case(case)?)))
        .collect::<Result<_, vbc_oracle::vim::Error>>()?;

    for (id, state) in &states[1..] {
        assert_eq!(
            without_screen_text(state),
            without_screen_text(&states[0].1),
            "the case `{id}` differs from `{}` in a dimension other than the screen text, so it \
             is not the case the review reported",
            states[0].0
        );
    }
    for (index, (id, state)) in states.iter().enumerate() {
        for (other_id, other) in &states[index + 1..] {
            assert_ne!(
                state, other,
                "the cases `{id}` and `{other_id}` still end in the same state"
            );
            assert_eq!(
                Dimension::of(&state.diff(other)),
                vec![Dimension::ScreenText],
                "the cases `{id}` and `{other_id}` diverge somewhere other than the screen text"
            );
        }
    }
    assert_eq!(
        states
            .iter()
            .map(|(_, state)| state.screen_text.row(LAYOUT_ROW))
            .collect::<Vec<Option<&str>>>(),
        vec![Some("ilon zeta"), Some("    > ilon zeta"), Some("~")],
        "the second line is no longer drawn three different ways"
    );
    Ok(())
}

#[test]
fn a_double_width_line_is_captured_in_the_cells_vim_drew() -> anyhow::Result<()> {
    let driver = VimDriver::new()?;
    let wide = case("cjk-screen", CJK_BUFFER, LAYOUT_WIDTH, Options::default());

    let screen = driver.run_case(&wide)?.screen_text;

    assert_eq!(
        screen.rows().iter().take(3).collect::<Vec<&String>>(),
        vec!["中文测试行一二三四五六七", "八九十", "ascii"],
        "a double-width character is not captured as the two cells vim draws it in"
    );
    assert_eq!(
        screen
            .row(0)
            .expect("the viewport draws a first row")
            .chars()
            .count(),
        usize::from(LAYOUT_WIDTH) / 2,
        "the first row holds one character for every two cells of the viewport"
    );
    Ok(())
}

#[test]
fn a_combining_mark_is_captured_in_the_cell_vim_drew() -> anyhow::Result<()> {
    let driver = VimDriver::new()?;
    let marked = case(
        "combining-screen",
        COMBINING_BUFFER,
        LAYOUT_WIDTH,
        Options::default(),
    );

    let screen = driver.run_case(&marked)?.screen_text;

    assert_eq!(
        screen.row(0),
        Some("cafe\u{301} nai\u{308}ve \u{2764}\u{fe0f}"),
        "a cell is captured without the marks vim drew on top of its character"
    );
    Ok(())
}

#[test]
fn a_joined_emoji_is_captured_in_the_cells_vim_drew() -> anyhow::Result<()> {
    let driver = VimDriver::new()?;
    let joined = case(
        "emoji-screen",
        JOINED_EMOJI_BUFFER,
        LAYOUT_WIDTH,
        Options::default(),
    );
    let unjoined = Case {
        buffer: UNJOINED_EMOJI_BUFFER.to_owned(),
        ..joined.clone()
    };

    let drawn = driver.run_case(&joined)?.screen_text;
    let drawn_unjoined = driver.run_case(&unjoined)?.screen_text;

    let row = drawn.row(0).expect("the viewport draws a first row");
    assert!(row.starts_with('a'), "{row:?}");
    assert!(row.ends_with('c'), "{row:?}");
    for cluster in ["\u{1f469}", "\u{1f4bb}", "\u{1f1ef}\u{1f1f5}"] {
        assert!(
            row.contains(cluster),
            "the row {row:?} lost the cluster {cluster:?}"
        );
    }
    assert_ne!(
        drawn.row(0),
        drawn_unjoined.row(0),
        "the joiner is not captured, so a line that joins two emoji and one that does not are \
         drawn the same way"
    );
    Ok(())
}

#[test]
fn every_showbreak_case_draws_its_marker_below_the_first_row() -> anyhow::Result<()> {
    let corpus = repository_corpus()?;
    let driver = VimDriver::new()?;
    let marked: Vec<&Case> = corpus.with_tag(Tag::Showbreak).collect();

    assert_ne!(marked, Vec::<&Case>::new());
    for case in marked {
        let marker = case.options.showbreak.as_str();
        assert!(
            !case.buffer.contains(marker),
            "the case `{}` holds its own marker in its buffer, so drawing it proves nothing",
            case.id
        );
        let plain = Case {
            options: Options {
                showbreak: String::new(),
                ..case.options.clone()
            },
            ..case.clone()
        };

        let drawn = driver.run_case(case)?.screen_text;
        let drawn_plain = driver.run_case(&plain)?.screen_text;

        assert!(
            drawn.rows().iter().skip(1).any(|row| row.contains(marker)),
            "the case `{}` draws its marker {marker:?} on no continuation row: {:?}",
            case.id,
            drawn.rows()
        );
        assert!(
            !drawn_plain.rows().iter().any(|row| row.contains(marker)),
            "the case `{}` draws the marker {marker:?} even with `showbreak` cleared, so the \
             marker is not what put it on the screen",
            case.id
        );
    }
    Ok(())
}

#[test]
fn every_breakindent_case_repeats_its_indent_below_the_first_row() -> anyhow::Result<()> {
    let corpus = repository_corpus()?;
    let driver = VimDriver::new()?;
    let indented: Vec<&Case> = corpus.with_tag(Tag::Breakindent).collect();

    assert_ne!(indented, Vec::<&Case>::new());
    for case in indented {
        let flat = Case {
            options: Options {
                breakindent: false,
                ..case.options.clone()
            },
            ..case.clone()
        };

        let drawn = driver.run_case(case)?.screen_text;
        let drawn_flat = driver.run_case(&flat)?.screen_text;

        let continuation = drawn.row(1).expect("a wrapped line has a second row");
        let flat_continuation = drawn_flat.row(1).expect("a wrapped line has a second row");
        assert_ne!(
            continuation, flat_continuation,
            "the case `{}` draws its continuation row the same way with and without \
             `breakindent`",
            case.id
        );
        assert!(
            continuation.starts_with(' '),
            "the case `{}` draws its continuation row {continuation:?} with no indent in front \
             of the text",
            case.id
        );
        assert!(
            !flat_continuation.starts_with(' '),
            "the case `{}` indents its continuation row {flat_continuation:?} even with \
             `breakindent` cleared, so the option is not what indents it",
            case.id
        );
    }
    Ok(())
}

#[test]
fn most_corpus_cases_draw_a_screen_their_buffers_do_not_describe() -> anyhow::Result<()> {
    let corpus = repository_corpus()?;
    let driver = VimDriver::new()?;

    let mut informative: Vec<&str> = Vec::new();
    let mut redundant: Vec<&str> = Vec::new();
    for case in corpus.cases() {
        let state = driver.run_case(case)?;
        assert_eq!(
            state.screen_text.height(),
            u64::from(case.viewport_height),
            "the case `{}` was not captured over the whole of its viewport",
            case.id
        );
        if state.screen_text == buffer_as_screen(&state.buffer, case) {
            redundant.push(case.id.as_str());
        } else {
            informative.push(case.id.as_str());
        }
    }

    println!(
        "{} of {} cases draw a screen the other dimensions do not describe; the rest are {:?}",
        informative.len(),
        corpus.cases().len(),
        redundant
    );
    for case in corpus.cases().iter().filter(|case| is_laid_out(&case.id)) {
        assert!(
            informative.contains(&case.id.as_str()),
            "the case `{}` wraps, scrolls or expands a tab and still draws its buffer's own lines",
            case.id
        );
    }
    assert!(
        !redundant.is_empty(),
        "no case draws the screen its buffer describes, which a capture that comes back short \
         looks like as much as an informative screen does"
    );
    assert!(
        informative.len() >= INFORMATIVE_CASES,
        "only {} of the corpus's {} cases draw a screen the other dimensions do not describe",
        informative.len(),
        corpus.cases().len()
    );
    Ok(())
}

#[test]
fn text_typed_into_an_open_insert_is_on_the_screen() -> anyhow::Result<()> {
    let driver = VimDriver::new()?;
    let typing = Case {
        keys: "ihello".to_owned(),
        ..case(
            "insert-screen",
            LAYOUT_BUFFER,
            LAYOUT_WIDTH,
            Options::default(),
        )
    };

    let state = driver.run_case(&typing)?;

    assert_eq!(state.mode, Mode::Insert);
    assert_eq!(
        state.screen_text.row(0),
        Some("helloalpha"),
        "the screen was captured before the keys reached it"
    );
    Ok(())
}

#[test]
fn a_viewport_of_thousands_of_cells_is_captured_whole() -> anyhow::Result<()> {
    let driver = VimDriver::new()?;
    let wide = case(
        "wide-screen",
        LAYOUT_BUFFER,
        WIDE_VIEWPORT_WIDTH,
        Options::default(),
    );

    let screen = driver.run_case(&wide)?.screen_text;

    assert_eq!(
        screen.height(),
        u64::from(corpus::DEFAULT_VIEWPORT_HEIGHT),
        "a viewport of {} cells was not captured whole",
        u32::from(WIDE_VIEWPORT_WIDTH) * u32::from(corpus::DEFAULT_VIEWPORT_HEIGHT)
    );
    assert_eq!(screen.row(0), Some("alpha"));
    assert_eq!(
        screen.row(1),
        Some("    beta gamma delta epsilon zeta"),
        "a line that fits the viewport is drawn on one row"
    );
    Ok(())
}

#[test]
fn a_case_that_is_replayed_twice_is_drawn_the_same_way() -> anyhow::Result<()> {
    let corpus = repository_corpus()?;
    let driver = VimDriver::new()?;
    let case = corpus_case(&corpus, "wrap-w24-breakindent-showbreak");

    assert_eq!(
        driver.run_case(case)?.screen_text,
        driver.run_case(case)?.screen_text
    );
    Ok(())
}

#[test]
fn every_case_records_the_screen_a_repaint_draws() -> anyhow::Result<()> {
    let corpus = repository_corpus()?;
    let driver = VimDriver::new()?;

    let mut borrowed: Vec<String> = Vec::new();
    for case in corpus.cases() {
        let repainted = Case {
            keys: format!("{}{REPAINT_KEYS}", case.keys),
            ..case.clone()
        };

        let drawn = driver.run_case(case)?;
        let drawn_again = driver.run_case(&repainted)?;

        for divergence in drawn.diff(&drawn_again) {
            borrowed.push(format!("the case `{}` diverges: {divergence:?}", case.id));
        }
    }

    assert_eq!(
        borrowed,
        Vec::<String>::new(),
        "a case is recorded as something other than the screen vim draws it, so a recorded row \
         does not read back as the text it draws: it holds a character in a cell that character \
         does not start in, which only an earlier draw of the same screen put there"
    );
    Ok(())
}

#[test]
fn a_double_width_character_at_an_odd_column_is_recorded_once() -> anyhow::Result<()> {
    let driver = VimDriver::new()?;
    let shifted = Case {
        keys: SHIFT_ONTO_AN_ODD_COLUMN_KEYS.to_owned(),
        ..case(
            "shifted-wide-screen",
            SHIFTED_WIDE_BUFFER,
            LAYOUT_WIDTH,
            Options::default(),
        )
    };

    let state = driver.run_case(&shifted)?;

    assert_eq!(
        state.display_position.column, ODD_COLUMN,
        "vim draws the double-width character in another column than the odd one the capture is \
         held to here"
    );
    assert_eq!(
        state.screen_text.row(0),
        Some(SHIFTED_WIDE_ROW),
        "a double-width character starting at an odd column is recorded in the cell it spans as \
         well as in the cell it starts in"
    );
    Ok(())
}

#[test]
fn a_double_width_character_at_an_even_column_is_recorded_once() -> anyhow::Result<()> {
    let driver = VimDriver::new()?;
    let unshifted = Case {
        keys: DELETE_LAST_KEYS.to_owned(),
        ..case(
            "unshifted-wide-screen",
            UNSHIFTED_WIDE_BUFFER,
            LAYOUT_WIDTH,
            Options::default(),
        )
    };

    let state = driver.run_case(&unshifted)?;

    assert_eq!(
        state.display_position.column, EVEN_COLUMN,
        "vim draws the double-width character in another column than the even one the capture is \
         held to here"
    );
    assert_eq!(
        state.screen_text.row(0),
        Some(UNSHIFTED_WIDE_ROW),
        "a double-width character starting at an even column is no longer recorded once"
    );
    Ok(())
}

#[test]
fn the_cjk_case_the_capture_doubled_records_every_character_once() -> anyhow::Result<()> {
    let corpus = repository_corpus()?;
    let driver = VimDriver::new()?;
    let case = corpus_case(&corpus, CJK_MIXED_LATIN_CASE);
    let baseline = Baseline::read(&baseline::default_path())?;

    let state = driver.run_case(case)?;

    assert_eq!(
        state.buffer,
        format!("{CJK_MIXED_LATIN_ROW}\n"),
        "the case no longer ends in the buffer whose row the capture doubled"
    );
    assert_eq!(
        state.screen_text.row(0),
        Some(CJK_MIXED_LATIN_ROW),
        "the case is captured with a character in a cell it does not start in"
    );
    assert_eq!(
        baseline
            .cases
            .get(CJK_MIXED_LATIN_CASE)
            .and_then(|recorded| recorded.screen_text.row(0)),
        Some(CJK_MIXED_LATIN_ROW),
        "the baseline records the case with a character in a cell it does not start in"
    );
    Ok(())
}
