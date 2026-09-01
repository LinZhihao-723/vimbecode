//! Command-line entry point checking the pull request title given as the sole argument.

use std::env::args;
use std::process::ExitCode;

use vbc_ci::pr_title;

/// # Returns
///
/// [`ExitCode::SUCCESS`] if the title given as the sole argument follows the convention, and
/// [`ExitCode::FAILURE`] otherwise.
fn main() -> ExitCode {
    let Some(title) = args().nth(1) else {
        eprintln!("Usage: pr-title-lint <title>");
        return ExitCode::FAILURE;
    };

    if let Err(e) = pr_title::validate(&title) {
        eprintln!("Invalid pull request title `{title}`: {e}.");
        eprintln!("Expected `type: Subject sentence.`, for example `feat: Add the vim oracle.`.");
        return ExitCode::FAILURE;
    }

    println!("Pull request title `{title}` follows the convention.");
    ExitCode::SUCCESS
}
