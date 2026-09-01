//! The differential runner driven end to end against stub engines.
//!
//! A stub answers every case with a state derived from the case alone, so a stub that perturbs
//! exactly one dimension is an engine that is broken in exactly one way. Running the corpus
//! against such a stub checks that the runner catches a break in that dimension and in no other,
//! which is what keeps the oracle from silently degrading into a buffer-only comparison.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error as StdError;
use std::fmt::{Display, Formatter, Result as FmtResult};

use serde_json::{json, Value};
use vbc_oracle::corpus::{self, Case, Corpus, Options, Tag};
use vbc_oracle::runner::{self, Dimension, Engine, Outcome, Report};
use vbc_oracle::state::{Cursor, DisplayPosition, EditorState, Mode, Register, RegisterType};
use vbc_oracle::vim::VimDriver;

/// The single dimension a stub deliberately gets wrong.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Perturbation {
    /// The stub reports the state every other unbroken stub reports.
    Nothing,

    /// The stub appends a character to the buffer.
    Buffer,

    /// The stub moves the cursor one column further.
    Cursor,

    /// The stub draws the cursor one screen row further down, leaving its position alone.
    DisplayPosition,

    /// The stub reports insert mode instead of normal mode.
    Mode,

    /// The stub reports the same register text with a different type.
    Register,

    /// The stub cannot replay the case at all.
    Failure,
}

/// The reason a stub could not replay a case.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct StubError;

impl Display for StubError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        formatter.write_str("the stub was asked to fail")
    }
}

impl StdError for StubError {}

/// An engine that answers a case with a state derived from the case alone, broken in at most one
/// dimension.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Stub {
    name: &'static str,
    perturbation: Perturbation,
    target: Option<&'static str>,
}

impl Stub {
    /// # Returns
    ///
    /// An engine that reports the unperturbed state for every case.
    fn matching(name: &'static str) -> Self {
        Self {
            name,
            perturbation: Perturbation::Nothing,
            target: None,
        }
    }

    /// # Returns
    ///
    /// An engine that is broken in the given dimension for every case.
    fn broken(name: &'static str, perturbation: Perturbation) -> Self {
        Self {
            name,
            perturbation,
            target: None,
        }
    }

    /// # Returns
    ///
    /// An engine that is broken in the given dimension for one case, and unbroken for every other.
    fn broken_on(name: &'static str, perturbation: Perturbation, target: &'static str) -> Self {
        Self {
            name,
            perturbation,
            target: Some(target),
        }
    }
}

impl Engine for Stub {
    type Error = StubError;

    fn name(&self) -> &str {
        self.name
    }

    fn replay(&self, case: &Case) -> Result<EditorState, Self::Error> {
        let mut state = unperturbed_state(case);
        if self.target.is_some_and(|target| target != case.id) {
            return Ok(state);
        }
        match self.perturbation {
            Perturbation::Nothing => {}
            Perturbation::Buffer => state.buffer.push('!'),
            Perturbation::Cursor => state.cursor.column += 1,
            Perturbation::DisplayPosition => state.display_position.row += 1,
            Perturbation::Mode => state.mode = Mode::Insert,
            Perturbation::Register => {
                state.registers.insert(
                    '"',
                    Register {
                        text: case.id.clone(),
                        register_type: RegisterType::Linewise,
                    },
                );
            }
            Perturbation::Failure => return Err(StubError),
        }

        Ok(state)
    }
}

/// # Returns
///
/// The state every unbroken stub reports for the case, which fixes all five dimensions to values
/// derived from the case.
fn unperturbed_state(case: &Case) -> EditorState {
    let column = u64::try_from(case.keys.len()).expect("a key sequence fits in a `u64`");

    EditorState {
        buffer: case.buffer.clone(),
        cursor: Cursor { line: 0, column },
        display_position: DisplayPosition { row: 0, column },
        mode: Mode::Normal,
        registers: BTreeMap::from([(
            '"',
            Register {
                text: case.id.clone(),
                register_type: RegisterType::Charwise,
            },
        )]),
    }
}

/// # Returns
///
/// The repository's own corpus on success.
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
/// A case built by the tests, which the stubs turn into a state.
fn case(id: &'static str, tags: &[Tag]) -> Case {
    Case {
        id: id.to_owned(),
        description: "A case the tests build.".to_owned(),
        buffer: "alpha beta\n".to_owned(),
        keys: "dw".to_owned(),
        viewport_width: 40,
        viewport_height: corpus::DEFAULT_VIEWPORT_HEIGHT,
        tags: tags.iter().copied().collect(),
        options: Options::default(),
    }
}

/// Replays the whole corpus against a stub broken in exactly one dimension, and asserts that every
/// case is reported as diverging in that dimension and in no other.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`repository_corpus`]'s return values on failure.
///
/// # Panics
///
/// Panics if a case is not reported as diverging in exactly the given dimension.
fn assert_corpus_catches(perturbation: Perturbation, dimension: Dimension) -> anyhow::Result<()> {
    let corpus = repository_corpus()?;
    let report = runner::run_corpus(
        &corpus,
        &Stub::matching("reference"),
        &Stub::broken("subject", perturbation),
    );

    assert!(!report.all_agreed());
    assert_eq!(report.summary.overall.total, corpus.cases().len());
    assert_eq!(report.summary.overall.diverged, corpus.cases().len());
    assert_eq!(report.summary.overall.agreed, 0);
    assert_eq!(report.summary.overall.failed, 0);
    for case in &report.cases {
        let Outcome::Diverged { dimensions, .. } = &case.outcome else {
            panic!("the case `{}` was reported as {:?}", case.id, case.outcome);
        };
        assert_eq!(
            dimensions.as_slice(),
            &[dimension][..],
            "the case `{}` named the wrong dimensions",
            case.id
        );
    }

    Ok(())
}

#[test]
fn a_stub_broken_only_in_the_buffer_is_caught() -> anyhow::Result<()> {
    assert_corpus_catches(Perturbation::Buffer, Dimension::Buffer)
}

#[test]
fn a_stub_broken_only_in_the_cursor_is_caught() -> anyhow::Result<()> {
    assert_corpus_catches(Perturbation::Cursor, Dimension::Cursor)
}

#[test]
fn a_stub_broken_only_in_the_display_position_is_caught() -> anyhow::Result<()> {
    assert_corpus_catches(Perturbation::DisplayPosition, Dimension::DisplayPosition)
}

#[test]
fn a_stub_broken_only_in_the_mode_is_caught() -> anyhow::Result<()> {
    assert_corpus_catches(Perturbation::Mode, Dimension::Mode)
}

#[test]
fn a_stub_broken_only_in_a_register_is_caught() -> anyhow::Result<()> {
    assert_corpus_catches(Perturbation::Register, Dimension::Register)
}

#[test]
fn a_matching_stub_yields_no_failures() -> anyhow::Result<()> {
    let corpus = repository_corpus()?;
    let report = runner::run_corpus(
        &corpus,
        &Stub::matching("reference"),
        &Stub::matching("subject"),
    );

    assert!(report.all_agreed());
    assert_eq!(report.summary.overall.total, corpus.cases().len());
    assert_eq!(report.summary.overall.agreed, corpus.cases().len());
    assert_eq!(report.summary.overall.diverged, 0);
    assert_eq!(report.summary.overall.failed, 0);
    let reported: Vec<&str> = report.cases.iter().map(|case| case.id.as_str()).collect();
    let expected: Vec<&str> = corpus.cases().iter().map(|case| case.id.as_str()).collect();
    assert_eq!(reported, expected);
    let unexpected: Vec<&str> = report
        .cases
        .iter()
        .filter(|case| !case.outcome.agreed())
        .map(|case| case.id.as_str())
        .collect();
    assert_eq!(unexpected, Vec::<&str>::new());

    Ok(())
}

#[test]
fn a_full_corpus_run_counts_every_tag() -> anyhow::Result<()> {
    let corpus = repository_corpus()?;
    let report = runner::run_corpus(
        &corpus,
        &Stub::matching("reference"),
        &Stub::broken("subject", Perturbation::Cursor),
    );

    let counted: BTreeMap<Tag, usize> = report
        .summary
        .tags
        .iter()
        .map(|(tag, counts)| (*tag, counts.total))
        .collect();
    assert_eq!(counted, corpus.tag_counts());
    for (tag, counts) in &report.summary.tags {
        assert_eq!(counts.diverged, counts.total, "the tag {tag:?} miscounted");
        assert_eq!(counts.agreed, 0, "the tag {tag:?} miscounted");
        assert_eq!(counts.failed, 0, "the tag {tag:?} miscounted");
    }

    Ok(())
}

#[test]
fn a_full_corpus_run_serializes_the_same_way_every_time() -> anyhow::Result<()> {
    let corpus = repository_corpus()?;
    let run = || {
        runner::run_corpus(
            &corpus,
            &Stub::matching("reference"),
            &Stub::broken("subject", Perturbation::Register),
        )
    };

    assert_eq!(run().to_json()?, run().to_json()?);

    Ok(())
}

#[test]
fn a_report_names_the_case_the_status_and_the_dimensions() -> anyhow::Result<()> {
    let cases = [
        case("agreeing-case", &[Tag::Ascii]),
        case("diverging-case", &[Tag::Ascii, Tag::Code]),
    ];
    let report = runner::run_cases(
        &cases,
        &Stub::matching("reference"),
        &Stub::broken_on("subject", Perturbation::Cursor, "diverging-case"),
    );

    assert_eq!(
        serde_json::from_str::<Value>(&report.to_json()?)?,
        json!({
            "reference": "reference",
            "subject": "subject",
            "cases": [
                {
                    "id": "agreeing-case",
                    "keys": "dw",
                    "tags": ["ascii"],
                    "status": "agreed",
                },
                {
                    "id": "diverging-case",
                    "keys": "dw",
                    "tags": ["ascii", "code"],
                    "status": "diverged",
                    "dimensions": ["cursor"],
                    "divergences": [
                        {
                            "Cursor": {
                                "left": {"line": 0, "column": 2},
                                "right": {"line": 0, "column": 3},
                            },
                        },
                    ],
                },
            ],
            "summary": {
                "overall": {"total": 2, "agreed": 1, "diverged": 1, "failed": 0},
                "tags": {
                    "ascii": {"total": 2, "agreed": 1, "diverged": 1, "failed": 0},
                    "code": {"total": 1, "agreed": 0, "diverged": 1, "failed": 0},
                },
            },
        })
    );

    Ok(())
}

#[test]
fn a_report_names_the_display_position_dimension_in_its_json() -> anyhow::Result<()> {
    let cases = [case("display-only-case", &[Tag::Wrap])];
    let report = runner::run_cases(
        &cases,
        &Stub::matching("reference"),
        &Stub::broken("subject", Perturbation::DisplayPosition),
    );

    let rendered: Value = serde_json::from_str(&report.to_json()?)?;

    assert_eq!(
        rendered["cases"][0]["dimensions"],
        json!(["display-position"])
    );
    assert_eq!(
        rendered["cases"][0]["divergences"],
        json!([
            {
                "DisplayPosition": {
                    "left": {"row": 0, "column": 2},
                    "right": {"row": 1, "column": 2},
                },
            },
        ])
    );

    Ok(())
}

#[test]
fn an_engine_that_cannot_replay_a_case_is_reported_as_a_failure() -> anyhow::Result<()> {
    let cases = [case("failing-case", &[Tag::Ascii])];
    let report = runner::run_cases(
        &cases,
        &Stub::matching("reference"),
        &Stub::broken("subject", Perturbation::Failure),
    );

    assert!(!report.all_agreed());
    assert_eq!(report.summary.overall.failed, 1);
    assert_eq!(report.summary.overall.diverged, 0);
    assert_eq!(
        report.cases.first().map(|case| &case.outcome),
        Some(&Outcome::Failed {
            engine: "subject".to_owned(),
            message: StubError.to_string(),
        })
    );
    let rendered = report
        .render_case("failing-case")
        .expect("the run replayed the case");
    assert!(rendered.contains("failing-case"), "{rendered}");
    assert!(rendered.contains("subject"), "{rendered}");
    assert!(rendered.contains(&StubError.to_string()), "{rendered}");

    Ok(())
}

#[test]
fn a_reference_that_cannot_replay_a_case_is_reported_as_a_failure() -> anyhow::Result<()> {
    let corpus = repository_corpus()?;
    let report = runner::run_corpus(
        &corpus,
        &Stub::broken("reference", Perturbation::Failure),
        &Stub::matching("subject"),
    );

    assert!(!report.all_agreed());
    assert_eq!(report.summary.overall.failed, corpus.cases().len());
    assert_eq!(report.summary.overall.agreed, 0);
    assert_eq!(report.summary.overall.diverged, 0);
    for case in &report.cases {
        assert_eq!(
            case.outcome,
            Outcome::Failed {
                engine: "reference".to_owned(),
                message: StubError.to_string(),
            },
            "the case `{}` was not blamed on the reference",
            case.id
        );
    }

    Ok(())
}

#[test]
fn a_reference_that_fails_is_named_ahead_of_a_subject_that_also_fails() -> anyhow::Result<()> {
    let cases = [case("failing-case", &[Tag::Ascii])];
    let report = runner::run_cases(
        &cases,
        &Stub::broken("reference", Perturbation::Failure),
        &Stub::broken("subject", Perturbation::Failure),
    );

    assert_eq!(
        report.cases.first().map(|case| &case.outcome),
        Some(&Outcome::Failed {
            engine: "reference".to_owned(),
            message: StubError.to_string(),
        })
    );

    Ok(())
}

#[test]
fn a_report_read_back_from_its_json_is_the_report_that_was_written() -> anyhow::Result<()> {
    let corpus = repository_corpus()?;
    let diverging = runner::run_corpus(
        &corpus,
        &Stub::matching("reference"),
        &Stub::broken_on("subject", Perturbation::Register, "cjk-ambiwidth-double"),
    );
    let failing = runner::run_corpus(
        &corpus,
        &Stub::matching("reference"),
        &Stub::broken_on("subject", Perturbation::Failure, "cjk-ambiwidth-double"),
    );

    assert_eq!(diverging.summary.overall.diverged, 1);
    assert_eq!(failing.summary.overall.failed, 1);
    for report in [&diverging, &failing] {
        assert_eq!(serde_json::from_str::<Report>(&report.to_json()?)?, *report);
    }

    Ok(())
}

#[test]
fn the_diff_of_a_failing_case_names_the_case_and_the_dimension() {
    let cases = [
        case("agreeing-case", &[Tag::Ascii]),
        case("cursor-only-case", &[Tag::Ascii]),
    ];
    let report = runner::run_cases(
        &cases,
        &Stub::matching("vim"),
        &Stub::broken_on("vimbecode", Perturbation::Cursor, "cursor-only-case"),
    );

    let rendered = report
        .render_case("cursor-only-case")
        .expect("the run replayed the case");
    assert!(rendered.contains("cursor-only-case"), "{rendered}");
    assert!(rendered.contains("diverged in cursor"), "{rendered}");
    assert!(rendered.contains("vim"), "{rendered}");
    assert!(rendered.contains("vimbecode"), "{rendered}");
    assert!(rendered.contains("line 0, column 2"), "{rendered}");
    assert!(rendered.contains("line 0, column 3"), "{rendered}");
    assert!(!rendered.contains("buffer"), "{rendered}");
    assert!(!rendered.contains("mode"), "{rendered}");
    assert!(!rendered.contains("register"), "{rendered}");

    assert_eq!(report.render_case("no-such-case"), None);
    assert!(
        report
            .render_case("agreeing-case")
            .expect("the run replayed the case")
            .contains("agreed"),
        "an agreeing case renders as agreeing"
    );
}

#[test]
fn the_diff_of_a_display_position_break_names_the_display_dimension_alone() {
    let cases = [case("display-only-case", &[Tag::Wrap])];
    let report = runner::run_cases(
        &cases,
        &Stub::matching("vim"),
        &Stub::broken("vimbecode", Perturbation::DisplayPosition),
    );

    let rendered = report
        .render_case("display-only-case")
        .expect("the run replayed the case");
    assert!(
        rendered.contains("diverged in display position"),
        "{rendered}"
    );
    assert!(
        rendered.contains("screen row 0, screen column 2"),
        "{rendered}"
    );
    assert!(
        rendered.contains("screen row 1, screen column 2"),
        "{rendered}"
    );
    assert!(!rendered.contains("cursor"), "{rendered}");
    assert!(!rendered.contains("buffer"), "{rendered}");
    assert!(!rendered.contains("mode"), "{rendered}");
    assert!(!rendered.contains("register"), "{rendered}");
}

#[test]
fn the_whole_corpus_replays_against_vim_without_divergence() -> anyhow::Result<()> {
    let corpus = repository_corpus()?;
    let reference = VimDriver::new()?;
    let subject = VimDriver::new()?;
    let report = runner::run_corpus(&corpus, &reference, &subject);

    assert!(report.all_agreed(), "{report}");
    assert_eq!(report.summary.overall.total, corpus.cases().len());

    Ok(())
}

#[test]
fn a_tag_selects_the_cases_it_labels() -> anyhow::Result<()> {
    let corpus = repository_corpus()?;
    let selected: Vec<&Case> = corpus.with_tag(Tag::WordMotion).collect();
    let report = runner::run_cases(
        selected.iter().copied(),
        &Stub::matching("reference"),
        &Stub::matching("subject"),
    );

    assert!(report.all_agreed());
    assert_eq!(report.summary.overall.total, selected.len());
    assert_eq!(
        report.summary.tags.keys().copied().collect::<BTreeSet<_>>(),
        selected
            .iter()
            .flat_map(|case| case.tags.iter().copied())
            .collect::<BTreeSet<_>>()
    );

    Ok(())
}
