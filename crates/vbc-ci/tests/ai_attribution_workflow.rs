//! The workflow that runs the attribution check, and the proof that reading it bites.
//!
//! A check nothing runs is prose again, which is what the rule already was when it was broken. So
//! the workflow is read here rather than trusted: it has to be triggered by a pull request, it has
//! to hold a job that nothing excuses from running or from failing, and that job has to check out
//! the history the check reads and run the linter over the range and the body it reads it from.
//!
//! What a reading like that is worth is not an argument either. Every way the workflow could stop
//! running the check is written into a copy of it -- the job excused, the job renamed, the trigger
//! swapped, narrowed to some branches or to some paths, struck off one of the moments a request
//! changes at, the command commented out where it still reads as one, each flag struck off, the
//! history shortened to the tip -- and the reading is required to report each. A guard that has
//! stopped covering the workflow it names fails here rather than passing quietly.

use std::fs;
use std::path::{Path, PathBuf};

/// The workflow that must run the check, the key its trigger is written under, and the job of it
/// that every pull request runs.
const WORKFLOW: [&str; 3] = [".github", "workflows", "ai-attribution.yaml"];
const TRIGGER: &str = "on:";
const WORKFLOW_JOB: &str = "lint-attribution";

/// The event the workflow must be triggered by, and every moment of it a request has to be read
/// at. A request opened with its commits already pushed is read at no moment other than `opened`,
/// one reopened at none other than `reopened`, a body rewritten afterwards at none other than
/// `edited`, and a commit pushed afterwards at none other than `synchronize`. So a check run at
/// three of the four lets a request through at the fourth, reporting nothing where nothing was
/// read.
const PULL_REQUEST: &str = "pull_request:";
const MOMENTS: [&str; 4] = [
    "\"edited\"",
    "\"opened\"",
    "\"reopened\"",
    "\"synchronize\"",
];

/// The keys that would narrow the trigger to some pull requests rather than all of them. A request
/// the trigger passes over runs the check nowhere, and a check that was never run reads on the
/// request exactly as a clean one does.
const NARROWINGS: [&str; 4] = ["branches:", "branches-ignore:", "paths:", "paths-ignore:"];

/// What a key of the workflow is written under, and what everything inside one is indented past.
const UNDER_A_KEY: &str = "  ";
const INSIDE_A_KEY: &str = "    ";

/// The keys that would excuse a job of the workflow, or a step of one, from being run and from
/// failing, neither of which a job every pull request has to pass may hold.
const CONDITION: &str = "if:";
const FORGIVEN: &str = "continue-on-error:";

/// What opens a line of the workflow that is read rather than run.
const COMMENT: &str = "#";

/// The depth the history must be checked out to. The check reads every commit between the base and
/// the head, so a checkout that fetched only the tip would leave it nothing to read.
const WHOLE_HISTORY: &str = "fetch-depth: 0";

/// Everything the job has to run, each of which is a way the check would stop reading what it is
/// for if it went missing: the linter itself, the range that is every commit of the request, the
/// title and the body written where they can be read, and the two names that range is measured
/// between.
const REQUIRED: [&str; 8] = [
    "--bin ai-attribution-lint",
    "--range \"$BASE_SHA..$HEAD_SHA\"",
    "--title-file \"$RUNNER_TEMP/pr-title.txt\"",
    "--body-file \"$RUNNER_TEMP/pr-body.txt\"",
    "BASE_SHA: \"${{ github.event.pull_request.base.sha }}\"",
    "HEAD_SHA: \"${{ github.event.pull_request.head.sha }}\"",
    "PR_TITLE: \"${{ github.event.pull_request.title }}\"",
    "PR_BODY: \"${{ github.event.pull_request.body }}\"",
];

#[test]
fn continuous_integration_checks_the_attribution_of_every_pull_request() {
    assert_eq!(Vec::<String>::new(), unrun_by(&workflow()));
}

#[test]
fn a_workflow_that_stopped_running_the_check_is_caught() {
    let workflow = workflow();
    let mut stopped = vec![
        excused_by(
            &workflow,
            &format!("{CONDITION} \"github.event_name == 'push'\""),
        ),
        excused_by(&workflow, &format!("{FORGIVEN} true")),
        workflow.replace(
            &format!("{UNDER_A_KEY}{WORKFLOW_JOB}:"),
            "  lint-something-else:",
        ),
        workflow.replace(PULL_REQUEST, "schedule:"),
        workflow.replace(WHOLE_HISTORY, "fetch-depth: 1"),
        commented_out(&workflow, "cargo run"),
        narrowed_by(&workflow, "paths: [\"crates/**\"]"),
        narrowed_by(&workflow, "branches: [\"main\"]"),
    ];
    stopped.extend(
        REQUIRED
            .into_iter()
            .chain(MOMENTS)
            .map(|required| workflow.replace(required, "")),
    );

    for workflow in stopped {
        assert_ne!(
            Vec::<String>::new(),
            unrun_by(&workflow),
            "a workflow that stopped running the check was passed:\n{workflow}"
        );
    }
}

/// # Returns
///
/// The workflow that must run the check.
///
/// # Panics
///
/// Panics if the workflow cannot be read.
fn workflow() -> String {
    let path = WORKFLOW
        .iter()
        .fold(workspace(), |path, name| path.join(name));

    fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} is readable: {error}", path.display()))
}

/// # Returns
///
/// What keeps `workflow` from checking every pull request's attribution, empty if nothing does.
fn unrun_by(workflow: &str) -> Vec<String> {
    let mut complaints = Vec::new();
    let job = job(workflow, WORKFLOW_JOB);
    let run = collapsed(&job);
    let trigger = trigger(workflow);
    let triggered = collapsed(&trigger);

    if !trigger.iter().any(|line| line.trim() == PULL_REQUEST) {
        complaints.push(format!("the workflow is not triggered by `{PULL_REQUEST}`"));
    }
    for moment in MOMENTS {
        if !triggered.contains(moment) {
            complaints.push(format!("the workflow is not triggered on {moment}"));
        }
    }
    for narrowing in NARROWINGS {
        if narrows(&trigger, narrowing) {
            complaints.push(format!("the trigger is narrowed by `{narrowing}`"));
        }
    }
    if job.is_empty() {
        complaints.push(format!("the workflow holds no `{WORKFLOW_JOB}` job"));
    }
    for excuse in excuses(&job) {
        let excuse = excuse.trim();
        complaints.push(format!("`{WORKFLOW_JOB}` is excused by `{excuse}`"));
    }
    if !run.contains(WHOLE_HISTORY) {
        complaints.push(format!(
            "`{WORKFLOW_JOB}` does not check out `{WHOLE_HISTORY}`"
        ));
    }
    for required in REQUIRED {
        if !run.contains(required) {
            complaints.push(format!("`{WORKFLOW_JOB}` does not run `{required}`"));
        }
    }

    complaints
}

/// # Returns
///
/// The lines of the trigger of `workflow`, which are the lines under `on:` indented inside it.
fn trigger(workflow: &str) -> Vec<&str> {
    workflow
        .lines()
        .skip_while(|line| line.trim_end() != TRIGGER)
        .skip(1)
        .take_while(|line| line.trim().is_empty() || line.starts_with(UNDER_A_KEY))
        .collect()
}

/// # Returns
///
/// Whether a trigger is narrowed by `narrowing` to some pull requests rather than all of them.
fn narrows(trigger: &[&str], narrowing: &str) -> bool {
    trigger
        .iter()
        .any(|line| line.trim().starts_with(narrowing))
}

/// # Returns
///
/// The lines of one job of `workflow`, which are the lines under it indented inside it.
fn job<'workflow>(workflow: &'workflow str, name: &str) -> Vec<&'workflow str> {
    let opening = format!("{UNDER_A_KEY}{name}:");

    workflow
        .lines()
        .skip_while(|line| line.trim_end() != opening)
        .skip(1)
        .take_while(|line| line.trim().is_empty() || line.starts_with(INSIDE_A_KEY))
        .collect()
}

/// # Returns
///
/// The lines of a job that would excuse it, or a step of it, from being run and from failing.
fn excuses<'job>(job: &[&'job str]) -> Vec<&'job str> {
    job.iter()
        .filter(|line| {
            let key = line.trim().trim_start_matches("- ");
            key.starts_with(CONDITION) || key.starts_with(FORGIVEN)
        })
        .copied()
        .collect()
}

/// # Returns
///
/// The lines a job runs, with the breaks and the blank space between their words collapsed, so
/// that a command a workflow wrapped over several lines reads as the one command it runs and a
/// command written where it is only read reads as nothing.
fn collapsed(lines: &[&str]) -> String {
    lines
        .iter()
        .filter(|line| !line.trim_start().starts_with(COMMENT))
        .copied()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('\\', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// # Returns
///
/// The workflow with a command commented out where it is written, which is a step that still reads
/// as one and runs nothing.
fn commented_out(workflow: &str, opening: &str) -> String {
    let mut commenting = false;

    workflow
        .lines()
        .map(|line| {
            let written = line.trim_start();
            if written.starts_with(opening) {
                commenting = true;
            } else if written.is_empty() || !line.starts_with(INSIDE_A_KEY) {
                commenting = false;
            }
            if !commenting {
                return line.to_owned();
            }
            let indent = &line[..line.len() - written.len()];

            format!("{indent}{COMMENT} {written}")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// # Returns
///
/// The workflow with `filter` written into its trigger, which is a trigger that passes over every
/// pull request the filter leaves out.
fn narrowed_by(workflow: &str, filter: &str) -> String {
    let opening = format!("{UNDER_A_KEY}{PULL_REQUEST}\n");

    workflow.replace(&opening, &format!("{opening}{INSIDE_A_KEY}{filter}\n"))
}

/// # Returns
///
/// The workflow with `excuse` written into the job that runs the check.
fn excused_by(workflow: &str, excuse: &str) -> String {
    let opening = format!("{UNDER_A_KEY}{WORKFLOW_JOB}:\n");

    workflow.replace(&opening, &format!("{opening}{INSIDE_A_KEY}{excuse}\n"))
}

/// # Returns
///
/// The root of the workspace this crate belongs to.
///
/// # Panics
///
/// Panics if this crate does not sit two directories below a workspace root.
fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("this crate sits two directories below its workspace root")
        .to_owned()
}
