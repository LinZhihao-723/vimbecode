//! The program itself: the backend it draws through, and the run it makes of a real terminal.
//!
//! A library that can draw a frame is not yet a program, and the two things a program adds are the
//! ones nothing else exercises: a backend that turns cells into the bytes a terminal understands,
//! and a loop that draws, reads a key and stops. Both are checked here, and neither is checked by
//! looking at the library's own cells -- what is asserted is the byte stream that left the backend,
//! and the exit status of a process.
//!
//! The backend is exercised without a terminal at all: `CrosstermBackend` writes to anything that
//! takes bytes, so a frame is drawn into a vector and what it wrote is read back. That runs
//! everywhere, CI included.
//!
//! The program itself needs a terminal to be a program, so it is run under `script`, which makes
//! one. Nothing is typed at it until it has said it took the terminal over, because a key sent
//! before then is a key the terminal is still buffering by line. Without `script` on the machine
//! there is nothing to run in, and the test says it was skipped rather than passing quietly --
//! except in continuous integration, where a run that never ran the program is a run that checked
//! nothing, so a missing `script` fails there instead of being reported to nobody.

use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use anyhow::{anyhow, Result};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::{Terminal, TerminalOptions, Viewport};
use vbc_editor::app::App;
use vbc_layout::buffer::Buffer;
use vbc_layout::viewport::Command as Scroll;

/// The columns and rows a frame is drawn into, which are fewer rows than the fixture has lines.
const WIDTH: u16 = 24;
const HEIGHT: u16 = 4;

/// The text the backend draws, whose lines are told apart in the byte stream by their own words.
const FIXTURE: &str = "\
first line 中文
second line
third line
fourth line
fifth line 中文";

/// The escape sequences the program is required to write, which are the terminal it took over and
/// the terminal it gave back.
const ENTER_ALTERNATE_SCREEN: &str = "\u{1b}[?1049h";
const LEAVE_ALTERNATE_SCREEN: &str = "\u{1b}[?1049l";

/// Words of the program's built-in passage, each of which the byte stream holds unbroken: a
/// terminal is written a run of cells at a time, and a run ends wherever a cell was left as it
/// already was.
const DRAWN_WORDS: [&str; 3] = ["gutter", "numbers", "logical"];

/// The keys typed at the running program, and what its terminal is written once they have been
/// answered. `gUU` upper-cases the line `j` moved onto, which nothing but a keystroke that reached
/// the engine can put on the screen, and `i` puts the editor in the mode its status line names.
/// What is looked for is a run of cells a frame changed rather than the whole word it changed them
/// in, because a terminal is written the cells that differ: `T` upper-cases to itself, and the
/// blanks around the words of a status line are blanks the frame before it drew there too.
const TYPED_EDIT: &[u8] = b"jgUU";
const UPPER_CASED: &str = "GUTTER";
const TYPED_INSERT: &[u8] = b"i";
const INSERTING: &str = "INSERT";

/// The interrupt the program is stopped by once it is in a mode its own `q` is text in.
const INTERRUPT: &[u8] = b"\x03";

/// A character of the built-in passage that no terminal measures by counting characters, which is
/// what says the frame that reached the terminal went through the layout.
const DRAWN_CJK: &str = "折";

/// The rows and columns the program's terminal is given, since a terminal `script` makes for a
/// process whose own output is a pipe has no size of its own.
const TERMINAL_SIZE: &str = "stty rows 24 cols 80";

/// The cells the gutter draws the fixture's first and third lines in, escape sequence and all,
/// which is what says a frame reached the backend numbered as well as wrapped.
const FIRST_LINE_GUTTER: &str = "\u{1b}[38;5;8;49m  1 ";
const THIRD_LINE_GUTTER: &str = "\u{1b}[38;5;8;49m  3 ";

/// How long the program is given to draw its first frame, and to stop once it has been asked to.
const PATIENCE: Duration = Duration::from_secs(20);

/// The variable a continuous-integration run sets, which is where a terminal to run the program in
/// is required rather than looked for.
const CONTINUOUS_INTEGRATION: &str = "CI";

/// Validation 4: a frame drawn through `CrosstermBackend` reaches the bytes a terminal is written
/// with, rather than stopping at the library's own cells.
#[test]
fn a_frame_reaches_the_crossterm_backend() -> Result<()> {
    let area = Rect::new(0, 0, WIDTH, HEIGHT);
    let mut app = App::new(Buffer::from_text(FIXTURE));

    let unscrolled = written(&app, area)?;
    app.scroll(area, Scroll::HalfPageDown)?;
    let scrolled = written(&app, area)?;

    assert!(
        holds(&unscrolled, "first") && !holds(&unscrolled, "fifth"),
        "the backend was written a frame the window does not show: {:?}",
        String::from_utf8_lossy(&unscrolled)
    );
    assert!(
        holds(&scrolled, "fifth") && !holds(&scrolled, "first"),
        "the scroll never reached the backend: {:?}",
        String::from_utf8_lossy(&scrolled)
    );
    assert!(
        holds(&unscrolled, FIRST_LINE_GUTTER) && holds(&scrolled, THIRD_LINE_GUTTER),
        "the gutter the frames were drawn with never reached the backend"
    );
    assert!(
        holds(&unscrolled, "中") && holds(&unscrolled, "文"),
        "the wide text never reached the backend: {:?}",
        String::from_utf8_lossy(&unscrolled)
    );

    Ok(())
}

/// Draws one frame of an application through a `CrosstermBackend` writing into memory.
///
/// # Returns
///
/// The bytes the backend wrote on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`ratatui::Terminal::with_options`]'s return values on failure.
/// * Forwards [`ratatui::Terminal::draw`]'s return values on failure.
fn written(app: &App, area: Rect) -> Result<Vec<u8>> {
    let mut bytes: Vec<u8> = Vec::new();
    let mut terminal = Terminal::with_options(
        CrosstermBackend::new(&mut bytes),
        TerminalOptions {
            viewport: Viewport::Fixed(area),
        },
    )?;
    terminal.draw(|frame| app.render(frame))?;
    drop(terminal);

    Ok(bytes)
}

/// Validation 4: the program builds, takes a terminal over, draws a frame into it, edits its text
/// by the vim keys typed at it, says which mode it is in, gives the terminal back and stops.
#[test]
fn the_binary_draws_a_frame_types_vim_keys_and_quits() -> Result<()> {
    let Some(mut editor) = start()? else {
        assert!(
            std::env::var_os(CONTINUOUS_INTEGRATION).is_none(),
            "`script` is not installed, so the program was never run in a terminal"
        );
        eprintln!("skipped: `script` is not installed, so there is no terminal to run in");
        return Ok(());
    };
    let mut keys = editor
        .stdin
        .take()
        .ok_or_else(|| anyhow!("the program was started without a standard input"))?;
    let chunks = read_chunks(&mut editor)?;

    let mut written: Vec<u8> = Vec::new();
    let mut typed = 0;
    let waypoints: [(&str, &[u8]); 3] = [
        (ENTER_ALTERNATE_SCREEN, TYPED_EDIT),
        (UPPER_CASED, TYPED_INSERT),
        (INSERTING, INTERRUPT),
    ];
    loop {
        match chunks.recv_timeout(PATIENCE) {
            Ok(chunk) => {
                written.extend(chunk);
                while let Some((seen, next)) = waypoints.get(typed) {
                    if !holds(&written, seen) {
                        break;
                    }
                    keys.write_all(next)?;
                    keys.flush()?;
                    typed += 1;
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {
                let _ = editor.kill();
                return Err(anyhow!(
                    "the program neither drew nor stopped in {PATIENCE:?}: {:?}",
                    String::from_utf8_lossy(&written)
                ));
            }
        }
    }
    drop(keys);
    let status = editor.wait()?;
    let seen = String::from_utf8_lossy(&written).into_owned();

    assert_eq!(
        waypoints.len(),
        typed,
        "the program answered {typed} of the keys typed at it: {seen:?}"
    );
    for answer in [UPPER_CASED, INSERTING] {
        assert!(
            holds(&written, answer),
            "the program drew none of `{answer}`, so the keys typed at it reached no engine: \
             {seen:?}"
        );
    }
    for word in DRAWN_WORDS {
        assert!(
            holds(&written, word),
            "the program drew none of `{word}` into its terminal: {seen:?}"
        );
    }
    assert!(
        holds(&written, DRAWN_CJK),
        "the program drew none of its wide text: {seen:?}"
    );
    assert!(
        holds(&written, LEAVE_ALTERNATE_SCREEN),
        "the program kept the terminal it took over: {seen:?}"
    );
    assert!(status.success(), "the program stopped with {status}");

    Ok(())
}

/// # Returns
///
/// Whether `written` holds `text`, read as the bytes a terminal was written with rather than as a
/// string, so that a chunk split through a character is still searched.
fn holds(written: &[u8], text: &str) -> bool {
    written
        .windows(text.len())
        .any(|window| window == text.as_bytes())
}

/// Starts the program in a terminal of its own.
///
/// # Returns
///
/// The running program on success, or [`None`] if the machine has no `script` to make a terminal
/// with.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`std::process::Command::spawn`]'s return values on failure.
fn start() -> Result<Option<Child>> {
    if Command::new("script")
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_err()
    {
        return Ok(None);
    }

    let editor = Command::new("script")
        .args([
            "--quiet",
            "--flush",
            "--command",
            &format!(
                "{TERMINAL_SIZE}; exec '{}'",
                env!("CARGO_BIN_EXE_vimbecode")
            ),
            "/dev/null",
        ])
        .env("TERM", "xterm-256color")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;

    Ok(Some(editor))
}

/// Reads what the program writes to its terminal on a thread of its own, so the test can wait on
/// it with a deadline.
///
/// # Returns
///
/// The chunks the program writes, ending when it stops writing, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`anyhow::Error`] if the program was started without a standard output.
fn read_chunks(editor: &mut Child) -> Result<Receiver<Vec<u8>>> {
    let mut written = editor
        .stdout
        .take()
        .ok_or_else(|| anyhow!("the program was started without a standard output"))?;
    let (sender, chunks) = mpsc::channel();
    thread::spawn(move || {
        let mut chunk = [0_u8; 4096];
        while let Ok(read) = written.read(&mut chunk) {
            if 0 == read {
                return;
            }
            if sender.send(chunk[..read].to_vec()).is_err() {
                return;
            }
        }
    });

    Ok(chunks)
}
