//! What highlighting the code in a conversation costs, counted while a session is read.
//!
//! The panel is rebuilt over the whole conversation on every arrival, so a panel that highlighted
//! its code as it was built would cost every code block said so far on every arrival, and one that
//! highlighted as it was drawn would cost them on every frame. Highlighting belongs to a block
//! arriving and to nothing else, and the count of what was highlighted while a conversation of two
//! thousand blocks takes one more, and while that conversation is drawn, says which it is.
//!
//! The count is the program's own, so this is the only case in its binary: a case beside it would
//! highlight while this one counted.

#![cfg(target_os = "linux")]

use std::fs;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer as Cells;
use ratatui::layout::Rect;
use serde_json::{json, Value};
use tempfile::TempDir;
use vbc_editor::app::App;
use vbc_editor::chat::block::Kind;
use vbc_editor::chat::highlight::highlighted;
use vbc_editor::event::Event;
use vbc_editor::session::identity::{Identity, SessionId};
use vbc_editor::session::live::Session;
use vbc_editor::session::spawn::Spawn;
use vbc_layout::buffer::Buffer;

/// The stand-in the application is driven against.
const STUB: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/session/stub.sh");

/// The window the application is drawn in.
const COLUMNS: u16 = 120;
const ROWS: u16 = 40;

/// How long the application is driven waiting for the stand-in to say something, and how long it
/// is left between two of those frames.
const PATIENCE: Duration = Duration::from_secs(60);
const TICK: Duration = Duration::from_millis(5);

#[test]
fn an_arrival_highlights_the_block_it_brings_and_a_frame_highlights_none() -> Result<()> {
    let blocks = 2_000;
    let code_blocks = 500;
    let mut items = Vec::new();
    for said in 0..code_blocks {
        items.push(json!({"type": "thinking", "thinking": format!("thought {said}")}));
        items.push(text(&format!(
            "before {said}\n\n{}\nafter {said}",
            fenced(&format!("fn answer_{said}() -> u32 {{ {said} }}"))
        )));
    }
    let first = said(&items);
    let second = said(&[text(&fenced(
        "fn main() {\n    println!(\"Hello, world!\");\n}",
    ))]);

    let directory = TempDir::new()?;
    let mut app = answering(directory.path(), &[&first, &second])?;

    let before = highlighted();
    say(&mut app, "say a lot");
    assert!(
        settled(&mut app, |app| code_blocks == coded(app)),
        "the session sent {} code blocks in {PATIENCE:?} rather than {code_blocks}",
        coded(&mut app)
    );
    let said_so_far = app.panel().transcript().len();
    assert!(
        blocks <= said_so_far,
        "the conversation holds {said_so_far} blocks rather than {blocks}"
    );
    assert_eq!(
        before + u64::try_from(code_blocks)?,
        highlighted(),
        "reading {code_blocks} code blocks highlighted something other than each of them once"
    );

    let before = highlighted();
    say(&mut app, "and one more");
    assert!(
        settled(&mut app, |app| code_blocks + 1 == coded(app)),
        "the session sent no further code block in {PATIENCE:?}"
    );
    assert!(
        said_so_far < app.panel().transcript().len(),
        "the panel was not rebuilt over what arrived"
    );
    assert_eq!(
        before + 1,
        highlighted(),
        "one code block arriving in a conversation of {said_so_far} blocks holding \
         {code_blocks} code blocks highlighted {} of them",
        highlighted() - before
    );

    let before = highlighted();
    let mut cells = Cells::empty(area());
    app.handle(area(), &Event::Key(control('t')));
    for _ in 0..64 {
        app.press(area(), control('e'));
        app.draw(&mut cells, area());
    }
    assert_eq!(before, highlighted(), "drawing the panel highlighted code");

    Ok(())
}

/// # Returns
///
/// The application a reader types at, over a session on the stand-in started in `directory`,
/// which answers its Nth turn with the Nth of `turns`, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`std::fs::write`]'s return values on failure.
/// * Forwards [`Session::started`]'s return values on failure.
fn answering(directory: &Path, turns: &[&str]) -> Result<App> {
    for (turn, frames) in turns.iter().enumerate() {
        fs::write(directory.join(format!("said.{}", turn + 1)), frames)?;
    }
    let spawn = Spawn::new(Identity::Fresh(SessionId::generated()))
        .with_binary(STUB)
        .with_directory(directory);

    Ok(App::new(Buffer::from_text("a file the reader left open"))
        .with_status(true)
        .with_session(Session::started(&spawn)?))
}

/// # Returns
///
/// The frames a turn answering with the assistant content `items` is written as: the assistant
/// frame holding them, and the frame ending the turn.
fn said(items: &[Value]) -> String {
    let session = "0f9c1c8a-0000-4000-8000-000000000004";
    let assistant = json!({
        "type": "assistant",
        "parent_tool_use_id": Option::<String>::None,
        "session_id": session,
        "message": {
            "id": "msg_cost",
            "type": "message",
            "role": "assistant",
            "model": "stub",
            "content": items,
        },
    });
    let ended = json!({
        "type": "result",
        "subtype": "success",
        "session_id": session,
        "is_error": false,
        "num_turns": 1,
        "result": "",
        "permission_denials": [],
    });

    format!("{assistant}\n{ended}\n")
}

/// # Returns
///
/// A text item of an assistant frame saying `said`.
fn text(said: &str) -> Value {
    json!({"type": "text", "text": said})
}

/// # Returns
///
/// `code` fenced as Rust, the way a model writes it.
fn fenced(code: &str) -> String {
    format!("```rust\n{code}\n```")
}

/// # Returns
///
/// The number of code blocks the panel's transcript holds.
fn coded(app: &mut App) -> usize {
    app.panel()
        .transcript()
        .blocks()
        .iter()
        .filter(|block| matches!(block.kind(), Kind::Code { .. }))
        .count()
}

/// Types an ex line saying `text` to the session and enters it.
fn say(app: &mut App, text: &str) {
    for key in format!(":ask {text}").chars() {
        app.handle(
            area(),
            &Event::Key(KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE)),
        );
    }
    app.handle(
        area(),
        &Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );
}

/// Drives the application on the timer's own tick until `done`, which is what a reader watching a
/// turn arrive does.
///
/// # Returns
///
/// Whether it came about within [`PATIENCE`].
fn settled(app: &mut App, done: impl Fn(&mut App) -> bool) -> bool {
    let deadline = Instant::now() + PATIENCE;
    loop {
        app.handle(area(), &Event::Redraw);
        if done(app) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(TICK);
    }
}

/// # Returns
///
/// The area the case is driven in.
fn area() -> Rect {
    Rect::new(0, 0, COLUMNS, ROWS)
}

/// # Returns
///
/// The key `character` typed with `CTRL` held.
fn control(character: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(character), KeyModifiers::CONTROL)
}
