//! The history panel read the way a reader reads it: who said each thing, and what stays in view
//! while Claude is still talking.
//!
//! Every case drives the application the binary builds -- [`App::chat`] over a session -- through
//! [`App::handle`], with a stand-in for `claude` answering over real pipes, and reads back the
//! cells a frame wrote. `stub.sh` answers a turn with the frames a case wrote for it, and `drip.sh`
//! answers one block at a time, so a case can look at the panel between one arrival and the next.

#![cfg(target_os = "linux")]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::fs;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer as Cells;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier};
use serde_json::{json, Value};
use tempfile::TempDir;
use vbc_editor::app::{App, Focus};
use vbc_editor::chat::chrome::GUTTER;
use vbc_editor::engine::{typed, Position as Caret};
use vbc_editor::event::Event;
use vbc_editor::session::identity::{Identity, SessionId};
use vbc_editor::session::live::Session;
use vbc_editor::session::spawn::Spawn;

/// The stand-ins: the one answering a turn with the frames a case wrote for it, and the one
/// answering a block at a time.
const STUB: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/session/stub.sh");
const DRIP: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/session/drip.sh");

/// The file `stub.sh` answers its first turn out of, and the files `drip.sh` reads how many blocks
/// it answers with and how far apart.
const SAID: &str = "said.1";
const DROPS_FILE: &str = "drip";
const PAUSE_FILE: &str = "pause";

/// The session the frames a case writes name.
const SESSION: &str = "0f9c1c8a-0000-4000-8000-000000000003";

/// What the reader asks first, which the history draws in two rows, and what they ask second.
const ASKED: &str = "Explain how the history panel tells the prompts a reader wrote from the \
                     replies Claude wrote, and how it keeps the view still while a long reply is \
                     streaming in";
const AGAIN: &str = "and again";

/// What the stand-in answers the first question with: a reply written in markdown, a build, and a
/// file written inside the session's directory.
const REPLY: &str = "Hello **there**, friend.";
const BOLD: &str = "there";
const BUILD_ID: &str = "toolu_build";
const COMMAND: &str = "cargo build";
const BUILT: &str = "Finished dev profile";
const WRITE_ID: &str = "toolu_write";
const WRITTEN_FILE: &str = "hello.rs";
const WROTE: &str = "File created successfully";

/// The headers the two calls are drawn under.
const BUILD_HEADER: &str = "Bash(cargo build)";
const WRITE_HEADER: &str = "Write(hello.rs)";

/// What `stub.sh` says in a turn it was given no frames for.
const SECOND_TURN: &str = "turn 2";

/// The marks the history draws in its gutter.
const PROMPT_MARK: &str = "›";
const SAID_MARK: &str = "●";
const ANSWER_MARK: &str = "⎿";

/// The marks the row between the panels carries.
const DIVIDERS: [&str; 2] = ["▲ history", "▼ prompt"];

/// What `drip.sh` says in each block, how many blocks it answers with, and how far apart.
const DROP: &str = "drop ";
const DROPS: usize = 50;
const PAUSE: &str = "0.03";

/// The fewest arrivals a case following them must see one at a time for what it asserts to be
/// about arrivals at all.
const WATCHED: usize = 10;

/// The blocks that have arrived when a reader starts reading, which are more than the history of
/// the narrow window has rows.
const BEFORE_READING: usize = 25;

/// The blocks the short and the long session say before the arrival a frame is measured after,
/// and what every one of them says.
const SHORT: usize = 50;
const LONG: usize = 5_000;
const SAME: &str = "every block of this session says the same thing";

/// How long a case waits for a session to say something, and how long it waits between frames.
const PATIENCE: Duration = Duration::from_secs(60);
const TICK: Duration = Duration::from_millis(5);

/// The allocator every measurement here is read through.
#[global_allocator]
static ALLOCATOR: Counting = Counting;

thread_local! {
    /// The bytes this thread has asked for since the last [`counted`] began, given back or not.
    static ASKED_FOR: Cell<usize> = const { Cell::new(0) };

    /// The number of times it asked for them.
    static CALLS: Cell<usize> = const { Cell::new(0) };
}

/// An allocator that counts what it was asked for and hands the asking on.
struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = System.alloc(layout);
        if !pointer.is_null() {
            let _ = ASKED_FOR.try_with(|asked| asked.set(asked.get() + layout.size()));
            let _ = CALLS.try_with(|calls| calls.set(calls.get() + 1));
        }

        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        System.dealloc(pointer, layout);
    }
}

/// Validation 1: a prompt's rows are drawn on a band across the whole width of the panel behind
/// `›`, a reply's behind `●` and on no band, a call as the header naming it with what it was
/// answered with under `⎿`, the reply's bold in bold, and a turn apart from the one before it.
#[test]
fn a_prompt_is_drawn_on_a_band_behind_its_mark_and_a_reply_behind_its_own() -> Result<()> {
    let area = wide();
    let directory = TempDir::new()?;
    let app = answered(directory.path(), area)?;
    let cells = drawn(&app, area);
    let frame = rows(&cells);

    let prompts = marked(&cells, PROMPT_MARK);
    let &[first, second] = prompts.as_slice() else {
        return Err(anyhow!("the history drew no two prompts: {frame:#?}"));
    };
    let band = cells[(0, first)].bg;
    assert!(
        matches!(band, Color::Indexed(16..) | Color::Rgb(..)),
        "the prompt's band is {band:?}, which is a colour a terminal theme redefines"
    );
    for y in [first, first + 1] {
        for x in 0..area.width {
            assert_eq!(
                band,
                cells[(x, y)].bg,
                "the prompt's row {y} is off its band at column {x}: {frame:#?}"
            );
        }
    }
    assert_eq!(
        ASKED,
        format!("{}{}", text(&cells, first), text(&cells, first + 1)),
        "the prompt's two rows are not the prompt: {frame:#?}"
    );
    assert_eq!(" ", cells[(0, first + 1)].symbol());

    let reply = row_of(&cells, REPLY)?;
    assert_eq!(SAID_MARK, cells[(0, reply)].symbol());
    let mark = cells[(0, reply)].fg;
    assert!(
        matches!(mark, Color::Indexed(16..) | Color::Rgb(..)),
        "the reply's mark is drawn in {mark:?}"
    );
    for x in 0..area.width {
        assert_eq!(
            Color::Reset,
            cells[(x, reply)].bg,
            "the reply's row is on a band at column {x}"
        );
    }
    let bold = GUTTER + REPLY.find(BOLD).expect("the reply holds the bold word");
    for x in bold..bold + BOLD.len() {
        let x = u16::try_from(x)?;
        assert!(
            cells[(x, reply)].modifier.contains(Modifier::BOLD),
            "`{BOLD}` is not drawn bold at column {x}"
        );
    }

    for (header, answer) in [(BUILD_HEADER, BUILT), (WRITE_HEADER, WROTE)] {
        let call = row_of(&cells, header)?;
        assert_eq!(SAID_MARK, cells[(0, call)].symbol());
        assert_eq!(
            ANSWER_MARK,
            cells[(0, call + 1)].symbol(),
            "what `{header}` was answered with is not under it: {frame:#?}"
        );
        assert_eq!(answer, text(&cells, call + 1));
    }

    assert_eq!(
        "",
        frame[usize::from(second - 1)],
        "no blank row sets the second turn apart from the first: {frame:#?}"
    );
    for x in 0..area.width {
        assert_eq!(Color::Reset, cells[(x, second - 1)].bg);
    }

    Ok(())
}

/// Validation 2: `yy` on a prompt and `V` over a reply take what was said, and none of the marks
/// or the band it was drawn with.
#[test]
fn a_yank_of_a_prompt_or_a_reply_takes_its_source_and_none_of_its_marks() -> Result<()> {
    let area = wide();
    let directory = TempDir::new()?;
    let mut app = answered(directory.path(), area)?;
    reading(&mut app, area)?;

    typing(&mut app, area, "ggyy");
    assert_eq!(Some(format!("{ASKED}\n")), unnamed(&mut app));

    typing(&mut app, area, "jVy");
    assert_eq!(Some(format!("{REPLY}\n")), unnamed(&mut app));

    Ok(())
}

/// Validation 3, the first half: while the cursor is on the history's last row, every block that
/// arrives is drawn on the last row the history draws, and the cursor goes with it.
#[test]
fn at_the_bottom_every_arrival_keeps_the_newest_block_in_view() -> Result<()> {
    let area = wide();
    let directory = TempDir::new()?;
    let mut app = dripping(directory.path())?;
    typing(&mut app, area, "watch this\u{1b}:wq\r");
    reading(&mut app, area)?;

    let watched = watching(&mut app, area, |app| {
        let newest = app
            .panel()
            .transcript()
            .blocks()
            .last()
            .map(|block| block.source().to_owned())
            .unwrap_or_default();
        let history = history(app, area);
        let last = history.iter().rev().find(|row| !row.is_empty());

        assert!(
            last.is_some_and(|row| row.ends_with(&newest)),
            "`{newest}` arrived and is not the last row the history draws: {history:#?}"
        );
        assert!(
            at_the_bottom(app),
            "the cursor was left behind by `{newest}`"
        );
    })?;

    assert!(
        WATCHED <= watched,
        "only {watched} arrivals were seen one at a time"
    );

    Ok(())
}

/// Validation 3, the second half: after `gg`, what arrives moves neither the history's rows nor
/// its cursor.
#[test]
fn after_gg_arrivals_move_neither_the_view_nor_the_cursor() -> Result<()> {
    let area = wide();
    let directory = TempDir::new()?;
    let mut app = dripping(directory.path())?;
    typing(&mut app, area, "watch this\u{1b}:wq\r");
    assert!(
        settled(&mut app, area, |app| 3 <= dropped(app)),
        "the stand-in dripped nothing in {PATIENCE:?}"
    );
    reading(&mut app, area)?;
    typing(&mut app, area, "gg");

    let (rows, caret, cell) = standing(&mut app, area);
    let watched = watching(&mut app, area, |app| {
        let (now, moved, drawn) = standing(app, area);
        assert_eq!(
            (rows.first(), caret, cell),
            (now.first(), moved, drawn),
            "an arrival moved the top row or the cursor of the history the reader had moved to \
             its top"
        );
    })?;

    assert!(
        WATCHED <= watched,
        "only {watched} arrivals were seen one at a time"
    );

    Ok(())
}

/// Validation 3, issue #87's own steps: a reader who moved up a few rows from the bottom of a
/// history taller than the window is neither carried back down nor thrown to its top by what goes
/// on arriving.
#[test]
fn a_reader_who_moved_up_from_the_bottom_is_not_carried_off_by_what_arrives() -> Result<()> {
    let area = narrow();
    let directory = TempDir::new()?;
    let mut app = dripping(directory.path())?;
    typing(&mut app, area, "watch this\u{1b}:wq\r");
    assert!(
        settled(&mut app, area, |app| BEFORE_READING <= dropped(app)),
        "the stand-in dripped nothing in {PATIENCE:?}"
    );
    reading(&mut app, area)?;
    typing(&mut app, area, "Gkkkk");

    let kept = standing(&mut app, area);
    assert_ne!(
        Some(&"watch this".to_owned()),
        kept.0
            .first()
            .map(|row| row.chars().skip(GUTTER).collect::<String>())
            .as_ref(),
        "the history is not taller than the window, so a throw to its top could not be seen"
    );
    let watched = watching(&mut app, area, |app| {
        assert_eq!(
            kept,
            standing(app, area),
            "an arrival moved the history the reader had moved up in"
        );
    })?;

    assert!(
        WATCHED <= watched,
        "only {watched} arrivals were seen one at a time"
    );

    Ok(())
}

/// `gg` and `G` carry the cursor further through a long history than a follow walks, and the
/// history is drawn from where they carried it all the same.
#[test]
fn gg_and_g_in_a_long_history_draw_the_row_they_carried_the_cursor_to() -> Result<()> {
    let area = wide();
    let directory = TempDir::new()?;
    fs::write(directory.path().join(SAID), same(LONG))?;
    let mut app = chatting(directory.path(), STUB)?;
    typing(&mut app, area, "fill it\u{1b}:wq\r");
    if !settled(&mut app, area, |app| LONG < app.panel().transcript().len()) {
        return Err(anyhow!(
            "the stand-in said no {LONG} blocks in {PATIENCE:?}"
        ));
    }
    reading(&mut app, area)?;

    typing(&mut app, area, "gg");
    let (rows, _, cell) = standing(&mut app, area);
    assert_eq!(
        Some("fill it".to_owned()),
        rows.first().map(|row| row.chars().skip(GUTTER).collect()),
        "`gg` did not draw the history's first row at its top"
    );
    assert_eq!(
        Some(Position::new(u16::try_from(GUTTER)?, 0)),
        cell,
        "the cursor `gg` carried to the first row is not drawn there"
    );

    typing(&mut app, area, "G");
    let (rows, _, cell) = standing(&mut app, area);
    assert!(
        rows.iter()
            .rev()
            .find(|row| !row.is_empty())
            .is_some_and(|row| row.ends_with(SAME)),
        "`G` did not draw the history's last row: {rows:#?}"
    );
    assert!(
        cell.is_some(),
        "the cursor `G` carried to the last row is not drawn"
    );

    Ok(())
}

/// Validation 4: the frame drawn after an arrival costs the rows it draws, so one deep in a
/// session of five thousand blocks asks the allocator for what one in a session of fifty does.
#[test]
fn a_frame_after_an_arrival_deep_in_a_long_session_asks_for_what_one_in_a_short_session_does(
) -> Result<()> {
    let short = arrived(SHORT)?;
    let long = arrived(LONG)?;

    assert_eq!(
        short, long,
        "a frame after an arrival into {LONG} blocks asked for {long:?}, and one after an arrival \
         into {SHORT} asked for {short:?}"
    );

    Ok(())
}

/// # Returns
///
/// The application over a session that answered the first question with [`answer`] and the second
/// with what `stub.sh` says of its own accord, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`anyhow::Error`] if the stand-in answered either question with nothing.
/// * Forwards [`std::fs::write`]'s return values on failure.
/// * Forwards [`chatting`]'s return values on failure.
fn answered(directory: &Path, area: Rect) -> Result<App> {
    fs::write(directory.join(SAID), answer(directory))?;
    let mut app = chatting(directory, STUB)?;

    typing(&mut app, area, &format!("{ASKED}\u{1b}:wq\r"));
    if !settled(&mut app, area, |app| app.panel().text().contains(WROTE)) {
        return Err(anyhow!("the stand-in answered nothing in {PATIENCE:?}"));
    }
    typing(&mut app, area, &format!("{AGAIN}\u{1b}:wq\r"));
    if !settled(&mut app, area, |app| {
        app.panel().text().contains(SECOND_TURN)
    }) {
        return Err(anyhow!(
            "the stand-in answered no second turn in {PATIENCE:?}"
        ));
    }

    Ok(app)
}

/// # Returns
///
/// The application over a session answered by `drip.sh`, [`DROPS`] blocks a turn, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`std::fs::write`]'s return values on failure.
/// * Forwards [`chatting`]'s return values on failure.
fn dripping(directory: &Path) -> Result<App> {
    fs::write(directory.join(DROPS_FILE), DROPS.to_string())?;
    fs::write(directory.join(PAUSE_FILE), PAUSE)?;

    chatting(directory, DRIP)
}

/// # Returns
///
/// What a frame drawn right after an arrival into a session that already said `blocks` blocks asks
/// the allocator for, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`anyhow::Error`] if the stand-in answered either question with nothing.
/// * Forwards [`std::fs::write`]'s return values on failure.
/// * Forwards [`chatting`]'s return values on failure.
fn arrived(blocks: usize) -> Result<(usize, usize)> {
    let area = wide();
    let directory = TempDir::new()?;
    fs::write(directory.path().join(SAID), same(blocks))?;
    let mut app = chatting(directory.path(), STUB)?;

    typing(&mut app, area, "fill it\u{1b}:wq\r");
    if !settled(&mut app, area, |app| {
        blocks < app.panel().transcript().len() && app.turn().as_deref() == Some("idle")
    }) {
        return Err(anyhow!(
            "the stand-in said no {blocks} blocks in {PATIENCE:?}"
        ));
    }
    typing(&mut app, area, &format!("{AGAIN}\u{1b}:wq\r"));
    if !settled(&mut app, area, |app| {
        app.panel().text().contains(SECOND_TURN) && app.turn().as_deref() == Some("idle")
    }) {
        return Err(anyhow!(
            "the stand-in answered no second turn in {PATIENCE:?}"
        ));
    }

    let mut cells = Cells::empty(area);
    app.draw(&mut cells, area);
    let (_, cost) = counted(|| app.draw(&mut cells, area));

    Ok(cost)
}

/// Drives the application on the timer's own tick until the stand-in has dripped every block of
/// its turn, running `check` after every tick that took in a block.
///
/// # Returns
///
/// How many ticks took in a block, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`anyhow::Error`] if the stand-in stopped dripping before [`PATIENCE`] ran out.
fn watching(app: &mut App, area: Rect, check: impl Fn(&mut App)) -> Result<usize> {
    let deadline = Instant::now() + PATIENCE;
    let mut said = app.panel().transcript().len();
    let mut watched = 0;
    while dropped(app) < DROPS {
        if deadline <= Instant::now() {
            return Err(anyhow!(
                "the stand-in dripped {} of {DROPS} blocks in {PATIENCE:?}",
                dropped(app)
            ));
        }
        app.handle(area, &Event::Redraw);
        let now = app.panel().transcript().len();
        if now != said {
            said = now;
            watched += 1;
            check(app);
        }
        thread::sleep(TICK);
    }

    Ok(watched)
}

/// Moves the keys from the prompt, in the insert mode `:wq` left it in, up to the history.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`anyhow::Error`] if the keys did not reach the history.
fn reading(app: &mut App, area: Rect) -> Result<()> {
    typing(app, area, "\u{1b}");
    press(app, area, control('w'));
    typing(app, area, "k");
    if Focus::History != app.focus() {
        return Err(anyhow!("`<C-W>k` did not reach the history"));
    }

    Ok(())
}

/// # Returns
///
/// Where the reader stands in the history: the rows it draws, where its cursor rests in the text
/// and the cell the frame draws that cursor in.
fn standing(app: &mut App, area: Rect) -> (Vec<String>, Caret, Option<Position>) {
    let mut cells = Cells::empty(area);
    let cell = app.draw(&mut cells, area);
    let caret = app.panel().cursor();

    (history(app, area), caret, cell)
}

/// # Returns
///
/// How many of the blocks the stand-in dripped have arrived.
fn dropped(app: &mut App) -> usize {
    app.panel()
        .transcript()
        .blocks()
        .iter()
        .filter(|block| block.source().starts_with(DROP))
        .count()
}

/// # Returns
///
/// Whether the history's cursor rests on its last line.
fn at_the_bottom(app: &mut App) -> bool {
    let last = app.panel().text().lines().count().saturating_sub(1);

    last == app.panel().cursor().line
}

/// # Returns
///
/// What the unnamed register holds, or `None` where it holds nothing.
fn unnamed(app: &mut App) -> Option<String> {
    app.panel().register('"').map(|held| held.text)
}

/// # Returns
///
/// The frames the stand-in answers the first question with: the session announcing itself in
/// `directory`, a reply, a build and what it wrote, a file written inside `directory` and what the
/// tool said about it, and the end of the turn.
fn answer(directory: &Path) -> String {
    let written = directory.join(WRITTEN_FILE);

    [
        json!({
            "type": "system",
            "subtype": "init",
            "session_id": SESSION,
            "claude_code_version": "stub",
            "model": "stub",
            "cwd": directory.display().to_string(),
            "permissionMode": "default",
            "tools": ["Bash", "Read", "Edit", "AskUserQuestion", "EnterPlanMode", "ExitPlanMode"],
            "capabilities": ["interrupt_receipt_v1"],
            "slash_commands": ["clear"],
        })
        .to_string(),
        framed("assistant", json!([{"type": "text", "text": REPLY}])),
        framed(
            "assistant",
            json!([{"type": "tool_use", "id": BUILD_ID, "name": "Bash",
                    "input": {"command": COMMAND}}]),
        ),
        framed(
            "user",
            json!([{"type": "tool_result", "tool_use_id": BUILD_ID, "content": BUILT}]),
        ),
        framed(
            "assistant",
            json!([{"type": "tool_use", "id": WRITE_ID, "name": "Write",
                    "input": {"file_path": written.display().to_string(), "content": "fn main() {}"}}]),
        ),
        framed(
            "user",
            json!([{"type": "tool_result", "tool_use_id": WRITE_ID, "content": WROTE}]),
        ),
        ended(),
    ]
    .map(|frame| format!("{frame}\n"))
    .concat()
}

/// # Returns
///
/// The frames of a turn saying [`SAME`] in `blocks` blocks, and its end.
fn same(blocks: usize) -> String {
    let mut frames = String::new();
    for _ in 0..blocks {
        frames.push_str(&framed(
            "assistant",
            json!([{"type": "text", "text": SAME}]),
        ));
        frames.push('\n');
    }
    frames.push_str(&ended());
    frames.push('\n');

    frames
}

/// # Returns
///
/// A frame of the type `said` carrying `content`, written the way a session writes one.
fn framed(said: &str, content: Value) -> String {
    json!({
        "type": said,
        "parent_tool_use_id": Option::<String>::None,
        "session_id": SESSION,
        "message": {
            "id": "msg_history",
            "type": "message",
            "role": said,
            "model": "stub",
            "content": content,
        },
    })
    .to_string()
}

/// # Returns
///
/// The frame ending a turn.
fn ended() -> String {
    json!({
        "type": "result",
        "subtype": "success",
        "session_id": SESSION,
        "is_error": false,
        "num_turns": 1,
        "result": "done",
        "permission_denials": [],
    })
    .to_string()
}

/// # Returns
///
/// The conversation screen over a session answered by the stand-in `binary` in `directory`, on
/// success.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`Session::started`]'s return values on failure.
fn chatting(directory: &Path, binary: &str) -> Result<App> {
    let spawn = Spawn::new(Identity::Fresh(SessionId::generated()))
        .with_binary(binary)
        .with_directory(directory);

    Ok(App::chat().with_session(Session::started(&spawn)?))
}

/// Types the characters of `keys` at `app` through the loop's own door, an escape standing for the
/// escape key and a carriage return for the return key.
fn typing(app: &mut App, area: Rect, keys: &str) {
    for character in keys.chars() {
        let key = match character {
            '\r' => KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            '\u{1b}' => KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            character => typed(character),
        };
        press(app, area, key);
    }
}

/// Hands the application one key through the loop's own door.
fn press(app: &mut App, area: Rect, key: KeyEvent) {
    app.handle(area, &Event::Key(key));
}

/// Drives the application on the timer's own tick until `done`.
///
/// # Returns
///
/// Whether it came about within [`PATIENCE`].
fn settled(app: &mut App, area: Rect, done: impl Fn(&mut App) -> bool) -> bool {
    let deadline = Instant::now() + PATIENCE;
    loop {
        app.handle(area, &Event::Redraw);
        if done(app) {
            return true;
        }
        if deadline <= Instant::now() {
            return false;
        }
        thread::sleep(TICK);
    }
}

/// # Returns
///
/// A frame of `app` drawn into `area`.
fn drawn(app: &App, area: Rect) -> Cells {
    let mut cells = Cells::empty(area);
    app.draw(&mut cells, area);

    cells
}

/// # Returns
///
/// The text of every row of `cells`, top to bottom, trimmed of the blanks at its end.
fn rows(cells: &Cells) -> Vec<String> {
    let area = cells.area;

    (area.y..area.bottom())
        .map(|y| {
            let row: String = (area.x..area.right())
                .map(|x| cells[(x, y)].symbol().to_owned())
                .collect();

            row.trim_end().to_owned()
        })
        .collect()
}

/// # Returns
///
/// The text of the row `y` of `cells` right of the gutter, trimmed of the blanks at its end.
fn text(cells: &Cells, y: u16) -> String {
    let area = cells.area;
    let from = area.x + u16::try_from(GUTTER).expect("the gutter is narrow");
    let row: String = (from..area.right())
        .map(|x| cells[(x, y)].symbol().to_owned())
        .collect();

    row.trim_end().to_owned()
}

/// # Returns
///
/// Every row of `cells` whose gutter opens with `mark`, top to bottom.
fn marked(cells: &Cells, mark: &str) -> Vec<u16> {
    let area = cells.area;

    (area.y..area.bottom())
        .filter(|y| mark == cells[(area.x, *y)].symbol())
        .collect()
}

/// # Returns
///
/// The first row of `cells` whose text right of the gutter is `said`, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`anyhow::Error`] if no row draws it.
fn row_of(cells: &Cells, said: &str) -> Result<u16> {
    let area = cells.area;

    (area.y..area.bottom())
        .find(|y| said == text(cells, *y))
        .ok_or_else(|| anyhow!("no row draws `{said}`: {:#?}", rows(cells)))
}

/// # Returns
///
/// The rows of a frame of `app` the history is drawn in, which are those above the row dividing
/// it from the prompt.
fn history(app: &App, area: Rect) -> Vec<String> {
    let frame = rows(&drawn(app, area));
    let divider = frame
        .iter()
        .position(|row| DIVIDERS.iter().any(|mark| row.contains(mark)))
        .unwrap_or(frame.len());

    frame[..divider].to_vec()
}

/// Runs `measure` with the allocator counting.
///
/// # Returns
///
/// What `measure` returned, together with the bytes it asked the allocator for and the number of
/// times it asked.
fn counted<ValueType>(measure: impl FnOnce() -> ValueType) -> (ValueType, (usize, usize)) {
    ASKED_FOR.with(|asked| asked.set(0));
    CALLS.with(|calls| calls.set(0));
    let value = measure();

    (value, (ASKED_FOR.with(Cell::get), CALLS.with(Cell::get)))
}

/// # Returns
///
/// The window the program is watched in.
fn wide() -> Rect {
    Rect::new(0, 0, 120, 40)
}

/// # Returns
///
/// The smallest window a terminal is usually left at.
fn narrow() -> Rect {
    Rect::new(0, 0, 80, 24)
}

/// # Returns
///
/// The key `character` typed with `CTRL` held.
fn control(character: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(character), KeyModifiers::CONTROL)
}
