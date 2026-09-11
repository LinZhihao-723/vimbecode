//! Checks of the `ai-attribution-lint` binary against real repositories, which is the entry point
//! the attribution workflow invokes.
//!
//! The rule these checks are about was written down in every set of instructions this repository
//! hands out, and was broken anyway, by tooling that appended a trailer rather than by anybody
//! typing one. So it is checked here the way the title convention is checked: by running the thing
//! continuous integration runs, over commits that were really made, rather than by asking a
//! function about a string.
//!
//! What the range is worth is the point of most of what follows. A pull request is squashed before
//! it lands and a squash carries every trailer it folds forward, so a check that reads the tip
//! reads the one commit whose message was most likely written by hand. Every commit is read here
//! instead, and that is proved by a branch whose first commit is the dirty one and whose second is
//! clean: a check that stopped at the tip would call it green. The title and the body a request is
//! described by are read beside them, because a squash writes the one as the subject of the commit
//! that lands and folds the other into its message.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::{fs, process};

/// The `ai-attribution-lint` binary Cargo built for this test, and the flags it is told where the
/// title and the body a pull request is described by are written.
const LINT_BIN: &str = env!("CARGO_BIN_EXE_ai-attribution-lint");
const TITLE_FLAG: &str = "--title-file";
const BODY_FLAG: &str = "--body-file";

/// The trailer the repository's own history carries, which is the credit this check was written
/// for and therefore the fixture it is held to catching.
const OFFENCE: &str = "Co-authored-by: Claude Opus 5 (1M context) <noreply@anthropic.com>";

/// The variants of that credit which have to be caught beside it.
const VARIANTS: [&str; 6] = [
    "Generated with [Claude Code](https://claude.com/claude-code)",
    "- Co-authored-by: Claude",
    "noreply@anthropic.com",
    "CO-AUTHORED-BY: CLAUDE OPUS 5",
    "co-authored-by: claude",
    "Assisted-by: ChatGPT",
];

/// The link to a session tooling appends on a line of its own, at an id made up for the check.
const SESSION_LINK: &str = "https://claude.ai/code/session_01EXAMPLEEXAMPLE";

/// The trailer tooling appends to a commit to say which session wrote it, and the variants of it
/// which have to be caught beside it.
const SESSIONS: [&str; 6] = [
    "Claude-Session: https://claude.ai/code/session_01EXAMPLEEXAMPLE",
    "claude-session: 01EXAMPLEEXAMPLE",
    "CLAUDE-SESSION: 01EXAMPLEEXAMPLE",
    "- Claude-Session: 01EXAMPLEEXAMPLE",
    SESSION_LINK,
    "HTTPS://CLAUDE.AI/CODE/SESSION_01EXAMPLEEXAMPLE",
];

/// Messages that name a model without signing anything over to one, which this repository writes
/// constantly and which the check must leave alone.
const INNOCENT: [&str; 5] = [
    "ci: Add the guard.\n\nThe guard was worked out beside Claude Code at a terminal.",
    "docs: Explain the harness.\n\nAnthropic's harness is what the sessions run in.",
    "fix: Correct the gutter.\n\nThis corrects a width Claude Code got wrong.",
    "test: Cover the range.\n\nGenerated with a script, which Claude Code did not write.",
    "docs: Point at the harness.\n\nA Claude Code session runs at https://claude.ai/code.",
];

/// How many repositories this run has built, so that two of them never share a directory.
static BUILT: AtomicUsize = AtomicUsize::new(0);

/// A repository built for one check, which is thrown away with it.
struct Repository {
    path: PathBuf,
}

impl Repository {
    /// # Returns
    ///
    /// A repository holding one commit, which is the base a range is measured from.
    ///
    /// # Panics
    ///
    /// Panics if the repository cannot be built.
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "vbc-ai-attribution-{}-{}",
            process::id(),
            BUILT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("a temporary directory can be made");

        let repository = Self { path };
        repository.git(&["init", "--quiet", "--initial-branch", "main"]);
        repository.commit("chore: Lay the base down.");

        repository
    }

    /// Adds one commit carrying `message`.
    ///
    /// # Panics
    ///
    /// Panics if the commit cannot be made.
    fn commit(&self, message: &str) {
        self.git(&["commit", "--quiet", "--allow-empty", "--message", message]);
    }

    /// # Returns
    ///
    /// The name of the commit the repository is at.
    ///
    /// # Panics
    ///
    /// Panics if the commit cannot be named.
    fn head(&self) -> String {
        self.git(&["rev-parse", "HEAD"]).trim().to_owned()
    }

    /// # Returns
    ///
    /// The range running from the repository's first commit to the one it is at, which is the
    /// range a pull request is read over.
    ///
    /// # Panics
    ///
    /// Panics if the first commit cannot be named.
    fn range(&self) -> String {
        let first = self
            .git(&["rev-list", "--max-parents=0", "HEAD"])
            .trim()
            .to_owned();

        format!("{first}..HEAD")
    }

    /// # Returns
    ///
    /// What `git` printed, on success.
    ///
    /// # Panics
    ///
    /// Panics if `git` cannot be run, or reports failure.
    fn git(&self, arguments: &[&str]) -> String {
        let output = Command::new("git")
            .current_dir(&self.path)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .args([
                "-c",
                "user.name=A Tester",
                "-c",
                "user.email=tester@example.com",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(arguments)
            .output()
            .expect("git can be run");
        assert!(
            output.status.success(),
            "`git {}` failed: {}",
            arguments.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );

        String::from_utf8_lossy(&output.stdout).into_owned()
    }
}

impl Drop for Repository {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// What one run of the linter reported.
struct Report {
    accepted: bool,
    said: String,
}

/// # Returns
///
/// What the linter made of a range of a repository, and of each text `described` names a file for.
///
/// # Panics
///
/// Panics if the linter cannot be run.
fn lint(repository: &Repository, range: &str, described: &[(&str, &Path)]) -> Report {
    let mut command = Command::new(LINT_BIN);
    command
        .current_dir(&repository.path)
        .args(["--range", range]);
    for (flag, path) in described {
        command.arg(flag).arg(path);
    }
    let output = command.output().expect("the linter can be run");

    Report {
        accepted: output.status.success(),
        said: format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ),
    }
}

/// # Returns
///
/// Whether the linter accepted a repository holding one commit carrying `message`.
fn accepts(message: &str) -> bool {
    let repository = Repository::new();
    repository.commit(message);

    lint(&repository, &repository.range(), &[]).accepted
}

#[test]
fn the_trailer_the_history_carries_is_rejected() {
    let message = format!("ci: Do a thing.\n\nA body that explains it.\n\n{OFFENCE}");

    assert!(!accepts(&message));
}

#[test]
fn every_variant_of_the_credit_is_rejected() {
    for variant in VARIANTS {
        let message = format!("ci: Do a thing.\n\n{variant}");
        assert!(!accepts(&message), "`{variant}` was accepted");
    }
}

#[test]
fn a_clean_commit_is_accepted() {
    assert!(accepts("ci: Do a thing.\n\nA body that explains it."));
}

#[test]
fn a_commit_that_only_names_a_model_is_accepted() {
    for message in INNOCENT {
        assert!(accepts(message), "`{message}` was rejected");
    }
}

#[test]
fn a_commit_that_names_its_session_is_rejected() {
    for session in SESSIONS {
        let message = format!("ci: Do a thing.\n\nA body that explains it.\n\n{session}");
        assert!(!accepts(&message), "`{session}` was accepted");
    }
}

#[test]
fn a_body_that_links_a_session_is_rejected_though_every_commit_is_clean() -> anyhow::Result<()> {
    let repository = Repository::new();
    repository.commit("ci: Do a thing.\n\nA body that explains it.");
    let body = repository.path.join("body.txt");

    fs::write(
        &body,
        format!("## Summary\n\nA change.\n\n{SESSION_LINK}\n"),
    )?;
    let linked = lint(&repository, &repository.range(), &[(BODY_FLAG, &body)]);

    fs::write(
        &body,
        "## Summary\n\nA change worked out in a Claude Code session, which https://claude.ai \
         hosts.\n",
    )?;
    let innocent = lint(&repository, &repository.range(), &[(BODY_FLAG, &body)]);

    assert!(!linked.accepted, "{}", linked.said);
    assert!(
        linked.said.contains("the pull request body: line 5:"),
        "the link went unnamed:\n{}",
        linked.said
    );
    assert!(innocent.accepted, "{}", innocent.said);

    Ok(())
}

#[test]
fn every_commit_of_a_range_is_read_rather_than_its_tip() {
    let repository = Repository::new();
    repository.commit(&format!("ci: Do a thing.\n\n{OFFENCE}"));
    let dirty = repository.head();
    repository.commit("ci: Do another thing.\n\nA body that explains it.");
    let tip = repository.head();

    let report = lint(&repository, &repository.range(), &[]);

    assert!(
        !report.accepted,
        "a range whose first commit is dirty was accepted:\n{}",
        report.said
    );
    assert!(
        report.said.contains(&dirty),
        "the dirty commit `{dirty}` went unnamed:\n{}",
        report.said
    );
    assert!(
        !report.said.contains(&tip),
        "the clean tip `{tip}` was named:\n{}",
        report.said
    );
}

#[test]
fn a_title_that_credits_a_model_is_rejected_though_every_commit_is_clean() {
    let repository = Repository::new();
    repository.commit("ci: Do a thing.\n\nA body that explains it.");
    let title = repository.path.join("title.txt");

    fs::write(&title, "ci: Add the guard, generated with Claude Code.")
        .expect("the title can be written");
    let credited = lint(&repository, &repository.range(), &[(TITLE_FLAG, &title)]);

    fs::write(&title, "ci: Add the guard Claude Code is read by.")
        .expect("the title can be written");
    let innocent = lint(&repository, &repository.range(), &[(TITLE_FLAG, &title)]);

    assert!(!credited.accepted, "{}", credited.said);
    assert!(innocent.accepted, "{}", innocent.said);
}

#[test]
fn a_body_that_credits_a_model_is_rejected_though_every_commit_is_clean() {
    let repository = Repository::new();
    repository.commit("ci: Do a thing.\n\nA body that explains it.");
    let body = repository.path.join("body.txt");

    fs::write(
        &body,
        "## Summary\n\nA change.\n\nGenerated with [Claude Code]\n",
    )
    .expect("the body can be written");
    let credited = lint(&repository, &repository.range(), &[(BODY_FLAG, &body)]);

    fs::write(
        &body,
        "## Summary\n\nA change worked out beside Claude Code.\n",
    )
    .expect("the body can be written");
    let innocent = lint(&repository, &repository.range(), &[(BODY_FLAG, &body)]);

    assert!(!credited.accepted, "{}", credited.said);
    assert!(innocent.accepted, "{}", innocent.said);
}

#[test]
fn a_range_naming_no_commit_is_rejected_rather_than_passed_quietly() {
    let repository = Repository::new();
    repository.commit("ci: Do a thing.");

    let report = lint(&repository, "HEAD..HEAD", &[]);

    assert!(
        !report.accepted,
        "an empty range was accepted:\n{}",
        report.said
    );
}

#[test]
fn a_body_that_cannot_be_read_is_rejected() {
    let repository = Repository::new();
    repository.commit("ci: Do a thing.");
    let missing = repository.path.join("no-such-body.txt");

    let report = lint(&repository, &repository.range(), &[(BODY_FLAG, &missing)]);

    assert!(
        !report.accepted,
        "a missing body was accepted:\n{}",
        report.said
    );
}

#[test]
fn a_missing_range_is_rejected() {
    let accepted = Command::new(LINT_BIN)
        .status()
        .expect("the linter can be run")
        .success();

    assert!(!accepted);
}
