//! The corpus replayed against vim in the viewport and under the display options each case
//! declares.
//!
//! A case that is laid out under vim's own defaults instead of its own is not a test of anything:
//! a screen-line motion in a viewport that is not the case's lands somewhere else, and two cases
//! that differ only in a display option end in the same state. Every test here pins a case, or a
//! group of cases, to the state the case's own layout produces.

use vbc_oracle::corpus::{self, Case, Corpus, Options};
use vbc_oracle::state::{Cursor, EditorState};
use vbc_oracle::vim::VimDriver;

/// The four decorations the same wrapped line is laid out with, and the column each leaves the
/// cursor in after `gjgj` in a twenty-four column viewport.
const WRAP_VARIANTS: [(&str, u64); 4] = [
    ("wrap-w24-plain", 48),
    ("wrap-w24-breakindent", 44),
    ("wrap-w24-showbreak", 46),
    ("wrap-w24-breakindent-showbreak", 42),
];

/// The keys of a replay that does nothing, against which a case's own keys are measured.
const IDLE_KEYS: &str = "<Esc>";

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
/// The corpus case with the given identifier.
///
/// # Panics
///
/// Panics if the corpus holds no case with that identifier.
fn case<'corpus_lifetime>(corpus: &'corpus_lifetime Corpus, id: &str) -> &'corpus_lifetime Case {
    corpus
        .cases()
        .iter()
        .find(|case| case.id == id)
        .unwrap_or_else(|| panic!("the corpus must hold the case `{id}`"))
}

/// Replays a case in its own viewport and under its own display options.
///
/// # Returns
///
/// The state vim ends in on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`VimDriver::run_case`]'s return values on failure.
fn replay(driver: &VimDriver, case: &Case) -> anyhow::Result<EditorState> {
    Ok(driver.run_case(case)?)
}

#[test]
fn a_screen_line_motion_moves_the_cursor_in_a_narrow_viewport() -> anyhow::Result<()> {
    let corpus = repository_corpus()?;
    let case = case(&corpus, "wrap-w20-plain");
    let driver = VimDriver::new()?;

    assert_eq!(
        replay(&driver, case)?.cursor,
        Cursor {
            line: 0,
            column: 40
        }
    );
    assert_eq!(
        driver.run(&case.buffer, &case.keys)?.cursor,
        Cursor { line: 0, column: 0 },
        "the case's keys are a no-op outside its own viewport, so the viewport is what makes the \
         case a test"
    );
    Ok(())
}

#[test]
fn the_wrap_variants_of_one_line_end_in_mutually_distinct_states() -> anyhow::Result<()> {
    let corpus = repository_corpus()?;
    let driver = VimDriver::new()?;

    let mut states: Vec<(&str, EditorState)> = Vec::new();
    for (id, column) in WRAP_VARIANTS {
        let state = replay(&driver, case(&corpus, id))?;
        assert_eq!(
            state.cursor,
            Cursor { line: 0, column },
            "the case `{id}` did not end where its decoration puts the cursor"
        );
        states.push((id, state));
    }
    for (index, (id, state)) in states.iter().enumerate() {
        for (other_id, other) in &states[index + 1..] {
            assert_ne!(
                state, other,
                "the cases `{id}` and `{other_id}` end in the same state"
            );
        }
    }
    Ok(())
}

#[test]
fn the_two_tabstops_end_in_different_states() -> anyhow::Result<()> {
    let corpus = repository_corpus()?;
    let driver = VimDriver::new()?;

    let eight = replay(&driver, case(&corpus, "tab-leading-indent-ts8"))?;
    let four = replay(&driver, case(&corpus, "tab-leading-indent-ts4"))?;

    assert_eq!(eight.cursor, Cursor { line: 2, column: 2 });
    assert_eq!(
        four.cursor,
        Cursor {
            line: 2,
            column: 10
        }
    );
    Ok(())
}

#[test]
fn the_two_ambiwidths_end_in_different_states() -> anyhow::Result<()> {
    let corpus = repository_corpus()?;
    let driver = VimDriver::new()?;

    let single = replay(&driver, case(&corpus, "cjk-ambiwidth-single"))?;
    let double = replay(&driver, case(&corpus, "cjk-ambiwidth-double"))?;

    assert_eq!(
        single.cursor,
        Cursor {
            line: 0,
            column: 27
        }
    );
    assert_eq!(
        double.cursor,
        Cursor {
            line: 0,
            column: 23
        }
    );
    Ok(())
}

#[test]
fn the_two_showbreak_variants_end_in_different_states() -> anyhow::Result<()> {
    let corpus = repository_corpus()?;
    let driver = VimDriver::new()?;

    let plain = replay(&driver, case(&corpus, "wrap-w80-plain"))?;
    let marked = replay(&driver, case(&corpus, "wrap-w80-showbreak"))?;

    assert_eq!(
        plain.cursor,
        Cursor {
            line: 0,
            column: 119
        }
    );
    assert_eq!(
        marked.cursor,
        Cursor {
            line: 0,
            column: 117
        },
        "the marker takes two cells from every continuation line, so the screen-line motion lands \
         two characters earlier"
    );
    Ok(())
}

#[test]
fn the_viewport_width_is_the_width_of_the_text() -> anyhow::Result<()> {
    let corpus = repository_corpus()?;
    let boundary = case(&corpus, "wrap-w20-boundary-exact");
    let driver = VimDriver::new()?;

    let state = replay(&driver, boundary)?;

    assert_eq!(
        state.cursor,
        Cursor {
            line: 0,
            column: u64::from(boundary.viewport_width)
        },
        "the second screen line starts at the cell after the configured width, so a gutter would \
         have taken cells from the text"
    );
    Ok(())
}

#[test]
fn clearing_an_option_a_case_sets_changes_its_layout() -> anyhow::Result<()> {
    let corpus = repository_corpus()?;
    let driver = VimDriver::new()?;
    let decorated = case(&corpus, "wrap-w24-breakindent");
    let plain = Case {
        options: Options {
            breakindent: false,
            ..decorated.options.clone()
        },
        ..decorated.clone()
    };

    assert_ne!(
        replay(&driver, decorated)?,
        replay(&driver, &plain)?,
        "clearing 'breakindent' left the layout unchanged, so the option is not reaching vim"
    );
    Ok(())
}

#[test]
fn a_case_is_laid_out_in_a_viewport_as_tall_as_it_asks_for() -> anyhow::Result<()> {
    let corpus = repository_corpus()?;
    let driver = VimDriver::new()?;
    let tall = Case {
        buffer: (1..=40).map(|line| format!("line {line}\n")).collect(),
        keys: "L".to_owned(),
        viewport_height: 24,
        ..case(&corpus, "wrap-w20-plain").clone()
    };
    let short = Case {
        viewport_height: 10,
        ..tall.clone()
    };

    assert_eq!(
        replay(&driver, &tall)?.cursor,
        Cursor {
            line: 22,
            column: 0
        }
    );
    assert_eq!(
        replay(&driver, &short)?.cursor,
        Cursor { line: 8, column: 0 },
        "the lowest line on the screen is the one a shorter viewport ends at"
    );
    Ok(())
}

#[test]
fn every_case_replays_in_its_own_viewport_and_its_keys_leave_a_trace() -> anyhow::Result<()> {
    let corpus = repository_corpus()?;
    let driver = VimDriver::new()?;

    for case in corpus.cases() {
        let state = replay(&driver, case)
            .map_err(|error| anyhow::anyhow!("the case `{}` failed: {error}", case.id))?;
        let idle = Case {
            keys: IDLE_KEYS.to_owned(),
            ..case.clone()
        };
        assert_ne!(
            state,
            replay(&driver, &idle)?,
            "the case `{}` ends where it started, so it would still pass against an engine that \
             ignored its keys",
            case.id
        );
    }
    Ok(())
}
