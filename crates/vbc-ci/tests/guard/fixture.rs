//! A copy of a tree that a guard can be broken in without breaking the workspace.
//!
//! A guard is only worth what it catches, and the only way to know what one catches is to write
//! the offence it is meant to catch and watch it go red. The offences are therefore written into a
//! copy of this workspace's own crates rather than into a tree invented for the occasion, so a
//! guard that passes over the copy is passing over the code it really guards.

use std::fs;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicUsize, Ordering};

/// What tells one live fixture from another, so that tests running side by side never share a
/// tree.
static FIXTURES: AtomicUsize = AtomicUsize::new(0);

/// A copy of a tree, removed when it goes out of scope.
#[derive(Debug)]
pub struct Fixture {
    root: PathBuf,
}

impl Fixture {
    /// Factory function.
    ///
    /// # Returns
    ///
    /// A newly created copy of the crates of the tree rooted at `source`.
    ///
    /// # Panics
    ///
    /// Panics if the crates cannot be copied.
    #[must_use]
    pub fn of(source: &Path) -> Self {
        let fixture = Self::empty();
        copy(&source.join("crates"), &fixture.root.join("crates"));

        fixture
    }

    /// Factory function.
    ///
    /// # Returns
    ///
    /// A newly created tree holding nothing at all.
    ///
    /// # Panics
    ///
    /// Panics if the tree cannot be created.
    #[must_use]
    pub fn empty() -> Self {
        let root = std::env::temp_dir().join(format!(
            "vbc-guard-{}-{}",
            process::id(),
            FIXTURES.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root)
            .unwrap_or_else(|error| panic!("{} is writable: {error}", root.display()));

        Self { root }
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Writes `code` to the end of one of the fixture's sources, which is how an offence is put
    /// where the guards will find it.
    ///
    /// # Panics
    ///
    /// Panics if the source cannot be read or written.
    pub fn append(&self, path: &str, code: &str) {
        let path = self.root.join(path);
        let mut source = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("{} is readable: {error}", path.display()));
        source.push('\n');
        source.push_str(code);
        fs::write(&path, source)
            .unwrap_or_else(|error| panic!("{} is writable: {error}", path.display()));
    }

    /// Takes one of the fixture's files away, which is how a tree that a scan cannot read the
    /// whole of is made.
    ///
    /// # Panics
    ///
    /// Panics if the file cannot be removed.
    pub fn remove(&self, path: &str) {
        let path = self.root.join(path);
        fs::remove_file(&path)
            .unwrap_or_else(|error| panic!("{} is removable: {error}", path.display()));
    }

    /// Writes `code` to one of the fixture's files, creating the directories above it.
    ///
    /// # Panics
    ///
    /// Panics if the file cannot be written.
    pub fn write(&self, path: &str, code: &str) {
        let path = self.root.join(path);
        if let Some(directory) = path.parent() {
            fs::create_dir_all(directory)
                .unwrap_or_else(|error| panic!("{} is writable: {error}", directory.display()));
        }
        fs::write(&path, code)
            .unwrap_or_else(|error| panic!("{} is writable: {error}", path.display()));
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// Copies a directory and everything under it.
///
/// # Panics
///
/// Panics if the directory cannot be copied.
fn copy(source: &Path, destination: &Path) {
    fs::create_dir_all(destination)
        .unwrap_or_else(|error| panic!("{} is writable: {error}", destination.display()));

    let entries = fs::read_dir(source)
        .unwrap_or_else(|error| panic!("{} is readable: {error}", source.display()));
    for entry in entries {
        let path = entry
            .unwrap_or_else(|error| panic!("{} is readable: {error}", source.display()))
            .path();
        let Some(name) = path.file_name() else {
            continue;
        };
        let copied = destination.join(name);
        if path.is_dir() {
            copy(&path, &copied);
        } else {
            fs::copy(&path, &copied).unwrap_or_else(|error| {
                panic!(
                    "{} is copied to {}: {error}",
                    path.display(),
                    copied.display()
                )
            });
        }
    }
}
