//! The environment the child is handed, read off the child rather than off the spawn.
//!
//! vimbecode is often started from inside a Claude Code session, and such a session exports
//! variables the child reads as a claim to be part of it. Inheriting them silently disables the
//! child's transcript persistence: a completed multi-turn session leaves nothing on disk at all,
//! with no warning and no error, so the whole record of a conversation is lost to a variable
//! nobody set on purpose. One variable has to go the other way -- file checkpointing has no flag,
//! cannot be changed once the child is up, and is what a rewind depends on.
//!
//! Neither of those is a property of the spawn's own account of itself, so neither is asserted
//! against it. The stand-in writes down the environment it was actually handed, and what is read
//! here is that file. The variables the strip is required to remove are put into this process
//! first, so a strip that stopped happening is a test that goes red rather than one with nothing
//! left to catch.
//!
//! This is the only test in this binary, which is what makes putting variables into the process
//! safe: no other test is running beside it to read the environment while it is being written.

#![cfg(target_os = "linux")]

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use tempfile::TempDir;
use vbc_editor::session::client::Client;
use vbc_editor::session::identity::Identity;
use vbc_editor::session::spawn::{
    Spawn, CHECKPOINTING, CHECKPOINTING_VALUE, INHERITED_NAMES, INHERITED_PREFIX,
};

/// The stand-in, which writes the environment it was handed into the directory it was started in.
const STUB: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/session/stub.sh");
const ENVIRONMENT: &str = "env";

/// The variables put into this process, which stand for a vimbecode started from inside a Claude
/// Code session. The first is the one variable the strip has to take and then put back, held here
/// at the value that would turn checkpointing off, so that a strip applied after the setting --
/// which would hand the child the inherited value and leave a rewind with nothing to rewind to --
/// is a test that goes red. [`KEPT`] is not one of them at all.
const EXPORTED: [(&str, &str); 6] = [
    (CHECKPOINTING, "false"),
    ("CLAUDE_CODE_ENTRYPOINT", "cli"),
    (
        "CLAUDE_CODE_SESSION_ID",
        "3d1c0e2a-0000-4000-8000-000000000000",
    ),
    ("CLAUDECODE", "1"),
    ("CLAUDE_PID", "424242"),
    KEPT,
];

/// A variable of somebody else's, which is what says the strip takes the variables it is for
/// rather than everything it can reach.
const KEPT: (&str, &str) = ("VBC_KEPT", "a variable of somebody else's");

/// How long the child is given to write its environment down and answer a turn.
const TURN: Duration = Duration::from_secs(10);

#[test]
fn the_child_is_handed_no_session_of_ours_and_the_checkpointing_it_has_no_flag_for() -> Result<()> {
    for (key, value) in EXPORTED {
        std::env::set_var(key, value);
    }

    let directory = TempDir::new()?;
    let mut session = Client::start(
        &Spawn::new(Identity::default())
            .with_binary(STUB)
            .with_directory(directory.path()),
    )?;
    session.turn("hello", TURN)?;
    let handed = environment(directory.path())?;

    let inherited: Vec<&String> = handed
        .keys()
        .filter(|key| {
            CHECKPOINTING != key.as_str()
                && (key.starts_with(INHERITED_PREFIX) || INHERITED_NAMES.contains(&key.as_str()))
        })
        .collect();
    assert_eq!(
        Vec::<&String>::new(),
        inherited,
        "the child was handed variables of the session vimbecode is running inside, which is what \
         leaves a finished conversation with no transcript on disk"
    );

    assert_eq!(
        Some(&CHECKPOINTING_VALUE.to_owned()),
        handed.get(CHECKPOINTING),
        "the child was started without file checkpointing, which nothing can turn on afterwards"
    );

    assert_eq!(
        Some(&KEPT.1.to_owned()),
        handed.get(KEPT.0),
        "the strip took a variable that is none of its business"
    );

    Ok(())
}

/// # Returns
///
/// The environment the child was handed, as the child itself wrote it down, on success. A value
/// holding a newline of its own is read as far as that newline, which no variable this reads has.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`std::fs::read_to_string`]'s return values on failure.
fn environment(directory: &Path) -> Result<BTreeMap<String, String>> {
    Ok(fs::read_to_string(directory.join(ENVIRONMENT))?
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect())
}
