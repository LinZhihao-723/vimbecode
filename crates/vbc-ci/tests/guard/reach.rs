//! What a keystroke can reach, read off the import graph the workspace's binaries are the roots of.
//!
//! A module the application has drifted away from is not a module that fails a test. Its own tests
//! go on passing, its crate goes on compiling, and the only thing that has gone wrong is that no
//! run of the program can arrive at it. A workspace can accumulate a great deal of such work
//! without a single test going red, which is what this reads for.
//!
//! Reachability is derived rather than listed. The roots are the binaries the tree holds, found by
//! walking it rather than written down here, so a binary added later is a root without anyone
//! remembering to say so. The edges are the paths one source names in another module: a `use`
//! tree, a path written out where it is called, an import branch shared under a brace. What is not
//! an edge is a declaration, because `pub mod chat;` hands a module to the compiler without any
//! code reaching for it, and neither is a path written where the tests alone are compiled, whether
//! that is a `#[cfg(test)]` module or a single import under the same attribute, because a module
//! its own tests import is exactly the module this exists to name.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use super::shape;
use super::{coverage, read, relative, sources, Error, Finding};

/// The source a crate's library is reached through, and the one its own binary is.
const LIBRARY_ROOT: &str = "lib.rs";
const BINARY_ROOT: &str = "main.rs";

/// The directory a crate's further binaries are written in.
const BINARY_DIRECTORY: &str = "bin";

/// The source that stands for the directory around it rather than for a module beside it.
const DIRECTORY_ROOT: &str = "mod.rs";

/// The names a module reaches its own crate, itself and the module above it through.
const CRATE: &str = "crate";
const SELF: &str = "self";
const PARENT: &str = "super";

/// A module of the workspace: a source of a crate's own tree, together with the path the rest of
/// the workspace reaches it through.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Module {
    name: String,
    path: String,
    crate_name: String,
    segments: Vec<String>,
    binary: bool,
}

impl Module {
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// # Returns
    ///
    /// Whether the source is a root a run starts at, which is a crate's binary rather than a
    /// module of one.
    #[must_use]
    pub fn binary(&self) -> bool {
        self.binary
    }

    /// # Returns
    ///
    /// Whether the source is a crate's root rather than a module the rest of the workspace can
    /// name, which a binary's root and a library's root both are.
    #[must_use]
    pub fn root(&self) -> bool {
        self.segments.is_empty()
    }
}

/// Reads the modules of a tree off its crates' source trees.
///
/// # Returns
///
/// Every module of every crate of the tree, in the order its sources are read, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * Forwards [`sources`]'s return values on failure.
pub fn modules(root: &Path) -> Result<Vec<Module>, Error> {
    Ok(sources(root)?
        .iter()
        .filter_map(|path| module(&relative(root, path)))
        .collect())
}

/// Scans a tree for the modules no binary of it reaches.
///
/// # Returns
///
/// Every module of the tree that no run can arrive at, in order by name, on success. A module is
/// reported at the head of the source declaring it rather than at a word of it, because what broke
/// the rule is the module itself and not anything written in it.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`Error::NoBinaries`] if the tree holds no binary, and therefore no run to reach anything
///   from.
/// * Forwards [`sources`]'s return values on failure.
/// * Forwards [`coverage`]'s return values on failure.
/// * Forwards [`read`]'s return values on failure.
pub fn unreachable(root: &Path) -> Result<Vec<Finding>, Error> {
    let scanned = sources(root)?;
    coverage(root, &scanned)?;

    let modules = modules(root)?;
    if !modules.iter().any(Module::binary) {
        return Err(Error::NoBinaries {
            root: root.display().to_string(),
        });
    }

    let crates: BTreeSet<&str> = modules
        .iter()
        .map(|module| module.crate_name.as_str())
        .collect();
    let index: BTreeMap<(&str, &[String]), usize> = modules
        .iter()
        .enumerate()
        .filter(|(_, module)| !module.root())
        .map(|(at, module)| ((module.crate_name.as_str(), module.segments.as_slice()), at))
        .collect();
    let roots: BTreeMap<&str, usize> = modules
        .iter()
        .enumerate()
        .filter(|(_, module)| module.root() && !module.binary())
        .map(|(at, module)| (module.crate_name.as_str(), at))
        .collect();

    let mut edges: Vec<BTreeSet<usize>> = Vec::with_capacity(modules.len());
    for module in &modules {
        let source = read(&root.join(&module.path))?;
        edges.push(named(&source, module, &crates, &roots, &index));
    }

    let mut reached: BTreeSet<usize> = BTreeSet::new();
    let mut pending: Vec<usize> = modules
        .iter()
        .enumerate()
        .filter(|(_, module)| module.binary())
        .map(|(at, _)| at)
        .collect();
    while let Some(at) = pending.pop() {
        if !reached.insert(at) {
            continue;
        }
        pending.extend(edges[at].iter().filter(|next| !reached.contains(next)));
    }

    let mut findings: Vec<Finding> = modules
        .iter()
        .enumerate()
        .filter(|(at, module)| !module.root() && !reached.contains(at))
        .map(|(_, module)| Finding {
            path: module.path.clone(),
            line: 1,
            word: module.name.clone(),
        })
        .collect();
    findings.sort_by(|left, right| left.word.cmp(&right.word));

    Ok(findings)
}

/// # Returns
///
/// The module a source of a crate's own tree is, or [`None`] where the path is not one.
fn module(path: &str) -> Option<Module> {
    let mut parts = path.split('/');
    if Some("crates") != parts.next() {
        return None;
    }
    let crate_name = parts.next()?.replace('-', "_");
    if Some("src") != parts.next() {
        return None;
    }

    let rest: Vec<&str> = parts.collect();
    let (first, last) = (*rest.first()?, *rest.last()?);
    let binary = (BINARY_ROOT == last && 1 == rest.len()) || BINARY_DIRECTORY == first;
    let mut segments: Vec<String> = rest[..rest.len() - 1]
        .iter()
        .map(|part| (*part).to_owned())
        .collect();
    if binary || (LIBRARY_ROOT == last && 1 == rest.len()) {
        segments.clear();
    } else if DIRECTORY_ROOT != last {
        segments.push(last.trim_end_matches(".rs").to_owned());
    }

    let name = if segments.is_empty() {
        path.to_owned()
    } else {
        format!("{crate_name}::{}", segments.join("::"))
    };

    Some(Module {
        name,
        path: path.to_owned(),
        crate_name,
        segments,
        binary,
    })
}

/// # Returns
///
/// Every module a source names, which is what a run arriving at that source can go on to reach.
/// Naming another crate reaches that crate's root as well as the module under it, because a root
/// that hands a module out under another name is a path to the module rather than a declaration of
/// one.
fn named(
    source: &str,
    module: &Module,
    crates: &BTreeSet<&str>,
    roots: &BTreeMap<&str, usize>,
    index: &BTreeMap<(&str, &[String]), usize>,
) -> BTreeSet<usize> {
    let mut named = BTreeSet::new();
    for path in shape::paths(source) {
        if path.tested() {
            continue;
        }
        if let Some(at) = path
            .segments()
            .first()
            .and_then(|first| roots.get(first.as_str()))
        {
            named.insert(*at);
        }
        for (crate_name, segments) in resolved(path.segments(), module, crates) {
            for length in 1..=segments.len() {
                if let Some(at) = index.get(&(crate_name, &segments[..length])) {
                    named.insert(*at);
                }
            }
        }
    }

    named
}

/// # Returns
///
/// Every crate and module path a source's path could stand for, read against the module the source
/// is: `crate`, `self` and `super` are answered from where the path is written, and a leading crate
/// name is answered from the workspace.
fn resolved<'names>(
    path: &[String],
    module: &'names Module,
    crates: &BTreeSet<&'names str>,
) -> Vec<(&'names str, Vec<String>)> {
    let Some(first) = path.first() else {
        return Vec::new();
    };

    let own = module.crate_name.as_str();
    if CRATE == first {
        return vec![(own, path[1..].to_vec())];
    }
    if SELF == first {
        return vec![(own, [module.segments.as_slice(), &path[1..]].concat())];
    }
    if PARENT == first {
        let above = path.iter().take_while(|segment| PARENT == *segment).count();
        let Some(kept) = module.segments.len().checked_sub(above) else {
            return Vec::new();
        };
        return vec![(own, [&module.segments[..kept], &path[above..]].concat())];
    }

    let mut resolved = Vec::new();
    if let Some(named) = crates.get(first.as_str()) {
        resolved.push((*named, path[1..].to_vec()));
    }
    if 1 < path.len() {
        resolved.push((own, [module.segments.as_slice(), path].concat()));
    }

    resolved
}
