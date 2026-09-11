//! The colours code is drawn in, at the depth the terminal says it can draw.
//!
//! A terminal says it draws 24 bits of colour through `COLORTERM`, and one that says nothing --
//! tmux under `screen-256color`, as the reader runs it -- is drawn to in the xterm 256-colour
//! palette. The variable is the process's own, so this is the only case in its binary: a case
//! beside it would read the variable while this one set it.

use anyhow::{anyhow, Result};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer as Cells;
use ratatui::layout::Rect;
use ratatui::style::Color;
use serde_json::json;
use vbc_editor::app::{App, Focus};
use vbc_editor::chat::palette::COLORTERM;
use vbc_editor::session::blocks::Conversation;
use vbc_editor::session::event::Event;
use vbc_layout::buffer::Buffer;

/// The code the answer fences, and the first line of it, which is how its rows are found.
const CODE: &str = "fn main() {\n    println!(\"Hello, world!\");\n    let n: u32 = 42;\n}";

/// The window the application is drawn in.
const COLUMNS: u16 = 100;
const ROWS: u16 = 30;

#[test]
fn code_is_drawn_in_256_colours_unless_the_terminal_says_it_draws_24_bits() -> Result<()> {
    for (colorterm, truecolor) in [
        (None, false),
        (Some("truecolor"), true),
        (Some("24bit"), true),
        (Some("256color"), false),
    ] {
        match colorterm {
            Some(said) => std::env::set_var(COLORTERM, said),
            None => std::env::remove_var(COLORTERM),
        }

        let colours = drawn()?;
        let mut distinct = colours.clone();
        distinct.sort_by_key(|colour| format!("{colour:?}"));
        distinct.dedup();
        assert!(
            3 <= distinct.len(),
            "under COLORTERM={colorterm:?} the code was drawn in {distinct:?}"
        );
        for colour in colours {
            let indexed = matches!(colour, Color::Indexed(_));
            let rgb = matches!(colour, Color::Rgb(..));
            assert!(
                if truecolor { rgb } else { indexed },
                "under COLORTERM={colorterm:?} the code was drawn in {colour:?}"
            );
        }
    }

    Ok(())
}

/// Reads an answer fencing [`CODE`] into a conversation, builds the application over it, and
/// draws a frame.
///
/// # Returns
///
/// Every foreground other than the terminal's own that the rows drawing the code are drawn in, on
/// success.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`anyhow::Error`] if no row of the frame draws the code.
/// * Forwards [`Event::decoded`]'s return values on failure.
fn drawn() -> Result<Vec<Color>> {
    let frame = json!({
        "type": "assistant",
        "parent_tool_use_id": Option::<String>::None,
        "session_id": "0f9c1c8a-0000-4000-8000-000000000003",
        "message": {
            "id": "msg_depth",
            "type": "message",
            "role": "assistant",
            "model": "stub",
            "content": [{"type": "text", "text": format!("```rust\n{CODE}\n```")}],
        },
    });
    let mut conversation = Conversation::new();
    conversation.asked("show me hello world");
    conversation.read(&Event::decoded(&frame.to_string())?);
    let (transcript, tags) = conversation.into_panel();
    let mut app = App::new(Buffer::from_text("a file the reader left open"))
        .with_status(true)
        .with_conversation(transcript, tags);

    let area = Rect::new(0, 0, COLUMNS, ROWS);
    app.press(
        area,
        KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL),
    );
    if Focus::Transcript != app.focus() {
        return Err(anyhow!("`<C-T>` reached no panel"));
    }
    let mut cells = Cells::empty(area);
    app.draw(&mut cells, area);

    let rows: Vec<u16> = (area.y..area.bottom())
        .filter(|y| {
            let drawn: String = (area.x..area.right())
                .map(|x| cells[(x, *y)].symbol())
                .collect();
            CODE.lines().any(|line| drawn.trim_end() == line)
        })
        .collect();
    if CODE.lines().count() != rows.len() {
        return Err(anyhow!("the frame draws {} rows of the code", rows.len()));
    }

    Ok(rows
        .into_iter()
        .flat_map(|y| (area.x..area.right()).map(move |x| (x, y)))
        .map(|at| cells[at].fg)
        .filter(|colour| Color::Reset != *colour)
        .collect())
}
