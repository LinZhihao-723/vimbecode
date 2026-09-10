//! The gate a directory passes before a session is started in it, read off what the project ran.
//!
//! What is asserted here is an absence, and an absence is the easiest thing in the world to assert
//! by accident. A fixture whose hook was never going to run, a stand-in that ignores the flag, a
//! spawn that failed before it began -- every one of them leaves the same empty directory a
//! working gate leaves. So no test here asserts the absence alone: each one that requires the
//! project's code not to have run is paired with the same fixture, the same stand-in and the same
//! spawn, trusted, requiring that it did. The pair is what says the gate is what stopped it.
//!
//! The failing spawn is the same shape and is the one the recorded failure was found in. A gate
//! written as error handling -- start the child, notice the directory is not trusted, kill it --
//! looks correct until the child fails on its own: the hook has run by then, the MCP servers are
//! up, and there is nothing left to abort. So the stand-in runs the project's code on the way up
//! and quits immediately afterwards, and the pair is asserted over that: restricted, nothing ran;
//! trusted, it ran, and the session failed both times.
//!
//! The record is the reader's own `~/.claude.json`, which a live Claude Code session writes too,
//! so it is asserted from the outside: a record holding somebody else's keys is written, a grant
//! is made, and what the file holds afterwards is read back. The concurrency test is the same
//! assertion made continuously -- a reader that never once sees half a record, while other writers
//! are replacing the whole of it.
//!
//! What the record ends up as is not the whole of what a grant leaves lying about, either. The
//! version being written stands in a file of its own beside the record until it is renamed over
//! it, and that file holds the same account the record does under a name anybody can guess, so it
//! is watched while a grant is in flight rather than read once it has landed.
//!
//! What that test does not assert is that a grant made while another writer is part way through
//! its own read and write survives it. Nothing here locks the record, so it does not: two writers
//! that read the same version write it back one after the other, and the second carries the first
//! away. The grant whose survival is asserted is therefore the one made after the other writers
//! have finished, which is what the failure of the race amounts to -- the reader is asked about
//! that directory again. It cannot amount to more than that: a version written back from an older
//! read holds what its reader had already said, so a lost update can only ever restore a trust
//! that was granted, never invent one that was not.

#![cfg(target_os = "linux")]

use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use tempfile::TempDir;
use vbc_editor::session::client::Client;
use vbc_editor::session::identity::Identity;
use vbc_editor::session::spawn::Spawn;
use vbc_editor::session::trust::{
    Admission, Answer, Gate, Key, Standing, ACCEPTED, PROJECTS, RECORD, SETTING_SOURCES_FLAG,
    SETTING_SOURCES_USER,
};

/// The stand-in, which runs the project's code unless the session was restricted and hands over to
/// the client's own stand-in once it has.
const STUB: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/session/trusting.sh");

/// The project's own code, what it leaves behind when it runs, and the file that tells the
/// stand-in to quit the way a session that could not start quits.
const HOOK: &str = ".claude/session-start";
const SENTINEL: &str = "sentinel";
const FAILS: &str = "fail";

/// What the project's code is, which is a line written into the directory it was run in. Nothing
/// about it matters except that the project owns it and the reader did not write it.
const PROJECT_CODE: &str = "#!/bin/sh\nprintf 'the project ran its own code\\n' > sentinel\n";

/// How long a turn against a process on this machine is given. No model is behind any of it.
const TURN: Duration = Duration::from_secs(10);

/// A record of somebody else's, holding what a reader's own `~/.claude.json` holds: their account,
/// their settings, a project they trusted long ago and a project they have talked to and not
/// trusted. None of it is a grant's to touch.
fn theirs(key: &str) -> Value {
    json!({
        "installMethod": "native",
        "numStartups": 41,
        "oauthAccount": {"emailAddress": "reader@example.com"},
        "tipsHistory": {"shift-enter": 12},
        PROJECTS: {
            "/somewhere/else": {"hasTrustDialogAccepted": true, "allowedTools": ["Bash"]},
            key: {"history": [{"display": "an earlier prompt"}]},
        },
    })
}

/// How many times a record is granted, replaced and read while all three are happening at once.
const GRANTS: usize = 24;
const REPLACEMENTS: usize = 24;
const READS: usize = 32;

/// What a file the reader alone may read is, and what a record holding their account has to stay.
const OWNER_ONLY: u32 = 0o600;

/// What a record somebody has opened up to their own group is. It is nobody's to narrow either: a
/// grant hands the record back under the permissions it found, and only a record written where
/// there was none is a decision this process makes.
const READABLE_BY_A_GROUP: u32 = 0o640;

/// How long the padding a torn record would be caught by is. A record written over in place is
/// briefly shorter than this, and a reader of it would see a value that stops in the middle.
const PADDING: usize = 256 * 1024;

/// What the file a version of the record travels through is called, written down here rather than
/// read from the module for the same reason the version it replaces is: a name the reader can find
/// their own record beside is one a test says out loud.
const PARTIAL: &str = "vimbecode.partial";

/// How much record has to be in flight for the file it travels through to be caught existing at
/// all. Whether it is readable is not a question about how long it lasts.
const TRAVELLING: usize = 4 * 1024 * 1024;

#[test]
fn a_directory_the_reader_has_not_trusted_does_not_run_the_project_code_in_it() -> Result<()> {
    let home = TempDir::new()?;
    let project = TempDir::new()?;
    seeded(project.path())?;

    let gate = Gate::of_record(home.path().join(RECORD));
    let admission = gate.admit(project.path())?;
    assert_eq!(Standing::Restricted, admission.standing());
    assert!(
        admission.asks(),
        "a directory the record says nothing about was admitted without the reader being asked"
    );

    run(&admission)?;

    assert!(
        !ran(project.path()),
        "a session in a directory the reader has never trusted ran the project's own code, which \
         is what pointing vimbecode at a cloned repository would do"
    );

    Ok(())
}

#[test]
fn the_same_directory_trusted_runs_it() -> Result<()> {
    let home = TempDir::new()?;
    let project = TempDir::new()?;
    seeded(project.path())?;

    let gate = Gate::of_record(home.path().join(RECORD));
    let granted = gate.answered(&gate.admit(project.path())?, Answer::Granted)?;
    assert_eq!(Standing::Trusted, granted.standing());
    assert!(!granted.asks());

    let read_back = gate.admit(project.path())?;
    assert_eq!(
        Standing::Trusted,
        read_back.standing(),
        "a grant was not what the record was asked afterwards"
    );

    run(&read_back)?;

    assert!(
        ran(project.path()),
        "the project's code did not run in a directory the reader trusted, so the fixture proves \
         nothing about what suppressed it in the directory they did not"
    );

    Ok(())
}

#[test]
fn the_gate_holds_where_the_spawn_it_precedes_fails() -> Result<()> {
    let home = TempDir::new()?;
    let gate = Gate::of_record(home.path().join(RECORD));

    let refused = TempDir::new()?;
    seeded(refused.path())?;
    fs::write(refused.path().join(FAILS), "")?;
    let admission = gate.admit(refused.path())?;
    assert_eq!(Standing::Restricted, admission.standing());

    let Err(error) = run(&admission) else {
        panic!("a session whose child quit on the way up answered a turn");
    };
    assert!(
        !ran(refused.path()),
        "a spawn that failed had already run the project's own code, which is what a gate written \
         as error handling cannot undo: {error}"
    );

    let trusted = TempDir::new()?;
    seeded(trusted.path())?;
    fs::write(trusted.path().join(FAILS), "")?;
    let granted = gate.answered(&gate.admit(trusted.path())?, Answer::Granted)?;

    let Err(_failed) = run(&granted) else {
        panic!("a session whose child quit on the way up answered a turn");
    };
    assert!(
        ran(trusted.path()),
        "the project's code did not run in the trusted half of the pair either, so the failing \
         spawn is what stopped it and the assertion above is about nothing"
    );

    Ok(())
}

#[test]
fn a_question_nobody_answered_leaves_the_directory_untrusted() -> Result<()> {
    let home = TempDir::new()?;
    let project = TempDir::new()?;
    seeded(project.path())?;

    let record = home.path().join(RECORD);
    let gate = Gate::of_record(&record);
    let admission = gate.admit(project.path())?;

    assert_eq!(Answer::Withheld, Answer::default());
    let answered = gate.answered(&admission, Answer::default())?;
    assert_eq!(Standing::Restricted, answered.standing());
    assert!(
        !record.exists(),
        "a question nobody answered was written into the record as an answer"
    );

    run(&answered)?;

    assert!(
        !ran(project.path()),
        "a directory the reader was asked about and did not answer for ran its own code anyway"
    );

    Ok(())
}

#[test]
fn a_spawn_no_gate_admitted_runs_on_none_of_the_project_configuration() -> Result<()> {
    let home = TempDir::new()?;
    let project = TempDir::new()?;

    let restricted = vec![
        SETTING_SOURCES_FLAG.to_owned(),
        SETTING_SOURCES_USER.to_owned(),
    ];
    assert!(
        Spawn::new(Identity::default())
            .arguments()
            .windows(2)
            .any(|pair| pair == restricted.as_slice()),
        "a spawn that has been through no gate at all was started on the project's configuration"
    );

    let gate = Gate::of_record(home.path().join(RECORD));
    let granted = gate.answered(&gate.admit(project.path())?, Answer::Granted)?;
    let elsewhere = TempDir::new()?;

    let spawn = Spawn::new(Identity::default())
        .with_admission(&granted)
        .with_directory(elsewhere.path());
    assert_eq!(
        Standing::Restricted,
        spawn.standing(),
        "a spawn kept the trust of the directory it was admitted for after being pointed at \
         another one, so an admission trusts wherever it is followed by"
    );

    Ok(())
}

#[test]
fn two_directories_of_one_repository_are_trusted_together() -> Result<()> {
    let home = TempDir::new()?;
    let repository = TempDir::new()?;
    initialised(repository.path())?;

    let one = repository.path().join("crates/one");
    let two = repository.path().join("crates/two");
    fs::create_dir_all(&one)?;
    fs::create_dir_all(&two)?;

    assert_eq!(Key::of(&one)?, Key::of(&two)?);
    assert_eq!(
        fs::canonicalize(repository.path())?,
        Key::of(&one)?.as_path()
    );

    let gate = Gate::of_record(home.path().join(RECORD));
    gate.answered(&gate.admit(&one)?, Answer::Granted)?;
    assert_eq!(
        Standing::Trusted,
        gate.admit(&two)?.standing(),
        "trusting one directory of a repository did not trust another of the same repository"
    );

    let linked = TempDir::new()?;
    fs::write(
        linked.path().join(".git"),
        "gitdir: /elsewhere/.git/worktrees/one\n",
    )?;
    let inside = linked.path().join("crates");
    fs::create_dir_all(&inside)?;
    assert_eq!(
        fs::canonicalize(linked.path())?,
        Key::of(&inside)?.as_path(),
        "a working copy linked to a repository kept elsewhere was not read as a working copy"
    );

    Ok(())
}

#[test]
fn a_directory_reached_through_a_link_is_trusted_as_what_it_resolves_to() -> Result<()> {
    let home = TempDir::new()?;
    let real = TempDir::new()?;
    let links = TempDir::new()?;
    let link = links.path().join("project");
    symlink(real.path(), &link)?;

    assert_eq!(Key::of(&link)?, Key::of(real.path())?);
    assert_eq!(fs::canonicalize(real.path())?, Key::of(&link)?.as_path());

    let gate = Gate::of_record(home.path().join(RECORD));
    gate.answered(&gate.admit(real.path())?, Answer::Granted)?;
    assert_eq!(
        Standing::Trusted,
        gate.admit(&link)?.standing(),
        "a link to a trusted directory was admitted as a directory of its own"
    );

    Ok(())
}

#[test]
fn the_key_a_directory_is_trusted_under_is_not_the_one_its_transcript_is_stored_under() -> Result<()>
{
    let home = TempDir::new()?;
    let repository = TempDir::new()?;
    initialised(repository.path())?;
    let inside = repository.path().join("crates/one");
    fs::create_dir_all(&inside)?;

    let transcript = fs::canonicalize(&inside)?;
    let key = Key::of(&inside)?;
    assert_ne!(
        transcript.as_path(),
        key.as_path(),
        "trust was keyed on the directory a transcript is stored under, so a reader who trusted a \
         repository would be asked again in every directory of it"
    );
    assert_eq!(fs::canonicalize(repository.path())?, key.as_path());

    let record = home.path().join(RECORD);
    let gate = Gate::of_record(&record);
    gate.answered(&gate.admit(&inside)?, Answer::Granted)?;

    assert_eq!(
        vec![key.as_str().to_owned()],
        recorded(&record)?,
        "the record names a directory the reader was never asked about"
    );

    Ok(())
}

#[test]
fn granting_keeps_everything_else_the_record_holds() -> Result<()> {
    let home = TempDir::new()?;
    let project = TempDir::new()?;
    let record = home.path().join(RECORD);
    let gate = Gate::of_record(&record);

    let admission = gate.admit(project.path())?;
    let key = admission.key().as_str().to_owned();
    let before = serde_json::to_string_pretty(&theirs(&key))?;
    fs::write(&record, &before)?;

    gate.grant(&admission)?;

    let held = theirs(&key);
    let after = read(&record)?;
    for kept in [
        "installMethod",
        "numStartups",
        "oauthAccount",
        "tipsHistory",
    ] {
        assert_eq!(
            held.get(kept),
            after.get(kept),
            "granting one directory's trust rewrote `{kept}`, which is none of its business"
        );
    }

    let projects = after
        .get(PROJECTS)
        .and_then(Value::as_object)
        .ok_or(anyhow!("the record kept no projects"))?;
    assert_eq!(
        held.get(PROJECTS)
            .and_then(|projects| projects.get("/somewhere/else")),
        projects.get("/somewhere/else"),
        "granting one directory's trust rewrote another directory's entry"
    );

    let granted = projects
        .get(&key)
        .and_then(Value::as_object)
        .ok_or(anyhow!(
            "the record kept nothing about the directory granted"
        ))?;
    assert_eq!(Some(&Value::Bool(true)), granted.get(ACCEPTED));
    assert_eq!(
        Some(&json!([{"display": "an earlier prompt"}])),
        granted.get("history"),
        "granting a directory's trust replaced its entry instead of adding to it"
    );

    assert_eq!(
        before,
        fs::read_to_string(kept_beside(&record))?,
        "the version the grant replaced was not kept beside the record"
    );

    Ok(())
}

#[test]
fn granting_leaves_the_record_no_more_readable_than_it_found_it() -> Result<()> {
    let home = TempDir::new()?;
    let project = TempDir::new()?;
    let record = home.path().join(RECORD);
    let gate = Gate::of_record(&record);
    let admission = gate.admit(project.path())?;

    gate.grant(&admission)?;
    assert_eq!(
        OWNER_ONLY,
        mode(&record)?,
        "a record holding the reader's account was created for anybody on this machine to read"
    );

    fs::write(
        &record,
        serde_json::to_string_pretty(&theirs(admission.key().as_str()))?,
    )?;
    fs::set_permissions(&record, fs::Permissions::from_mode(OWNER_ONLY))?;
    gate.grant(&admission)?;

    assert_eq!(
        OWNER_ONLY,
        mode(&record)?,
        "granting a directory's trust handed the reader's own record back wider open than it was, \
         because the file it was replaced from was created under this process's umask"
    );
    assert_eq!(OWNER_ONLY, mode(&kept_beside(&record))?);

    Ok(())
}

#[test]
fn the_permissions_a_grant_hands_the_record_back_under_are_the_ones_it_found() -> Result<()> {
    let home = TempDir::new()?;
    let project = TempDir::new()?;
    let record = home.path().join(RECORD);
    let gate = Gate::of_record(&record);
    let admission = gate.admit(project.path())?;

    fs::write(
        &record,
        serde_json::to_string_pretty(&theirs(admission.key().as_str()))?,
    )?;
    fs::set_permissions(&record, fs::Permissions::from_mode(READABLE_BY_A_GROUP))?;
    gate.grant(&admission)?;

    assert_eq!(
        READABLE_BY_A_GROUP,
        mode(&record)?,
        "granting a directory's trust changed who may read the reader's own record, which is a \
         decision they made about their own file and none of a grant's business either way"
    );
    assert_eq!(READABLE_BY_A_GROUP, mode(&kept_beside(&record))?);

    Ok(())
}

#[test]
fn the_file_a_grant_is_written_through_is_never_wider_than_the_record() -> Result<()> {
    let home = TempDir::new()?;
    let project = TempDir::new()?;
    let record = home.path().join(RECORD);
    let gate = Gate::of_record(&record);
    let admission = gate.admit(project.path())?;

    let mut held = theirs(admission.key().as_str());
    held["padding"] = Value::String("x".repeat(TRAVELLING));
    fs::write(&record, serde_json::to_string(&held)?)?;
    fs::set_permissions(&record, fs::Permissions::from_mode(OWNER_ONLY))?;

    let done = AtomicBool::new(false);
    let wider = AtomicUsize::new(0);
    let seen = AtomicUsize::new(0);
    let beside = home.path().to_owned();

    thread::scope(|scope| -> Result<()> {
        let watcher = scope.spawn(|| {
            while !done.load(Ordering::Relaxed) {
                let Ok(entries) = fs::read_dir(&beside) else {
                    continue;
                };
                for travelling in entries.flatten() {
                    if !travelling.file_name().to_string_lossy().contains(PARTIAL) {
                        continue;
                    }
                    let Ok(held) = travelling.metadata() else {
                        continue;
                    };
                    seen.fetch_add(1, Ordering::Relaxed);
                    if 0 != held.permissions().mode() & !OWNER_ONLY & 0o777 {
                        wider.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        });

        for _granting in 0..GRANTS {
            gate.grant(&admission)?;
        }
        done.store(true, Ordering::Relaxed);
        watcher
            .join()
            .map_err(|_panicked| anyhow!("the concurrent watcher panicked"))?;

        Ok(())
    })?;

    assert!(
        0 < seen.load(Ordering::Relaxed),
        "the file a grant is written through was never caught existing, so nothing below is about \
         anything"
    );
    assert_eq!(
        0,
        wider.load(Ordering::Relaxed),
        "the whole of the reader's record, their account in it, stood in a file anybody on this \
         machine could read under a name anybody can guess, out of {} sightings, on its way to a \
         record that is theirs alone",
        seen.load(Ordering::Relaxed)
    );

    Ok(())
}

#[test]
fn a_directory_of_a_trusted_repository_is_run_where_it_stands() -> Result<()> {
    let home = TempDir::new()?;
    let repository = TempDir::new()?;
    initialised(repository.path())?;

    let one = repository.path().join("crates/one");
    let two = repository.path().join("crates/two");
    fs::create_dir_all(&one)?;
    fs::create_dir_all(&two)?;
    seeded(&one)?;
    seeded(&two)?;

    let gate = Gate::of_record(home.path().join(RECORD));
    gate.answered(&gate.admit(&one)?, Answer::Granted)?;

    let elsewhere = gate.admit(&two)?;
    assert_eq!(Standing::Trusted, elsewhere.standing());
    run(&elsewhere)?;

    assert!(
        ran(&two),
        "a directory of a repository the reader trusted did not run its own code"
    );
    assert!(
        !ran(&one),
        "the session was started in the directory the key names rather than the one the reader \
         chose, so a grant taken anywhere in a repository moves every session to its root"
    );

    Ok(())
}

#[test]
fn a_record_being_granted_is_never_read_half_written() -> Result<()> {
    let home = TempDir::new()?;
    let project = TempDir::new()?;
    let record = home.path().join(RECORD);
    let gate = Gate::of_record(&record);

    let admission = gate.admit(project.path())?;
    let mut held = theirs(admission.key().as_str());
    held["padding"] = Value::String("x".repeat(PADDING));
    fs::write(&record, serde_json::to_string_pretty(&held)?)?;

    let done = AtomicBool::new(false);
    let torn = AtomicUsize::new(0);
    let reads = AtomicUsize::new(0);

    thread::scope(|scope| -> Result<()> {
        let reader = scope.spawn(|| {
            while !done.load(Ordering::Relaxed) {
                let seen = fs::read_to_string(&record)
                    .ok()
                    .and_then(|text| serde_json::from_str::<Value>(&text).ok())
                    .and_then(|held| {
                        held.get("padding")
                            .and_then(Value::as_str)
                            .map(|padding| PADDING == padding.len())
                    });
                if Some(true) != seen {
                    torn.fetch_add(1, Ordering::Relaxed);
                }
                reads.fetch_add(1, Ordering::Relaxed);
            }
        });

        let writer = scope.spawn(|| -> Result<()> {
            let elsewhere = Gate::of_record(&record);
            for _replacement in 0..REPLACEMENTS {
                let directory = TempDir::new()?;
                elsewhere.grant(&elsewhere.admit(directory.path())?)?;
            }

            Ok(())
        });

        for _granting in 0..GRANTS {
            gate.grant(&admission)?;
        }
        writer
            .join()
            .map_err(|_panicked| anyhow!("the concurrent writer panicked"))??;

        while reads.load(Ordering::Relaxed) < READS {
            gate.grant(&admission)?;
        }
        gate.grant(&admission)?;
        done.store(true, Ordering::Relaxed);
        reader
            .join()
            .map_err(|_panicked| anyhow!("the concurrent reader panicked"))?;

        Ok(())
    })?;

    assert!(READS <= reads.load(Ordering::Relaxed));
    assert_eq!(
        0,
        torn.load(Ordering::Relaxed),
        "a reader of the record saw a version of it that was neither the one being replaced nor \
         the one replacing it, out of {} reads while it was being granted",
        reads.load(Ordering::Relaxed)
    );

    let after = read(&record)?;
    assert_eq!(
        Some(PADDING),
        after.get("padding").and_then(Value::as_str).map(str::len),
        "what the record held before the grants did not survive them whole"
    );
    assert_eq!(Standing::Trusted, gate.admit(project.path())?.standing());

    Ok(())
}

/// Puts a project's own code into a directory, which is what a cloned repository holds and the
/// reader did not write.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`std::fs::write`]'s return values on failure.
fn seeded(directory: &Path) -> Result<()> {
    let hook = directory.join(HOOK);
    fs::create_dir_all(
        hook.parent()
            .ok_or(anyhow!("the project's code is not in a directory"))?,
    )?;
    fs::write(&hook, PROJECT_CODE)?;
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755))?;

    Ok(())
}

/// # Returns
///
/// Whether the project's own code ran in the directory.
fn ran(directory: &Path) -> bool {
    directory.join(SENTINEL).exists()
}

/// Starts a session on the stand-in as an admission admits it and takes one turn against it.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`Client::start`]'s return values on failure.
/// * Forwards [`Client::turn`]'s return values on failure.
fn run(admission: &Admission) -> Result<()> {
    let mut session = Client::start(
        &Spawn::new(Identity::default())
            .with_binary(STUB)
            .with_admission(admission),
    )?;
    session.turn("hello", TURN)?;

    Ok(())
}

/// Makes a directory the root of a working copy, with the tool that makes one rather than by
/// writing what such a root is expected to hold.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`anyhow::Error`] if `git` could not be run, or refused to make one.
fn initialised(directory: &Path) -> Result<()> {
    let made = Command::new("git")
        .arg("init")
        .arg("--quiet")
        .current_dir(directory)
        .status()?;
    if !made.success() {
        return Err(anyhow!("a working copy could not be made: {made}"));
    }

    Ok(())
}

/// # Returns
///
/// What the record holds, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`std::fs::read_to_string`]'s return values on failure.
/// * Forwards [`serde_json::from_str`]'s return values on failure.
fn read(record: &Path) -> Result<Value> {
    Ok(serde_json::from_str(&fs::read_to_string(record)?)?)
}

/// # Returns
///
/// Every directory the record names, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`anyhow::Error`] if the record keeps no projects.
/// * Forwards [`read`]'s return values on failure.
fn recorded(record: &Path) -> Result<Vec<String>> {
    Ok(read(record)?
        .get(PROJECTS)
        .and_then(Value::as_object)
        .ok_or(anyhow!("the record kept no projects"))?
        .keys()
        .cloned()
        .collect())
}

/// # Returns
///
/// Who may read and write a file, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`std::fs::metadata`]'s return values on failure.
fn mode(path: &Path) -> Result<u32> {
    Ok(fs::metadata(path)?.permissions().mode() & 0o777)
}

/// # Returns
///
/// Where the version a grant replaced is kept.
fn kept_beside(record: &Path) -> PathBuf {
    let mut name = record
        .file_name()
        .map(ToOwned::to_owned)
        .unwrap_or_default();
    name.push(".vimbecode.backup");

    record.with_file_name(name)
}
