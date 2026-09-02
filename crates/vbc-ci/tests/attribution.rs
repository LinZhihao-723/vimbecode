//! The attribution the workspace owes the code it ships, and the proof that reading it bites.
//!
//! The Apache licence asks a distributor to carry the licence of the work it redistributes and to
//! name it in the notice that travels with the distribution. modalkit's published package carries
//! no licence text of its own, so this workspace keeps a copy of it, and a copy no notice names is
//! an obligation that has quietly lapsed.
//!
//! The scan is therefore over the copies rather than over a list written here: every licence the
//! tree holds must be named in `NOTICE`, so a licence added later is covered without anyone
//! remembering to add it, and a notice edited down to nothing is red. The engine the editor was
//! built on is named on its own besides, because that is the attribution this workspace exists to
//! owe.
//!
//! A scan is only worth what it would catch, so it is also run against a notice the engine has
//! been struck out of, and is required to report it.

use std::fs;
use std::path::{Path, PathBuf};

/// The notice that travels with a distribution of the workspace.
const NOTICE: &str = "NOTICE";

/// Where the licences of the code the workspace ships are kept.
const LICENCES: &str = "third_party";

/// What each of those directories holds.
const LICENCE: &str = "LICENSE";

/// The words an Apache licence opens with, which is what the kept copies are required to be.
const APACHE: [&str; 2] = ["Apache License", "Version 2.0, January 2004"];

/// The engine the editor was built on, whose licence is the one this scan was written for.
const ENGINE: &str = "modalkit";

#[test]
fn the_licence_of_the_engine_ships_with_the_workspace() -> anyhow::Result<()> {
    let licence = fs::read_to_string(workspace().join(LICENCES).join(ENGINE).join(LICENCE))?;

    for opening in APACHE {
        assert!(
            licence.contains(opening),
            "the licence kept for `{ENGINE}` is not an Apache one"
        );
    }

    Ok(())
}

#[test]
fn every_licence_the_workspace_keeps_is_named_in_the_notice() -> anyhow::Result<()> {
    let kept = kept()?;

    assert_ne!(Vec::<String>::new(), kept, "no licence is kept to be named");
    assert!(kept.contains(&ENGINE.to_owned()));
    assert_eq!(
        Vec::<String>::new(),
        unnamed(&fs::read_to_string(workspace().join(NOTICE))?, &kept)
    );

    Ok(())
}

#[test]
fn a_licence_the_notice_stopped_naming_is_caught() -> anyhow::Result<()> {
    let notice = fs::read_to_string(workspace().join(NOTICE))?;
    let struck: String = notice
        .lines()
        .filter(|line| !line.contains(ENGINE))
        .map(|line| format!("{line}\n"))
        .collect();

    assert_eq!(vec![ENGINE.to_owned()], unnamed(&struck, &kept()?));

    Ok(())
}

/// # Returns
///
/// The name of every piece of code the workspace keeps a licence for, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`fs::read_dir`]'s return values on failure.
fn kept() -> anyhow::Result<Vec<String>> {
    let mut kept = Vec::new();
    for entry in fs::read_dir(workspace().join(LICENCES))? {
        let entry = entry?;
        if entry.path().is_dir() {
            kept.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    kept.sort();

    Ok(kept)
}

/// # Returns
///
/// The name of every piece of code a notice fails to name.
fn unnamed(notice: &str, kept: &[String]) -> Vec<String> {
    kept.iter()
        .filter(|name| !notice.contains(name.as_str()))
        .cloned()
        .collect()
}

/// # Returns
///
/// The root of the workspace this crate belongs to.
///
/// # Panics
///
/// Panics if this crate does not sit two directories below a workspace root.
fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("this crate sits two directories below its workspace root")
        .to_owned()
}
