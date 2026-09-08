//! The check that the undocumented flag every tool approval goes through is still doing something.
//!
//! Both catalogs these run against were written by claude 2.1.263, one turn apart, into the same
//! directory, differing in one argument: `--permission-prompt-tool stdio`. Every field that names
//! this machine rather than the protocol was replaced and the MCP tools were dropped from both,
//! and nothing else was touched -- the tool lists are the binary's own, and the difference between
//! them is the whole of what the probe reads.
//!
//! What makes that worth reading is that it is a difference the binary does not otherwise report.
//! With the flag the catalog offers `AskUserQuestion`, `EnterPlanMode` and `ExitPlanMode`; without
//! it the very same catalog offers none of the three, and every other field -- the permission mode
//! included, which reads `default` either way -- is identical. A session that lost the flag would
//! answer, take turns, and deny every tool call needing approval, and this is the one place the
//! difference shows before that happens.
//!
//! The refusal is the other failure and the loud one: a binary that no longer knows the flag never
//! starts. The line it writes on its way out is commander's own, recorded from this binary
//! refusing an option it does not have, with the option renamed to the one a release that dropped
//! the flag would name.

use std::fs;

use anyhow::{anyhow, Result};
use vbc_editor::session::error::Error;
use vbc_editor::session::event::Event;
use vbc_editor::session::probe::{self, GATED_TOOLS, PERMISSION_PROMPT_TOOL};

/// The catalogs the binary announced with the flag and without it.
const HONOURED: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/session/init-with-flag.json"
);
const IGNORED: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/session/init-without-flag.json"
);

/// What the binary writes when it is handed a flag it does not have.
const REFUSED: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/session/refused.stderr");

#[test]
fn a_session_that_honoured_the_flag_is_let_through() -> Result<()> {
    probe::honoured(init(HONOURED)?.init().ok_or(anyhow!("an init frame"))?)?;

    Ok(())
}

#[test]
fn a_session_that_ignored_the_flag_is_reported_by_the_flags_own_name() -> Result<()> {
    let frame = init(IGNORED)?;
    let announced = frame.init().ok_or(anyhow!("an init frame"))?;

    let Err(error) = probe::honoured(announced) else {
        panic!(
            "a catalog offering none of {GATED_TOOLS:?} was let through, so a release that took \
             the flag and ignored it would deny every tool call with nothing said about it"
        );
    };
    let said = error.to_string();
    assert!(
        said.contains(PERMISSION_PROMPT_TOOL),
        "the failure does not name `{PERMISSION_PROMPT_TOOL}`: {said}"
    );
    assert!(
        said.contains("auto-denied"),
        "the failure does not say what goes wrong when the flag does nothing: {said}"
    );

    Ok(())
}

#[test]
fn the_two_catalogs_differ_by_the_tools_the_probe_reads_and_by_nothing_else() -> Result<()> {
    let (honoured, ignored) = (init(HONOURED)?, init(IGNORED)?);
    let (honoured, ignored) = (
        honoured.init().ok_or(anyhow!("an init frame"))?,
        ignored.init().ok_or(anyhow!("an init frame"))?,
    );

    assert_eq!(
        GATED_TOOLS.to_vec(),
        honoured
            .tools
            .iter()
            .filter(|tool| GATED_TOOLS.contains(&tool.as_str()))
            .map(String::as_str)
            .collect::<Vec<&str>>()
    );
    assert_eq!(
        Vec::<&str>::new(),
        ignored
            .tools
            .iter()
            .filter(|tool| GATED_TOOLS.contains(&tool.as_str()))
            .map(String::as_str)
            .collect::<Vec<&str>>()
    );
    assert_eq!(
        honoured.permission_mode, ignored.permission_mode,
        "the permission mode differs between the two, so the flag is not the only thing the \
         catalogs disagree about and the probe may be reading the wrong difference"
    );
    assert_eq!(honoured.capabilities, ignored.capabilities);
    assert_eq!(honoured.slash_commands, ignored.slash_commands);

    Ok(())
}

#[test]
fn a_release_that_renamed_one_gated_tool_is_still_talked_to() -> Result<()> {
    let frame = init(HONOURED)?;
    let mut announced = frame.init().ok_or(anyhow!("an init frame"))?.clone();
    announced.tools.retain(|tool| GATED_TOOLS[0] != tool);

    probe::honoured(&announced)?;

    Ok(())
}

#[test]
fn a_binary_that_refused_the_flag_is_reported_by_the_flags_own_name() -> Result<()> {
    let said = fs::read_to_string(REFUSED)?;

    let Error::FlagRejected { flag, detail } = probe::ending(&said) else {
        panic!("a binary that refused `{PERMISSION_PROMPT_TOOL}` was reported as a plain ending");
    };
    assert_eq!(PERMISSION_PROMPT_TOOL, flag);
    assert_eq!(said.trim(), detail);
    assert!(probe::ending(&said)
        .to_string()
        .contains(PERMISSION_PROMPT_TOOL));

    Ok(())
}

#[test]
fn an_ending_that_is_not_about_the_flag_is_not_blamed_on_it() -> Result<()> {
    let said = "Invalid API key. Please run /login.";

    assert!(
        matches!(probe::ending(said), Error::Ended { .. }),
        "an ending with nothing to do with the flag was reported as the flag being refused"
    );

    Ok(())
}

/// # Returns
///
/// The init frame a recording holds, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`std::fs::read_to_string`]'s return values on failure.
/// * Forwards [`Event::decoded`]'s return values on failure.
fn init(path: &str) -> Result<Event> {
    Ok(Event::decoded(fs::read_to_string(path)?.trim_end())?)
}
