//! The code in a reply, highlighted, read the way a reader reads it.
//!
//! Every case starts a session on the stand-in for the `claude` binary, types `:ask` at an
//! [`App`], and lets the frames the stand-in writes arrive on the timer's own tick, so what is
//! highlighted is what a session sent and what is checked is what the application drew or took.
//!
//! Two things are held to. The code is drawn in colour, and in the colour of what each token is
//! rather than in one colour for the block: a keyword, a string and a macro are three colours in
//! the cells of the frame. And the colour is chrome, so `yac` over the block still takes the code
//! as it was fenced.

#![cfg(target_os = "linux")]

use std::fs;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer as Cells;
use ratatui::layout::Rect;
use ratatui::style::Color;
use serde_json::{json, Value};
use tempfile::TempDir;
use vbc_editor::app::{App, Focus};
use vbc_editor::chat::block::Kind;
use vbc_editor::event::Event;
use vbc_editor::session::identity::{Identity, SessionId};
use vbc_editor::session::live::Session;
use vbc_editor::session::spawn::Spawn;
use vbc_layout::buffer::Buffer;

/// The stand-in the application is driven against.
const STUB: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/session/stub.sh");

/// The file the reader has open behind the transcript, which is where a put lands.
const FILE: &str = "a file the reader left open";

/// The code the answer fences, byte for byte as it is sent, and the tokens of it that are drawn in
/// three colours: a keyword, a string literal and a macro.
const CODE: &str = "fn main() {\n    println!(\"Hello, world!\");\n}";
const KEYWORD: &str = "fn";
const STRING: &str = "\"Hello, world!\"";
const MACRO: &str = "println!";

/// The window the application is drawn in.
const COLUMNS: u16 = 120;
const ROWS: u16 = 40;

/// How long the application is driven waiting for the stand-in to say something, and how long it
/// is left between two of those frames.
const PATIENCE: Duration = Duration::from_secs(60);
const TICK: Duration = Duration::from_millis(5);

#[test]
fn a_rust_block_is_drawn_with_its_keyword_its_string_and_its_macro_in_three_colours() -> Result<()>
{
    let directory = TempDir::new()?;
    let mut app = answering(directory.path(), &[&said(&[text(&fenced("rust", CODE))])])?;

    say(&mut app, "show me hello world");
    assert!(
        settled(&mut app, |app| line_of(app, CODE).is_some()),
        "the session sent no code the panel drew in {PATIENCE:?}; it drew {:?}",
        app.panel().text()
    );
    cross(&mut app);
    assert_eq!(Focus::Transcript, app.focus(), "`<C-T>` reached no panel");

    let mut cells = Cells::empty(area());
    app.draw(&mut cells, area());
    let keyword = drawn_in(&cells, &format!("{KEYWORD} main"))?[..KEYWORD.len()].to_vec();
    let string = drawn_in(&cells, STRING)?;
    let called = drawn_in(&cells, MACRO)?;

    for (token, colours) in [(KEYWORD, &keyword), (STRING, &string), (MACRO, &called)] {
        assert!(
            colours
                .iter()
                .all(|colour| Color::Reset != *colour && colours[0] == *colour),
            "{token} was not drawn in one colour of its own: {colours:?}"
        );
    }
    assert_ne!(
        keyword[0], string[0],
        "the keyword is drawn as the string is"
    );
    assert_ne!(
        keyword[0], called[0],
        "the keyword is drawn as the macro is"
    );
    assert_ne!(string[0], called[0], "the string is drawn as the macro is");

    Ok(())
}

#[test]
fn yac_over_a_highlighted_block_takes_the_code_as_it_was_fenced() -> Result<()> {
    let directory = TempDir::new()?;
    let mut app = answering(directory.path(), &[&said(&[text(&fenced("rust", CODE))])])?;

    say(&mut app, "show me hello world");
    assert!(
        settled(&mut app, |app| line_of(app, CODE).is_some()),
        "the session sent no code the panel drew in {PATIENCE:?}"
    );
    let spans = app
        .panel()
        .transcript()
        .blocks()
        .iter()
        .find(|block| matches!(block.kind(), Kind::Code { .. }))
        .map(|block| block.spans().len())
        .ok_or(anyhow!("the transcript holds no code block"))?;
    assert!(1 < spans, "the code block was not highlighted");

    let line = line_of(&mut app, CODE).ok_or(anyhow!("the panel lost the code"))?;
    cross(&mut app);
    assert_eq!(Focus::Transcript, app.focus(), "`<C-T>` reached no panel");
    typing(&mut app, &"j".repeat(line));
    typing(&mut app, "yac");
    cross(&mut app);
    typing(&mut app, "p");

    assert_eq!(format!("{FILE}\n{CODE}"), app.text().text());

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

    Ok(App::new(Buffer::from_text(FILE))
        .with_status(true)
        .with_session(Session::started(&spawn)?))
}

/// # Returns
///
/// The frames a turn answering with the assistant content `items` is written as: the assistant
/// frame holding them, and the frame ending the turn.
fn said(items: &[Value]) -> String {
    let session = "0f9c1c8a-0000-4000-8000-000000000002";
    let assistant = json!({
        "type": "assistant",
        "parent_tool_use_id": Option::<String>::None,
        "session_id": session,
        "message": {
            "id": "msg_code",
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
/// `code` fenced as `language`, the way a model writes it.
fn fenced(language: &str, code: &str) -> String {
    format!("```{language}\n{code}\n```")
}

/// # Returns
///
/// The foreground of each cell of the first row of `cells` drawing `token`, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`anyhow::Error`] if no row draws `token`.
fn drawn_in(cells: &Cells, token: &str) -> Result<Vec<Color>> {
    let area = cells.area;
    for y in area.y..area.bottom() {
        let row: Vec<&str> = (area.x..area.right())
            .map(|x| cells[(x, y)].symbol())
            .collect();
        let Some(at) = row.concat().find(token) else {
            continue;
        };
        let column = u16::try_from(at)?;

        return (0..u16::try_from(token.len())?)
            .map(|offset| Ok(cells[(area.x + column + offset, y)].fg))
            .collect();
    }

    Err(anyhow!("no row of the frame draws {token:?}"))
}

/// # Returns
///
/// The last line of the folded panel that is written from nothing but the first line of `said`, or
/// `None` where the panel draws no such line.
fn line_of(app: &mut App, said: &str) -> Option<usize> {
    let first = said.lines().next()?;

    app.panel()
        .text()
        .lines()
        .enumerate()
        .filter(|(_, line)| first == *line)
        .map(|(at, _)| at)
        .last()
}

/// Types an ex line saying `text` to the session and enters it.
fn say(app: &mut App, text: &str) {
    typing(app, &format!(":ask {text}"));
    app.handle(
        area(),
        &Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );
}

/// Types `keys` at the application, one key at a time, through the loop's own door.
fn typing(app: &mut App, keys: &str) {
    for key in keys.chars() {
        app.handle(
            area(),
            &Event::Key(KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE)),
        );
    }
}

/// Gives the keys to the other half of the application, as `<C-T>` does.
fn cross(app: &mut App) {
    app.handle(area(), &Event::Key(control('t')));
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
/// The area every case is driven in.
fn area() -> Rect {
    Rect::new(0, 0, COLUMNS, ROWS)
}

/// # Returns
///
/// The key `character` typed with `CTRL` held.
fn control(character: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(character), KeyModifiers::CONTROL)
}
