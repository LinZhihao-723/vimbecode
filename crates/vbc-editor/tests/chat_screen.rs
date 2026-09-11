//! The conversation screen, driven the way a reader drives it.
//!
//! The binary is the history panel over the prompt panel over the status bar, and a reader writes
//! the next message in the prompt with vim's own keys and sends it with `:wq`. So every case here
//! builds the application the binary builds -- [`App::chat`] over a session -- and does what a
//! reader does: types at it through [`App::handle`], waits for the session on the timer's own tick,
//! and reads back what the stand-in was sent and the frame the reader would be looking at.
//!
//! What stands in for the model is the pair of stand-ins the client's own tests are driven
//! against, each of which writes down every line it is sent, so that what a keystroke sent is read
//! off the child rather than off the application that believes it sent it.
//!
//! The screen is drawn in two windows, the one the program is watched in and the smallest a
//! terminal is usually left at, because the prompt's share of the window and the room the status
//! bar has for the session's identifier are both properties of the window a fixture would fix.

#![cfg(target_os = "linux")]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::fs;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use modalkit::env::vim::VimMode;
use ratatui::buffer::Buffer as Cells;
use ratatui::layout::Rect;
use serde_json::{json, Value};
use tempfile::TempDir;
use vbc_editor::app::{App, Focus, Outcome};
use vbc_editor::engine::typed;
use vbc_editor::event::Event;
use vbc_editor::session::identity::{Identity, SessionId};
use vbc_editor::session::live::Session;
use vbc_editor::session::spawn::Spawn;
use vbc_layout::buffer::Buffer;

/// The stand-ins the screen is driven against: the one that answers turns, and the one that stops
/// for approvals and starts turns it never ends.
const STUB: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/session/stub.sh");
const CONTROL: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/session/control.sh");

/// The file a stand-in writes down every line it is sent in, the file that tells the control
/// stand-in to start a turn it never ends, the file it writes down what an interrupt asked of its
/// queue in, and the file it writes once the write it asked about is allowed.
const HEARD: &str = "heard";
const INTERRUPT: &str = "interrupt";
const CANCEL: &str = "cancel";
const WRITTEN_FILE: &str = "note.txt";

/// The file the stand-in answers its first turn out of, and the lines of the answer it is given
/// there, which are many more than the history has rows.
const SAID: &str = "said.1";
const LONG_ANSWER: usize = 200;

/// What the stand-ins say: the answer to a first turn and to a second, the turn that never ends,
/// and the turn a queue the interrupt kept drains into.
const TURN: &str = "turn 1";
const SECOND_TURN: &str = "turn 2";
const WORKING: &str = "working on it";
const DRAINED: &str = "the queue drained";

/// What the status bar says.
const INSERTING: &str = "-- INSERT --";
const READING: &str = "-- HISTORY --";
const RESPONDING: &str = "Claude is responding";
const IDLE: &str = "idle";
const WAITING: &str = "waiting on `Write` -- `:allow` or `:deny`";
const UNSENT: &str = "the draft is not sent";
const QUEUED: &str = "queued behind the turn that is running";
const UNDRAFTED: &str = "the draft is empty, so nothing was sent";
const UNINTERRUPTED: &str = "no turn is running to interrupt";
const MODEL: &str = "stub";

/// The marks the row between the panels carries, naming the panel that has the keys.
const PROMPT_MARK: &str = "▼ prompt";
const HISTORY_MARK: &str = "▲ history";

/// The fewest rows the prompt is drawn in, and the most it is drawn in in each of the two windows,
/// which is 40% of their rows.
const PROMPT_FLOOR: usize = 3;
const WIDE_CAP: usize = 16;
const NARROW_CAP: usize = 9;

/// The lines of the two drafts a frame's cost is compared over, both of which fill the prompt and
/// both of which are numbered in a gutter as wide as the other's.
const SHORT_DRAFT: usize = 2_000;
const LONG_DRAFT: usize = 9_000;

/// How long a case waits for a session to say something, and how long it is left between two
/// frames of the wait.
const PATIENCE: Duration = Duration::from_secs(20);
const TICK: Duration = Duration::from_millis(5);

/// How long a case goes on reading a session that is expected to say nothing more.
const QUIET: Duration = Duration::from_millis(300);

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

/// Validation 1: `ihello<Esc>:wq<CR>` sends exactly `hello` and leaves an empty draft in insert
/// mode with the keys still at the prompt, which is also where the screen opens.
#[test]
fn wq_sends_the_draft_and_leaves_an_empty_one_in_insert_mode() -> Result<()> {
    let area = wide();
    let directory = TempDir::new()?;
    let mut app = chatting(directory.path(), STUB)?;

    assert_eq!(
        Focus::Prompt,
        app.focus(),
        "the screen opened with the keys away from the prompt"
    );
    assert_eq!(
        VimMode::Insert,
        app.mode(),
        "the screen opened on a prompt a message cannot be typed straight into"
    );

    typing(&mut app, area, "\u{1b}ihello\u{1b}:wq\r");

    assert_eq!("", app.text().text(), "`:wq` kept the draft it sent");
    assert_eq!(
        Focus::Prompt,
        app.focus(),
        "`:wq` moved the keys away from the prompt"
    );
    assert_eq!(
        VimMode::Insert,
        app.mode(),
        "`:wq` left a prompt the next message cannot be typed straight into"
    );
    assert!(
        settled(&mut app, area, |_| !heard(directory.path()).is_empty()),
        "the stand-in was sent nothing in {PATIENCE:?}"
    );
    assert_eq!(vec!["hello".to_owned()], heard(directory.path()));

    typing(&mut app, area, "again\u{1b}:wq\r");

    assert!(
        settled(&mut app, area, |_| 2 == heard(directory.path()).len()),
        "the stand-in was sent no second message in {PATIENCE:?}"
    );
    assert_eq!(
        vec!["hello".to_owned(), "again".to_owned()],
        heard(directory.path())
    );

    typing(&mut app, area, "\u{1b}:wq\r");
    quiet(&mut app, area);

    assert_eq!(UNDRAFTED, app.status(), "an empty draft was sent silently");
    assert_eq!(
        2,
        heard(directory.path()).len(),
        "an empty draft reached the stand-in"
    );

    Ok(())
}

/// Validation 2: the prompt is a vim editor: `0dw` deletes, `u` restores what it deleted and
/// `<C-R>` deletes it again, and what `:wq` sends is the draft those keys left -- two lines made
/// with `o` included, newline and all.
#[test]
fn the_prompt_is_edited_with_vim_s_own_keys_and_sends_what_they_leave() -> Result<()> {
    let area = wide();
    let directory = TempDir::new()?;
    let mut app = chatting(directory.path(), STUB)?;

    typing(&mut app, area, "\u{1b}ione two\u{1b}0dw");

    assert_eq!(
        "two",
        app.text().text(),
        "`0dw` did not delete the first word"
    );

    typing(&mut app, area, "u");

    assert_eq!(
        "one two",
        app.text().text(),
        "`u` did not restore what `dw` deleted"
    );

    press(&mut app, area, control('r'));

    assert_eq!("two", app.text().text(), "`<C-R>` did not redo the delete");

    typing(&mut app, area, ":wq\r");

    assert!(
        settled(&mut app, area, |_| !heard(directory.path()).is_empty()),
        "the stand-in was sent nothing in {PATIENCE:?}"
    );
    assert_eq!(vec!["two".to_owned()], heard(directory.path()));

    typing(&mut app, area, "first\u{1b}osecond\u{1b}:wq\r");

    assert!(
        settled(&mut app, area, |_| 2 == heard(directory.path()).len()),
        "the stand-in was sent no second message in {PATIENCE:?}"
    );
    assert_eq!(
        "first\nsecond",
        heard(directory.path())[1],
        "a draft of two lines was sent without the newline between them"
    );

    Ok(())
}

/// Validation 3: `:q!` empties the draft and leaves the program running in insert mode, `:qa`
/// refuses to leave a draft nothing has sent, and `:qa!` leaves anyway.
#[test]
fn q_bang_discards_the_draft_and_qa_refuses_one_until_it_is_forced() {
    let area = wide();
    let mut app = App::chat();
    typing(&mut app, area, "a draft\u{1b}");

    assert_eq!(
        Outcome::Continues,
        typing(&mut app, area, ":q!\r"),
        "`:q!` ended the program rather than discarding the draft"
    );
    assert_eq!("", app.text().text(), "`:q!` kept the draft it discards");
    assert_eq!(
        VimMode::Insert,
        app.mode(),
        "`:q!` left a prompt the next message cannot be typed straight into"
    );

    typing(&mut app, area, "another\u{1b}");

    assert_eq!(
        Outcome::Continues,
        typing(&mut app, area, ":qa\r"),
        "`:qa` left a draft nothing had sent"
    );
    assert_eq!(
        "another",
        app.text().text(),
        "`:qa` refused and threw the draft away anyway"
    );
    assert!(
        app.status().contains(UNSENT),
        "`:qa` said {:?} rather than why it refused",
        app.status()
    );
    assert_eq!(
        Outcome::Stops,
        typing(&mut app, area, ":qa!\r"),
        "`:qa!` did not leave"
    );
}

/// Validation 3: `:q` in the history leaves the program as `:qa` does, refusing a draft nothing
/// has sent as `:qa` does, and `:q` in the prompt leaves over an empty draft.
#[test]
fn q_in_the_history_leaves_as_qa_does() {
    let area = wide();
    let mut app = App::chat();
    typing(&mut app, area, "words\u{1b}");
    press(&mut app, area, control('w'));
    typing(&mut app, area, "k");

    assert_eq!(Focus::History, app.focus());
    assert_eq!(
        Outcome::Continues,
        typing(&mut app, area, ":q\r"),
        "`:q` in the history left a draft nothing had sent"
    );
    assert_eq!("words", app.text().text());
    assert_eq!(
        Outcome::Stops,
        typing(&mut app, area, ":q!\r"),
        "`:q!` in the history did not leave"
    );

    let mut empty = App::chat();

    assert_eq!(
        Outcome::Stops,
        typing(&mut empty, area, "\u{1b}:q\r"),
        "`:q` over an empty draft did not leave"
    );
}

/// Validation 4: `<C-W>k` and `<C-W>j` move the keys to the history and back from normal and
/// visual mode in either panel, `<C-W>w` and `<C-W><C-W>` to the other panel, `<C-T>` from insert
/// mode, and insert mode's own `<C-W>` still deletes the word before the cursor.
#[test]
fn ctrl_w_moves_the_keys_between_the_panels_and_deletes_a_word_in_insert_mode() {
    let area = wide();
    let mut app = App::chat();
    typing(&mut app, area, "one two");
    press(&mut app, area, control('w'));

    assert_eq!(
        "one ",
        app.text().text(),
        "insert mode's `<C-W>` did not delete the word before the cursor"
    );
    assert_eq!(
        Focus::Prompt,
        app.focus(),
        "insert mode's `<C-W>` moved the keys"
    );

    typing(&mut app, area, "\u{1b}");
    press(&mut app, area, control('w'));
    typing(&mut app, area, "k");

    assert_eq!(
        Focus::History,
        app.focus(),
        "`<C-W>k` did not move the keys up"
    );
    let rows = frame(&app, area);
    assert!(
        rows.iter().any(|row| row.contains(HISTORY_MARK)),
        "the history has the keys and the screen does not say so: {rows:#?}"
    );
    assert!(
        rows[rows.len() - 1].contains(READING),
        "the status bar does not say the history has the keys: {rows:#?}"
    );

    press(&mut app, area, control('w'));
    typing(&mut app, area, "j");

    assert_eq!(
        Focus::Prompt,
        app.focus(),
        "`<C-W>j` did not move the keys down"
    );
    assert!(
        frame(&app, area)
            .iter()
            .any(|row| row.contains(PROMPT_MARK)),
        "the prompt has the keys and the screen does not say so"
    );

    press(&mut app, area, control('w'));
    press(&mut app, area, control('w'));

    assert_eq!(Focus::History, app.focus(), "`<C-W><C-W>` moved nothing");

    press(&mut app, area, control('w'));
    typing(&mut app, area, "w");

    assert_eq!(Focus::Prompt, app.focus(), "`<C-W>w` moved nothing");

    typing(&mut app, area, "v");
    press(&mut app, area, control('w'));
    typing(&mut app, area, "k");

    assert_eq!(
        Focus::History,
        app.focus(),
        "`<C-W>k` did not move the keys from visual mode in the prompt"
    );

    typing(&mut app, area, "v");
    press(&mut app, area, control('w'));
    typing(&mut app, area, "j");

    assert_eq!(
        Focus::Prompt,
        app.focus(),
        "`<C-W>j` did not move the keys from visual mode in the history"
    );

    typing(&mut app, area, "\u{1b}i");
    press(&mut app, area, control('t'));

    assert_eq!(
        Focus::History,
        app.focus(),
        "`<C-T>` did not move the keys from insert mode"
    );

    press(&mut app, area, control('t'));

    assert_eq!(Focus::Prompt, app.focus());
    assert_eq!(
        "one ",
        app.text().text(),
        "moving between the panels changed the draft"
    );
}

/// Validation 5: one frame draws the history over the prompt over the status bar, in both
/// windows: the question and its answer above, an empty draft numbered in the prompt's gutter
/// below, and the session's identifier, its model and the prompt's mode along the bottom.
#[test]
fn one_frame_draws_the_history_over_the_prompt_over_the_status_bar() -> Result<()> {
    for area in [wide(), narrow()] {
        let directory = TempDir::new()?;
        let mut app = chatting(directory.path(), STUB)?;
        typing(&mut app, area, "hello\u{1b}:wq\r");

        assert!(
            settled(&mut app, area, |app| app.panel().text().contains(TURN)),
            "the session answered nothing in {PATIENCE:?}"
        );

        let id = app
            .session()
            .map(|session| session.id().to_owned())
            .ok_or_else(|| anyhow!("the application holds no session"))?;
        let rows = frame(&app, area);
        let divider = rows
            .iter()
            .position(|row| row.contains(PROMPT_MARK))
            .ok_or_else(|| anyhow!("no row divides the panels in {area:?}: {rows:#?}"))?;
        let status = &rows[rows.len() - 1];

        assert!(
            rows[..divider].iter().any(|row| row.contains("hello")),
            "the history drew no question above the prompt in {area:?}: {rows:#?}"
        );
        assert!(
            rows[..divider].iter().any(|row| row.contains(TURN)),
            "the history drew no answer above the prompt in {area:?}: {rows:#?}"
        );
        assert_eq!(
            PROMPT_FLOOR,
            rows.len() - 2 - divider,
            "an empty draft was drawn in another number of rows in {area:?}: {rows:#?}"
        );
        assert!(
            rows[divider + 1].trim_start().starts_with('1'),
            "the prompt's gutter numbers no line in {area:?}: {rows:#?}"
        );
        assert!(
            status.contains(&id),
            "the status bar in {area:?} does not show the session's identifier: {status:?}"
        );
        assert!(
            status.contains(MODEL) && status.contains(INSERTING) && status.contains(IDLE),
            "the status bar in {area:?} does not show the model, the mode and the turn: \
             {status:?}"
        );
    }

    Ok(())
}

/// The status bar keeps the session's identifier where the row has no room left for the model
/// beside it, because the identifier is what the session is resumed by.
#[test]
fn the_status_bar_keeps_the_session_id_where_the_model_does_not_fit() -> Result<()> {
    let area = slim();
    let directory = TempDir::new()?;
    let mut app = chatting(directory.path(), STUB)?;
    typing(&mut app, area, "hello\u{1b}:wq\r");

    assert!(
        settled(&mut app, area, |app| app.panel().text().contains(TURN)),
        "the session answered nothing in {PATIENCE:?}"
    );

    let id = app
        .session()
        .map(|session| session.id().to_owned())
        .ok_or_else(|| anyhow!("the application holds no session"))?;
    let rows = frame(&app, area);
    let status = &rows[rows.len() - 1];

    assert!(
        status.contains(&id) && !status.contains(MODEL),
        "a row too narrow for the model and the identifier dropped the wrong one: {status:?}"
    );

    Ok(())
}

/// Validation 5: the prompt is as tall as its draft is drawn, wrapped rows included, between its
/// floor and its share of the window, and gives the rows back once the draft is discarded.
#[test]
fn the_prompt_grows_with_its_draft_and_stops_at_its_share_of_the_window() {
    for (area, cap) in [(wide(), WIDE_CAP), (narrow(), NARROW_CAP)] {
        let mut app = App::chat();

        assert_eq!(PROMPT_FLOOR, prompt_rows(&app, area));

        typing(&mut app, area, &numbered("first", 10).join("\r"));

        assert_eq!(
            cap.min(10),
            prompt_rows(&app, area),
            "a ten-line draft in {area:?}"
        );

        typing(
            &mut app,
            area,
            &format!("\r{}", numbered("second", 30).join("\r")),
        );

        assert_eq!(
            cap,
            prompt_rows(&app, area),
            "a forty-line draft in {area:?}"
        );
        let rows = frame(&app, area);
        assert!(
            rows.iter().any(|row| row.ends_with("second 30")),
            "the prompt does not draw the line the cursor is on in {area:?}: {rows:#?}"
        );

        typing(&mut app, area, "\u{1b}:q!\r");

        assert_eq!(
            PROMPT_FLOOR,
            prompt_rows(&app, area),
            "the prompt kept the rows of a draft it discarded in {area:?}"
        );
    }

    let mut wrapped = App::chat();
    typing(&mut wrapped, narrow(), &"x".repeat(300));

    assert_eq!(
        4,
        prompt_rows(&wrapped, narrow()),
        "a line of 300 columns wrapped into 76 was not drawn in four rows"
    );
}

/// Validation 5: a frame of the screen over a draft far taller than the prompt asks for what one
/// over a shorter draft asks for, because the prompt's height is counted no further than the rows
/// it can draw.
#[test]
fn a_frame_over_a_long_draft_asks_for_what_one_over_a_short_draft_asks_for() {
    let area = wide();
    let short = App::new(Buffer::from_text(&numbered("line", SHORT_DRAFT).join("\n"))).composing();
    let long = App::new(Buffer::from_text(&numbered("line", LONG_DRAFT).join("\n"))).composing();
    let mut cells = Cells::empty(area);

    short.draw(&mut cells, area);
    let (_, over_short) = counted(|| short.draw(&mut cells, area));
    long.draw(&mut cells, area);
    let (_, over_long) = counted(|| long.draw(&mut cells, area));

    assert_ne!(
        (0, 0),
        over_short,
        "the allocator counted nothing, so the comparison below compares nothing"
    );
    assert_eq!(
        over_short, over_long,
        "a frame over a {LONG_DRAFT}-line draft asked for {over_long:?}, and one over a \
         {SHORT_DRAFT}-line draft asked for {over_short:?}"
    );
}

/// The history follows what arrives while the prompt has the keys, so an answer to a conversation
/// taller than the history is drawn along its bottom rather than below its last row -- and it is
/// still drawn there once the keys move up to the history.
#[test]
fn an_answer_below_the_history_s_last_row_is_drawn_along_its_bottom() -> Result<()> {
    let area = wide();
    let directory = TempDir::new()?;
    fs::write(directory.path().join(SAID), spoken(LONG_ANSWER))?;
    let mut app = chatting(directory.path(), STUB)?;

    typing(&mut app, area, "first\u{1b}:wq\r");

    assert!(
        settled(&mut app, area, |app| app.panel().text().lines().count()
            > LONG_ANSWER),
        "the stand-in sent no long answer in {PATIENCE:?}"
    );

    typing(&mut app, area, "second\u{1b}:wq\r");

    assert!(
        settled(&mut app, area, |app| app
            .panel()
            .text()
            .contains(SECOND_TURN)),
        "the stand-in answered no second turn in {PATIENCE:?}"
    );
    let rows = frame(&app, area);
    let divider = rows
        .iter()
        .position(|row| row.contains(PROMPT_MARK))
        .ok_or_else(|| anyhow!("no row divides the panels: {rows:#?}"))?;
    assert!(
        rows[..divider].iter().any(|row| row.contains(SECOND_TURN)),
        "the answer arrived below the last row the history draws: {rows:#?}"
    );

    press(&mut app, area, key(KeyCode::Esc));
    press(&mut app, area, control('w'));
    typing(&mut app, area, "k");

    assert!(
        frame(&app, area)
            .iter()
            .any(|row| row.contains(SECOND_TURN)),
        "moving the keys up to the history carried it away from the answer it was drawing"
    );

    Ok(())
}

/// Validation 6: `<C-C>` during a turn sends the stand-in an interrupt that cancels the queue,
/// from the insert mode `:wq` left the prompt in, and a message queued behind the turn does not
/// run afterwards.
#[test]
fn ctrl_c_interrupts_the_running_turn_and_drops_what_was_queued_behind_it() -> Result<()> {
    let area = wide();
    let directory = TempDir::new()?;
    fs::write(directory.path().join(INTERRUPT), "")?;
    let mut app = chatting(directory.path(), CONTROL)?;

    typing(&mut app, area, "first\u{1b}:wq\r");

    assert!(
        settled(&mut app, area, |app| app.panel().text().contains(WORKING)),
        "the turn never began in {PATIENCE:?}: {:?}",
        app.panel().text()
    );
    assert_eq!(Some(RESPONDING.to_owned()), app.turn());
    let rows = frame(&app, area);
    assert!(
        rows[rows.len() - 1].contains(RESPONDING),
        "the status bar does not say a turn is running: {rows:#?}"
    );

    typing(&mut app, area, "second\u{1b}:wq\r");

    assert!(
        app.status().contains(QUEUED),
        "a message sent during a turn was not said to be queued: {:?}",
        app.status()
    );
    assert!(
        settled(&mut app, area, |_| 2 == heard(directory.path()).len()),
        "the queued message never reached the stand-in in {PATIENCE:?}"
    );

    press(&mut app, area, control('c'));

    assert!(
        settled(&mut app, area, |_| Some("true")
            == cancel(directory.path()).as_deref()),
        "no interrupt cancelling the queue reached the stand-in in {PATIENCE:?}: {:?}",
        heard_lines(directory.path())
    );
    assert!(
        settled(&mut app, area, |app| Some(IDLE.to_owned()) == app.turn()),
        "the turn still runs after it was interrupted: {:?}",
        app.turn()
    );

    quiet(&mut app, area);

    assert!(
        !app.panel().text().contains(DRAINED),
        "the message queued behind the interrupted turn ran anyway"
    );

    Ok(())
}

/// Validation 6: `<C-C>` with no turn running sends the session nothing and leaves the program
/// running, before the first turn and after one has ended, from either panel.
#[test]
fn ctrl_c_with_no_turn_running_sends_nothing_and_leaves_the_program_running() -> Result<()> {
    let area = wide();
    let directory = TempDir::new()?;
    let mut app = chatting(directory.path(), STUB)?;

    assert_eq!(
        Outcome::Continues,
        press(&mut app, area, control('c')),
        "the interrupt ended the program before any turn"
    );
    assert_eq!(Some(UNINTERRUPTED), app.notice());

    typing(&mut app, area, "hello\u{1b}:wq\r");

    assert!(
        settled(&mut app, area, |app| {
            app.panel().text().contains(TURN) && Some(IDLE.to_owned()) == app.turn()
        }),
        "the turn did not end in {PATIENCE:?}: {:?}",
        app.turn()
    );
    assert_eq!(
        Outcome::Continues,
        press(&mut app, area, control('c')),
        "the interrupt ended the program after a turn had ended"
    );

    press(&mut app, area, control('t'));

    assert_eq!(Focus::History, app.focus());
    assert_eq!(
        Outcome::Continues,
        press(&mut app, area, control('c')),
        "the interrupt ended the program from the history"
    );
    assert_eq!(Some(UNINTERRUPTED), app.notice());

    quiet(&mut app, area);

    assert_eq!(
        1,
        heard_lines(directory.path()).len(),
        "an interrupt with no turn running reached the stand-in: {:?}",
        heard_lines(directory.path())
    );

    Ok(())
}

/// The status bar says what a turn is waiting on, and `:allow` typed at the prompt answers it.
#[test]
fn the_status_bar_says_what_the_turn_waits_on_and_the_prompt_answers_it() -> Result<()> {
    let area = wide();
    let directory = TempDir::new()?;
    let mut app = chatting(directory.path(), CONTROL)?;

    typing(&mut app, area, "hello\u{1b}:wq\r");

    assert!(
        settled(&mut app, area, |app| Some(WAITING.to_owned()) == app.turn()),
        "the turn never said what it waits on in {PATIENCE:?}: {:?}",
        app.turn()
    );
    let rows = frame(&app, area);
    assert!(
        rows[rows.len() - 1].contains(WAITING),
        "the status bar does not say what the turn waits on: {rows:#?}"
    );

    typing(&mut app, area, "\u{1b}:allow\r");

    assert!(
        settled(&mut app, area, |_| directory
            .path()
            .join(WRITTEN_FILE)
            .exists()),
        "`:allow` typed at the prompt never reached the session in {PATIENCE:?}"
    );
    assert!(
        settled(&mut app, area, |app| Some(IDLE.to_owned()) == app.turn()),
        "the turn did not end once it was answered: {:?}",
        app.turn()
    );

    Ok(())
}

/// # Returns
///
/// The screen the binary builds, over a session of the stand-in `binary` started in `directory`,
/// on success.
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

/// Types the characters of `keys` at `app` through the loop's own door, one at a time, a carriage
/// return standing for the return key and an escape for the escape key.
///
/// # Returns
///
/// What the last of them left the application asking for.
fn typing(app: &mut App, area: Rect, keys: &str) -> Outcome {
    let mut outcome = Outcome::Continues;
    for character in keys.chars() {
        outcome = match character {
            '\r' => press(app, area, key(KeyCode::Enter)),
            '\u{1b}' => press(app, area, key(KeyCode::Esc)),
            character => press(app, area, typed(character)),
        };
    }

    outcome
}

/// Hands the application one key through the loop's own door, so that whatever the session said
/// while it was being typed is read as well.
///
/// # Returns
///
/// What the key left the application asking for.
fn press(app: &mut App, area: Rect, key: KeyEvent) -> Outcome {
    app.handle(area, &Event::Key(key))
}

/// Drives the application on the timer's own tick until `done`, which is what a reader watching a
/// turn arrive does.
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
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(TICK);
    }
}

/// Drives the application on the timer's own tick for [`QUIET`], so that anything a session was
/// going to say after the last thing waited for has had the time to arrive.
fn quiet(app: &mut App, area: Rect) {
    let deadline = Instant::now() + QUIET;
    while Instant::now() < deadline {
        app.handle(area, &Event::Redraw);
        thread::sleep(TICK);
    }
}

/// # Returns
///
/// Every line the stand-in in `directory` was sent, which is none where it was sent nothing.
fn heard_lines(directory: &Path) -> Vec<String> {
    fs::read_to_string(directory.join(HEARD))
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

/// # Returns
///
/// The text of every message the stand-in in `directory` was sent from the reader, in the order it
/// was sent them.
fn heard(directory: &Path) -> Vec<String> {
    heard_lines(directory)
        .iter()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|frame| Some("user") == frame.get("type").and_then(Value::as_str))
        .filter_map(|frame| said_in(&frame))
        .collect()
}

/// # Returns
///
/// The text a user frame carries, whether its content is a string or a list of text parts, or
/// [`None`] where it carries no content.
fn said_in(frame: &Value) -> Option<String> {
    let content = frame.get("message")?.get("content")?;
    if let Some(text) = content.as_str() {
        return Some(text.to_owned());
    }

    Some(
        content
            .as_array()?
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect(),
    )
}

/// # Returns
///
/// What the control stand-in in `directory` wrote down an interrupt asked of its queue, or
/// [`None`] where it was sent no interrupt.
fn cancel(directory: &Path) -> Option<String> {
    fs::read_to_string(directory.join(CANCEL))
        .ok()
        .map(|asked| asked.trim().to_owned())
}

/// # Returns
///
/// The text of every row of a frame of `app` drawn into `area`, top to bottom, each trimmed of the
/// blanks the frame padded it to the width with.
fn frame(app: &App, area: Rect) -> Vec<String> {
    let mut cells = Cells::empty(area);
    app.draw(&mut cells, area);

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
/// The rows a frame of `app` drawn into `area` gives the prompt, which are those between the row
/// dividing it from the history and the status bar.
///
/// # Panics
///
/// Panics if the frame draws no row dividing the prompt from the history.
fn prompt_rows(app: &App, area: Rect) -> usize {
    let rows = frame(app, area);
    let divider = rows
        .iter()
        .position(|row| row.contains(PROMPT_MARK))
        .expect("a frame of the screen divides the prompt from the history");

    rows.len() - 2 - divider
}

/// # Returns
///
/// One assistant frame saying `lines` lines, written the way a session writes one.
fn spoken(lines: usize) -> String {
    let said = numbered("a line of a long answer", lines).join("\n");

    format!(
        "{}\n",
        json!({
            "type": "assistant",
            "parent_tool_use_id": Option::<String>::None,
            "session_id": "0f9c1c8a-0000-4000-8000-000000000002",
            "message": {
                "id": "msg_long",
                "type": "message",
                "role": "assistant",
                "model": MODEL,
                "content": [{"type": "text", "text": said}],
            },
        })
    )
}

/// # Returns
///
/// The lines `word 1` to `word count`, which are told apart by their numbers.
fn numbered(word: &str, count: usize) -> Vec<String> {
    (1..=count).map(|at| format!("{word} {at}")).collect()
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
/// A window narrower than the status bar's mode, turn, model and identifier together, and wide
/// enough for all of them but the model.
fn slim() -> Rect {
    Rect::new(0, 0, 60, 24)
}

/// # Returns
///
/// The key `character` typed with `CTRL` held.
fn control(character: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(character), KeyModifiers::CONTROL)
}

/// # Returns
///
/// The key `code` typed with no modifier held.
fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}
