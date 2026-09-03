//! Checks the five things the clipboard helper's life is for: that it is up before the first paste
//! asks anything of it, that a clipboard which fails does not cost us the process serving it, that
//! a clipboard which keeps failing is left alone rather than hammered, that a process which has
//! died is replaced, and that a session which ends takes its helper with it.
//!
//! Every one of these is a property of a real process rather than of a type, so a real process is
//! what they are run against. The helper the editor ships is PowerShell, which the machines these
//! tests run on need not have; the stand-in is a shell script speaking the same protocol over the
//! same pipes, and being a stand-in is what makes it useful here, because it can be told to fail on
//! command, told to stop failing, and killed. It records what was done to it in files of its own --
//! every start, with its pid, and the time of every request it was handed -- so what these tests
//! assert is not the supervisor's own account of itself. The counts and the pids agree, or the test
//! is red.
//!
//! The two costs are measured rather than assumed. A helper started with the session is worth
//! having only if the startup is really paid there, so the stand-in is given a slow start and the
//! first read afterwards is timed against it; and a backoff floor is worth having only if it really
//! holds the attempts apart, so the intervals asserted are the ones between the requests the
//! stand-in wrote down, not the ones the supervisor believes it left.

#![cfg(target_os = "linux")]

use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{self, Command};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use vbc_editor::clipboard::helper::{Error, Helper, Launch, Script, BACKOFF_FLOOR};
use vbc_editor::clipboard::protocol::Response;

/// The shell the stand-in helper is run by, and the stand-in itself.
const SHELL: &str = "/bin/sh";
const STUB: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/clipboard/stub.sh");

/// The script the real helper runs, which is the one the editor ships.
const SHIPPED_SCRIPT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/clipboard/helper.ps1");

/// The files the stand-in is steered by and writes its account of itself to.
const SPAWN_LOG: &str = "spawns";
const REQUEST_LOG: &str = "requests";
const FAIL_FLAG: &str = "fail";
const DEATH_FLAG: &str = "die";

/// What the stand-in answers a read with.
const CLIPBOARD_TEXT: &str = "what the clipboard holds";

/// How long the stand-in spends starting up when a test is measuring where that cost lands.
const STARTUP_DELAY: Duration = Duration::from_millis(1000);

/// The longest a read is allowed to take once the session has started. A read that paid
/// [`STARTUP_DELAY`] could not come in under this.
const READ_BOUND: Duration = Duration::from_millis(200);

/// The floor the backoff is required to respect, written out here rather than taken from the code:
/// it is the number the editor's responsiveness under a locked workstation rests on, so lowering it
/// should be a red test rather than an agreement between the code and itself.
const FLOOR: Duration = Duration::from_secs(5);

/// The attempts a failing clipboard is watched for, which is enough of them to measure the
/// intervals in between rather than one interval.
const ATTEMPTS_WATCHED: usize = 3;

/// The longest that watch waits before giving up on the retries arriving at all.
const WATCH_LIMIT: Duration = Duration::from_secs(30);

/// The longest a request refused by the backoff may take. The point of the floor is that a degraded
/// clipboard costs nothing, so a refusal that blocks is as bad as the retry it replaced.
const REFUSAL_BOUND: Duration = Duration::from_millis(50);

/// How long a process is waited on before the wait is called a failure.
const PROCESS_LIMIT: Duration = Duration::from_secs(5);

/// How often those waits look.
const POLL: Duration = Duration::from_millis(2);

/// A directory a test steers its stand-in through, taken away with the test.
struct Scratch {
    root: PathBuf,
}

impl Scratch {
    /// # Returns
    ///
    /// A directory of this test's own, on success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * Forwards [`std::fs::create_dir_all`]'s return values on failure.
    fn named(name: &str) -> Result<Self> {
        let root = env::temp_dir().join(format!("vbc-clipboard-{}-{name}", process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root)?;

        Ok(Self { root })
    }

    /// # Returns
    ///
    /// The path of one of the files in it.
    fn path(&self, leaf: &str) -> PathBuf {
        self.root.join(leaf)
    }

    /// # Returns
    ///
    /// The lines of one of those files, which is empty for one that was never written.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * Forwards [`std::fs::read_to_string`]'s return values on failure.
    fn lines(&self, leaf: &str) -> Result<Vec<String>> {
        let path = self.path(leaf);
        if !path.exists() {
            return Ok(Vec::new());
        }

        Ok(fs::read_to_string(path)?
            .lines()
            .map(str::to_owned)
            .collect())
    }

    /// Raises one of the flags the stand-in watches.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * Forwards [`std::fs::write`]'s return values on failure.
    fn raise(&self, leaf: &str) -> Result<()> {
        fs::write(self.path(leaf), [])?;

        Ok(())
    }

    /// Lowers one of them.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * Forwards [`std::fs::remove_file`]'s return values on failure.
    fn lower(&self, leaf: &str) -> Result<()> {
        fs::remove_file(self.path(leaf))?;

        Ok(())
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn the_first_read_of_a_session_does_not_pay_the_helpers_startup_cost() -> Result<()> {
    let scratch = Scratch::named("startup")?;
    let delay = STARTUP_DELAY.as_millis().to_string();
    let launch =
        stub(&scratch).with_environment("VBC_STUB_STARTUP_DELAY_MS".into(), OsString::from(&delay));

    let launching = Instant::now();
    let mut helper = Helper::launch(launch)?;
    let launched = launching.elapsed();

    let reading = Instant::now();
    let answer = helper.read()?;
    let read = reading.elapsed();

    assert_eq!(Response::Text(CLIPBOARD_TEXT.to_owned()), answer);
    assert!(
        READ_BOUND < STARTUP_DELAY,
        "a startup of {STARTUP_DELAY:?} is not slow enough for a read bounded at {READ_BOUND:?} \
         to say where the cost landed"
    );
    assert!(
        STARTUP_DELAY <= launched,
        "the session started in {launched:?}, which is less than the {STARTUP_DELAY:?} the helper \
         spends starting up, so the startup was not paid at launch"
    );
    assert!(
        read < READ_BOUND,
        "the first read took {read:?}, over the bound of {READ_BOUND:?}, so it was still paying \
         for the helper's startup"
    );
    assert_eq!(
        vec![helper
            .process_id()
            .ok_or_else(|| anyhow!("the helper is not running"))?
            .to_string()],
        scratch.lines(SPAWN_LOG)?,
        "the session started more than the one helper it needs"
    );

    Ok(())
}

#[test]
fn a_failing_clipboard_neither_costs_the_helper_nor_keeps_it_from_recovering() -> Result<()> {
    let scratch = Scratch::named("failure")?;
    let mut helper = Helper::launch(stub(&scratch))?;
    let started = helper
        .process_id()
        .ok_or_else(|| anyhow!("the helper is not running"))?;

    assert_eq!(
        Response::Text(CLIPBOARD_TEXT.to_owned()),
        helper.read()?,
        "the clipboard was not read before it was made to fail"
    );

    scratch.raise(FAIL_FLAG)?;
    let failed = helper.read()?;

    assert!(
        matches!(failed, Response::Failed(_)),
        "a clipboard that cannot be read answered {failed:?}"
    );
    assert_eq!(1, helper.launches(), "the failure restarted the helper");
    assert_eq!(
        Some(started),
        helper.process_id(),
        "the failure replaced the process serving it"
    );
    assert!(
        running(started),
        "the process that reported the failure is gone"
    );
    assert_eq!(vec![started.to_string()], scratch.lines(SPAWN_LOG)?);
    assert!(
        helper.degraded_until().is_some(),
        "a clipboard that failed is not degraded"
    );

    let refused = helper.read();

    assert!(
        matches!(refused, Err(Error::Degraded { .. })),
        "a request inside the backoff was answered with {refused:?} rather than refused"
    );

    scratch.lower(FAIL_FLAG)?;
    wait_out(&helper);
    let recovered = helper.read()?;

    assert_eq!(
        Response::Text(CLIPBOARD_TEXT.to_owned()),
        recovered,
        "the clipboard did not recover when the failure cleared"
    );
    assert_eq!(1, helper.launches());
    assert_eq!(
        vec![started.to_string()],
        scratch.lines(SPAWN_LOG)?,
        "recovering cost a restart"
    );
    assert_eq!(None, helper.degraded_until());

    Ok(())
}

#[test]
fn the_backoff_floor_holds_the_attempts_at_a_failing_clipboard_apart() -> Result<()> {
    assert!(
        FLOOR <= BACKOFF_FLOOR,
        "the floor is {BACKOFF_FLOOR:?}, under the {FLOOR:?} a degraded clipboard must be left \
         alone for"
    );

    let scratch = Scratch::named("backoff")?;
    let asked = stub(&scratch).with_backoff(Duration::from_millis(1));

    assert_eq!(
        BACKOFF_FLOOR,
        asked.backoff(),
        "a backoff under the floor was taken at its word"
    );

    scratch.raise(FAIL_FLAG)?;
    let mut helper = Helper::launch(asked)?;
    let watching = Instant::now();
    let mut refusals: u64 = 0;
    let mut slowest = Duration::ZERO;
    while scratch.lines(REQUEST_LOG)?.len() < ATTEMPTS_WATCHED && watching.elapsed() < WATCH_LIMIT {
        let asking = Instant::now();
        let answer = helper.read();
        if matches!(answer, Err(Error::Degraded { .. })) {
            refusals += 1;
            slowest = slowest.max(asking.elapsed());
        }
        thread::sleep(POLL);
    }

    let attempts = stamps(&scratch)?;

    assert!(
        ATTEMPTS_WATCHED <= attempts.len(),
        "only {} of {ATTEMPTS_WATCHED} attempts reached the helper in {WATCH_LIMIT:?}, so the \
         intervals between them say nothing",
        attempts.len()
    );
    for pair in attempts.windows(2) {
        let apart = pair[1] - pair[0];
        assert!(
            FLOOR <= apart,
            "two attempts at a failing clipboard were {apart:?} apart, under the floor of {FLOOR:?}"
        );
    }
    assert!(
        0 < refusals,
        "no request was refused between the attempts, so nothing was held back"
    );
    assert!(
        slowest < REFUSAL_BOUND,
        "a request refused by the backoff took {slowest:?}, over the bound of {REFUSAL_BOUND:?}, \
         which is the freeze the backoff exists to prevent"
    );

    Ok(())
}

#[test]
fn a_helper_that_died_is_started_again_and_answers_the_next_read() -> Result<()> {
    let scratch = Scratch::named("death")?;
    let mut helper = Helper::launch(stub(&scratch))?;
    let started = helper
        .process_id()
        .ok_or_else(|| anyhow!("the helper is not running"))?;

    assert!(running(started), "the helper is not running to be killed");

    kill(started)?;
    await_death(started)?;
    let answer = helper.read()?;
    let replacement = helper
        .process_id()
        .ok_or_else(|| anyhow!("the helper was not started again"))?;

    assert_eq!(
        Response::Text(CLIPBOARD_TEXT.to_owned()),
        answer,
        "the read after the helper died was not answered"
    );
    assert_ne!(started, replacement, "the dead process answered");
    assert_eq!(2, helper.launches());
    assert_eq!(
        vec![started.to_string(), replacement.to_string()],
        scratch.lines(SPAWN_LOG)?,
        "the helper that answered is not the one the supervisor says it started"
    );
    assert_eq!(
        None,
        helper.degraded_until(),
        "a dead process degraded the clipboard, which is a failing clipboard's answer"
    );

    Ok(())
}

#[test]
fn a_helper_that_dies_part_way_through_is_replaced_once_and_then_the_clipboard_degrades(
) -> Result<()> {
    let scratch = Scratch::named("mid-request")?;
    let mut helper = Helper::launch(stub(&scratch))?;

    scratch.raise(DEATH_FLAG)?;
    let broken = helper.read();

    assert!(
        matches!(broken, Err(Error::Broken(_))),
        "a helper that stopped answering came back as {broken:?}"
    );
    assert_eq!(
        2,
        helper.launches(),
        "a helper dying under every request was restarted more than the once, or not at all"
    );
    assert_eq!(2, scratch.lines(SPAWN_LOG)?.len());
    assert!(
        helper.degraded_until().is_some(),
        "a helper that could not be replaced left the clipboard undegraded, so the next request \
         would start another one"
    );

    Ok(())
}

#[test]
fn a_session_that_shuts_down_leaves_no_helper_behind() -> Result<()> {
    let scratch = Scratch::named("shutdown")?;
    let mut helper = Helper::launch(stub(&scratch))?;
    let started = helper
        .process_id()
        .ok_or_else(|| anyhow!("the helper is not running"))?;

    assert!(
        running(started),
        "the helper is not running to be shut down"
    );

    helper.shutdown();

    assert_eq!(None, helper.process_id());
    await_exit(started)?;

    Ok(())
}

#[test]
fn a_session_dropped_without_a_shutdown_leaves_no_helper_behind() -> Result<()> {
    let scratch = Scratch::named("drop")?;
    let helper = Helper::launch(stub(&scratch))?;
    let started = helper
        .process_id()
        .ok_or_else(|| anyhow!("the helper is not running"))?;

    assert!(running(started), "the helper is not running to be dropped");

    drop(helper);
    await_exit(started)?;

    Ok(())
}

#[test]
fn the_helper_script_is_laid_down_where_windows_cannot_take_it_away() -> Result<()> {
    let scratch = Scratch::named("script")?;
    let shipped = fs::read_to_string(SHIPPED_SCRIPT)?;
    let path = {
        let script = Script::lay_down(&scratch.root)?;
        let path = script.path().to_path_buf();

        assert_eq!(
            shipped,
            fs::read_to_string(&path)?,
            "the script laid down is not the one the editor ships"
        );
        assert!(
            path.starts_with(&scratch.root),
            "the script was laid down at {path:?}, outside the directory it was given"
        );
        assert!(
            !path.starts_with("/mnt/"),
            "the script was laid down at {path:?}, on a filesystem Windows can delete it from"
        );

        path
    };

    assert!(
        !path.exists(),
        "the script at {path:?} outlived the session that laid it down"
    );

    Ok(())
}

#[test]
fn the_helper_the_editor_ships_serves_the_real_clipboard() -> Result<()> {
    let Some(powershell) = windows() else {
        eprintln!("skipped: this machine has no Windows to serve a clipboard");
        return Ok(());
    };

    let scratch = Scratch::named("windows")?;
    let launching = Instant::now();
    let mut helper = Helper::launch(Launch::windows_clipboard(&scratch.root)?)?;
    let launched = launching.elapsed();
    let started = helper
        .process_id()
        .ok_or_else(|| anyhow!("the helper is not running"))?;

    assert!(
        READ_BOUND < launched,
        "the helper started in {launched:?}, so there was no startup cost to have moved off the \
         first paste"
    );

    if helper.degraded_until().is_some() {
        let refused = helper.read();

        eprintln!(
            "this machine's clipboard is unavailable, which is the failure this helper is \
                   long-lived for"
        );
        assert!(
            matches!(refused, Err(Error::Degraded { .. })),
            "a request made of an unavailable clipboard was answered with {refused:?} rather than \
             refused"
        );
        assert_eq!(
            1,
            helper.launches(),
            "an unavailable clipboard cost the process serving it"
        );
        assert!(
            running(started),
            "an unavailable clipboard cost the process serving it"
        );
    } else {
        let text = format!("vimbecode clipboard helper {}", process::id());
        let stored = helper.write(text.clone())?;
        let reading = Instant::now();
        let held = helper.read()?;
        let read = reading.elapsed();
        let oracle = Command::new(&powershell)
            .args(["-NoProfile", "-NonInteractive", "-Command", "Get-Clipboard"])
            .output()?;
        let seen = String::from_utf8_lossy(&oracle.stdout).into_owned();

        assert_eq!(Response::Stored, stored, "the yank was not stored");
        assert_eq!(
            Response::Text(text.clone()),
            held,
            "the helper did not read back what it wrote"
        );
        assert!(
            seen.contains(&text),
            "Windows itself holds {seen:?}, which is not what the helper wrote"
        );
        assert!(
            read < READ_BOUND,
            "a read of the live clipboard took {read:?}, over the bound of {READ_BOUND:?}, so the \
             session is paying a startup cost at every paste"
        );
    }

    helper.shutdown();
    await_exit(started)?;

    Ok(())
}

/// # Returns
///
/// The launch of the stand-in helper, steered through a test's own directory.
fn stub(scratch: &Scratch) -> Launch {
    Launch::of(SHELL.into(), vec![STUB.into()])
        .with_environment("VBC_STUB_SPAWN_LOG".into(), scratch.path(SPAWN_LOG).into())
        .with_environment(
            "VBC_STUB_REQUEST_LOG".into(),
            scratch.path(REQUEST_LOG).into(),
        )
        .with_environment("VBC_STUB_FAIL_FLAG".into(), scratch.path(FAIL_FLAG).into())
        .with_environment(
            "VBC_STUB_DEATH_FLAG".into(),
            scratch.path(DEATH_FLAG).into(),
        )
        .with_environment("VBC_STUB_TEXT".into(), CLIPBOARD_TEXT.into())
}

/// # Returns
///
/// The times the stand-in was handed a request, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`anyhow::Error`] if a line of the record is not a time.
/// * Forwards [`Scratch::lines`]'s return values on failure.
fn stamps(scratch: &Scratch) -> Result<Vec<Duration>> {
    scratch
        .lines(REQUEST_LOG)?
        .iter()
        .map(|line| Ok(Duration::from_nanos(line.parse::<u64>()?)))
        .collect()
}

/// Waits until the clipboard's backoff has run out.
fn wait_out(helper: &Helper) {
    let Some(until) = helper.degraded_until() else {
        return;
    };
    let now = Instant::now();
    if now < until {
        thread::sleep(until - now + POLL);
    }
}

/// # Returns
///
/// Whether a process is running, which a process that has exited and not yet been waited for is
/// not.
fn running(pid: u32) -> bool {
    let Ok(status) = fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    let Some(fields) = status.rsplit(')').next() else {
        return false;
    };

    !matches!(fields.split_whitespace().next(), Some("Z") | None)
}

/// Kills a process.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`anyhow::Error`] if the kill reported a failure.
/// * Forwards [`std::process::Command::status`]'s return values on failure.
fn kill(pid: u32) -> Result<()> {
    let killed = Command::new("kill")
        .args(["-KILL", &pid.to_string()])
        .status()?;
    if !killed.success() {
        return Err(anyhow!("{pid} could not be killed: {killed}"));
    }

    Ok(())
}

/// Waits for a process to stop running, whether or not anything has waited for it yet.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`anyhow::Error`] if it is still running after [`PROCESS_LIMIT`].
fn await_death(pid: u32) -> Result<()> {
    let waiting = Instant::now();
    while waiting.elapsed() < PROCESS_LIMIT {
        if !running(pid) {
            return Ok(());
        }
        thread::sleep(POLL);
    }

    Err(anyhow!("{pid} was still running after {PROCESS_LIMIT:?}"))
}

/// Waits for a process to be gone from the system altogether, which is what a process nobody is
/// left to wait for would not be.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`anyhow::Error`] if it is still there after [`PROCESS_LIMIT`].
fn await_exit(pid: u32) -> Result<()> {
    let waiting = Instant::now();
    while waiting.elapsed() < PROCESS_LIMIT {
        if !Path::new(&format!("/proc/{pid}")).exists() {
            return Ok(());
        }
        thread::sleep(POLL);
    }

    Err(anyhow!(
        "{pid} was still on the system after {PROCESS_LIMIT:?}, which a helper the session took \
         with it would not be"
    ))
}

/// # Returns
///
/// The PowerShell this machine reaches Windows through, if it has one.
fn windows() -> Option<OsString> {
    let found = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", "exit 0"])
        .output();

    match found {
        Ok(output) if output.status.success() => Some("powershell.exe".into()),
        _ => None,
    }
}
