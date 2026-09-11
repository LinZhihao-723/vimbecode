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
//! one, and it needs a session to hold, so the stand-in the client's own tests are driven against
//! is put on its path as `claude`. It is started in a directory of its own under a home of its
//! own, where the trust gate asks before anything is started and is told no. Nothing is typed at
//! it until it has said what it is waiting for, because a key sent before then is a key the
//! terminal is still buffering by line. Without `script` on the machine there is nothing to run
//! in, and the test says it was skipped rather than passing quietly -- except in continuous
//! integration, where a run that never ran the program is a run that checked nothing, so a missing
//! `script` fails there instead of being reported to nobody.

#![cfg(target_os = "linux")]

use std::io::{Read, Write};
use std::os::unix::fs::symlink;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use anyhow::{anyhow, Result};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::{Terminal, TerminalOptions, Viewport};
use tempfile::TempDir;
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

/// The cells the gutter draws the fixture's first and third lines in, escape sequence and all,
/// which is what says a frame reached the backend numbered as well as wrapped.
const FIRST_LINE_GUTTER: &str = "\u{1b}[38;5;8;49m  1 ";
const THIRD_LINE_GUTTER: &str = "\u{1b}[38;5;8;49m  3 ";

/// The stand-in put on the program's path as the `claude` it starts.
const STUB: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/session/stub.sh");
const CLAUDE: &str = "claude";

/// The file the stand-in writes down every line it is sent in.
const HEARD: &str = "heard";

/// What the trust gate asks on the terminal before the program has taken it over, and the answer
/// it is given, which is no.
const ASKED: &str = "[y/N]";
const WITHHELD: &[u8] = b"n\n";

/// What the status bar says while the prompt is in insert mode, which is the mode it opens in.
const INSERTING: &str = "INSERT";

/// The keys a draft is written and sent by, each run of them written on its own: the draft typed
/// straight into the insert mode the prompt opens in, the escape out of it, and `:wq`.
const TYPED_DRAFT: &[&[u8]] = &[b"hello", b"\x1b", b":wq\r"];
const DRAFT: &str = "hello";

/// The word the stand-in's answer to the first turn begins with, looked for on its own because a
/// terminal is written the cells a frame changed and the blank after it is a blank already.
const ANSWERED: &str = "turn";

/// The keys the program is left by: the escape out of the insert mode `:wq` left the prompt in,
/// and `:qa`, which leaves over the empty draft `:wq` left.
const TYPED_QUIT: &[&[u8]] = &[b"\x1b", b":qa\r"];

/// What the program says on its way out, ahead of the identifier the session is resumed by.
const RESUMED: &str = "vimbecode -r ";

/// How long the program is left between two runs of the keys typed at it, because an escape a
/// terminal reads in the same breath as the key after it is that key with alt held rather than an
/// escape.
const SETTLED: Duration = Duration::from_millis(200);

/// The rows and columns the program's terminal is given, since a terminal `script` makes for a
/// process whose own output is a pipe has no size of its own.
const TERMINAL_SIZE: &str = "stty rows 24 cols 80";

/// How long the program is given to say what it is waiting for, and to stop once it has been
/// asked to.
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

/// Validation 4 and 7: the program asks about the directory before it starts anything in it,
/// takes the terminal over, opens on a prompt in insert mode, sends the draft typed there with
/// `:wq`, draws the answer, shows the session's identifier, leaves on `:qa`, gives the terminal
/// back and stops cleanly.
#[test]
fn the_binary_sends_a_draft_to_its_session_and_quits() -> Result<()> {
    let home = TempDir::new()?;
    let directory = TempDir::new()?;
    let searched = TempDir::new()?;
    symlink(STUB, searched.path().join(CLAUDE))?;

    let Some(mut program) = start(home.path(), directory.path(), searched.path())? else {
        assert!(
            std::env::var_os(CONTINUOUS_INTEGRATION).is_none(),
            "`script` is not installed, so the program was never run in a terminal"
        );
        eprintln!("skipped: `script` is not installed, so there is no terminal to run in");
        return Ok(());
    };
    let mut keys = program
        .stdin
        .take()
        .ok_or_else(|| anyhow!("the program was started without a standard input"))?;
    let chunks = read_chunks(&mut program)?;

    let mut written: Vec<u8> = Vec::new();
    let mut typed = 0;
    let waypoints: [(&str, &[&[u8]]); 3] = [
        (ASKED, &[WITHHELD]),
        (INSERTING, TYPED_DRAFT),
        (ANSWERED, TYPED_QUIT),
    ];
    loop {
        match chunks.recv_timeout(PATIENCE) {
            Ok(chunk) => {
                written.extend(chunk);
                while let Some((seen, next)) = waypoints.get(typed) {
                    if !holds(&written, seen) {
                        break;
                    }
                    for run in *next {
                        keys.write_all(run)?;
                        keys.flush()?;
                        thread::sleep(SETTLED);
                    }
                    typed += 1;
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {
                let _ = program.kill();
                return Err(anyhow!(
                    "the program neither drew nor stopped in {PATIENCE:?}: {:?}",
                    String::from_utf8_lossy(&written)
                ));
            }
        }
    }
    drop(keys);
    let status = program.wait()?;
    let seen = String::from_utf8_lossy(&written).into_owned();

    assert_eq!(
        waypoints.len(),
        typed,
        "the program answered {typed} of the keys typed at it: {seen:?}"
    );
    assert!(
        std::fs::read_to_string(directory.path().join(HEARD))
            .unwrap_or_default()
            .contains(DRAFT),
        "the draft never reached the session: {seen:?}"
    );
    assert!(
        holds(&written, ENTER_ALTERNATE_SCREEN),
        "the program never took the terminal over: {seen:?}"
    );
    assert!(
        holds(&written, LEAVE_ALTERNATE_SCREEN),
        "the program kept the terminal it took over: {seen:?}"
    );
    assert!(status.success(), "the program stopped with {status}");

    let said = seen
        .find(RESUMED)
        .ok_or_else(|| anyhow!("the program never said how to resume its session: {seen:?}"))?;
    let id = seen[said + RESUMED.len()..]
        .split('`')
        .next()
        .unwrap_or_default();
    assert!(
        !id.is_empty() && seen[..said].contains(id),
        "the status bar never showed the identifier `{id}` the session is resumed by: {seen:?}"
    );

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

/// Starts the program in a terminal of its own, in `directory`, under the home `home`, finding the
/// `claude` it starts in `searched` ahead of everywhere else.
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
/// * Forwards [`std::env::join_paths`]'s return values on failure.
/// * Forwards [`std::process::Command::spawn`]'s return values on failure.
fn start(home: &Path, directory: &Path, searched: &Path) -> Result<Option<Child>> {
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

    let inherited = std::env::var_os("PATH").unwrap_or_default();
    let path = std::env::join_paths(
        std::iter::once(searched.to_owned()).chain(std::env::split_paths(&inherited)),
    )?;
    let program = Command::new("script")
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
        .current_dir(directory)
        .env("HOME", home)
        .env("PATH", path)
        .env("TERM", "xterm-256color")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;

    Ok(Some(program))
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
fn read_chunks(program: &mut Child) -> Result<Receiver<Vec<u8>>> {
    let mut written = program
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
