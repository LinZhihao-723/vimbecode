//! The corpus baseline recorded, checked, and driven off course on purpose.
//!
//! A baseline only earns its place if a check fails when the reference side moves, so every test
//! here moves one thing -- a recorded state, a corpus case, the vim behind the reference -- and
//! asserts what the check makes of it. The repository's own baseline is checked too, as far as it
//! can be from whatever vim is installed here.

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::error::Error as StdError;
use std::fmt::{Display, Formatter, Result as FmtResult};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{self, Command};

use vbc_oracle::baseline::{self, Baseline, Check, Drift, Error, Reference, SCHEMA_VERSION};
use vbc_oracle::corpus::{Case, Corpus};
use vbc_oracle::runner::{Dimension, Engine};
use vbc_oracle::state::{Cursor, DisplayPosition, EditorState, Mode};
use vbc_oracle::vim::{VimDriver, VimVersion};

/// A corpus the tests own, small enough to replay in a moment and editable without touching the
/// repository's own.
const SECTION: &str = r#"
[[case]]
id = "baseline-delete-word"
description = "Deleting the first word."
buffer = """
alpha beta
"""
keys = "dw"
viewport_width = 40
tags = ["ascii", "word-motion"]

[[case]]
id = "baseline-yank-line"
description = "Yanking the first line."
buffer = """
alpha beta
gamma
"""
keys = "yy"
viewport_width = 40
tags = ["ascii"]
"#;

/// The same corpus with one case's keys changed, which is the edit a stale baseline must catch.
const EDITED_SECTION: &str = r#"
[[case]]
id = "baseline-delete-word"
description = "Deleting the first WORD."
buffer = """
alpha beta
"""
keys = "dW"
viewport_width = 40
tags = ["ascii", "word-motion"]

[[case]]
id = "baseline-yank-line"
description = "Yanking the first line."
buffer = """
alpha beta
gamma
"""
keys = "yy"
viewport_width = 40
tags = ["ascii"]
"#;

/// A directory the test writes a corpus and a baseline into, removed when the test ends.
struct Workspace {
    path: PathBuf,
}

impl Workspace {
    /// Factory function.
    ///
    /// Creates an empty directory of its own for the named test.
    ///
    /// # Returns
    ///
    /// A newly created workspace on success.
    ///
    /// # Errors
    ///
    /// Returns an error if the directory could not be created.
    fn new(name: &str) -> anyhow::Result<Self> {
        let path = env::temp_dir().join(format!("vbc-baseline-{}-{name}", process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path)?;

        Ok(Self { path })
    }

    /// # Returns
    ///
    /// The path of the corpus directory the workspace holds.
    fn corpus(&self) -> &Path {
        &self.path
    }

    /// # Returns
    ///
    /// The path the workspace's baseline is written to.
    fn baseline(&self) -> PathBuf {
        self.path.join("baseline.json")
    }

    /// Writes the corpus the workspace holds.
    ///
    /// # Errors
    ///
    /// Returns an error if the section could not be written.
    fn write_corpus(&self, section: &str) -> anyhow::Result<()> {
        fs::write(self.path.join("cases.toml"), section)?;

        Ok(())
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// The reason a stub could not replay a case.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct StubError;

impl Display for StubError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        formatter.write_str("the stub holds no state for the case")
    }
}

impl StdError for StubError {}

/// A reference that answers every case with a state derived from the case alone, and reports the
/// vim version it is told to rather than one of a vim that exists.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Stub {
    version: VimVersion,
}

impl Stub {
    /// # Returns
    ///
    /// A reference reporting the given vim version.
    fn reporting(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            version: VimVersion {
                major,
                minor,
                patch,
            },
        }
    }
}

impl Engine for Stub {
    type Error = StubError;

    fn name(&self) -> &str {
        "stub"
    }

    fn replay(&self, case: &Case) -> Result<EditorState, Self::Error> {
        Ok(EditorState {
            buffer: case.buffer.clone(),
            cursor: Cursor { line: 0, column: 0 },
            display_position: DisplayPosition { row: 0, column: 0 },
            mode: Mode::Normal,
            registers: BTreeMap::new(),
        })
    }
}

impl Reference for Stub {
    fn vim_version(&self) -> VimVersion {
        self.version
    }
}

#[test]
fn a_freshly_recorded_baseline_holds() -> anyhow::Result<()> {
    let workspace = Workspace::new("freshly-recorded")?;
    workspace.write_corpus(SECTION)?;
    let corpus = Corpus::load_dir(workspace.corpus())?;
    let reference = VimDriver::new()?;
    let baseline = Baseline::record(&corpus, &reference)?;

    let check = baseline.check(&corpus, &reference)?;

    assert!(check.matched(), "{check}");
    assert_eq!(
        check,
        Check::Matched {
            vim_version: reference.version(),
            cases: corpus.cases().len(),
        }
    );

    Ok(())
}

#[test]
fn a_recorded_state_that_moved_fails_the_check() -> anyhow::Result<()> {
    let workspace = Workspace::new("state-that-moved")?;
    workspace.write_corpus(SECTION)?;
    let corpus = Corpus::load_dir(workspace.corpus())?;
    let reference = VimDriver::new()?;
    let mut baseline = Baseline::record(&corpus, &reference)?;
    let moved = baseline
        .cases
        .get_mut("baseline-delete-word")
        .expect("the baseline records every case of the corpus");
    moved.cursor = Cursor {
        line: moved.cursor.line,
        column: moved.cursor.column + 1,
    };

    let check = baseline.check(&corpus, &reference)?;

    assert!(check.drifted(), "{check}");
    let Check::Drifted { drifts, .. } = &check else {
        panic!("the check must report the case that moved: {check}");
    };
    assert_eq!(drifts.len(), 1, "{check}");
    assert_eq!(drifts[0].id(), "baseline-delete-word");
    let Drift::Changed { dimensions, .. } = &drifts[0] else {
        panic!("the case moved in a dimension the check must name: {check}");
    };
    assert_eq!(*dimensions, vec![Dimension::Cursor]);
    let rendered = check.to_string();
    assert!(rendered.contains("baseline-delete-word"), "{rendered}");
    assert!(rendered.contains("cursor"), "{rendered}");
    assert!(!rendered.contains("baseline-yank-line"), "{rendered}");

    Ok(())
}

#[test]
fn an_edited_corpus_fails_the_check_until_the_baseline_is_recorded_again() -> anyhow::Result<()> {
    let workspace = Workspace::new("edited-corpus")?;
    workspace.write_corpus(SECTION)?;
    let corpus = Corpus::load_dir(workspace.corpus())?;
    let reference = VimDriver::new()?;
    let baseline = Baseline::record(&corpus, &reference)?;

    workspace.write_corpus(EDITED_SECTION)?;
    let edited = Corpus::load_dir(workspace.corpus())?;
    let failure = baseline
        .check(&edited, &reference)
        .expect_err("an edited corpus is not the corpus the baseline was recorded over");

    assert!(
        matches!(failure, Error::CorpusChanged { .. }),
        "{failure:?}"
    );
    let message = failure.to_string();
    assert!(message.contains(&baseline.header.corpus_hash), "{message}");
    assert!(
        message.contains(&baseline::corpus_hash(&edited)?),
        "{message}"
    );
    assert!(Baseline::record(&edited, &reference)?
        .check(&edited, &reference)?
        .matched());

    Ok(())
}

#[test]
fn recording_the_same_corpus_twice_writes_the_same_bytes() -> anyhow::Result<()> {
    let corpus = Corpus::load_dir(&vbc_oracle::corpus::default_dir())?;
    let reference = VimDriver::new()?;

    let first = Baseline::record(&corpus, &reference)?;
    let second = Baseline::record(&corpus, &reference)?;

    assert_eq!(first, second);
    assert_eq!(first.to_json()?, second.to_json()?);
    assert_eq!(first.cases.len(), corpus.cases().len());

    Ok(())
}

#[test]
fn a_baseline_round_trips_through_the_file_it_is_written_to() -> anyhow::Result<()> {
    let workspace = Workspace::new("round-trip")?;
    workspace.write_corpus(SECTION)?;
    let corpus = Corpus::load_dir(workspace.corpus())?;
    let reference = VimDriver::new()?;
    let baseline = Baseline::record(&corpus, &reference)?;

    baseline.write(&workspace.baseline())?;

    assert_eq!(Baseline::read(&workspace.baseline())?, baseline);
    assert_eq!(
        fs::read_to_string(workspace.baseline())?,
        baseline.to_json()?
    );

    Ok(())
}

#[test]
fn another_vim_leaves_the_recorded_states_uncompared_and_says_so() -> anyhow::Result<()> {
    let corpus = Corpus::load_dir(&vbc_oracle::corpus::default_dir())?;
    let recorded = Stub::reporting(9, 1, 100);
    let baseline = Baseline::record(&corpus, &recorded)?;

    let check = baseline.check(&corpus, &Stub::reporting(8, 2, 3995))?;

    assert_eq!(
        check,
        Check::Skipped {
            recorded: recorded.vim_version(),
            running: VimVersion {
                major: 8,
                minor: 2,
                patch: 3995,
            },
        }
    );
    assert!(!check.matched());
    assert!(!check.drifted());
    let message = check.to_string();
    assert!(message.contains("9.1.100"), "{message}");
    assert!(message.contains("8.2.3995"), "{message}");
    assert!(message.contains("not compared"), "{message}");
    assert!(
        message.contains("continuous integration pins vim 9.1"),
        "{message}"
    );

    Ok(())
}

#[test]
fn another_patch_level_of_the_recorded_vim_still_compares() -> anyhow::Result<()> {
    let corpus = Corpus::load_dir(&vbc_oracle::corpus::default_dir())?;
    let baseline = Baseline::record(&corpus, &Stub::reporting(9, 1, 100))?;

    let check = baseline.check(&corpus, &Stub::reporting(9, 1, 999))?;

    assert!(check.matched(), "{check}");

    Ok(())
}

#[test]
fn a_baseline_written_in_another_schema_is_rejected() -> anyhow::Result<()> {
    let corpus = Corpus::load_dir(&vbc_oracle::corpus::default_dir())?;
    let stub = Stub::reporting(9, 1, 100);
    let mut baseline = Baseline::record(&corpus, &stub)?;
    baseline.header.schema_version = SCHEMA_VERSION + 1;

    let failure = baseline
        .check(&corpus, &stub)
        .expect_err("a baseline of another schema cannot be compared");

    assert!(
        matches!(failure, Error::SchemaVersion { .. }),
        "{failure:?}"
    );

    Ok(())
}

#[test]
fn the_repository_baseline_covers_the_repository_corpus() -> anyhow::Result<()> {
    let corpus = Corpus::load_dir(&vbc_oracle::corpus::default_dir())?;
    let baseline = Baseline::read(&baseline::default_path())?;

    assert_eq!(baseline.header.schema_version, SCHEMA_VERSION);
    assert_eq!(
        baseline.header.corpus_hash,
        baseline::corpus_hash(&corpus)?,
        "the corpus was edited without recording the baseline again"
    );
    assert_eq!(
        baseline
            .cases
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        corpus
            .cases()
            .iter()
            .map(|case| case.id.as_str())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
    );

    Ok(())
}

#[test]
fn continuous_integration_checks_the_baseline_on_every_pull_request() -> anyhow::Result<()> {
    let workflow = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join(".github")
            .join("workflows")
            .join("ci.yaml"),
    )?;

    assert!(workflow.contains("pull_request:"), "{workflow}");
    assert!(
        workflow.contains("--check-baseline --strict-vim-version"),
        "{workflow}"
    );

    Ok(())
}

#[test]
fn the_command_line_records_a_baseline_and_checks_it() -> anyhow::Result<()> {
    let workspace = Workspace::new("command-line")?;
    workspace.write_corpus(SECTION)?;

    let recorded = run_differential(&["--record-baseline"], &workspace)?;
    assert!(recorded.status.success(), "{recorded:?}");
    assert!(
        String::from_utf8_lossy(&recorded.stdout).contains("recorded 2 cases from vim"),
        "{recorded:?}"
    );

    let checked = run_differential(&["--check-baseline", "--strict-vim-version"], &workspace)?;
    assert!(checked.status.success(), "{checked:?}");
    assert!(
        String::from_utf8_lossy(&checked.stdout).contains("the baseline holds"),
        "{checked:?}"
    );

    Ok(())
}

#[test]
fn the_command_line_fails_the_check_on_a_state_that_moved() -> anyhow::Result<()> {
    let workspace = Workspace::new("command-line-drift")?;
    workspace.write_corpus(SECTION)?;
    assert!(run_differential(&["--record-baseline"], &workspace)?
        .status
        .success());
    let mut baseline = Baseline::read(&workspace.baseline())?;
    baseline
        .cases
        .get_mut("baseline-yank-line")
        .expect("the baseline records every case of the corpus")
        .buffer
        .push_str("drift\n");
    baseline.write(&workspace.baseline())?;

    let checked = run_differential(&["--check-baseline"], &workspace)?;

    assert!(!checked.status.success(), "{checked:?}");
    let printed = String::from_utf8_lossy(&checked.stdout);
    assert!(printed.contains("baseline-yank-line"), "{printed}");
    assert!(printed.contains("buffer"), "{printed}");

    Ok(())
}

/// Runs the entry point against the workspace's own corpus and baseline.
///
/// # Returns
///
/// What the entry point printed and exited with on success.
///
/// # Errors
///
/// Returns an error if the entry point could not be run.
fn run_differential(arguments: &[&str], workspace: &Workspace) -> anyhow::Result<process::Output> {
    Ok(Command::new(env!("CARGO_BIN_EXE_differential-run"))
        .args(arguments)
        .arg("--corpus")
        .arg(workspace.corpus())
        .arg("--baseline")
        .arg(workspace.baseline())
        .output()?)
}
