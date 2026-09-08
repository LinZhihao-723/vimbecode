//! The check that the flag every tool approval goes through is still doing something.
//!
//! `--permission-prompt-tool stdio` is undocumented. It is absent from `claude --help`; it was
//! found in the Agent SDK's shipped source, which passes it whenever a caller registers a
//! permission callback, and confirmed against the bare binary. Without it every tool call needing
//! approval is auto-denied by a one-way notification with no way to answer, under
//! `--permission-mode manual` as much as under any other, so the flag and not the mode is the
//! gate. A release that dropped it would leave vimbecode running, connected, answering -- and
//! denying every edit, with nothing in the protocol to say why.
//!
//! There are two ways it can stop working and they fail differently, so they are checked
//! differently. A binary that no longer knows the flag refuses to start at all and says so on its
//! standard error, which is what [`ending`] reads. A binary that takes the flag and ignores it
//! starts normally, and the only account of it is the tool catalog in the init frame: the tools
//! that cannot be used without asking somebody -- [`GATED_TOOLS`] -- are offered when the flag is
//! honoured and are absent from the very same catalog when it is not. That difference was measured
//! against claude 2.1.263 rather than reasoned about, and the two catalogs it was measured from
//! are what [`honoured`] is tested against.
//!
//! What this cannot see is a release that goes on advertising those tools while denying them, and
//! no reading of the init frame could: the flag would have to be caught failing at the round trip
//! it exists for, which is the first permission request a session is asked to answer. Until that
//! round trip is implemented, this is the loud failure and that one is the gap.

use super::error::Error;
use super::event::Init;

/// The flag every tool approval goes through, and the value that routes it over the pipes this
/// client already owns.
pub const PERMISSION_PROMPT_TOOL: &str = "--permission-prompt-tool";
pub const PERMISSION_PROMPT_TOOL_VALUE: &str = "stdio";

/// The tools a session cannot offer without somewhere to ask a question, which is why a catalog
/// holding none of them is a session whose permission flag did nothing.
pub const GATED_TOOLS: [&str; 3] = ["AskUserQuestion", "EnterPlanMode", "ExitPlanMode"];

/// Checks that a session took the permission flag seriously.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`Error::FlagIgnored`] if the session's tool catalog holds none of [`GATED_TOOLS`], which is
///   what a catalog looks like when the flag was accepted and had no effect. A catalog that holds
///   any of them has somewhere to ask, so a release that renames one of the three is not a session
///   this refuses to talk to.
pub fn honoured(init: &Init) -> Result<(), Error> {
    let missing: Vec<String> = GATED_TOOLS
        .iter()
        .filter(|gated| !init.tools.iter().any(|tool| tool == *gated))
        .map(|gated| (*gated).to_owned())
        .collect();
    if missing.len() < GATED_TOOLS.len() {
        return Ok(());
    }

    Err(Error::FlagIgnored {
        flag: PERMISSION_PROMPT_TOOL.to_owned(),
        missing,
    })
}

/// # Returns
///
/// The error a session whose child has ended is reported as, which is the flag being refused where
/// what the child wrote on its way out names the flag, and the ending itself otherwise.
#[must_use]
pub fn ending(stderr: &str) -> Error {
    if stderr.contains(PERMISSION_PROMPT_TOOL) {
        return Error::FlagRejected {
            flag: PERMISSION_PROMPT_TOOL.to_owned(),
            detail: stderr.trim().to_owned(),
        };
    }

    Error::Ended {
        detail: stderr.trim().to_owned(),
    }
}
