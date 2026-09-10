//! The round trips, driven end to end against a process that asks and then waits.
//!
//! What is stood in for here is the model and nothing else. A child of its own asks the questions,
//! over the same three pipes and in the same envelope the real binary uses, and it does what a
//! session does with the answer: it writes the file an approval let it write, it leaves unwritten
//! the one a refusal stopped, it names the refusal in its result frame, and it tells the model the
//! reader declined to answer when an approval carries no answers. Every assertion below is
//! therefore about the client's own frames, read by something that had to parse them.
//!
//! Two of these cannot be run against the real binary at all, which is the reason the stand-in
//! exists. claude 2.1.263 will not hold two questions open -- it asks about one parallel call,
//! waits for the answer, and asks about the next -- so the queue's whole point, that an answer
//! resolves the question it names rather than the question that arrived last, has nowhere to
//! happen. And an interrupt that keeps its queue is a session that carries on after the reader
//! stopped it, which is a thing worth having a test for and not a thing worth asking a real
//! session to do to somebody's machine.
//!
//! `session_control_live.rs` is the other half of this: the same round trips against a real
//! session, where what answers is a model rather than a script.

#![cfg(target_os = "linux")]

use std::fs;
use std::path::Path;
use std::time::Duration;

use anyhow::{anyhow, Result};
use serde_json::json;
use tempfile::TempDir;
use vbc_editor::session::client::Client;
use vbc_editor::session::control::{Answer, Ask, Decision, Subject};
use vbc_editor::session::error::Error;
use vbc_editor::session::event::{Denial, Event, Kind, Turn};
use vbc_editor::session::identity::Identity;
use vbc_editor::session::queue::Queue;
use vbc_editor::session::spawn::Spawn;

/// The stand-in the client is driven against.
const STUB: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/session/control.sh");

/// The files the stand-in is steered by, and the two it reports through.
const TWO: &str = "two";
const QUESTION: &str = "question";
const PLAN: &str = "plan";
const OTHER: &str = "other";
const AUTO: &str = "auto";
const INTERRUPT: &str = "interrupt";
const ANSWERED: &str = "answered";
const CANCEL: &str = "cancel";

/// How long a turn against a process on this machine is given. Nothing here waits on a model.
const TURN: Duration = Duration::from_secs(10);

/// How long a session that should have nothing left to say is listened to before it is believed.
const SILENCE: Duration = Duration::from_millis(500);

#[test]
fn a_call_the_reader_allowed_is_made_and_one_they_refused_is_not() -> Result<()> {
    let directory = TempDir::new()?;
    let mut session = Client::start(&spawn(directory.path()))?;
    let mut queue = Queue::new();

    let events = session.turn_answering("write it", TURN, &mut queue, |queue| {
        answering(queue, &Decision::Allowed)
    })?;

    assert_eq!(
        "hello\n",
        fs::read_to_string(directory.path().join("note.txt"))?,
        "the call the reader allowed was not made"
    );
    assert_eq!(&[] as &[Denial], ended(&events)?.denials.as_slice());
    assert_eq!(&[] as &[Ask], queue.outstanding());

    let refused = TempDir::new()?;
    let mut session = Client::start(&spawn(refused.path()))?;
    let events = session.turn_answering("write it", TURN, &mut queue, |queue| {
        answering(queue, &Decision::Denied("the reader said no".to_owned()))
    })?;

    assert!(
        !refused.path().join("note.txt").exists(),
        "the call the reader refused was made anyway"
    );
    assert_eq!(
        &[Denial {
            tool_name: "Write".to_owned(),
            tool_use_id: "toolu-req-write".to_owned(),
            input: json!({}),
        }],
        ended(&events)?.denials.as_slice(),
        "the refusal is named nowhere the reader could be shown it"
    );

    Ok(())
}

#[test]
fn an_approval_the_reader_changed_is_what_the_call_is_made_with() -> Result<()> {
    let directory = TempDir::new()?;
    let mut session = Client::start(&spawn(directory.path()))?;
    let mut queue = Queue::new();

    let changed =
        Decision::Changed(json!({"file_path": "note.txt", "content": "the reader's own"}));
    session.turn_answering("write it", TURN, &mut queue, |queue| {
        answering(queue, &changed)
    })?;

    assert_eq!(
        "the reader's own\n",
        fs::read_to_string(directory.path().join("note.txt"))?,
        "the call was made with the input it asked with rather than the one it was answered with"
    );

    Ok(())
}

#[test]
fn two_questions_are_answered_out_of_order_and_each_resolves_the_one_it_names() -> Result<()> {
    let directory = TempDir::new()?;
    fs::write(directory.path().join(TWO), "")?;
    let mut session = Client::start(&spawn(directory.path()))?;
    let mut queue = Queue::new();
    let mut order: Vec<String> = Vec::new();

    let events = session.turn_answering("write both", TURN, &mut queue, |queue| {
        let answering = match (queue.len(), order.as_slice()) {
            (2, _) => queue.outstanding().last(),
            (1, [_first]) => queue.oldest(),
            _ => None,
        };
        let Some(ask) = answering else {
            return Vec::new();
        };
        order.push(ask.request_id().to_owned());

        vec![ask.answer(&Decision::Allowed)]
    })?;

    assert_eq!(
        vec!["req-second", "req-first"],
        order,
        "answering the newer question left the older one held under the newer one's name, so the \
         queue is a stack rather than a set of questions each answerable on its own"
    );
    assert_eq!(
        vec!["req-second allow", "req-first allow"],
        fs::read_to_string(directory.path().join(ANSWERED))?
            .lines()
            .collect::<Vec<&str>>(),
        "the answers did not reach the session in the order the reader gave them"
    );
    assert_eq!(
        "first\n",
        fs::read_to_string(directory.path().join("first.txt"))?,
        "the older question was resolved by the answer to the newer one"
    );
    assert_eq!(
        "second\n",
        fs::read_to_string(directory.path().join("second.txt"))?
    );
    assert_eq!(&[] as &[Ask], queue.outstanding());
    assert_eq!(&[] as &[Denial], ended(&events)?.denials.as_slice());

    Ok(())
}

#[test]
fn a_plan_is_read_as_a_plan_and_approving_it_is_answered_by_a_change_of_mode() -> Result<()> {
    let directory = TempDir::new()?;
    fs::write(directory.path().join(PLAN), "")?;
    let mut session = Client::start(&spawn(directory.path()))?;
    let mut queue = Queue::new();
    let mut asked = Vec::new();

    let events = session.turn_answering("plan it", TURN, &mut queue, |queue| {
        asked.extend(
            queue
                .outstanding()
                .iter()
                .map(|ask| (ask.subject(), ask.interactive())),
        );

        answering(queue, &Decision::Allowed)
    })?;

    assert_eq!(
        vec![(
            Subject::Plan("# Plan\n\n- read the file\n- write it back".to_owned()),
            true
        )],
        asked,
        "the plan a session handed over was read as a tool call like any other"
    );
    assert_eq!(
        vec![&Kind::System("status".to_owned())],
        events
            .iter()
            .map(Event::kind)
            .filter(|kind| matches!(kind, Kind::System(_)))
            .collect::<Vec<&Kind>>(),
        "approving the plan announced no change of mode, which is the only account there is of one"
    );

    Ok(())
}

#[test]
fn a_control_request_that_is_not_a_question_is_not_read_as_one() -> Result<()> {
    let directory = TempDir::new()?;
    fs::write(directory.path().join(OTHER), "")?;
    let mut session = Client::start(&spawn(directory.path()))?;
    let mut queue = Queue::new();

    let events = session.turn("do it", TURN)?;

    assert_eq!(
        0,
        queue.read_all(&events),
        "a control request that asks the reader nothing was queued as a question, so the reader \
         would be shown a prompt nobody wrote and the session an answer to a question it never \
         asked"
    );
    assert_eq!("nothing was asked of the reader", ended(&events)?.text);

    Ok(())
}

#[test]
fn a_question_is_answered_through_the_input_and_approving_it_answers_nothing() -> Result<()> {
    let approved = TempDir::new()?;
    fs::write(approved.path().join(QUESTION), "")?;
    let mut session = Client::start(&spawn(approved.path()))?;
    let mut queue = Queue::new();

    let events = session.turn_answering("ask me", TURN, &mut queue, |queue| {
        answering(queue, &Decision::Allowed)
    })?;

    assert_eq!(
        "unanswered",
        ended(&events)?.text,
        "a bare approval was read as an answer, so a reader who answered would be told they had \
         not"
    );

    let answered = TempDir::new()?;
    fs::write(answered.path().join(QUESTION), "")?;
    let mut session = Client::start(&spawn(answered.path()))?;
    let mut queue = Queue::new();
    let mut asked = Vec::new();

    let events = session.turn_answering("ask me", TURN, &mut queue, |queue| {
        queue
            .outstanding()
            .iter()
            .map(|ask| {
                asked.push(ask.subject());
                ask.answer(&Decision::answering(&question(ask), "tea"))
            })
            .collect()
    })?;

    assert_eq!("answered:tea", ended(&events)?.text);
    assert_eq!(
        vec!["Tea or coffee?"],
        asked
            .iter()
            .flat_map(|subject| match subject {
                Subject::Questions(questions) => questions.clone(),
                _ => Vec::new(),
            })
            .map(|question| question.question)
            .collect::<Vec<String>>(),
        "the question the session asked was not read as a question"
    );

    Ok(())
}

#[test]
fn a_turn_that_is_asked_nothing_waits_for_nothing() -> Result<()> {
    let directory = TempDir::new()?;
    fs::write(directory.path().join(AUTO), "")?;
    let mut session = Client::start(&spawn(directory.path()))?;
    let mut queue = Queue::new();

    let events = session.turn("say something", TURN)?;

    assert_eq!(
        0,
        queue.read_all(&events),
        "a turn that asked nothing was read as asking something"
    );
    assert_eq!("no question was asked", ended(&events)?.text);

    Ok(())
}

#[test]
fn an_interrupt_stops_the_turn_and_leaves_nothing_to_drain() -> Result<()> {
    let directory = TempDir::new()?;
    fs::write(directory.path().join(INTERRUPT), "")?;
    let mut session = Client::start(&spawn(directory.path()))?;

    session.ask("work on it")?;
    session.next(TURN)?;
    session.next(TURN)?;
    session.ask("and then this")?;
    session.ask("and then this too")?;

    let mut passed = Vec::new();
    let receipt = session.interrupt(TURN, &mut passed)?;

    assert_eq!(Vec::<serde_json::Value>::new(), receipt.still_queued);
    assert_eq!(
        Some(Vec::new()),
        receipt.cancelled,
        "the receipt does not say the queue went with the turn, which is what a session that was \
         asked to keep it answers"
    );
    assert_eq!(
        "true\n",
        fs::read_to_string(directory.path().join(CANCEL))?,
        "the interrupt let the session keep its queue"
    );
    assert_eq!(
        &[] as &[Event],
        passed.as_slice(),
        "the receipt was not the first thing the session said after being stopped"
    );

    let stopped = session.next(TURN)?;
    assert_eq!(
        Some("error_during_execution"),
        stopped.turn().map(|turn| turn.subtype.as_str()),
        "the stopped turn ended as though it had finished"
    );

    let Err(error) = session.next(SILENCE) else {
        panic!(
            "the session took another turn after it was stopped, so what the reader queued before \
             pressing stop ran after they pressed it"
        );
    };
    assert!(
        matches!(error, Error::Silent { .. }),
        "the session said {error} after it was stopped"
    );

    Ok(())
}

/// # Returns
///
/// A spawn of the stand-in, in a directory of its own.
fn spawn(directory: &Path) -> Spawn {
    Spawn::new(Identity::default())
        .with_binary(STUB)
        .with_directory(directory)
}

/// # Returns
///
/// The same decision for everything the session is waiting on, which is what a reader who answers
/// as fast as they are asked amounts to.
fn answering(queue: &Queue, decision: &Decision) -> Vec<Answer> {
    queue
        .outstanding()
        .iter()
        .map(|ask| ask.answer(decision))
        .collect()
}

/// # Returns
///
/// The text of the first question a request carries, which is the key its answer is written under.
///
/// # Panics
///
/// Panics if the request carries no question, which would mean the case is answering something
/// other than what it asked to be asked.
fn question(ask: &Ask) -> String {
    let Subject::Questions(questions) = ask.subject() else {
        panic!("`{}` was not read as a question", ask.tool());
    };

    questions
        .first()
        .expect("a question carries at least one question")
        .question
        .clone()
}

/// # Returns
///
/// How a turn ended, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`anyhow::Error`] if the turn did not end with a result.
fn ended(events: &[Event]) -> Result<&Turn> {
    events
        .last()
        .and_then(Event::turn)
        .ok_or(anyhow!("the turn did not end with a result"))
}
