//! Checks of the `pr-title-lint` binary, which is the entry point the pull request title workflow
//! invokes.

use std::process::Command;

use vbc_ci::pr_title::MAX_TITLE_LEN;

/// The `pr-title-lint` binary Cargo built for this test.
const LINT_BIN: &str = env!("CARGO_BIN_EXE_pr-title-lint");

/// # Returns
///
/// Whether the linter accepted `title`.
///
/// # Panics
///
/// Panics if the linter cannot be run.
fn lint_accepts(title: &str) -> bool {
    Command::new(LINT_BIN)
        .arg(title)
        .status()
        .expect("failed to run the pr-title-lint binary")
        .success()
}

#[test]
fn conforming_title_is_accepted() {
    assert!(lint_accepts("build: Set up the Cargo workspace and CI."));
}

#[test]
fn missing_type_is_rejected() {
    assert!(!lint_accepts("Set up the Cargo workspace and CI."));
}

#[test]
fn lowercase_subject_is_rejected() {
    assert!(!lint_accepts("build: set up the Cargo workspace and CI."));
}

#[test]
fn missing_period_is_rejected() {
    assert!(!lint_accepts("build: Set up the Cargo workspace and CI"));
}

#[test]
fn over_length_title_is_rejected() {
    let title = format!("build: {}.", "A".repeat(MAX_TITLE_LEN));
    assert!(!lint_accepts(&title));
}

#[test]
fn missing_argument_is_rejected() {
    let accepted = Command::new(LINT_BIN)
        .status()
        .expect("failed to run the pr-title-lint binary")
        .success();
    assert!(!accepted);
}
