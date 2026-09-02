//! Checks the hazard the event source was designed around, against a real crossterm reading a
//! real terminal.
//!
//! crossterm reads the terminal it is attached to, so the only way to hand it a partial escape
//! sequence is to give it a terminal of its own. This test therefore runs a probe of itself under
//! `script`, which allocates one, types `ESC[` into it, and watches what the probe keeps
//! reporting. Two things are asserted: that crossterm's read stops returning, which is the reason
//! the reading has a thread to itself, and that redraw ticks keep arriving anyway, which is what
//! the application needs. Should a later crossterm stop wedging, the first assertion is what says
//! so.
//!
//! Without `script` on the machine there is no terminal to run in, and the test says it was
//! skipped rather than passing quietly.

#![cfg(target_os = "linux")]

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use crossterm::event::Event as TerminalEvent;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use vbc_editor::event::reader::{Reader, TerminalReader};
use vbc_editor::event::{Config, Event, Source};

/// The bytes typed into the probe's terminal to wedge crossterm's parser: a CSI introducer whose
/// sequence never arrives.
const BARE_CSI: &[u8] = b"\x1b[";

/// How long the probe runs before giving up on its parent, which only matters if the parent dies
/// without killing it.
const PROBE_LIFETIME: Duration = Duration::from_secs(20);

/// The name of the test the probe runs as.
const PROBE_TEST: &str = "the_probe_that_runs_in_a_terminal";

/// The environment variable that tells the probe it was started by its parent rather than by
/// somebody running the ignored tests by hand.
const PROBE_VARIABLE: &str = "VBC_PTY_PROBE";

/// How long the parent waits for a line it expects from the probe.
const PATIENCE: Duration = Duration::from_secs(10);

/// How long the parent gives the probe to reach the read that never returns, which is longer than
/// the read it may already have been waiting in when the partial sequence arrived.
const SETTLE: Duration = Duration::from_millis(500);

/// How long the parent watches the probe after wedging its terminal.
const WATCH: Duration = Duration::from_secs(2);

/// The tick interval the probe runs its timer at.
const TICK: Duration = Duration::from_millis(50);

/// A terminal that reports every read it returns from, so the parent can tell a read that is
/// merely idle from one that never returns.
struct ReportingReader {
    terminal: TerminalReader,
}

impl Reader for ReportingReader {
    type Error = <TerminalReader as Reader>::Error;

    fn poll(&mut self, timeout: Duration) -> Result<bool, Self::Error> {
        let ready = self.terminal.poll(timeout)?;
        println!("poll");
        Ok(ready)
    }

    fn read(&mut self) -> Result<TerminalEvent, Self::Error> {
        self.terminal.read()
    }
}

#[test]
fn a_bare_csi_prefix_wedges_crossterm_without_wedging_the_application() -> Result<()> {
    const TICKS: usize = 5;

    let Some(mut probe) = start_probe()? else {
        eprintln!("skipped: `script` is not installed, so there is no terminal to run in");
        return Ok(());
    };
    let lines = read_lines(&mut probe)?;
    let mut stdin = probe
        .stdin
        .take()
        .ok_or_else(|| anyhow!("the probe was started without a standard input"))?;

    expect(&lines, "ready")?;

    stdin.write_all(b"z")?;
    stdin.flush()?;
    expect(&lines, "key Char('z')")?;

    stdin.write_all(BARE_CSI)?;
    stdin.flush()?;

    thread::sleep(SETTLE);
    while lines.try_recv().is_ok() {}

    let watched = Instant::now();
    let mut ticks = 0;
    let mut polls = 0;
    while watched.elapsed() < WATCH {
        match lines.recv_timeout(WATCH) {
            Ok(line) if "tick" == line => ticks += 1,
            Ok(line) if "poll" == line => polls += 1,
            Ok(_) | Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }

    // The probe has served its purpose, and whether it is still alive to be killed and reaped
    // makes no difference to what was watched.
    let _ = probe.kill();
    let _ = probe.wait();

    assert_eq!(
        polls, 0,
        "crossterm returned from {polls} reads in {WATCH:?} after being handed a partial escape \
         sequence, so it no longer wedges and the reason the reading has a thread of its own has \
         changed"
    );
    assert!(
        ticks >= TICKS,
        "only {ticks} redraw ticks arrived in {WATCH:?} while crossterm's read was wedged"
    );
    Ok(())
}

#[test]
#[ignore = "the probe of `a_bare_csi_prefix_wedges_crossterm_without_wedging_the_application`"]
fn the_probe_that_runs_in_a_terminal() -> Result<()> {
    if std::env::var_os(PROBE_VARIABLE).is_none() {
        return Ok(());
    }

    enable_raw_mode()?;
    println!("ready");

    let source = Source::start(
        ReportingReader {
            terminal: TerminalReader::new(),
        },
        Config::default().with_tick_interval(TICK),
    );

    let started = Instant::now();
    while started.elapsed() < PROBE_LIFETIME {
        match source.recv_timeout(PROBE_LIFETIME) {
            Ok(Event::Redraw) => println!("tick"),
            Ok(Event::Key(key)) => println!("key {:?}", key.code),
            Ok(event) => println!("event {event:?}"),
            Err(_) => break,
        }
    }

    disable_raw_mode()?;
    Ok(())
}

/// Waits for the probe to report the given line.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`anyhow::Error`] if the probe stopped reporting before the line arrived.
fn expect(lines: &Receiver<String>, expected: &str) -> Result<()> {
    let deadline = Instant::now() + PATIENCE;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let reported = lines
            .recv_timeout(remaining)
            .map_err(|_| anyhow!("the probe never reported `{expected}`"))?;
        if expected == reported {
            return Ok(());
        }
    }
}

/// Reads what the probe reports on a thread of its own, so the parent can wait on lines with a
/// deadline.
///
/// # Returns
///
/// The lines the probe reports on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`anyhow::Error`] if the probe was started without a standard output.
fn read_lines(probe: &mut Child) -> Result<Receiver<String>> {
    let stdout = probe
        .stdout
        .take()
        .ok_or_else(|| anyhow!("the probe was started without a standard output"))?;
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if sender.send(line.trim_end().to_owned()).is_err() {
                return;
            }
        }
    });
    Ok(receiver)
}

/// Starts the probe in a terminal of its own.
///
/// # Returns
///
/// The running probe on success, or `None` if the machine has no `script` to make a terminal
/// with.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`std::env::current_exe`]'s return values on failure.
/// * Forwards [`std::process::Command::spawn`]'s return values on failure.
fn start_probe() -> Result<Option<Child>> {
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

    let probe = std::env::current_exe()?;
    let command = format!(
        "'{}' --exact {PROBE_TEST} --ignored --nocapture",
        probe.display()
    );
    let child = Command::new("script")
        .args(["--quiet", "--flush", "--command", &command, "/dev/null"])
        .env(PROBE_VARIABLE, "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(Some(child))
}
