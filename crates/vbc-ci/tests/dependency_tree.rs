//! The packages the workspace links into what it ships, and the proof that reading them bites.
//!
//! vimbecode adopts modalkit for the vim engine but not for the screen: modalkit's widgets live in
//! `modalkit-ratatui`, and this workspace's own layout and renderer are the authority on what a
//! frame holds. A widget crate arriving through a dependency of a dependency would bring a second
//! opinion about that, a second terminal backend to reconcile, and a system clipboard nothing here
//! asks for, and it would arrive without a line of this workspace's source changing.
//!
//! So the dependency graph is recorded rather than trusted. The recording is what `cargo tree`
//! reports over normal edges, which is what ships, with development and build edges left out. The
//! live graph is read back here and required to be the recorded one, so a version bump or a new
//! transitive dependency is a red test and a deliberate re-recording rather than a surprise, and
//! the three properties the recording exists to hold -- no widgets, one terminal backend, no
//! clipboard -- are asserted over the live graph in their own right.
//!
//! Re-record with `VBC_RECORD_DEPENDENCY_TREE=1 cargo test -p vbc-ci --test dependency_tree`.
//!
//! A check is only worth what it would catch, so each of the three is also run against a graph the
//! offence it names has been written into, and is required to report it.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The recording the live graph is held to, relative to the workspace root.
const RECORDING: &str = "crates/vbc-ci/tests/dependency_tree.txt";

/// The environment variable that rewrites the recording from the live graph instead of comparing
/// against it.
const RECORD: &str = "VBC_RECORD_DEPENDENCY_TREE";

/// The engine this workspace adopted, which every check that reads the graph must be able to find
/// in it.
const ENGINE: &str = "modalkit";

/// The widgets that come with that engine, which this workspace renders without.
const WIDGETS: &str = "modalkit-ratatui";

/// The terminal backend, which more than one version of would be two ways of describing a cell.
const BACKEND: &str = "ratatui";

/// The system clipboard the widgets reach for, which this workspace does not.
const CLIPBOARD: &str = "arboard";

#[test]
fn the_graph_the_workspace_links_is_the_one_that_was_recorded() -> anyhow::Result<()> {
    let live = recorded_form(&tree()?);
    if std::env::var_os(RECORD).is_some() {
        fs::write(workspace().join(RECORDING), &live)?;
    }
    let recorded = fs::read_to_string(workspace().join(RECORDING))?;

    let live: BTreeSet<&str> = live.lines().collect();
    let recorded: BTreeSet<&str> = recorded.lines().collect();

    assert_eq!(
        Vec::<&&str>::new(),
        live.difference(&recorded).collect::<Vec<_>>(),
        "packages the workspace links that {RECORDING} does not record; re-record with \
         `{RECORD}=1 cargo test -p vbc-ci --test dependency_tree` once the additions are meant"
    );
    assert_eq!(
        Vec::<&&str>::new(),
        recorded.difference(&live).collect::<Vec<_>>(),
        "packages {RECORDING} records that the workspace no longer links; re-record with \
         `{RECORD}=1 cargo test -p vbc-ci --test dependency_tree` once the removals are meant"
    );

    Ok(())
}

#[test]
fn the_graph_the_checks_read_holds_the_engine_they_were_written_for() -> anyhow::Result<()> {
    let linked = linked(&tree()?);

    assert_ne!(BTreeSet::new(), versions_of(&linked, ENGINE));
    assert_ne!(BTreeSet::new(), versions_of(&linked, BACKEND));

    Ok(())
}

#[test]
fn nothing_the_workspace_links_is_the_engines_widgets() -> anyhow::Result<()> {
    assert_eq!(BTreeSet::new(), versions_of(&linked(&tree()?), WIDGETS));

    Ok(())
}

#[test]
fn the_terminal_backend_is_linked_at_one_version() -> anyhow::Result<()> {
    for (name, versions) in linked(&tree()?) {
        if !name.starts_with(BACKEND) {
            continue;
        }

        assert_eq!(1, versions.len(), "`{name}` is linked at {versions:?}");
    }

    Ok(())
}

#[test]
fn nothing_the_workspace_links_reaches_the_system_clipboard() -> anyhow::Result<()> {
    assert_eq!(BTreeSet::new(), versions_of(&linked(&tree()?), CLIPBOARD));

    Ok(())
}

#[test]
fn a_graph_that_pulled_the_engines_widgets_in_is_caught() -> anyhow::Result<()> {
    let widgets = also(&tree()?, "modalkit-ratatui v0.0.25");

    assert_ne!(BTreeSet::new(), versions_of(&linked(&widgets), WIDGETS));

    Ok(())
}

#[test]
fn a_graph_that_pulled_a_second_terminal_backend_in_is_caught() -> anyhow::Result<()> {
    let split = also(&tree()?, "ratatui v0.29.0");

    assert_eq!(2, versions_of(&linked(&split), BACKEND).len());

    Ok(())
}

#[test]
fn a_graph_that_pulled_the_system_clipboard_in_is_caught() -> anyhow::Result<()> {
    let pasting = also(&tree()?, "arboard v3.6.1");

    assert_ne!(BTreeSet::new(), versions_of(&linked(&pasting), CLIPBOARD));

    Ok(())
}

#[test]
fn a_recording_the_graph_no_longer_matches_is_caught() -> anyhow::Result<()> {
    let live = recorded_form(&tree()?);
    let stale = recorded_form(&also(&live, "modalkit-ratatui v0.0.25"));

    assert_ne!(live, stale);

    Ok(())
}

/// # Returns
///
/// What `cargo tree` reports over the normal edges of every crate of the workspace, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`anyhow::Error`] if `cargo tree` could not be run, or reported a failure.
/// * Forwards [`String::from_utf8`]'s return values on failure.
fn tree() -> anyhow::Result<String> {
    let reported = Command::new(env!("CARGO"))
        .current_dir(workspace())
        .args([
            "tree",
            "--locked",
            "--workspace",
            "--edges",
            "normal",
            "--prefix",
            "none",
            "--format",
            "{p}",
        ])
        .output()?;
    anyhow::ensure!(
        reported.status.success(),
        "`cargo tree` failed: {}",
        String::from_utf8_lossy(&reported.stderr)
    );

    Ok(String::from_utf8(reported.stdout)?)
}

/// # Returns
///
/// Every package a dependency graph holds, as the versions each package name is linked at.
fn linked(tree: &str) -> BTreeMap<String, BTreeSet<String>> {
    let mut linked: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for line in tree.lines() {
        let mut fields = line.split_whitespace();
        let (Some(name), Some(version)) = (fields.next(), fields.next()) else {
            continue;
        };
        linked
            .entry(name.to_owned())
            .or_default()
            .insert(version.to_owned());
    }

    linked
}

/// # Returns
///
/// The versions a dependency graph links `name` at, which is empty if it does not link it at all.
fn versions_of(linked: &BTreeMap<String, BTreeSet<String>>, name: &str) -> BTreeSet<String> {
    linked.get(name).cloned().unwrap_or_default()
}

/// # Returns
///
/// A dependency graph in the form it is recorded in: one sorted line for every version of every
/// package it links, with the paths and the repetition `cargo tree` reports left out.
fn recorded_form(tree: &str) -> String {
    linked(tree)
        .iter()
        .flat_map(|(name, versions)| {
            versions
                .iter()
                .map(move |version| format!("{name} {version}\n"))
        })
        .collect()
}

/// # Returns
///
/// `tree` with one more package linked into it.
fn also(tree: &str, package: &str) -> String {
    format!("{tree}{package}\n")
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
