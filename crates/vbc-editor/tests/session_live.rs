//! A real Claude Code session, driven by the client, answering -- and read in the panel.
//!
//! These are the tests the stand-in's fidelity rests on. Everything `session_stub.rs` and
//! `session_blocks.rs` assert is asserted against a process that behaves the way this one was
//! observed to behave, and the observation is only worth what it was taken from, so it is taken
//! again here: a `claude` on the path, a login behind it, and a model at the end of it.
//!
//! The last three go further than the client. A session is asked for a fenced code block and the
//! transcript is required to hold that code as a block of its own, byte for byte; a session is
//! asked to run a command that writes colour and the block its output becomes is required to hold
//! none of the escapes that coloured it; and `yac` is typed at the application over the blocks a
//! real reply became, put into the file with `p`, and the file is read back. Nothing in those
//! three constructs a block: what the panel is asked to answer is what a model actually said.
//!
//! That is also why they are ignored by default. They cost a model call and a network, and they
//! are red on a machine with no login rather than absent -- which is the point of ignoring them
//! rather than skipping them at run time. A gate that switched itself off where the binary is
//! missing would report a broken client as a run of quiet successes.
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
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use vbc_editor::app::{App, Focus};
use vbc_editor::chat::block::{Block, Kind};
use vbc_editor::session::blocks::Conversation;
use vbc_editor::session::client::Client;
use vbc_editor::session::event::Event;
use vbc_editor::session::identity::{Identity, SessionId};
use vbc_editor::session::spawn::Spawn;
use vbc_editor::style::Span;
use vbc_layout::buffer::Buffer;

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

/// What a session is asked for a fenced code block with, and the code the fence is required to
/// hold, byte for byte.
const FENCE_ASKED: &str = "Reply with nothing but one fenced rust code block whose only line is \
                           exactly this: fn main() {}";
const FENCED: &str = "fn main() {}";

/// What a session is asked to run a colour-writing command with, and the word that command writes
/// in colour. `printf` is auto-approved by the binary's own classifier, so this needs no
/// permission round trip.
const COLOUR_ASKED: &str = "Run exactly this shell command with the Bash tool and then say DONE: \
                            printf '\\033[31mred\\033[0m plain\\n'";
const COLOURED_WORD: &str = "red";

/// The byte an escape sequence opens with, which is the one byte a block's source may not hold.
const ESCAPE: char = '\u{1b}';

/// The file the reader has open behind the transcript, which is where a put lands.
const FILE: &str = "a file the reader left open";

/// The window the application is driven in, wide enough that nothing a session says wraps.
const COLUMNS: u16 = 200;
const ROWS: u16 = 40;

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

/// Validation 1: a real reply becomes the blocks the panel reads, and what it fenced is a code
/// block of its own holding exactly the bytes that were sent.
#[test]
#[ignore = "starts a real Claude Code session, so it needs the binary, a login and a network"]
fn a_real_reply_becomes_a_code_block_holding_exactly_what_was_sent() -> Result<()> {
    let mut session = Client::start(&Spawn::new(Identity::default()).with_model(MODEL))?;
    let conversation = held(&mut session, FENCE_ASKED, TURN)?;
    session.finish(ENDING)?;

    let fenced: Vec<&str> = conversation
        .transcript()
        .blocks()
        .iter()
        .filter(|block| matches!(block.kind(), Kind::Code { .. }))
        .map(Block::source)
        .collect();

    assert!(
        fenced.contains(&FENCED),
        "no block of the transcript holds the code the session sent; it holds {fenced:?}"
    );

    Ok(())
}

/// Validation 3: the escapes a real command wrote are read as the styles they name, so none of
/// them is left in the text a reader yanks.
#[test]
#[ignore = "starts a real Claude Code session, so it needs the binary, a login and a network"]
fn the_escapes_a_real_command_wrote_become_styles_rather_than_text() -> Result<()> {
    let mut session = Client::start(&Spawn::new(Identity::default()).with_model(MODEL))?;
    let conversation = held(&mut session, COLOUR_ASKED, TURN)?;
    session.finish(ENDING)?;

    let coloured = conversation
        .transcript()
        .blocks()
        .iter()
        .find(|block| Kind::ToolResult == *block.kind() && block.source().contains(COLOURED_WORD))
        .ok_or(anyhow!(
            "the session ran no command that wrote {COLOURED_WORD:?}, so this case had nothing to \
             read"
        ))?;

    assert!(
        !coloured.source().contains(ESCAPE),
        "an escape survived into the text a reader yanks: {:?}",
        coloured.source()
    );
    assert_ne!(
        &[] as &[Span],
        coloured.spans(),
        "the escapes were dropped rather than read as the styles they named: {:?}",
        coloured.source()
    );

    Ok(())
}

/// Validation 2: the text objects and the yanks, unchanged, over blocks a real session wrote --
/// typed at the application rather than called on the panel.
#[test]
#[ignore = "starts a real Claude Code session, so it needs the binary, a login and a network"]
fn yac_takes_a_real_sessions_code_and_p_puts_it_in_the_file() -> Result<()> {
    let mut session = Client::start(&Spawn::new(Identity::default()).with_model(MODEL))?;
    let conversation = held(&mut session, FENCE_ASKED, TURN)?;
    session.finish(ENDING)?;

    let (transcript, tags) = conversation.into_panel();
    let mut app = App::new(Buffer::from_text(FILE)).with_conversation(transcript, tags);
    app.press(area(), control('t'));
    assert_eq!(Focus::Transcript, app.focus(), "`<C-T>` reached no panel");

    let down = line_of(&mut app, FENCED).ok_or(anyhow!(
        "the panel draws none of the code the session sent, so there is nowhere to type `yac`"
    ))?;
    for _ in 0..down {
        app.press(area(), typed('j'));
    }
    for key in "yac".chars() {
        app.press(area(), typed(key));
    }
    app.press(area(), control('t'));
    app.press(area(), typed('p'));

    assert_eq!(format!("{FILE}\n{FENCED}"), app.text().text());

    Ok(())
}

/// # Returns
///
/// The conversation one turn of `session` becomes, the question included, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`Client::turn`]'s return values on failure.
fn held(session: &mut Client, asked: &str, waiting: Duration) -> Result<Conversation> {
    let mut conversation = Conversation::new();
    conversation.asked(asked);
    conversation.read_all(&session.turn(asked, waiting)?);

    Ok(conversation)
}

/// # Returns
///
/// The last line of the folded panel that is written from nothing but `said`, or `None` where the
/// panel draws no such line. It is the last rather than the first because the question the reader
/// asked holds the code it asked for, and what a yank is being aimed at is the block the session
/// answered with.
fn line_of(app: &mut App, said: &str) -> Option<usize> {
    app.panel()
        .text()
        .lines()
        .enumerate()
        .filter(|(_, line)| said == *line)
        .map(|(at, _)| at)
        .last()
}

/// # Returns
///
/// The area the application is driven in.
fn area() -> Rect {
    Rect::new(0, 0, COLUMNS, ROWS)
}

/// # Returns
///
/// The key `character` typed with no modifier.
fn typed(character: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE)
}

/// # Returns
///
/// The key `character` typed with `CTRL` held.
fn control(character: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(character), KeyModifiers::CONTROL)
}
