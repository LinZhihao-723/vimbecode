//! The client driven end to end against a process, without a model behind it.
//!
//! Everything between the spawn and the typed event is real here: a child of its own, three pipes,
//! NDJSON written to one and read off another, and a stream that ends when the child does. What is
//! stood in for is only the part that needs a network and a login, so the two failures that matter
//! most can be produced on demand -- a binary that refuses the permission flag, and one that takes
//! it and does nothing about it -- neither of which any amount of waiting on the real binary would
//! ever show us.
//!
//! The stand-in is steered through the directory it is started in and reports through it too, so
//! the tests reach it entirely through the client's own interface and read what the child was
//! handed rather than what the spawn believes it handed over.

#![cfg(target_os = "linux")]

use std::fs;
use std::path::Path;
use std::time::Duration;

use anyhow::{anyhow, Result};
use tempfile::TempDir;
use vbc_editor::session::client::Client;
use vbc_editor::session::error::Error;
use vbc_editor::session::identity::{Identity, SessionId};
use vbc_editor::session::probe::{PERMISSION_PROMPT_TOOL, PERMISSION_PROMPT_TOOL_VALUE};
use vbc_editor::session::spawn::Spawn;

/// The stand-in the client is driven against.
const STUB: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/session/stub.sh");

/// The files the stand-in is steered by and the ones it reports through, all in the directory it
/// was started in.
const REJECT: &str = "reject";
const DROP: &str = "drop";
const HOOKED: &str = "hooked";
const ARGUMENTS: &str = "args";

/// The session the stand-in answers a forked spawn under, which is written into it rather than
/// derived from what it was resumed from.
const FORKED: &str = "11111111-1111-4111-8111-111111111111";

/// A session identifier a test chose, so that what the child was started with can be read against
/// something.
const CHOSEN: &str = "550e8400-e29b-41d4-a716-446655440000";

/// How long a turn against a process on this machine is given. Nothing here waits on a model, so
/// this is generous rather than tuned.
const TURN: Duration = Duration::from_secs(10);

#[test]
fn a_session_answers_a_turn_and_says_which_session_answered() -> Result<()> {
    let directory = TempDir::new()?;
    let mut session = Client::start(&spawn(directory.path(), Identity::Fresh(chosen())))?;

    let events = session.turn("hello", TURN)?;

    let init = events
        .iter()
        .find_map(|event| event.init())
        .ok_or(anyhow!("the turn announced no session"))?;
    assert_eq!(CHOSEN, init.session_id);
    assert_eq!("stub", init.version);

    let ended = events
        .last()
        .and_then(|event| event.turn())
        .ok_or(anyhow!("the turn did not end with a result"))?;
    assert_eq!("success", ended.subtype);
    assert!(!ended.failed);
    assert_eq!("turn 1", ended.text);

    Ok(())
}

#[test]
fn one_child_takes_every_turn_asked_of_it() -> Result<()> {
    let directory = TempDir::new()?;
    let mut session = Client::start(&spawn(directory.path(), Identity::Fresh(chosen())))?;

    let counted: Vec<u64> = (0..3)
        .map(|_| -> Result<u64> {
            let events = session.turn("again", TURN)?;
            Ok(events
                .last()
                .and_then(|event| event.turn())
                .ok_or(anyhow!("the turn did not end with a result"))?
                .turns)
        })
        .collect::<Result<Vec<u64>>>()?;

    assert_eq!(
        vec![1, 2, 3],
        counted,
        "the turns were not counted by one process, so a turn is costing a spawn"
    );

    Ok(())
}

#[test]
fn a_session_that_took_the_flag_and_ignored_it_fails_the_first_turn_by_the_flags_name() -> Result<()>
{
    let directory = TempDir::new()?;
    fs::write(directory.path().join(DROP), "")?;
    let mut session = Client::start(&spawn(directory.path(), Identity::Fresh(chosen())))?;

    let Err(error) = session.turn("hello", TURN) else {
        panic!(
            "a session that ignored `{PERMISSION_PROMPT_TOOL}` answered a turn, so every tool \
             call needing approval would be denied with nothing said about it"
        );
    };

    assert!(matches!(error, Error::FlagIgnored { .. }));
    assert!(
        error.to_string().contains(PERMISSION_PROMPT_TOOL),
        "the failure does not name the flag: {error}"
    );

    Ok(())
}

#[test]
fn a_binary_that_refused_the_flag_fails_by_the_flags_name() -> Result<()> {
    let directory = TempDir::new()?;
    fs::write(directory.path().join(REJECT), "")?;
    let mut session = Client::start(&spawn(directory.path(), Identity::Fresh(chosen())))?;

    let Err(error) = session.turn("hello", TURN) else {
        panic!("a binary that refused `{PERMISSION_PROMPT_TOOL}` answered a turn");
    };

    assert!(matches!(error, Error::FlagRejected { .. }));
    assert!(
        error.to_string().contains(PERMISSION_PROMPT_TOOL),
        "the failure does not name the flag: {error}"
    );

    Ok(())
}

#[test]
fn a_session_that_speaks_before_it_announces_itself_is_still_read_and_still_probed() -> Result<()> {
    let answered = TempDir::new()?;
    fs::write(answered.path().join(HOOKED), "")?;
    let mut session = Client::start(&spawn(answered.path(), Identity::Fresh(chosen())))?;

    let events = session.turn("hello", TURN)?;

    assert_eq!(
        Some(CHOSEN),
        events
            .iter()
            .find_map(|event| event.init())
            .map(|init| init.session_id.as_str()),
        "a session that spoke before announcing itself was read as announcing nothing"
    );

    let denied = TempDir::new()?;
    fs::write(denied.path().join(HOOKED), "")?;
    fs::write(denied.path().join(DROP), "")?;
    let mut session = Client::start(&spawn(denied.path(), Identity::Fresh(chosen())))?;

    let Err(error) = session.turn("hello", TURN) else {
        panic!(
            "a session that ignored `{PERMISSION_PROMPT_TOOL}` answered a turn because it spoke \
             before it announced itself, so the probe reads whichever frame arrives first rather \
             than the catalog, and a machine that configures a session hook would deny every tool \
             call with nothing said about it"
        );
    };
    assert!(matches!(error, Error::FlagIgnored { .. }));

    Ok(())
}

#[test]
fn a_child_that_says_nothing_is_reported_as_saying_nothing() -> Result<()> {
    let directory = TempDir::new()?;
    let mut session = Client::start(&spawn(directory.path(), Identity::Fresh(chosen())))?;

    let waited = Duration::from_millis(200);
    let Err(error) = session.next(waited) else {
        panic!("a child that had been asked nothing answered something");
    };

    assert!(
        matches!(error, Error::Silent { waited: given } if given == waited),
        "a silent child was reported as {error}"
    );

    Ok(())
}

#[test]
fn the_child_is_started_on_the_stream_protocol_with_the_permission_flag() -> Result<()> {
    let directory = TempDir::new()?;
    let mut session = Client::start(&spawn(directory.path(), Identity::Fresh(chosen())))?;
    session.turn("hello", TURN)?;

    assert_eq!(
        vec![
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            PERMISSION_PROMPT_TOOL,
            PERMISSION_PROMPT_TOOL_VALUE,
            "--model",
            "stub-model",
            "--session-id",
            CHOSEN,
        ],
        arguments(directory.path())?
    );

    Ok(())
}

#[test]
fn a_resumed_session_is_continued_and_a_forked_one_is_answered_under_a_new_name() -> Result<()> {
    let resumed = TempDir::new()?;
    let mut session = Client::start(&spawn(resumed.path(), Identity::Resumed(chosen())))?;
    let events = session.turn("hello", TURN)?;

    let started = arguments(resumed.path())?;
    assert_eq!(
        &["--resume".to_owned(), CHOSEN.to_owned()],
        &started[started.len() - 2..]
    );
    assert_eq!(
        Some(CHOSEN),
        events
            .iter()
            .find_map(|event| event.init())
            .map(|init| init.session_id.as_str())
    );

    let forked = TempDir::new()?;
    let mut session = Client::start(&spawn(forked.path(), Identity::Forked(chosen())))?;
    let events = session.turn("hello", TURN)?;

    let started = arguments(forked.path())?;
    assert_eq!(
        &[
            "--resume".to_owned(),
            CHOSEN.to_owned(),
            "--fork-session".to_owned()
        ],
        &started[started.len() - 3..]
    );
    assert_eq!(
        Some(FORKED),
        events
            .iter()
            .find_map(|event| event.init())
            .map(|init| init.session_id.as_str()),
        "a forked session was read as running under the identifier it was resumed from"
    );

    Ok(())
}

#[test]
fn a_session_whose_input_is_closed_ends() -> Result<()> {
    let directory = TempDir::new()?;
    let mut session = Client::start(&spawn(directory.path(), Identity::Fresh(chosen())))?;
    session.turn("hello", TURN)?;

    session.finish(TURN)?;
    let Err(error) = session.next(TURN) else {
        panic!("a session that had ended went on answering");
    };

    assert!(matches!(error, Error::Ended { .. }), "it ended as {error}");

    Ok(())
}

/// # Returns
///
/// A spawn of the stand-in, in a directory of its own and on a named model so that what the child
/// was started with can be read in full.
fn spawn(directory: &Path, identity: Identity) -> Spawn {
    Spawn::new(identity)
        .with_binary(STUB)
        .with_directory(directory)
        .with_model("stub-model")
}

/// # Returns
///
/// The identifier every test that names one uses.
fn chosen() -> SessionId {
    SessionId::known(CHOSEN)
}

/// # Returns
///
/// Every argument the child was started with, as the child itself wrote them down, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`std::fs::read_to_string`]'s return values on failure.
fn arguments(directory: &Path) -> Result<Vec<String>> {
    Ok(fs::read_to_string(directory.join(ARGUMENTS))?
        .lines()
        .map(str::to_owned)
        .collect())
}
