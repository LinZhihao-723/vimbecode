//! The program resumed: `-r` finds a session's transcript from any directory, opens on what the
//! session already said before anything new is said, and runs its child in the directory it was
//! started in -- and an identifier nothing is stored under stops the program before the terminal
//! is touched.
//!
//! The program is run the way `binary.rs` runs it: under `script`, which gives it a terminal, with
//! the stand-in the client's own tests are driven against on its path as `claude`, and under a
//! home of its own, which is where it looks for the transcripts Claude Code keeps. A transcript is
//! put there under the directory a session was started in, and the program is started from
//! another one.
//!
//! The long case puts twenty thousand entries there and reads off the terminal how long the first
//! frame and the history each took to be drawn. Without `script` there is nothing to run in, and
//! the cases say they were skipped -- except in continuous integration, where that fails.

#![cfg(target_os = "linux")]

use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::symlink;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use serde_json::json;
use tempfile::TempDir;
use vbc_editor::session::stored::{key, CONFIG_DIR, CONFIG_HOME, EXTENSION, PROJECTS};

/// The stand-in put on the program's path, and the name it is put there under.
const STUB: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/session/stub.sh");
const CLAUDE: &str = "claude";

/// A transcript Claude Code kept of a real session, the identifier it is kept under, and the
/// directory it names as the one the session was started in.
const RECORDED: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/session/resumed/db8d3c4f-a11c-447d-abda-eec9fa76c9e4.jsonl"
);
const RECORDED_ID: &str = "db8d3c4f-a11c-447d-abda-eec9fa76c9e4";
const RECORDED_IN: &str = "/tmp/s3-resume/a.b_c d-é";

/// What the recorded session said last, which is what the history panel draws along its bottom.
const LAST_SAID: &str = "PostCompact";

/// An identifier no session is stored under.
const UNKNOWN_ID: &str = "00000000-0000-4000-8000-000000000000";

/// The files the stand-in writes into the directory it is started in: the arguments it was given,
/// and every line it was sent.
const ARGS: &str = "args";
const HEARD: &str = "heard";

/// What the terminal is sent when the program takes it over.
const ENTER_ALTERNATE_SCREEN: &str = "\u{1b}[?1049h";

/// What the program asks a directory's trust with, and the answer it is given.
const ASKED: &str = "[y/N]";
const WITHHELD: &[u8] = b"n\n";

/// What the status bar says once the first frame is drawn, and the keys that leave the program.
const INSERTING: &str = "INSERT";
const TYPED_QUIT: &[&[u8]] = &[b"\x1b", b":qa\r"];

/// What the program says before the screen opens about where a resumed session runs.
const RESUMING: &str = "vimbecode: resuming ";

/// How many exchanges the long transcript holds, each of them [`EXCHANGE_ENTRIES`] entries.
const EXCHANGES: usize = 2_000;
const EXCHANGE_ENTRIES: usize = 10;

/// The rows and columns the program's terminal is given.
const TERMINAL_SIZE: &str = "stty rows 40 cols 120";

/// How long the program is left between two runs of the keys typed at it.
const SETTLED: Duration = Duration::from_millis(200);

/// How long the program is given to draw what is waited for, and to stop once asked to.
const PATIENCE: Duration = Duration::from_secs(20);

/// The variable a continuous-integration run sets.
const CONTINUOUS_INTEGRATION: &str = "CI";

/// Validation 4: an identifier nothing is stored under is said to be one, the program stops with a
/// failure, and it neither asks about a directory nor takes the terminal over first.
#[test]
fn an_identifier_nothing_is_stored_under_stops_before_the_terminal_is_taken() -> Result<()> {
    let home = TempDir::new()?;
    let directory = TempDir::new()?;
    let searched = searched()?;
    let Some(mut program) = start(
        home.path(),
        directory.path(),
        searched.path(),
        &["-r", UNKNOWN_ID],
    )?
    else {
        return skipped();
    };
    let chunks = read_chunks(&mut program)?;

    let written = collected(&mut program, &chunks, &[])?;
    let status = program.wait()?;
    let seen = String::from_utf8_lossy(&written);

    assert!(
        holds(&written, &format!("no session `{UNKNOWN_ID}`")),
        "the program never said the session is unknown: {seen:?}"
    );
    assert!(!status.success(), "the program stopped with {status}");
    assert!(
        !holds(&written, ENTER_ALTERNATE_SCREEN),
        "the program took the terminal over: {seen:?}"
    );
    assert!(
        !holds(&written, ASKED),
        "the program asked about a directory before it knew there was a session: {seen:?}"
    );
    assert!(
        !directory.path().join(ARGS).exists(),
        "the program started a child"
    );

    Ok(())
}

/// Validations 1 and 2: started from a directory other than the session's own, the program says
/// it is resuming the session where it was started, opens on what the session already said
/// without having said anything to it, and starts its child there, resuming it.
#[test]
fn a_resumed_session_opens_on_its_history_in_the_directory_it_was_started_in() -> Result<()> {
    let home = TempDir::new()?;
    let started = TempDir::new()?;
    let elsewhere = TempDir::new()?;
    let searched = searched()?;
    let started_in = fs::canonicalize(started.path())?;
    let transcript =
        fs::read_to_string(RECORDED)?.replace(RECORDED_IN, &started_in.display().to_string());
    kept(home.path(), &started_in, &transcript)?;
    let Some(mut program) = start(
        home.path(),
        elsewhere.path(),
        searched.path(),
        &["-r", RECORDED_ID],
    )?
    else {
        return skipped();
    };
    let chunks = read_chunks(&mut program)?;

    let waypoints: [(&str, &[&[u8]]); 2] = [(ASKED, &[WITHHELD]), (LAST_SAID, TYPED_QUIT)];
    let written = collected(&mut program, &chunks, &waypoints)?;
    let status = program.wait()?;
    let seen = String::from_utf8_lossy(&written);
    let args = fs::read_to_string(started.path().join(ARGS)).unwrap_or_default();

    assert!(
        status.success(),
        "the program stopped with {status}: {seen:?}"
    );
    assert!(
        holds(
            &written,
            &format!("{RESUMING}{RECORDED_ID} in {}", started_in.display())
        ),
        "the program never said where it resumed the session: {seen:?}"
    );
    assert!(
        args.contains(&format!("--resume\n{RECORDED_ID}\n")),
        "the child was not started in the session's own directory, resuming it: {args:?}"
    );
    assert!(
        !elsewhere.path().join(ARGS).exists(),
        "a child was started in the directory the program was started from"
    );
    assert!(
        !started.path().join(HEARD).exists(),
        "the history was drawn after something was said to the session"
    );

    Ok(())
}

/// Validation 5: twenty thousand entries of history hold up neither the first frame nor the
/// frames after it; the first frame is drawn before the history is, and both times are reported.
#[test]
fn a_long_history_is_drawn_after_the_first_frame_rather_than_ahead_of_it() -> Result<()> {
    let home = TempDir::new()?;
    let started = TempDir::new()?;
    let searched = searched()?;
    let started_in = fs::canonicalize(started.path())?;
    kept(home.path(), &started_in, &long(&started_in))?;
    let Some(mut program) = start(
        home.path(),
        started.path(),
        searched.path(),
        &["-r", RECORDED_ID],
    )?
    else {
        return skipped();
    };
    let mut keys = program
        .stdin
        .take()
        .ok_or_else(|| anyhow!("the program was started without a standard input"))?;
    let chunks = read_chunks(&mut program)?;
    let last = format!("done-{:05}", EXCHANGES - 1);

    let mut written: Vec<u8> = Vec::new();
    let mut answered: Option<Instant> = None;
    let mut first_frame: Option<(Duration, usize)> = None;
    let mut history: Option<(Duration, usize)> = None;
    while history.is_none() {
        let chunk = chunks.recv_timeout(PATIENCE).map_err(|_| {
            let _ = program.kill();
            anyhow!(
                "the program drew no history in {PATIENCE:?}: {:?}",
                String::from_utf8_lossy(&written)
            )
        })?;
        written.extend(chunk);
        if answered.is_none() && holds(&written, ASKED) {
            keys.write_all(WITHHELD)?;
            keys.flush()?;
            answered = Some(Instant::now());
        }
        let Some(since) = answered else {
            continue;
        };
        if first_frame.is_none() {
            first_frame = found(&written, INSERTING).map(|at| (since.elapsed(), at));
        }
        history = found(&written, &last).map(|at| (since.elapsed(), at));
    }
    thread::sleep(SETTLED);
    for run in TYPED_QUIT {
        keys.write_all(run)?;
        keys.flush()?;
        thread::sleep(SETTLED);
    }
    let written = collected(&mut program, &chunks, &[])?;
    drop(keys);
    let status = program.wait()?;

    let (first_frame, drawn_at) =
        first_frame.ok_or_else(|| anyhow!("the history was drawn before any frame was"))?;
    let (history, recalled_at) = history.ok_or_else(|| anyhow!("the history was never drawn"))?;
    eprintln!(
        "{} entries: first frame {first_frame:?} after the trust answer, history drawn after \
         {history:?}",
        EXCHANGES * EXCHANGE_ENTRIES
    );
    assert!(
        drawn_at < recalled_at,
        "the history was drawn in the first frame rather than after it"
    );
    assert!(
        status.success(),
        "the program stopped with {status}: {:?}",
        String::from_utf8_lossy(&written)
    );

    Ok(())
}

/// # Returns
///
/// A directory holding the stand-in under the name the program starts, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`tempfile::TempDir::new`]'s return values on failure.
/// * Forwards [`std::os::unix::fs::symlink`]'s return values on failure.
fn searched() -> Result<TempDir> {
    let searched = TempDir::new()?;
    symlink(STUB, searched.path().join(CLAUDE))?;

    Ok(searched)
}

/// Keeps `transcript` under `home` where Claude Code keeps the transcript of the recorded session
/// started in `started_in`.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`std::fs::create_dir_all`]'s return values on failure.
/// * Forwards [`std::fs::write`]'s return values on failure.
fn kept(home: &Path, started_in: &Path, transcript: &str) -> Result<()> {
    let project = home.join(CONFIG_HOME).join(PROJECTS).join(key(started_in));
    fs::create_dir_all(&project)?;
    fs::write(
        project.join(format!("{RECORDED_ID}.{EXTENSION}")),
        transcript,
    )?;

    Ok(())
}

/// # Returns
///
/// A transcript of [`EXCHANGES`] exchanges started in `started_in`, each of them the entries a
/// real exchange is stored as: a prompt, two attachments, a thinking block, a reply fencing some
/// code, a tool call and what it answered, a closing reply, a hook summary and a note of the last
/// prompt.
fn long(started_in: &Path) -> String {
    let directory = started_in.display().to_string();
    let mut transcript = String::new();
    for exchange in 0..EXCHANGES {
        let call = format!("toolu_{exchange:05}");
        let said = |content: serde_json::Value| {
            json!({
                "type": "assistant",
                "isSidechain": false,
                "cwd": directory,
                "message": {"role": "assistant", "model": "claude-haiku-4-5", "content": [content]},
            })
        };
        let entries = [
            json!({
                "type": "user",
                "isSidechain": false,
                "promptSource": "sdk",
                "cwd": directory,
                "message": {"role": "user", "content": [{
                    "type": "text",
                    "text": format!("question {exchange:05}: what does this directory hold?"),
                }]},
            }),
            json!({"type": "attachment", "cwd": directory, "attachment": {"type": "todo"}}),
            json!({"type": "attachment", "cwd": directory, "attachment": {"type": "skills"}}),
            said(json!({"type": "thinking", "thinking": "", "signature": ""})),
            said(json!({
                "type": "text",
                "text": format!(
                    "It holds one program, which prints the number of this exchange when it is \
                     run; the listing below says what else is there.\n\n```rust\nfn main() {{\n    \
                     println!(\"{exchange}\");\n}}\n```"
                ),
            })),
            said(json!({
                "type": "tool_use",
                "id": call,
                "name": "Bash",
                "input": {"command": "ls -la"},
            })),
            json!({
                "type": "user",
                "isSidechain": false,
                "cwd": directory,
                "message": {"role": "user", "content": [{
                    "type": "tool_result",
                    "tool_use_id": call,
                    "content": "total 8\ndrwxr-xr-x 2 a a 4096 .\ndrwxr-xr-x 3 a a 4096 ..",
                    "is_error": false,
                }]},
            }),
            said(json!({"type": "text", "text": format!("done-{exchange:05}")})),
            json!({"type": "system", "subtype": "stop_hook_summary", "cwd": directory}),
            json!({"type": "last-prompt", "lastPrompt": format!("question {exchange:05}")}),
        ];
        for entry in entries {
            transcript.push_str(&entry.to_string());
            transcript.push('\n');
        }
    }

    transcript
}

/// Starts the program with `arguments` in a terminal of its own, in `directory`, under the home
/// `home`, finding the `claude` it starts in `searched` ahead of everywhere else.
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
fn start(
    home: &Path,
    directory: &Path,
    searched: &Path,
    arguments: &[&str],
) -> Result<Option<Child>> {
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
            "--return",
            "--command",
            &format!(
                "{TERMINAL_SIZE}; exec '{}' {}",
                env!("CARGO_BIN_EXE_vimbecode"),
                arguments.join(" ")
            ),
            "/dev/null",
        ])
        .current_dir(directory)
        .env("HOME", home)
        .env_remove(CONFIG_DIR)
        .env("PATH", path)
        .env("TERM", "xterm-256color")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;

    Ok(Some(program))
}

/// Reads what the program writes until it stops writing, typing each waypoint's keys once the
/// waypoint has been written.
///
/// # Returns
///
/// Everything the program wrote, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`anyhow::Error`] if the program neither stopped nor wrote anything for [`PATIENCE`], or it
///   stopped before every waypoint was written.
/// * Forwards [`std::io::Write::write_all`]'s return values on failure.
/// * Forwards [`std::io::Write::flush`]'s return values on failure.
fn collected(
    program: &mut Child,
    chunks: &Receiver<Vec<u8>>,
    waypoints: &[(&str, &[&[u8]])],
) -> Result<Vec<u8>> {
    let mut keys = program.stdin.take();
    let mut written: Vec<u8> = Vec::new();
    let mut typed = 0;
    loop {
        match chunks.recv_timeout(PATIENCE) {
            Ok(chunk) => {
                written.extend(chunk);
                while let Some((seen, next)) = waypoints.get(typed) {
                    if !holds(&written, seen) {
                        break;
                    }
                    if let Some(keys) = keys.as_mut() {
                        for run in *next {
                            keys.write_all(run)?;
                            keys.flush()?;
                            thread::sleep(SETTLED);
                        }
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
    if waypoints.len() != typed {
        return Err(anyhow!(
            "the program wrote {typed} of the {} waypoints: {:?}",
            waypoints.len(),
            String::from_utf8_lossy(&written)
        ));
    }

    Ok(written)
}

/// # Returns
///
/// Whether `written` holds `text`, read as bytes so that a chunk split through a character is
/// still searched.
fn holds(written: &[u8], text: &str) -> bool {
    found(written, text).is_some()
}

/// # Returns
///
/// Where `text` is first written in `written`, or [`None`] where it is not.
fn found(written: &[u8], text: &str) -> Option<usize> {
    written
        .windows(text.len())
        .position(|window| window == text.as_bytes())
}

/// Says that a case was skipped for want of a terminal, which continuous integration refuses.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`anyhow::Error`] if the run is continuous integration's.
fn skipped() -> Result<()> {
    if std::env::var_os(CONTINUOUS_INTEGRATION).is_some() {
        return Err(anyhow!(
            "`script` is not installed, so the program was never run in a terminal"
        ));
    }
    eprintln!("skipped: `script` is not installed, so there is no terminal to run in");

    Ok(())
}

/// Reads what the program writes to its terminal on a thread of its own.
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
