//! The panel reading a transcript a session wrote, driven the way a reader drives it.
//!
//! The claim P.2 rests on is that the chat panel is unchanged: the folds, the text objects and the
//! yanks were written against a compiled-in exchange, and they have to answer a transcript built
//! out of a session's own frames without knowing that is what they are looking at. So nothing here
//! builds a [`vbc_editor::chat::block::Block`]. Every case decodes the frames a real Claude Code
//! session was recorded writing, hands the blocks they become to an [`App`], and types the keys a
//! reader types: `<C-T>` to reach the panel, `j` to walk it, `viac` to select the code and `yac`
//! to take it, `zR` to open what is folded, and `p` on the other side of `<C-T>` to put what was
//! taken into the file.
//!
//! Three of the cases are about what the panel must not have lost. `yac` takes the code the
//! session sent, byte for byte and without the fences it arrived inside, and `yat` takes the text
//! a tool wrote and none of the escapes that coloured it -- a leak of either is a reader pasting
//! something they did not read. And the nesting the frames arrived tagged with reaches the panel:
//! what a subagent said is a line of the panel once the call that started it is opened and not
//! before, which a panel handed the blocks without their tags would draw beside what the session
//! itself said.
//!
//! The last is the cost the anchored panel was built for, spelled against a session rather than
//! against a fixture. A conversation grows without bound, so a frame drawn deep in a long one has
//! to cost the screenful it draws and not the conversation above it. The long message is a frame
//! the session wrote like every other block here, its lines are tab-indented so that the rows they
//! take cannot be read off their lengths, and the frame is the one `App::draw` writes.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::fs;
use std::hint::black_box;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer as Cells;
use ratatui::layout::Rect;
use serde_json::json;
use vbc_editor::app::{App, Focus};
use vbc_editor::chat::selection::{Mode, Source};
use vbc_editor::session::blocks::Conversation;
use vbc_editor::session::event::Event;
use vbc_layout::buffer::Buffer;
use vbc_layout::width::Metrics;

/// The turn every case is read over, which is the one a real session was recorded writing.
const ANSWERED: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/session/answered.ndjson");

/// The window the panel is drawn in, which is a terminal a reader would use.
const COLUMNS: u16 = 80;
const ROWS: u16 = 20;

/// The file the reader has open behind the transcript, which is where a put lands.
const FILE: &str = "a file the reader left open";

/// What the reader asked, which the child never echoes and the outbox therefore puts in itself.
const QUESTION: &str = "add a todo to main, and show me the diff";

/// The code the recorded answer fenced, byte for byte as it was sent.
const CODE: &str = "fn main() {\n    todo!();\n}";

/// What the recorded build wrote, once the escapes that coloured it have been read as styles.
const WROTE: &str = "   Compiling vimbecode v0.0.0\n    Finished `dev` profile in 0.42s";

/// The byte an escape sequence opens with, which is the one byte a yank may not hand back.
const ESCAPE: char = '\u{1b}';

/// What the subagent the recorded session started said, which is a line of the panel only once
/// the call that started it has been opened.
const REPORTED: &str = "Running the suite now.";

/// The rows of the folded panel the reader walks down to: the first line of the code the answer
/// fenced, and, once every fold is open, the first line of what the build wrote.
const INSIDE_THE_CODE: usize = 3;
const INSIDE_WHAT_THE_TOOL_WROTE: usize = 8;

/// The lines of the message the cost is measured over, and the two rows of the panel a frame of it
/// is drawn from. The first is past everything else the session said, so both frames draw the same
/// message and the whole of the conversation sits above each of them; the second is ten thousand
/// tab-indented lines further down, which a panel that walked to the row it draws would lay out on
/// the way there.
const LONG: usize = 20_000;
const SHALLOW: usize = 64;
const DEEP: usize = 10_064;

/// The number of runs a timing takes the fastest of, which is what keeps a machine's own noise out
/// of a ratio.
const RUNS: usize = 9;

/// The factor by which a frame drawn deep in a long conversation may cost more in time than the
/// same frame at its top.
const MARGIN: u32 = 8;

/// The allocator every measurement here is read through.
#[global_allocator]
static ALLOCATOR: Counting = Counting;

thread_local! {
    /// The bytes this thread has asked for since the last [`counted`] began, given back or not.
    static ASKED: Cell<usize> = const { Cell::new(0) };

    /// The number of times it asked for them.
    static CALLS: Cell<usize> = const { Cell::new(0) };
}

/// An allocator that counts what it was asked for and hands the asking on.
struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = System.alloc(layout);
        if !pointer.is_null() {
            let _ = ASKED.try_with(|asked| asked.set(asked.get() + layout.size()));
            let _ = CALLS.try_with(|calls| calls.set(calls.get() + 1));
        }

        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        System.dealloc(pointer, layout);
    }
}

#[test]
fn a_session_is_what_the_panel_draws() -> Result<()> {
    let app = reading(None)?;
    let mut cells = Cells::empty(area());
    app.draw(&mut cells, area());
    let drawn = frame(&cells);

    assert_eq!(Some(&QUESTION.to_owned()), drawn.first());
    assert_eq!(
        Some(&"fn main() {".to_owned()),
        drawn.get(INSIDE_THE_CODE),
        "the code the session fenced is not drawn where the panel says it is: {drawn:?}"
    );
    assert!(
        drawn.iter().any(|row| row.starts_with("+--")),
        "nothing the session said folded away, so the panel drew a thinking block and a tool \
         result in full: {drawn:?}"
    );

    Ok(())
}

#[test]
fn what_a_subagent_said_is_folded_away_under_the_call_that_started_it() -> Result<()> {
    let mut app = reading(None)?;

    assert!(
        !app.panel().text().lines().any(|line| REPORTED == line),
        "what the subagent said is drawn beside what the session said, so the panel was handed \
         the blocks of the conversation without the calls they arrived beneath: {:?}",
        app.panel().text()
    );

    press(&mut app, "zR");

    assert!(
        app.panel().text().lines().any(|line| REPORTED == line),
        "opening every fold did not reach what the subagent said: {:?}",
        app.panel().text()
    );

    Ok(())
}

#[test]
fn iac_selects_the_code_a_session_sent() -> Result<()> {
    let mut app = reading(None)?;
    walk(&mut app, INSIDE_THE_CODE);
    press(&mut app, "viac");

    let selected = app
        .panel()
        .selection()
        .ok_or(anyhow!("`viac` left no selection behind"))?;
    let transcript = app.panel().transcript().clone();
    let block = transcript.block(selected.block()).ok_or(anyhow!(
        "`viac` selected a block the transcript does not hold"
    ))?;
    let source = Source::new(block.source(), Metrics::default());

    assert_eq!(Mode::Charwise, selected.selection().mode());
    assert_eq!(CODE, selected.selection().text(source));

    Ok(())
}

#[test]
fn yac_takes_the_code_a_session_sent_and_p_puts_it_in_the_file() -> Result<()> {
    let mut app = reading(None)?;
    walk(&mut app, INSIDE_THE_CODE);
    press(&mut app, "yac");
    cross(&mut app);
    press(&mut app, "p");

    assert_eq!(Focus::Prompt, app.focus(), "`<C-T>` did not come back");
    assert_eq!(format!("{FILE}\n{CODE}"), app.text().text());

    Ok(())
}

#[test]
fn yat_takes_what_the_tool_wrote_and_none_of_the_escapes_that_coloured_it() -> Result<()> {
    let mut app = reading(None)?;
    press(&mut app, "zR");
    walk(&mut app, INSIDE_WHAT_THE_TOOL_WROTE);
    press(&mut app, "yat");
    cross(&mut app);
    press(&mut app, "p");

    let put = app.text().text();

    assert_eq!(format!("{FILE}\n{WROTE}"), put);
    assert!(
        !put.contains(ESCAPE),
        "an escape the tool coloured its output with reached the file the reader put it in"
    );

    Ok(())
}

#[test]
fn a_frame_deep_in_a_long_session_asks_for_what_one_at_its_top_asks_for() -> Result<()> {
    let mut app = reading(Some(LONG))?;
    let mut cells = Cells::empty(area());

    scrolled(&mut app, SHALLOW);
    app.draw(&mut cells, area());
    let (_, at_the_top) = counted(|| app.draw(&mut cells, area()));
    let quickest = fastest(|| app.draw(&mut cells, area()));

    scrolled(&mut app, DEEP - SHALLOW);
    app.draw(&mut cells, area());
    let (_, deep) = counted(|| app.draw(&mut cells, area()));
    let taken = fastest(|| app.draw(&mut cells, area()));

    assert_eq!(
        at_the_top, deep,
        "a frame at row {DEEP} of a {LONG}-line message of a session asked for {deep:?}, and the \
         same frame at its row {SHALLOW} asked for {at_the_top:?}"
    );
    assert!(
        taken < quickest * MARGIN,
        "a frame at row {DEEP} of a {LONG}-line message of a session took {taken:?}, and the same \
         frame at its row {SHALLOW} took {quickest:?}"
    );

    Ok(())
}

/// # Returns
///
/// The application a reader types at, over the recorded session, with the transcript panel already
/// reached by `<C-T>`. Where `long` names a number of lines, the session says one more thing of
/// that many tab-indented lines, which is the message the cost is measured over.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`anyhow::Error`] if `<C-T>` did not reach the panel.
/// * Forwards [`session`]'s return values on failure.
fn reading(long: Option<usize>) -> Result<App> {
    let (transcript, tags) = session(long)?.into_panel();
    let mut app = App::new(Buffer::from_text(FILE)).with_conversation(transcript, tags);
    app.press(area(), control('t'));
    app.press(area(), typed('0'));

    if Focus::History != app.focus() {
        return Err(anyhow!("`<C-T>` reached no panel"));
    }

    Ok(app)
}

/// # Returns
///
/// The conversation the recorded frames become, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`std::fs::read_to_string`]'s return values on failure.
/// * Forwards [`Event::decoded`]'s return values on failure.
fn session(long: Option<usize>) -> Result<Conversation> {
    let recorded = fs::read_to_string(ANSWERED)?;
    let mut conversation = Conversation::new();
    conversation.asked(QUESTION);
    for line in recorded.lines().filter(|line| !line.trim().is_empty()) {
        conversation.read(&Event::decoded(line)?);
    }
    if let Some(lines) = long {
        conversation.read(&Event::decoded(&spoken(lines))?);
    }

    Ok(conversation)
}

/// # Returns
///
/// One assistant frame saying `lines` tab-indented lines, written the way the session writes one.
/// Every line is the same length, so two frames of the message differ by where they are drawn from
/// and by nothing else.
fn spoken(lines: usize) -> String {
    let mut said = String::new();
    for line in 0..lines {
        said.push_str(&format!("\tline {line:05} of a very long answer\n"));
    }

    json!({
        "type": "assistant",
        "parent_tool_use_id": Option::<String>::None,
        "session_id": "0f9c1c8a-0000-4000-8000-000000000001",
        "message": {
            "id": "msg_long",
            "type": "message",
            "role": "assistant",
            "model": "claude-haiku-4-5",
            "content": [{"type": "text", "text": said}],
        },
    })
    .to_string()
}

/// Types `keys` at the application, one key at a time, at whichever half has them.
fn press(app: &mut App, keys: &str) {
    for key in keys.chars() {
        app.press(area(), typed(key));
    }
}

/// Carries the cursor of whichever half has the keys `rows` rows down.
fn walk(app: &mut App, rows: usize) {
    press(app, &"j".repeat(rows));
}

/// Gives the keys to the other half of the application, as `<C-T>` does.
fn cross(app: &mut App) {
    app.press(area(), control('t'));
}

/// Scrolls the panel `rows` rows further down, a `CTRL-E` at a time, which is the only way a
/// reader moves it.
fn scrolled(app: &mut App, rows: usize) {
    for _ in 0..rows {
        app.press(area(), control('e'));
    }
}

/// Runs `measure` with the allocator counting.
///
/// # Returns
///
/// What `measure` returned, together with the bytes it asked the allocator for and the number of
/// times it asked.
fn counted<ValueType>(measure: impl FnOnce() -> ValueType) -> (ValueType, (usize, usize)) {
    ASKED.with(|asked| asked.set(0));
    CALLS.with(|calls| calls.set(0));
    let value = measure();

    (value, (ASKED.with(Cell::get), CALLS.with(Cell::get)))
}

/// Runs `measure` [`RUNS`] times, which is what keeps a machine's own noise out of a ratio.
///
/// # Returns
///
/// The fastest of those runs.
fn fastest<ValueType>(mut measure: impl FnMut() -> ValueType) -> Duration {
    let mut quickest = Duration::MAX;
    for _ in 0..RUNS {
        let started = Instant::now();
        let value = measure();
        quickest = quickest.min(started.elapsed());
        black_box(&value);
    }

    quickest
}

/// # Returns
///
/// The text of every row of the frame in `cells`, top to bottom, each trimmed of the blanks the
/// frame padded it to the width with.
fn frame(cells: &Cells) -> Vec<String> {
    let area = area();

    (area.y..area.bottom())
        .map(|y| {
            let drawn: String = (area.x..area.right())
                .map(|x| cells[(x, y)].symbol().to_owned())
                .collect();

            drawn.trim_end().to_owned()
        })
        .collect()
}

/// # Returns
///
/// The area every case is driven in.
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
