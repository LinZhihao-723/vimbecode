//! Checks the shape of the workspace's own source: that it declares one type for the text being
//! edited, that the layout stack, the invariants and the fuzz harness all address the text through
//! that one type, and that no layout laying a whole document out is reachable outside the tests.
//!
//! Both claims are properties of the code rather than of a run, so both are read off the sources.
//! A second text type is not a defect a case can catch -- it is two seams drifting apart, and what
//! it costs is paid later, by whoever has to pick one. A whole-document layout costs more plainly:
//! it lays every line of the buffer out on every call, which is the cost the anchored mapping
//! exists to avoid, so a renderer reaching for one gives that up without being told.
//!
//! Each scan counts the files it read, and the scan for a shape checks that the shape is one this
//! tree really takes, so a scan finding nothing because it looked nowhere fails rather than passes.

use std::fs;
use std::path::{Path, PathBuf};

/// The fewest source files the workspace's crates hold, which keeps a scan that finds nothing from
/// passing because it read nothing.
const MINIMUM_SOURCE_FILES: usize = 18;

/// The files these scans are about, which a scan that read anything else but missed one of these
/// has said nothing.
const REQUIRED_SOURCES: [&str; 3] = [
    "crates/vbc-layout/src/buffer.rs",
    "crates/vbc-layout/src/invariants.rs",
    "crates/vbc-layout/src/lib.rs",
];

/// The words naming a type that would hold the text being edited.
const TEXT_WORDS: [&str; 2] = ["buffer", "document"];

/// The one such type the workspace declares, and the file that declares it.
const TEXT_TYPE: &str = "Buffer";
const TEXT_MODULE: &str = "crates/vbc-layout/src/buffer.rs";

/// The seams that must address the text through that one type, each paired with the declaration
/// that shows it does.
const SEAMS: [(&str, &str); 3] = [
    (
        "crates/vbc-layout/src/invariants.rs",
        "pub buffer: &'view Buffer",
    ),
    (
        "crates/vbc-layout/tests/fuzz/harness.rs",
        "pub buffer: Buffer",
    ),
    (
        "crates/vbc-layout/tests/fuzz/reference.rs",
        "view.buffer.lines()",
    ),
];

/// The reference layout the invariant search runs against, which is the one whole-document layout
/// this tree holds.
const REFERENCE_LAYOUT: &str = "crates/vbc-layout/tests/fuzz/reference.rs";

#[test]
fn the_workspace_declares_one_type_for_the_text_being_edited() {
    let mut declared = Vec::new();
    let mut scanned = Vec::new();
    for path in sources() {
        let source = read(&path);
        scanned.push(relative(&path));
        for line in source.lines() {
            let Some(name) = declared_type(line.trim_start()) else {
                continue;
            };

            let lowercase = name.to_lowercase();
            if TEXT_WORDS.iter().any(|word| lowercase.contains(word)) {
                declared.push((relative(&path), name.to_owned()));
            }
        }
    }

    assert_scanned(&scanned);
    assert_eq!(
        vec![(TEXT_MODULE.to_owned(), TEXT_TYPE.to_owned())],
        declared
    );
}

#[test]
fn the_layout_stack_the_invariants_and_the_fuzz_harness_share_that_type() {
    for (file, declaration) in SEAMS {
        let source = read(&workspace().join(file));
        assert!(
            source.contains(declaration),
            "{file} does not hold `{declaration}`, so it addresses the text some other way"
        );
    }
}

#[test]
fn no_layout_outside_the_tests_lays_a_whole_document_out() {
    let mut scanned = Vec::new();
    for path in sources() {
        let source = read(&path);
        scanned.push(relative(&path));
        for (number, line) in source.lines().enumerate() {
            let code = line.trim_start();
            if code.starts_with("//") {
                continue;
            }

            assert!(
                !lays_a_whole_document_out(code),
                "{}:{} lays a whole document out: {code}",
                relative(&path),
                number + 1
            );
        }
    }
    assert_scanned(&scanned);

    // A scan for a shape says nothing unless the shape is one this tree really takes, and the
    // reference layout the invariant search runs against is where it now lives.
    let reference = read(&workspace().join(REFERENCE_LAYOUT));
    assert!(
        reference
            .lines()
            .any(|line| lays_a_whole_document_out(line.trim_start())),
        "{REFERENCE_LAYOUT} lays no whole document out, so the scan looks for nothing"
    );
}

/// Checks that a scan read the files it is about, and the rest of the workspace besides.
///
/// # Panics
///
/// Panics if a file of [`REQUIRED_SOURCES`] went unread, or if fewer than
/// [`MINIMUM_SOURCE_FILES`] were read.
fn assert_scanned(scanned: &[String]) {
    for required in REQUIRED_SOURCES {
        assert!(
            scanned.iter().any(|path| required == path),
            "{required} went unread, so the scan looked past what it is about"
        );
    }
    assert!(
        MINIMUM_SOURCE_FILES <= scanned.len(),
        "{} files were read, fewer than the {MINIMUM_SOURCE_FILES} the workspace holds",
        scanned.len()
    );
}

/// # Returns
///
/// The name of the type `code` declares, or `None` if it declares none.
fn declared_type(code: &str) -> Option<&str> {
    let declaration = code
        .strip_prefix("pub ")
        .or_else(|| code.strip_prefix("pub(crate) "))
        .unwrap_or(code);
    let named = ["struct ", "enum ", "type "]
        .into_iter()
        .find_map(|keyword| declaration.strip_prefix(keyword))?;
    let name = named
        .split(|character: char| !character.is_alphanumeric() && '_' != character)
        .next()?;

    (!name.is_empty()).then_some(name)
}

/// # Returns
///
/// Whether `code` implements the layout trait or builds a whole screen, which are the two shapes a
/// whole-document layout takes however it spells the names it does so under. Declaring the screen
/// type is neither, and naming one in a signature that hands it back is building one.
fn lays_a_whole_document_out(code: &str) -> bool {
    code.contains("Layout for") || (code.contains("Screen {") && !code.contains("struct Screen {"))
}

/// # Returns
///
/// The path of every source file the workspace's crates hold.
///
/// # Panics
///
/// Panics if the workspace's crates cannot be read.
fn sources() -> Vec<PathBuf> {
    let mut sources = Vec::new();
    let crates = fs::read_dir(workspace().join("crates")).expect("the workspace holds crates");
    for entry in crates {
        let path = entry.expect("a crates directory entry is readable").path();
        collect(&path.join("src"), &mut sources);
    }
    sources.sort();

    sources
}

/// Collects every Rust source file under a directory, its subdirectories included.
///
/// # Panics
///
/// Panics if the directory cannot be read.
fn collect(directory: &Path, sources: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(directory).expect("a crate holds a source directory");
    for entry in entries {
        let path = entry.expect("a source directory entry is readable").path();
        if path.is_dir() {
            collect(&path, sources);
        } else if path.extension().is_some_and(|extension| "rs" == extension) {
            sources.push(path);
        }
    }
}

/// # Returns
///
/// The contents of a file of this workspace.
///
/// # Panics
///
/// Panics if the file cannot be read.
fn read(path: &Path) -> String {
    fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("{} is readable: {error}", path.display()))
}

/// # Returns
///
/// `path` written relative to the workspace root, with forward slashes.
///
/// # Panics
///
/// Panics if `path` is not inside the workspace.
fn relative(path: &Path) -> String {
    path.strip_prefix(workspace())
        .expect("a scanned path is inside the workspace")
        .to_string_lossy()
        .replace('\\', "/")
}

/// # Returns
///
/// The root of the workspace this crate belongs to.
///
/// # Panics
///
/// Panics if this crate is not a member of a workspace.
fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("this crate sits two directories below its workspace root")
        .to_owned()
}
