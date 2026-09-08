//! A real Claude Code session, driven by the client, answering.
//!
//! These are the tests the stand-in's fidelity rests on. Everything `session_stub.rs` asserts is
//! asserted against a process that behaves the way this one was observed to behave, and the
//! observation is only worth what it was taken from, so it is taken again here: a `claude` on the
//! path, a login behind it, and a model at the end of it.
//!
//! That is also why they are ignored by default. They cost a model call and a network, and they
//! are red on a machine with no login rather than absent -- which is the point of ignoring them
//! rather than skipping them at run time. A gate that switched itself off where the binary is
//! missing would report a broken client as three quiet successes.
//!
//! Run them with:
//!
//! ```text
//! cargo nextest run -p vbc-editor --test session_live --run-ignored all --no-tests fail
//! ```
//!
//! They last passed against claude 2.1.263.

use std::time::Duration;

use anyhow::{anyhow, Result};
use vbc_editor::session::client::Client;
use vbc_editor::session::event::Event;
use vbc_editor::session::identity::{Identity, SessionId};
use vbc_editor::session::spawn::Spawn;

/// The model every one of these runs on, which is the cheapest one that answers.
const MODEL: &str = "haiku";

/// How long a turn is given. A model is at the end of it, so this is minutes rather than seconds.
const TURN: Duration = Duration::from_secs(180);

/// How long the child is given to exit once its input has been closed.
const ENDING: Duration = Duration::from_secs(30);

/// What a session is told in one turn and asked about in the next, and the answer that says the
/// second turn could see the first.
const TOLD: &str = "My favourite colour is teal. Just acknowledge it briefly.";
const ASKED: &str = "What is my favourite colour? Answer with the one word and nothing else.";
const ANSWER: &str = "teal";

#[test]
#[ignore = "starts a real Claude Code session, so it needs the binary, a login and a network"]
fn a_real_session_announces_itself_and_ends_its_turn() -> Result<()> {
    let identity = Identity::Fresh(SessionId::generated());
    let mut session = Client::start(&Spawn::new(identity.clone()).with_model(MODEL))?;

    let events = session.turn("Reply with the single word OK.", TURN)?;

    let init = events
        .iter()
        .find_map(Event::init)
        .ok_or(anyhow!("the session announced nothing"))?;
    assert_eq!(identity.session_id().as_str(), init.session_id);
    assert!(!init.version.is_empty());
    assert!(!init.tools.is_empty());

    let ended = events
        .last()
        .and_then(Event::turn)
        .ok_or(anyhow!("the turn did not end with a result"))?;
    assert_eq!("success", ended.subtype);
    assert!(!ended.failed, "the turn ended as {ended:?}");

    session.finish(ENDING)?;

    Ok(())
}

#[test]
#[ignore = "starts a real Claude Code session, so it needs the binary, a login and a network"]
fn a_second_turn_on_one_child_can_see_what_the_first_was_told() -> Result<()> {
    let mut session = Client::start(&Spawn::new(Identity::default()).with_model(MODEL))?;

    session.turn(TOLD, TURN)?;
    let events = session.turn(ASKED, TURN)?;
    let answered = ended(&events)?;

    assert!(
        answered.to_lowercase().contains(ANSWER),
        "a second turn could not see what the first was told, so the child is not one process \
         across turns; it answered: {answered}"
    );

    session.finish(ENDING)?;

    Ok(())
}

#[test]
#[ignore = "starts a real Claude Code session, so it needs the binary, a login and a network"]
fn a_session_is_resumed_by_its_own_name_and_forked_under_another() -> Result<()> {
    let started = SessionId::generated();
    let mut first = Client::start(&Spawn::new(Identity::Fresh(started.clone())).with_model(MODEL))?;
    first.turn(TOLD, TURN)?;
    first.finish(ENDING)?;

    let mut resumed =
        Client::start(&Spawn::new(Identity::Resumed(started.clone())).with_model(MODEL))?;
    let events = resumed.turn(ASKED, TURN)?;
    assert_eq!(
        started.as_str(),
        announced(&events)?,
        "a resumed session is running under a name it was not resumed by"
    );
    assert!(
        ended(&events)?.to_lowercase().contains(ANSWER),
        "a resumed session could not see what the session it resumed was told"
    );
    resumed.finish(ENDING)?;

    let mut forked =
        Client::start(&Spawn::new(Identity::Forked(started.clone())).with_model(MODEL))?;
    let events = forked.turn(ASKED, TURN)?;
    assert_ne!(
        started.as_str(),
        announced(&events)?,
        "a forked session is running under the name of the one it forked, so the history it was \
         given is the same history and a rewind would write over it"
    );
    assert!(
        ended(&events)?.to_lowercase().contains(ANSWER),
        "a forked session could not see what the session it forked was told"
    );
    forked.finish(ENDING)?;

    Ok(())
}

/// # Returns
///
/// The name a turn's session announced itself under, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`anyhow::Error`] if the turn announced nothing.
fn announced(events: &[Event]) -> Result<&str> {
    Ok(events
        .iter()
        .find_map(Event::init)
        .ok_or(anyhow!("the turn announced nothing"))?
        .session_id
        .as_str())
}

/// # Returns
///
/// What a turn ended with, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`anyhow::Error`] if the turn did not end with a result, or ended in a failure.
fn ended(events: &[Event]) -> Result<&str> {
    let turn = events
        .last()
        .and_then(Event::turn)
        .ok_or(anyhow!("the turn did not end with a result"))?;
    if turn.failed {
        return Err(anyhow!("the turn ended as {turn:?}"));
    }

    Ok(turn.text.as_str())
}
