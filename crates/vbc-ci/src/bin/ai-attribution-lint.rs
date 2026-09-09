//! Command-line entry point checking a pull request for credit given to an AI.
//!
//! A pull request is squashed before it lands, and a squash carries every trailer of every commit
//! it folds forward into the one commit that stays. So the tip is not what is read: the whole
//! range is, one commit at a time, and the title and the body the pull request is described by
//! beside it. The title is read because a squash writes it as the subject of the commit that
//! lands, which makes it the one line of a request that is certain to become history.

use std::env::args;
use std::fmt::Write as _;
use std::fs;
use std::process::{Command, ExitCode};

use vbc_ci::ai_attribution;

/// The flag naming the commits to read, whose value is a revision range `git log` is given as it
/// stands.
const RANGE_FLAG: &str = "--range";

/// The flags naming the files the pull request's title and body are written in. Each is read from
/// a file rather than from an argument because it is prose somebody typed, and prose belongs
/// nowhere near a shell.
const TITLE_FLAG: &str = "--title-file";
const BODY_FLAG: &str = "--body-file";

/// What each commit is reported as, which is its name on one line and its message under it, and
/// what separates one such report from the next.
const COMMIT_FORMAT: &str = "--format=%H%n%B%x00";
const COMMIT_SEPARATOR: char = '\0';

/// What the pull request's title and body are called where they are reported, since neither has a
/// name of its own.
const TITLE: &str = "the pull request title";
const BODY: &str = "the pull request body";

/// # Returns
///
/// [`ExitCode::SUCCESS`] if no commit of the range, and no line of the title or of the body,
/// credits an AI as an author or as a generator, and [`ExitCode::FAILURE`] otherwise.
fn main() -> ExitCode {
    let arguments: Vec<String> = args().skip(1).collect();
    let Some(range) = flag(&arguments, RANGE_FLAG) else {
        eprintln!(
            "Usage: ai-attribution-lint {RANGE_FLAG} <range> [{TITLE_FLAG} <path>] \
             [{BODY_FLAG} <path>]"
        );
        return ExitCode::FAILURE;
    };

    let described = [
        (TITLE, flag(&arguments, TITLE_FLAG)),
        (BODY, flag(&arguments, BODY_FLAG)),
    ];
    let texts = match read(range, &described) {
        Ok(texts) => texts,
        Err(reason) => {
            eprintln!("{reason}");
            return ExitCode::FAILURE;
        }
    };

    let mut report = String::new();
    for (name, text) in &texts {
        for credit in ai_attribution::scan(text) {
            let _ = writeln!(report, "{name}: {credit}");
        }
    }
    if !report.is_empty() {
        eprint!("{report}");
        eprintln!(
            "A commit, a title, or a body above credits an AI, a model, or an assistant as an \
             author or as a generator, which nothing that lands in this repository may do. Naming \
             one in prose is fine; signing its work over to one is not."
        );
        return ExitCode::FAILURE;
    }

    println!("{} texts credit no AI as an author.", texts.len());
    ExitCode::SUCCESS
}

/// # Returns
///
/// The value written after a flag, or [`None`] where the flag is not given.
fn flag<'arguments>(arguments: &'arguments [String], name: &str) -> Option<&'arguments str> {
    let at = arguments.iter().position(|argument| name == argument)?;

    arguments.get(at + 1).map(String::as_str)
}

/// # Returns
///
/// Every text a pull request is to be read for, which is the message of each of its commits and
/// each text of `described` that was written to a file, each under the name it is reported by, on
/// success.
///
/// # Errors
///
/// Returns an error saying why the commits, or one of the described texts, could not be read.
fn read(range: &str, described: &[(&str, Option<&str>)]) -> Result<Vec<(String, String)>, String> {
    let mut texts = messages(range)?;
    for (name, path) in described {
        let Some(path) = path else {
            continue;
        };
        let text = fs::read_to_string(path)
            .map_err(|error| format!("`{path}` could not be read: {error}."))?;
        texts.push(((*name).to_owned(), text));
    }

    Ok(texts)
}

/// # Returns
///
/// The message of every commit of a revision range, each under the name of the commit it is, on
/// success.
///
/// # Errors
///
/// Returns an error saying why `git log` could not report the range.
fn messages(range: &str) -> Result<Vec<(String, String)>, String> {
    let output = Command::new("git")
        .args(["log", COMMIT_FORMAT, range])
        .output()
        .map_err(|error| format!("`git log {range}` could not be run: {error}."))?;
    if !output.status.success() {
        let reason = String::from_utf8_lossy(&output.stderr);
        return Err(format!("`git log {range}` failed: {}", reason.trim()));
    }

    let messages: Vec<(String, String)> = String::from_utf8_lossy(&output.stdout)
        .split(COMMIT_SEPARATOR)
        .filter_map(|record| {
            let record = record.trim_start_matches('\n');
            let (name, message) = record.split_once('\n')?;

            Some((name.to_owned(), message.to_owned()))
        })
        .collect();
    if messages.is_empty() {
        return Err(format!(
            "`{range}` names no commit, so nothing was read. A range that matches nothing reports \
             success as loudly as one that was clean, which is the silence this check exists to \
             break."
        ));
    }

    Ok(messages)
}
