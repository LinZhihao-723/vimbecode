//! The round trips against a real Claude Code session, which is what the stand-in stands in for.
//!
//! `session_control.rs` drives the same client against a script, and everything that script does
//! it does because a real session was measured doing it. This is where the measurement is taken
//! again: a `claude` on the path, a login behind it, a model at the end of it, and a temporary
//! directory that a turn either writes a file into or does not.
//!
//! Two of these are about what a session does when the answer is the wrong shape rather than about
//! what it does when it is right. A question answered by approving it is answered wrongly, and the
//! session says so in words -- so the wrong shape is proven wrong here rather than assumed, and
//! the right one is not a guess that happens to work. An interrupt that keeps its queue is the
//! same kind of fact, and the client has no way to send one, so what is asserted is the receipt
//! that says the queue went with the turn and the silence afterwards that says it did.
//!
//! One of them exists to be red one day. claude 2.1.263 asks about one tool call at a time even
//! where it then runs two together, so a queue never holds two questions against this release; the
//! depth is asserted exactly, so a release that starts asking about both at once fails this case
//! rather than quietly making the queue's whole reason for existing untested.
//!
//! They are ignored by default because they cost a model call and a network, and ignored rather
//! than skipped at run time because a case that skipped itself where the binary is missing would
//! report a broken client as a run of quiet successes.
//!
//! Run them with:
//!
//! ```text
//! cargo nextest run -p vbc-editor --test session_control_live --run-ignored all --no-tests fail
//! ```
//!
//! They last passed against claude 2.1.263.

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use serde_json::Value;
use tempfile::TempDir;
use vbc_editor::session::client::Client;
use vbc_editor::session::control::{Answer, Decision, Subject};
use vbc_editor::session::error::Error;
use vbc_editor::session::event::{Event, Turn};
use vbc_editor::session::identity::Identity;
use vbc_editor::session::queue::Queue;
use vbc_editor::session::spawn::Spawn;

/// The model every one of these runs on, which is the cheapest one that answers.
const MODEL: &str = "haiku";

/// How long a turn is given. A model is at the end of it, so this is minutes rather than seconds.
const TURN: Duration = Duration::from_secs(180);

/// How long the child is given to exit once its input has been closed.
const ENDING: Duration = Duration::from_secs(30);

/// The file a turn is asked to write, and what it is asked to write into it.
const FILE: &str = "note.txt";
const WRITTEN: &str = "hello";

/// What a session is asked to write a file with. The tool is named because the point of the case
/// is the round trip a write goes through, and stopping is asked for so that a refusal ends the
/// turn instead of being worked around.
const WRITE_ASKED: &str = "Create a file called note.txt in the current directory whose only line \
                           is hello, using the Write tool. If you are not allowed to, stop and \
                           say STOPPED.";

/// What a session is asked to ask with, and the answer that is given back to it.
const QUESTION_ASKED: &str = "Use the AskUserQuestion tool to ask me one question: whether I \
                              prefer tea or coffee. Once you have my answer, reply with exactly \
                              ANSWER=<what I chose> and nothing else.";
const CHOSEN: &str = "tea";
const ANSWERED: &str = "ANSWER=tea";

/// What claude 2.1.263 puts in the tool result of a question that was approved rather than
/// answered. It is matched loosely because what is being asserted is that the session reported the
/// reader as not having answered, not the wording it reported it in.
const UNANSWERED: &str = "did not answer";

/// What a session is asked to run a command with, and the word it writes. `printf` is approved by
/// the binary's own classifier, so the turn asks nothing.
const COMMAND_ASKED: &str = "Run exactly this shell command with the Bash tool and then say DONE: \
                             printf 'red plain\\n'";
const COMMAND_DONE: &str = "DONE";

/// What a session is asked to make two calls with at once.
const BOTH_ASKED: &str = "In ONE message, make two parallel Write tool calls: first.txt whose \
                          only line is first, and second.txt whose only line is second. Then say \
                          DONE.";
const BOTH: [&str; 2] = ["first.txt", "second.txt"];

/// What a session is asked in a turn that is meant to be stopped part way through, and what is
/// queued behind it so that there is something to drain.
const LONG_ASKED: &str = "Write a 12000-word essay about the sea. It must be at least 12000 \
                          words. Take your time and be thorough.";
const QUEUED: [&str; 2] = [
    "Say the word one and nothing else.",
    "Say the word two and nothing else.",
];

/// How long a turn is left running before it is stopped, which is long enough to be past the
/// session's own start-up and far short of the answer it was asked for. The answer is asked to be
/// long enough that a fast model cannot have finished it: a turn that ended on its own is a case
/// that stopped nothing, and it fails rather than passing quietly.
const RUNNING: Duration = Duration::from_secs(6);

/// What a session writes into its own history where a turn was stopped rather than finished.
const INTERRUPTED: &str = "[Request interrupted by user]";

/// How long a session that has been stopped is listened to before its silence is believed. The
/// queue a session was not asked to cancel drains about twenty milliseconds after the receipt, so
/// this is three orders of magnitude more than a drain needs.
const SILENCE: Duration = Duration::from_secs(20);

/// Validation 1: a real session asks before it writes, the answer decides whether it does, and a
/// refusal is named in the result frame.
#[test]
#[ignore = "starts a real Claude Code session, so it needs the binary, a login and a network"]
fn a_real_session_writes_what_the_reader_allowed_and_not_what_they_refused() -> Result<()> {
    let allowed = TempDir::new()?;
    let mut session = Client::start(&spawn(allowed.path()))?;
    let mut queue = Queue::new();
    let mut asked = Vec::new();

    session.turn_answering(WRITE_ASKED, TURN, &mut queue, |queue| {
        asked.extend(queue.outstanding().iter().map(|ask| ask.tool().to_owned()));
        answering(queue, &Decision::Allowed)
    })?;
    session.finish(ENDING)?;

    assert!(
        asked.iter().any(|tool| "Write" == tool),
        "the session wrote a file without asking, so nothing was answered; it asked about {asked:?}"
    );
    assert_eq!(
        WRITTEN,
        fs::read_to_string(allowed.path().join(FILE))?.trim_end(),
        "the write the reader allowed did not happen"
    );

    let refused = TempDir::new()?;
    let mut session = Client::start(&spawn(refused.path()))?;
    let mut queue = Queue::new();

    let events = session.turn_answering(WRITE_ASKED, TURN, &mut queue, |queue| {
        answering(queue, &Decision::Denied("the reader said no".to_owned()))
    })?;
    session.finish(ENDING)?;

    assert!(
        !refused.path().join(FILE).exists(),
        "the write the reader refused happened anyway"
    );
    let denied: Vec<&str> = ended(&events)?
        .denials
        .iter()
        .map(|denial| denial.tool_name.as_str())
        .collect();
    assert!(
        denied.contains(&"Write"),
        "the refusal is named nowhere the reader could be shown it; the turn names {denied:?}"
    );

    Ok(())
}

/// Validation 2: a question is answered by what the approval carries, and approving it on its own
/// is the session being told the reader would not answer.
#[test]
#[ignore = "starts a real Claude Code session, so it needs the binary, a login and a network"]
fn a_real_question_is_answered_by_the_input_and_not_by_the_approval() -> Result<()> {
    let approved = TempDir::new()?;
    let mut session = Client::start(&spawn(approved.path()))?;
    let mut queue = Queue::new();

    let events = session.turn_answering(QUESTION_ASKED, TURN, &mut queue, |queue| {
        answering(queue, &Decision::Allowed)
    })?;
    session.finish(ENDING)?;

    assert!(
        said(&events).to_lowercase().contains(UNANSWERED),
        "a bare approval was taken as an answer, so the shape the answer is sent in is being \
         assumed rather than required; the turn said: {}",
        said(&events)
    );

    let answered = TempDir::new()?;
    let mut session = Client::start(&spawn(answered.path()))?;
    let mut queue = Queue::new();

    let events = session.turn_answering(QUESTION_ASKED, TURN, &mut queue, |queue| {
        queue
            .outstanding()
            .iter()
            .map(|ask| {
                let Subject::Questions(questions) = ask.subject() else {
                    return ask.answer(&Decision::Allowed);
                };
                let asked = questions
                    .first()
                    .map(|question| question.question.clone())
                    .unwrap_or_default();

                ask.answer(&Decision::answering(&asked, CHOSEN))
            })
            .collect()
    })?;
    session.finish(ENDING)?;

    let ending = &ended(&events)?.text;
    assert!(
        ending.contains(ANSWERED),
        "the session did not read the answer the reader gave; it ended with: {ending}"
    );

    Ok(())
}

/// Validation 3: an interrupt aborts the turn in flight, its receipt is read off the stream ahead
/// of the aborted turn's own result, and what was queued behind it does not run afterwards.
#[test]
#[ignore = "starts a real Claude Code session, so it needs the binary, a login and a network"]
fn an_interrupt_aborts_a_real_turn_and_the_queue_behind_it() -> Result<()> {
    let directory = TempDir::new()?;
    let mut session = Client::start(&spawn(directory.path()))?;

    session.ask(LONG_ASKED)?;
    let running = Instant::now();
    while running.elapsed() < RUNNING {
        let event = session.next(TURN)?;
        if let Some(turn) = event.turn() {
            return Err(anyhow!(
                "the turn ended in {:?}, before there was anything to stop: {turn:?}",
                running.elapsed()
            ));
        }
    }
    for queued in QUEUED {
        session.ask(queued)?;
    }

    let mut passed = Vec::new();
    let receipt = session.interrupt(TURN, &mut passed)?;

    assert_eq!(
        Vec::<Value>::new(),
        receipt.still_queued,
        "claude 2.1.263 folds what is queued into the running turn, so it reports none of it as \
         still queued; a release that reports it is a release whose queue this client can show"
    );
    assert_eq!(
        Some(Vec::new()),
        receipt.cancelled,
        "the receipt does not say the queue went with the turn, which is what a session answers \
         when it was asked to keep it"
    );

    let stopped = read_result(&mut session, TURN, &mut passed)?;
    assert_ne!(
        "success", stopped.subtype,
        "the stopped turn ended as though it had finished, so nothing was aborted: {stopped:?}"
    );
    assert!(
        said(&passed).contains(INTERRUPTED),
        "the session's own history does not say the turn was stopped"
    );

    let mut drained = Vec::new();
    let Err(error) = read_result(&mut session, SILENCE, &mut drained) else {
        panic!(
            "the session took another turn after it was stopped, so what the reader queued before \
             pressing stop ran after they pressed it"
        );
    };
    assert!(
        matches!(error.downcast_ref::<Error>(), Some(Error::Silent { .. })),
        "the session said {error} after it was stopped"
    );

    Ok(())
}

/// Validation 5: a call the binary's own classifier approves round-trips through nothing, so a
/// reader is never shown a question that was not asked and a turn is never waited on for one.
#[test]
#[ignore = "starts a real Claude Code session, so it needs the binary, a login and a network"]
fn a_call_the_binary_approves_for_itself_asks_the_reader_nothing() -> Result<()> {
    let directory = TempDir::new()?;
    let mut session = Client::start(&spawn(directory.path()))?;
    let mut queue = Queue::new();
    let mut asked = Vec::new();

    let events = session.turn_answering(COMMAND_ASKED, TURN, &mut queue, |queue| {
        asked.extend(queue.outstanding().iter().map(|ask| ask.tool().to_owned()));
        answering(queue, &Decision::Allowed)
    })?;
    session.finish(ENDING)?;

    assert_eq!(
        Vec::<String>::new(),
        asked,
        "a call the binary approves for itself was put to the reader, so a session waiting on \
         nothing would be drawn as a session waiting"
    );
    assert!(
        said(&events).contains(COMMAND_DONE),
        "the session did not run the command, so this case waited on nothing for the wrong reason"
    );

    Ok(())
}

/// Validation 4, as far as a real session goes: the queue is asserted never to have held two
/// questions, because claude 2.1.263 asks about one call at a time even where it then runs both
/// together. That is a fact about a release rather than about the protocol, and it is why the
/// answering-out-of-order case lives against the stand-in; a release that asks about both at once
/// fails this assertion, which is where the two cases meet.
#[test]
#[ignore = "starts a real Claude Code session, so it needs the binary, a login and a network"]
fn a_real_session_asks_about_one_call_at_a_time() -> Result<()> {
    let directory = TempDir::new()?;
    let mut session = Client::start(&spawn(directory.path()))?;
    let mut queue = Queue::new();
    let mut asked = 0_usize;
    let mut deepest = 0_usize;

    session.turn_answering(BOTH_ASKED, TURN, &mut queue, |queue| {
        asked += queue.len();
        deepest = deepest.max(queue.len());

        answering(queue, &Decision::Allowed)
    })?;
    session.finish(ENDING)?;

    for written in BOTH {
        assert!(
            directory.path().join(written).exists(),
            "the session did not write {written}, so this case counted the questions of a turn \
             that did something else"
        );
    }
    assert_eq!(
        (2, 1),
        (asked, deepest),
        "the session asked about the two calls in a way this release was not measured doing; the \
         queue held {deepest} question(s) at once"
    );

    Ok(())
}

/// # Returns
///
/// A spawn of a real session, in a directory of its own and on the cheapest model that answers.
fn spawn(directory: &Path) -> Spawn {
    Spawn::new(Identity::default())
        .with_directory(directory)
        .with_model(MODEL)
}

/// # Returns
///
/// The same decision for everything the session is waiting on.
fn answering(queue: &Queue, decision: &Decision) -> Vec<Answer> {
    queue
        .outstanding()
        .iter()
        .map(|ask| ask.answer(decision))
        .collect()
}

/// # Returns
///
/// Everything a turn's frames say, as one text. It is the whole of the raw frames rather than the
/// prose because what some of these cases read is what the session told the model about the
/// reader, which arrives as a tool result rather than as anything anybody said.
fn said(events: &[Event]) -> String {
    events
        .iter()
        .map(|event| event.raw().to_string())
        .collect::<Vec<String>>()
        .join("\n")
}

/// Reads until the session ends a turn.
///
/// # Returns
///
/// How that turn ended, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`Client::next`]'s return values on failure, which is [`Error::Silent`] where the
///   session ended no turn in the time allowed.
fn read_result(session: &mut Client, waiting: Duration, seen: &mut Vec<Event>) -> Result<Turn> {
    let deadline = Instant::now() + waiting;
    loop {
        let left = deadline
            .checked_duration_since(Instant::now())
            .unwrap_or_default();
        let event = session.next(left)?;
        let ending = event.turn().cloned();
        seen.push(event);
        if let Some(turn) = ending {
            return Ok(turn);
        }
    }
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
