//! The frames a session writes, read into the blocks the chat panel already draws.
//!
//! Nothing here builds an [`Event`]. Every case starts a child of its own, hands it the frames a
//! real Claude Code session was recorded writing, and reads what comes back off the client's own
//! stream, so the spawn, the NDJSON framing and the decode are the ones the program uses and only
//! the model at the far end is stood in for. A translation checked against events a test
//! constructed would prove the translation and say nothing about the protocol it translates.
//!
//! What the recordings hold was taken from claude 2.1.263 rather than invented: an assistant turn
//! whose prose fences a code block, a `Bash` call whose result still carries the escapes the
//! command wrote, an `Edit` call carrying the text it replaced and the text it wrote, a subagent
//! whose every frame is tagged with the call that started it and whose own call is tagged the same
//! way one level further in, the `conversation_reset` frame `/clear` leaves behind, the `is_meta`
//! answer a local command comes back as, a reply that fenced its code inside a numbered list, and
//! both ends a `/compact` reports through -- the one that replaced a history and the one that
//! answered that it had too little to replace. The frames are as they were written, with one
//! exception stated here: the two the compaction wrote carry a summary and a hook's stdout that
//! run to thousands of bytes, and each is kept to its first 240 and an ellipsis. What is asserted
//! about them is that they become no block at all, which is a claim their length says nothing
//! about.
//!
//! Five claims are what the cases are for. A code block is the code that was sent and not the
//! prose it arrived inside, nor the indentation the list around it was written under. A tool
//! result holds the text a terminal would have shown and none of the bytes that coloured it. A
//! tool call and the result answering it pair up, and a subagent's work folds away beneath the
//! call that started it at whatever depth it happened. `/clear` and a compaction that succeeded
//! are breaks in the history rather than turns Claude took, told apart by the frames the child
//! writes rather than by reading the words in them. And what the client writes to its own history
//! is nobody's turn: a compaction forwards the summary it replaced the history with and the stdout
//! of the hook it ran as user frames carrying no `is_meta` at all, and a transcript that read them
//! would answer `/compact` with a thousand words of summary attributed to the reader.

#![cfg(target_os = "linux")]

use std::fs;
use std::path::Path;
use std::time::Duration;

use anyhow::{anyhow, Result};
use tempfile::TempDir;
use vbc_editor::chat::block::{Block, Kind, Role};
use vbc_editor::chat::fold::{Fold, Folds};
use vbc_editor::session::blocks::{Break, Conversation, Reason};
use vbc_editor::session::client::Client;
use vbc_editor::session::identity::{Identity, SessionId};
use vbc_editor::session::spawn::Spawn;
use vbc_editor::style::Span;

/// The stand-in the recordings are replayed through, which is the same one the client is driven
/// against everywhere else.
const STUB: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/session/stub.sh");

/// The turns it replays: one whole answer, an answer whose code is fenced inside a list, the
/// `/clear` that follows one, a `/compact` that replaced the history and a `/compact` that could
/// not, and a local command's own response.
const ANSWERED: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/session/answered.ndjson");
const LISTED: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/session/listed.ndjson");
const CLEARED: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/session/cleared.ndjson");
const COMPACTED: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/session/compacted.ndjson"
);
const UNCOMPACTED: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/session/uncompacted.ndjson"
);
const LOCAL: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/session/local.ndjson");

/// The identifier every session here is started under.
const CHOSEN: &str = "550e8400-e29b-41d4-a716-446655440000";

/// How long a turn against a process on this machine is given. Nothing here waits on a model.
const TURN: Duration = Duration::from_secs(10);

/// What the reader asked, which the child never echoes and the outbox therefore puts in itself.
const ASKED: &str = "add a todo to main, and show me the diff";

/// The blocks the recorded answer becomes, by the index they are said at.
const QUESTION: usize = 0;
const THOUGHT: usize = 1;
const PREAMBLE: usize = 2;
const CODE: usize = 3;
const EPILOGUE: usize = 4;
const BUILD_CALL: usize = 5;
const BUILD_RESULT: usize = 6;
const EDIT: usize = 7;
const EDIT_RESULT: usize = 8;
const AGENT_CALL: usize = 9;
const AGENT_PROMPT: usize = 10;
const AGENT_SAID: usize = 11;
const NESTED_CALL: usize = 12;
const NESTED_RESULT: usize = 13;
const AGENT_REPORTED: usize = 14;
const AGENT_RESULT: usize = 15;
const ANSWER: usize = 16;

/// The code the answer fenced, byte for byte as it was sent.
const FENCED: &str = "fn main() {\n    todo!();\n}";

/// What the build wrote, with the escapes that coloured it and without them.
const COLOURED: &str = "\u{1b}[1;32m   Compiling\u{1b}[0m vimbecode v0.0.0\n\u{1b}[1;32m    \
                        Finished\u{1b}[0m `dev` profile in 0.42s";
const PLAIN: &str = "   Compiling vimbecode v0.0.0\n    Finished `dev` profile in 0.42s";

/// The byte an escape sequence opens with, which is the one byte no block's source may hold.
const ESCAPE: char = '\u{1b}';

/// The file the recorded edit was to, and the lines either side of it.
const EDITED: &str = "src/main.rs";
const REPLACED: &str = "-fn main() {}";
const WROTE: &str = "+    todo!();";

/// What the recorded turn cost and what it used.
const COST: f64 = 0.047_962;
const INPUT_TOKENS: u64 = 10;
const OUTPUT_TOKENS: u64 = 96;

/// The identifier the session announces itself under before and after `/clear`.
const FIRST: &str = "0f9c1c8a-0000-4000-8000-000000000001";
const SECOND: &str = "0f9c1c8a-0000-4000-8000-000000000002";

/// What the reader asked the listed answer for, and the blocks that answer becomes: the item the
/// code was written under, and the code itself, which the fence was indented two spaces and the
/// code was not.
const LISTED_ASKED: &str = "show me a numbered list with the code under it";
const ITEM: &str = "1. do this: \u{2014}";
const INDENTED: &str = "fn main() {}";

/// The opening of the summary a compaction replaces a history with, and the wrapper the hook it
/// runs has its stdout forwarded inside. Both arrive as user frames that carry no `is_meta`, and
/// neither is a word the reader typed.
const SUMMARISED: &str = "This session is being continued from a previous conversation";
const HOOKED: &str = "<local-command-stdout>";

#[test]
fn a_sessions_answer_becomes_the_blocks_the_panel_reads() -> Result<()> {
    let directory = TempDir::new()?;
    let conversation = answered(directory.path())?;
    let blocks = conversation.transcript().blocks();

    assert_eq!(
        vec![
            Kind::Message(Role::User),
            Kind::Thinking,
            Kind::Message(Role::Assistant),
            Kind::Code {
                language: Some("rust".to_owned())
            },
            Kind::Message(Role::Assistant),
            Kind::ToolCall {
                name: "Bash".to_owned()
            },
            Kind::ToolResult,
            Kind::Diff {
                path: EDITED.to_owned()
            },
            Kind::ToolResult,
            Kind::ToolCall {
                name: "Agent".to_owned()
            },
            Kind::Message(Role::User),
            Kind::Message(Role::Assistant),
            Kind::ToolCall {
                name: "Bash".to_owned()
            },
            Kind::ToolResult,
            Kind::Message(Role::Assistant),
            Kind::ToolResult,
            Kind::Message(Role::Assistant),
        ],
        blocks
            .iter()
            .map(Block::kind)
            .cloned()
            .collect::<Vec<Kind>>()
    );

    assert_eq!(ASKED, source(&conversation, QUESTION)?);
    assert_eq!(
        "The file is tiny, so replacing the body is safe.",
        source(&conversation, THOUGHT)?
    );
    assert_eq!("Here is the line to add:", source(&conversation, PREAMBLE)?);
    assert_eq!(
        FENCED,
        source(&conversation, CODE)?,
        "the code block holds the prose the code was fenced inside rather than the code itself"
    );
    assert_eq!(
        "I ran the build to check it.",
        source(&conversation, EPILOGUE)?
    );
    assert_eq!(
        "cargo build --color always",
        source(&conversation, BUILD_CALL)?
    );
    assert_eq!(
        "Run the test suite and report what failed.",
        source(&conversation, AGENT_CALL)?
    );
    assert_eq!("Running the suite now.", source(&conversation, AGENT_SAID)?);
    assert_eq!(
        "Done: the build is clean and the tests pass.",
        source(&conversation, ANSWER)?
    );

    Ok(())
}

#[test]
fn an_edit_becomes_the_diff_its_own_arguments_carry() -> Result<()> {
    let directory = TempDir::new()?;
    let conversation = answered(directory.path())?;
    let written = source(&conversation, EDIT)?;

    assert_eq!(
        &Kind::Diff {
            path: EDITED.to_owned()
        },
        conversation
            .transcript()
            .block(EDIT)
            .ok_or(anyhow!("the answer held no edit"))?
            .kind()
    );
    assert!(
        written.lines().any(|line| REPLACED == line),
        "the diff does not say the old line was taken out: {written}"
    );
    assert!(
        written.lines().any(|line| WROTE == line),
        "the diff does not say the new line was put in: {written}"
    );

    Ok(())
}

#[test]
fn the_escapes_a_tool_coloured_its_output_with_become_styles_rather_than_text() -> Result<()> {
    let directory = TempDir::new()?;
    let conversation = answered(directory.path())?;
    let result = conversation
        .transcript()
        .block(BUILD_RESULT)
        .ok_or(anyhow!("the answer held no tool result"))?;

    assert!(
        COLOURED.contains(ESCAPE),
        "the recording carries no escapes, so this case could not tell a stripped one from a \
         parsed one"
    );
    assert_eq!(PLAIN, result.source());
    assert!(
        !result.source().contains(ESCAPE),
        "an escape survived into the text a reader yanks"
    );
    assert_ne!(
        &[] as &[Span],
        result.spans(),
        "the escapes were stripped rather than read as the styles they named"
    );

    Ok(())
}

#[test]
fn a_call_pairs_with_its_result_and_a_subagents_work_nests_beneath_the_call_that_started_it(
) -> Result<()> {
    let directory = TempDir::new()?;
    let conversation = answered(directory.path())?;
    let folds = Folds::of(conversation.transcript(), conversation.tags());

    assert_eq!(
        vec![
            (THOUGHT, 0),
            (BUILD_CALL, 0),
            (BUILD_RESULT, 1),
            (EDIT_RESULT, 0),
            (AGENT_CALL, 0),
            (NESTED_CALL, 1),
            (NESTED_RESULT, 2),
            (AGENT_RESULT, 1),
        ],
        folds
            .folds()
            .iter()
            .map(|fold| (fold.head(), fold.depth()))
            .collect::<Vec<(usize, usize)>>(),
        "the calls and their results did not pair up the way the frames tagged them"
    );

    assert_eq!(
        Some(&[BUILD_CALL] as &[usize]),
        folds.at(BUILD_CALL).map(Fold::covered),
        "the call's fold holds the result it was answered with, so a closed call hides what came \
         of it"
    );
    assert_eq!(
        Some(&[BUILD_RESULT] as &[usize]),
        folds.at(BUILD_RESULT).map(Fold::covered),
        "the result of the build does not fold on its own"
    );
    assert_eq!(
        Some(&[
            AGENT_CALL,
            AGENT_PROMPT,
            AGENT_SAID,
            NESTED_CALL,
            NESTED_RESULT,
            AGENT_REPORTED,
        ] as &[usize]),
        folds.at(AGENT_CALL).map(Fold::covered),
        "the call that started the subagent does not fold away everything the subagent did"
    );
    assert_eq!(
        Some(&[NESTED_CALL] as &[usize]),
        folds.at(NESTED_CALL).map(Fold::covered),
        "the call the subagent made does not fold away on its own inside the call that started it"
    );

    Ok(())
}

#[test]
fn clearing_the_history_leaves_a_break_rather_than_a_turn_nobody_took() -> Result<()> {
    let directory = TempDir::new()?;
    let mut session = replaying(directory.path(), &[ANSWERED, CLEARED])?;

    let mut conversation = Conversation::new();
    conversation.asked(ASKED);
    conversation.read_all(&session.turn(ASKED, TURN)?);
    let said = conversation.transcript().len();
    assert_eq!(Some(FIRST), conversation.session_id());
    assert_eq!(&[] as &[Break], conversation.breaks());

    conversation.read_all(&session.turn("/clear", TURN)?);

    assert_eq!(
        said,
        conversation.transcript().len(),
        "clearing the history said something, so it was read as a turn rather than as a break"
    );
    assert_eq!(
        vec![(said, Reason::Cleared)],
        conversation
            .breaks()
            .iter()
            .map(|broken| (broken.after(), broken.reason()))
            .collect::<Vec<(usize, Reason)>>()
    );
    assert_eq!(
        Some(SECOND),
        conversation.session_id(),
        "the session went on being read as the conversation the clear threw away"
    );

    Ok(())
}

#[test]
fn a_compaction_leaves_a_break_and_none_of_the_summary_it_wrote() -> Result<()> {
    let directory = TempDir::new()?;
    let mut session = replaying(directory.path(), &[ANSWERED, COMPACTED])?;

    let mut conversation = Conversation::new();
    conversation.asked(ASKED);
    conversation.read_all(&session.turn(ASKED, TURN)?);
    let said = conversation.transcript().len();

    conversation.read_all(&session.turn("/compact", TURN)?);

    assert_eq!(
        vec![(said, Reason::Compacted)],
        conversation
            .breaks()
            .iter()
            .map(|broken| (broken.after(), broken.reason()))
            .collect::<Vec<(usize, Reason)>>(),
        "a compaction that replaced the history left no break where it replaced it"
    );
    assert_eq!(
        said,
        conversation.transcript().len(),
        "compacting the history said something, so a frame the client wrote to its own history \
         was read as a turn somebody took"
    );

    for (index, block) in conversation.transcript().blocks().iter().enumerate() {
        assert!(
            !block.source().contains(SUMMARISED),
            "block {index} holds the summary the history was replaced by, as something the \
             reader asked"
        );
        assert!(
            !block.source().contains(HOOKED),
            "block {index} holds the stdout of the hook the compaction ran, as something the \
             reader asked"
        );
    }

    Ok(())
}

#[test]
fn a_compaction_that_could_not_happen_leaves_the_history_whole() -> Result<()> {
    let directory = TempDir::new()?;
    let mut session = replaying(directory.path(), &[ANSWERED, UNCOMPACTED])?;

    let mut conversation = Conversation::new();
    conversation.asked(ASKED);
    conversation.read_all(&session.turn(ASKED, TURN)?);
    let said = conversation.transcript().len();

    conversation.read_all(&session.turn("/compact", TURN)?);

    assert_eq!(
        &[] as &[Break],
        conversation.breaks(),
        "the conversation was cut in half for a compaction that reported it could not happen"
    );
    assert_eq!(
        said,
        conversation.transcript().len(),
        "the refusal the client answered with was read as a turn Claude took"
    );

    Ok(())
}

#[test]
fn the_indentation_a_fence_was_written_under_is_no_part_of_the_code_it_holds() -> Result<()> {
    let directory = TempDir::new()?;
    let mut session = replaying(directory.path(), &[LISTED])?;

    let mut conversation = Conversation::new();
    conversation.read_all(&session.turn(LISTED_ASKED, TURN)?);

    assert_eq!(
        vec![
            Kind::Message(Role::Assistant),
            Kind::Code {
                language: Some("rust".to_owned())
            },
        ],
        conversation
            .transcript()
            .blocks()
            .iter()
            .map(Block::kind)
            .cloned()
            .collect::<Vec<Kind>>()
    );
    assert_eq!(ITEM, source(&conversation, 0)?);
    assert_eq!(
        INDENTED,
        source(&conversation, 1)?,
        "the code block holds the spaces the list indented the fence by, so a reader who yanked \
         it would put it back one indent deeper than it was sent"
    );

    Ok(())
}

#[test]
fn a_local_commands_own_answer_is_no_turn_of_the_conversation() -> Result<()> {
    let directory = TempDir::new()?;
    let mut session = replaying(directory.path(), &[LOCAL])?;

    let mut conversation = Conversation::new();
    conversation.read_all(&session.turn("/context", TURN)?);

    assert_eq!(
        &[] as &[Block],
        conversation.transcript().blocks(),
        "a command the client answered itself was read as something Claude said"
    );

    Ok(())
}

#[test]
fn a_turn_that_ended_carries_what_it_cost_and_what_it_announced() -> Result<()> {
    let directory = TempDir::new()?;
    let conversation = answered(directory.path())?;
    let ended = conversation
        .ended()
        .ok_or(anyhow!("the turn did not end with a result"))?;

    assert_eq!("success", ended.subtype);
    assert!(!ended.failed, "the turn ended as {ended:?}");
    assert!(
        (COST - ended.cost).abs() < f64::EPSILON,
        "the turn is reported as having cost {} rather than {COST}",
        ended.cost
    );
    assert_eq!(INPUT_TOKENS, ended.usage.input);
    assert_eq!(OUTPUT_TOKENS, ended.usage.output);
    assert_eq!(
        vec![
            "clear".to_owned(),
            "compact".to_owned(),
            "context".to_owned()
        ],
        conversation.commands(),
        "the slash commands the session offered were not read off the frame announcing them"
    );

    Ok(())
}

/// # Returns
///
/// The conversation the recorded answer becomes, read off a child that was handed the frames a
/// real session wrote, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`replaying`]'s return values on failure.
/// * Forwards [`Client::turn`]'s return values on failure.
fn answered(directory: &Path) -> Result<Conversation> {
    let mut session = replaying(directory, &[ANSWERED])?;
    let mut conversation = Conversation::new();
    conversation.asked(ASKED);
    conversation.read_all(&session.turn(ASKED, TURN)?);

    Ok(conversation)
}

/// # Returns
///
/// A session whose child answers its nth turn with the frames the nth recording of `turns` holds,
/// on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`std::fs::copy`]'s return values on failure.
/// * Forwards [`Client::start`]'s return values on failure.
fn replaying(directory: &Path, turns: &[&str]) -> Result<Client> {
    for (index, recorded) in turns.iter().enumerate() {
        fs::copy(recorded, directory.join(format!("said.{}", index + 1)))?;
    }

    let spawn = Spawn::new(Identity::Fresh(SessionId::known(CHOSEN)))
        .with_binary(STUB)
        .with_directory(directory);

    Ok(Client::start(&spawn)?)
}

/// # Returns
///
/// The source of the block `index` of `conversation`, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`anyhow::Error`] if the conversation holds no such block.
fn source(conversation: &Conversation, index: usize) -> Result<&str> {
    Ok(conversation
        .transcript()
        .block(index)
        .ok_or(anyhow!("the conversation holds no block {index}"))?
        .source())
}
