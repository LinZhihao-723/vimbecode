//! The scans the workspace's architecture is guarded by, and the walk that decides what they read.
//!
//! A guard is worth what it covers. Each of these scans walks every crate of the workspace and
//! every module of every crate, subdirectories included, rather than the one directory or the one
//! file the rule was first written against, because a rule that holds for a renderer holds for the
//! module that renders the gutter beside it and for the crate that has not been written yet.
//!
//! A scan that reads nothing finds nothing, and finding nothing is what a passing guard looks
//! like, so a scan says what it read and is held to it: the files it read are checked against the
//! crates the workspace holds and the modules those crates declare, both read off the tree rather
//! than written down here, and a scan that read fewer fails rather than passes.

pub mod fixture;
pub mod shape;

use std::collections::BTreeSet;
use std::error::Error as StdError;
use std::fmt::{Display, Formatter, Result as FmtResult};
use std::fs;
use std::path::{Path, PathBuf};

/// The sources outside the crates' own `src` trees that are held to the no-cache rule, which is
/// the reference layout the invariant search runs over.
const HELD_ELSEWHERE: [&str; 1] = ["crates/vbc-layout/tests/fuzz/reference.rs"];

/// The words that would name a cache, a memoized answer, or the mutable state one is kept in.
const CACHE_WORDS: [&str; 9] = [
    "cache",
    "lazy_static",
    "memo",
    "mutex",
    "oncecell",
    "oncelock",
    "refcell",
    "rwlock",
    "thread_local",
];

/// The calls that lay a line out or map a position, each of which costs the line it is given, and
/// therefore costs the document when it is made once for every line of one.
const LAYOUT_CALLS: [&str; 3] = [
    "char_idx_at_visual_offset",
    "lay_out",
    "visual_offset_from_anchor",
];

/// The words that would name the anchor mapping or the whole-document layout.
const MAPPING_WORDS: [&str; 5] = [
    "anchor",
    "char_idx",
    "lay_out",
    "visual_offset",
    "wrappedlayout",
];

/// The words that say a source writes into the cells of a terminal buffer, which is what makes it
/// a source that draws.
const DRAWING_WORDS: [&str; 3] = ["ratatui", "set_char", "set_symbol"];

/// The name of the dependency a crate that draws is built on.
const DRAWING_DEPENDENCY: &str = "ratatui";

/// The word naming the vocabulary a layout is checked in, and the stem of the module declaring it,
/// which is the one source that vocabulary is written in rather than reached for.
const INVARIANT: &str = "invariant";
const INVARIANT_MODULE: &str = "invariants";

/// The root modules a crate is reached through.
const ROOT_MODULES: [&str; 2] = ["lib.rs", "main.rs"];

/// Something a scan found, which is the word that broke the rule and where it was written.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Finding {
    path: String,
    line: usize,
    word: String,
}

impl Finding {
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    #[must_use]
    pub fn line(&self) -> usize {
        self.line
    }

    #[must_use]
    pub fn word(&self) -> &str {
        &self.word
    }
}

impl Display for Finding {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        write!(
            formatter,
            "{}:{} names `{}`",
            self.path, self.line, self.word
        )
    }
}

/// The reason a scan could say nothing about a tree.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Error {
    /// The tree holds no crates at all.
    NoCrates { root: String },

    /// A crate of the tree holds no source directory.
    NoSourceDirectory { name: String },

    /// A source the scans are held to is missing.
    MissingSource { path: String },

    /// A source could not be read.
    Unreadable { path: String, reason: String },

    /// The scan read no source at all, so whatever it found it found by looking nowhere.
    ReadNothing,

    /// A crate of the tree went unread, root module and all.
    UnreadCrate { name: String },

    /// A module a read source declares went unread.
    UnreadModule { path: String, module: String },
}

impl Display for Error {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        match self {
            Self::NoCrates { root } => write!(formatter, "`{root}` holds no crates"),
            Self::NoSourceDirectory { name } => {
                write!(formatter, "the crate `{name}` holds no source directory")
            }
            Self::MissingSource { path } => write!(formatter, "`{path}` is missing"),
            Self::Unreadable { path, reason } => {
                write!(formatter, "`{path}` is unreadable: {reason}")
            }
            Self::ReadNothing => write!(formatter, "the scan read no source at all"),
            Self::UnreadCrate { name } => {
                write!(formatter, "the crate `{name}` went unread")
            }
            Self::UnreadModule { path, module } => {
                write!(formatter, "`{path}` declares `{module}`, which went unread")
            }
        }
    }
}

impl StdError for Error {}

/// # Returns
///
/// The root of the workspace this crate belongs to.
///
/// # Panics
///
/// Panics if this crate is not a member of a workspace.
#[must_use]
pub fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("this crate sits two directories below its workspace root")
        .to_owned()
}

/// # Returns
///
/// The path of every source of every crate of a tree, subdirectories included, in order, on
/// success.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`Error::NoSourceDirectory`] if a crate of the tree holds no source directory.
/// * [`Error::ReadNothing`] if the crates hold no source between them.
/// * Forwards [`crates`]'s return values on failure.
/// * Forwards [`collect`]'s return values on failure.
pub fn sources(root: &Path) -> Result<Vec<PathBuf>, Error> {
    let mut sources = Vec::new();
    for name in crates(root)? {
        let directory = root.join("crates").join(&name).join("src");
        if !directory.is_dir() {
            return Err(Error::NoSourceDirectory { name });
        }
        collect(&directory, &mut sources)?;
    }
    sources.sort();

    if sources.is_empty() {
        return Err(Error::ReadNothing);
    }

    Ok(sources)
}

/// # Returns
///
/// The name of every crate of a tree, in order, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`Error::NoCrates`] if the tree holds no crates directory, or if it holds no crate.
/// * [`Error::Unreadable`] if an entry of the crates directory cannot be read, which would
///   otherwise take a crate out of every scan without saying so.
pub fn crates(root: &Path) -> Result<Vec<String>, Error> {
    let directory = root.join("crates");
    let entries = fs::read_dir(&directory).map_err(|_error| Error::NoCrates {
        root: root.display().to_string(),
    })?;

    let mut names = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|error| Error::Unreadable {
                path: directory.display().to_string(),
                reason: error.to_string(),
            })?
            .path();
        if !path.is_dir() {
            continue;
        }
        if let Some(name) = path.file_name() {
            names.push(name.to_string_lossy().to_string());
        }
    }
    names.sort();

    if names.is_empty() {
        return Err(Error::NoCrates {
            root: root.display().to_string(),
        });
    }

    Ok(names)
}

/// Checks that a scan read the tree it is about: every crate the tree holds, and every module
/// those crates declare, both of which are read off the tree rather than written down.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`Error::ReadNothing`] if the scan read no source at all.
/// * [`Error::UnreadCrate`] if a crate's root module went unread.
/// * [`Error::UnreadModule`] if a module a read source declares went unread.
/// * Forwards [`crates`]'s return values on failure.
/// * Forwards [`read`]'s return values on failure.
pub fn coverage(root: &Path, scanned: &[PathBuf]) -> Result<(), Error> {
    if scanned.is_empty() {
        return Err(Error::ReadNothing);
    }

    let scanned_paths: BTreeSet<&PathBuf> = scanned.iter().collect();
    for name in crates(root)? {
        let directory = root.join("crates").join(&name).join("src");
        if !ROOT_MODULES
            .iter()
            .any(|module| scanned_paths.contains(&directory.join(module)))
        {
            return Err(Error::UnreadCrate { name });
        }
    }

    for path in scanned {
        let source = read(path)?;
        for module in declared_modules(&source) {
            if !module_paths(path, &module)
                .iter()
                .any(|candidate| scanned_paths.contains(candidate))
            {
                return Err(Error::UnreadModule {
                    path: relative(root, path),
                    module,
                });
            }
        }
    }

    Ok(())
}

/// Scans a tree for a cache of what it laid out.
///
/// # Returns
///
/// Every word naming a cache, in order, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`Error::MissingSource`] if a source held to this rule from outside the crates' own source
///   trees is missing.
/// * Forwards [`sources`]'s return values on failure.
/// * Forwards [`coverage`]'s return values on failure.
/// * Forwards [`read`]'s return values on failure.
pub fn scan_for_caches(root: &Path) -> Result<Vec<Finding>, Error> {
    let mut scanned = sources(root)?;
    coverage(root, &scanned)?;

    for held in HELD_ELSEWHERE {
        let path = root.join(held);
        if !path.is_file() {
            return Err(Error::MissingSource {
                path: held.to_owned(),
            });
        }
        scanned.push(path);
    }

    let mut findings = Vec::new();
    for path in scanned {
        let source = read(&path)?;
        findings.extend(caches(&source).into_iter().map(|(line, word)| Finding {
            path: relative(root, &path),
            line,
            word,
        }));
    }

    Ok(findings)
}

/// Scans a tree for a layout laying a whole text out outside its tests.
///
/// # Returns
///
/// Every layout call made once for every line of a text, in order, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`sources`]'s return values on failure.
/// * Forwards [`coverage`]'s return values on failure.
/// * Forwards [`read`]'s return values on failure.
pub fn scan_for_whole_text_layouts(root: &Path) -> Result<Vec<Finding>, Error> {
    let scanned = sources(root)?;
    coverage(root, &scanned)?;

    let mut findings = Vec::new();
    for path in scanned {
        let source = read(&path)?;
        findings.extend(
            whole_text_layouts(&source)
                .into_iter()
                .map(|(line, word)| Finding {
                    path: relative(root, &path),
                    line,
                    word,
                }),
        );
    }

    Ok(findings)
}

/// Scans a tree for a source that draws rows and maps them as well.
///
/// # Returns
///
/// Every word naming the mapping in a source that draws, in order, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`sources`]'s return values on failure.
/// * Forwards [`coverage`]'s return values on failure.
/// * Forwards [`read`]'s return values on failure.
pub fn scan_for_mappings_where_rows_are_drawn(root: &Path) -> Result<Vec<Finding>, Error> {
    let scanned = sources(root)?;
    coverage(root, &scanned)?;

    let mut findings = Vec::new();
    for path in scanned {
        let source = read(&path)?;
        if !draws(&source) {
            continue;
        }
        findings.extend(mappings(&source).into_iter().map(|(line, word)| Finding {
            path: relative(root, &path),
            line,
            word,
        }));
    }

    Ok(findings)
}

/// Scans a tree for a source that ships reaching for the vocabulary a layout is checked in.
///
/// # Returns
///
/// Every word naming that vocabulary outside the module declaring it, in order, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`sources`]'s return values on failure.
/// * Forwards [`coverage`]'s return values on failure.
/// * Forwards [`read`]'s return values on failure.
pub fn scan_for_invariant_vocabulary(root: &Path) -> Result<Vec<Finding>, Error> {
    let scanned = sources(root)?;
    coverage(root, &scanned)?;

    let mut findings = Vec::new();
    for path in scanned {
        if path
            .file_stem()
            .is_some_and(|stem| INVARIANT_MODULE == stem)
        {
            continue;
        }

        let source = read(&path)?;
        findings.extend(
            invariant_vocabulary(&source)
                .into_iter()
                .map(|(line, word)| Finding {
                    path: relative(root, &path),
                    line,
                    word,
                }),
        );
    }

    Ok(findings)
}

/// # Returns
///
/// The path of every source of a tree that draws into a terminal buffer, in order, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`sources`]'s return values on failure.
/// * Forwards [`read`]'s return values on failure.
pub fn drawing_sources(root: &Path) -> Result<Vec<String>, Error> {
    let mut drawing = Vec::new();
    for path in sources(root)? {
        if draws(&read(&path)?) {
            drawing.push(relative(root, &path));
        }
    }

    Ok(drawing)
}

/// # Returns
///
/// The name of every crate of a tree that is built to draw, in order, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`crates`]'s return values on failure.
/// * Forwards [`read`]'s return values on failure.
pub fn crates_that_draw(root: &Path) -> Result<Vec<String>, Error> {
    let mut drawing = Vec::new();
    for name in crates(root)? {
        let manifest = read(&root.join("crates").join(&name).join("Cargo.toml"))?;
        if manifest
            .lines()
            .any(|line| line.trim_start().starts_with(DRAWING_DEPENDENCY))
        {
            drawing.push(name);
        }
    }

    Ok(drawing)
}

/// # Returns
///
/// Every word of a source naming a cache, each paired with the line it is written on.
#[must_use]
pub fn caches(source: &str) -> Vec<(usize, String)> {
    let words = shape::words(source);
    let mut found: Vec<(usize, String)> = words
        .iter()
        .filter_map(|word| {
            let lowercase = word.text().to_lowercase();
            CACHE_WORDS
                .iter()
                .find(|cache| lowercase.contains(*cache))
                .map(|cache| (word.line(), (*cache).to_owned()))
        })
        .collect();

    found.extend(
        words
            .windows(2)
            .filter(|pair| "static" == pair[0].text() && "mut" == pair[1].text())
            .map(|pair| (pair[0].line(), "static mut".to_owned())),
    );
    found.sort();

    found
}

/// # Returns
///
/// Every layout call a source makes once for every line of a text, outside its tests, each paired
/// with the line it is written on.
#[must_use]
pub fn whole_text_layouts(source: &str) -> Vec<(usize, String)> {
    shape::words(source)
        .iter()
        .filter(|word| word.called() && word.repeated() && !word.tested())
        .filter(|word| LAYOUT_CALLS.contains(&word.text()))
        .map(|word| (word.line(), word.text().to_owned()))
        .collect()
}

/// # Returns
///
/// Every word of a source naming the anchor mapping outside its tests, each paired with the line
/// it is written on.
#[must_use]
pub fn mappings(source: &str) -> Vec<(usize, String)> {
    shape::words(source)
        .iter()
        .filter(|word| !word.tested())
        .filter_map(|word| {
            let lowercase = word.text().to_lowercase();
            MAPPING_WORDS
                .iter()
                .find(|mapping| lowercase.contains(*mapping))
                .map(|mapping| (word.line(), (*mapping).to_owned()))
        })
        .collect()
}

/// # Returns
///
/// Every word of a source naming the vocabulary a layout is checked in outside its tests, each
/// paired with the line it is written on. The declaration handing the module to the tests names it
/// without reaching for it, so it is not one of them.
#[must_use]
pub fn invariant_vocabulary(source: &str) -> Vec<(usize, String)> {
    let words = shape::words(source);
    words
        .iter()
        .enumerate()
        .filter(|(index, word)| !word.tested() && (0 == *index || "mod" != words[index - 1].text()))
        .filter(|(_, word)| word.text().to_lowercase().contains(INVARIANT))
        .map(|(_, word)| (word.line(), INVARIANT.to_owned()))
        .collect()
}

/// # Returns
///
/// Whether a source writes into the cells of a terminal buffer outside its tests.
#[must_use]
pub fn draws(source: &str) -> bool {
    shape::words(source)
        .iter()
        .any(|word| !word.tested() && DRAWING_WORDS.contains(&word.text()))
}

/// # Returns
///
/// The contents of a source on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`Error::Unreadable`] if the source cannot be read.
pub fn read(path: &Path) -> Result<String, Error> {
    fs::read_to_string(path).map_err(|error| Error::Unreadable {
        path: path.display().to_string(),
        reason: error.to_string(),
    })
}

/// # Returns
///
/// `path` written relative to the root of the tree it belongs to, with forward slashes, or as it
/// stands where it belongs to another tree.
#[must_use]
pub fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Collects every Rust source under a directory, its subdirectories included.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`Error::Unreadable`] if the directory cannot be read.
fn collect(directory: &Path, sources: &mut Vec<PathBuf>) -> Result<(), Error> {
    let entries = fs::read_dir(directory).map_err(|error| Error::Unreadable {
        path: directory.display().to_string(),
        reason: error.to_string(),
    })?;

    for entry in entries {
        let path = entry
            .map_err(|error| Error::Unreadable {
                path: directory.display().to_string(),
                reason: error.to_string(),
            })?
            .path();
        if path.is_dir() {
            collect(&path, sources)?;
        } else if path.extension().is_some_and(|extension| "rs" == extension) {
            sources.push(path);
        }
    }

    Ok(())
}

/// # Returns
///
/// The name of every module a source declares in a file of its own.
fn declared_modules(source: &str) -> Vec<String> {
    source
        .lines()
        .filter_map(|line| {
            let declaration = line.trim();
            let declaration = declaration
                .strip_prefix("pub ")
                .or_else(|| declaration.strip_prefix("pub(crate) "))
                .unwrap_or(declaration);
            declaration
                .strip_prefix("mod ")
                .and_then(|named| named.strip_suffix(';'))
                .map(str::to_owned)
        })
        .collect()
}

/// # Returns
///
/// The paths a module declared by the source at `path` can be written in.
fn module_paths(path: &Path, module: &str) -> Vec<PathBuf> {
    let Some(directory) = path.parent() else {
        return Vec::new();
    };
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let directory = if ROOT_MODULES.iter().any(|root| path.ends_with(root)) || "mod" == stem {
        directory.to_owned()
    } else {
        directory.join(stem.as_ref())
    };

    vec![
        directory.join(format!("{module}.rs")),
        directory.join(module).join("mod.rs"),
    ]
}
