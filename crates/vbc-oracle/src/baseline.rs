//! The corpus baseline: a golden record of the state vim ends every corpus case in.
//!
//! A differential run compares two engines with each other, so it says nothing about the reference
//! side moving: a rewritten capture, a different vim, an edited corpus case all change what vim is
//! taken to say without either engine disagreeing with the other. A baseline pins that reference
//! side down. It records the state vim ends every case in, together with the vim version it was
//! recorded from, a hash of the corpus it was recorded over, and the version of its own schema. A
//! later check replays the corpus and reports every case whose state moved, naming the dimensions
//! it moved in.
//!
//! Two vim releases end a case in different states by themselves, so a baseline is authoritative
//! only for the vim release series it was recorded from. A check running against another series
//! therefore reports that the recorded states were not compared, rather than reporting drift that
//! is not drift. The continuous-integration job pins the series, records the baseline there and
//! checks it there, which is what makes the recorded states an authority rather than a snapshot of
//! whichever vim a developer happens to have.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error as StdError;
use std::fmt::{Display, Formatter, Result as FmtResult};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::corpus::{Case, Corpus};
use crate::runner::{Dimension, Engine};
use crate::state::{describe_register, Divergence, EditorState};
use crate::vim::{VimDriver, VimVersion};

/// The version of the schema a baseline file is written in, raised whenever a file an older build
/// wrote can no longer be compared against a freshly captured state.
pub const SCHEMA_VERSION: u32 = 1;

/// The engine a baseline records the states of: the reference side of a differential run, together
/// with the version of the vim behind it.
pub trait Reference: Engine {
    /// # Returns
    ///
    /// The version of the vim the states are captured from.
    fn vim_version(&self) -> VimVersion;
}

impl Reference for VimDriver {
    fn vim_version(&self) -> VimVersion {
        self.version()
    }
}

/// What a baseline was recorded from and over, which decides whether its states can be compared at
/// all.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Header {
    /// The version of the schema the baseline is written in.
    pub schema_version: u32,

    /// The version of the vim the states were captured from.
    pub vim_version: VimVersion,

    /// The hash of the corpus the states were captured over.
    pub corpus_hash: String,
}

/// The state vim ends every corpus case in, with what it was recorded from.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Baseline {
    /// What the states were recorded from and over.
    pub header: Header,

    /// The state vim ends each case in, keyed by the case's identifier.
    pub cases: BTreeMap<String, EditorState>,
}

impl Baseline {
    /// Captures the state the reference ends every case of the corpus in.
    ///
    /// # Type Parameters
    ///
    /// * `ReferenceType` - The engine the states are captured from.
    ///
    /// # Returns
    ///
    /// A newly recorded baseline on success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * [`Error::Unreplayable`] if the reference could not replay a case, since a baseline
    ///   holding only the cases that happened to replay guards the rest of the corpus against
    ///   nothing.
    /// * Forwards [`corpus_hash`]'s return values on failure.
    pub fn record<ReferenceType: Reference>(
        corpus: &Corpus,
        reference: &ReferenceType,
    ) -> Result<Self, Error> {
        let mut cases = BTreeMap::new();
        for case in corpus.cases() {
            let state = reference
                .replay(case)
                .map_err(|source| Error::Unreplayable {
                    id: case.id.clone(),
                    message: source.to_string(),
                })?;
            cases.insert(case.id.clone(), state);
        }

        Ok(Self {
            header: Header {
                schema_version: SCHEMA_VERSION,
                vim_version: reference.vim_version(),
                corpus_hash: corpus_hash(corpus)?,
            },
            cases,
        })
    }

    /// Reads a baseline from the file it was written to.
    ///
    /// # Returns
    ///
    /// The baseline the file holds on success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * [`Error::Io`] if the file could not be read.
    /// * [`Error::Decode`] if the file is not a baseline this build reads.
    pub fn read(path: &Path) -> Result<Self, Error> {
        let text = fs::read_to_string(path).map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })?;

        serde_json::from_str(&text).map_err(|source| Error::Decode {
            path: path.to_path_buf(),
            source,
        })
    }

    /// Writes the baseline to a file, byte for byte the same file for the same states.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * [`Error::Io`] if the file could not be written.
    /// * Forwards [`Baseline::to_json`]'s return values on failure.
    pub fn write(&self, path: &Path) -> Result<(), Error> {
        fs::write(path, self.to_json()?).map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })
    }

    /// Serializes the baseline as the file it is written to.
    ///
    /// # Returns
    ///
    /// The baseline as JSON on success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * [`Error::Encode`] if the baseline could not be serialized.
    pub fn to_json(&self) -> Result<String, Error> {
        let mut json =
            serde_json::to_string_pretty(self).map_err(|source| Error::Encode { source })?;
        json.push('\n');

        Ok(json)
    }

    /// Replays the corpus against the reference and compares what it captures with what the
    /// baseline records.
    ///
    /// The states are compared only when the reference runs a vim of the release series the
    /// baseline was recorded from. The schema version and the corpus hash are checked either way,
    /// since neither depends on the vim behind the reference.
    ///
    /// # Type Parameters
    ///
    /// * `ReferenceType` - The engine the states are captured from.
    ///
    /// # Returns
    ///
    /// What the check found on success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * [`Error::SchemaVersion`] if the baseline is written in a schema this build does not read.
    /// * [`Error::CorpusChanged`] if the corpus is not the one the baseline was recorded over.
    /// * Forwards [`corpus_hash`]'s return values on failure.
    pub fn check<ReferenceType: Reference>(
        &self,
        corpus: &Corpus,
        reference: &ReferenceType,
    ) -> Result<Check, Error> {
        if SCHEMA_VERSION != self.header.schema_version {
            return Err(Error::SchemaVersion {
                recorded: self.header.schema_version,
            });
        }
        let current = corpus_hash(corpus)?;
        if current != self.header.corpus_hash {
            return Err(Error::CorpusChanged {
                recorded: self.header.corpus_hash.clone(),
                current,
            });
        }
        let recorded = self.header.vim_version;
        let running = reference.vim_version();
        if !same_series(recorded, running) {
            return Ok(Check::Skipped { recorded, running });
        }

        let mut drifts = Vec::new();
        for case in corpus.cases() {
            let Some(state) = self.cases.get(&case.id) else {
                drifts.push(Drift::Unrecorded {
                    id: case.id.clone(),
                });
                continue;
            };
            match reference.replay(case) {
                Ok(captured) => {
                    let divergences = state.diff(&captured);
                    if !divergences.is_empty() {
                        drifts.push(Drift::Changed {
                            id: case.id.clone(),
                            dimensions: Dimension::of(&divergences),
                            divergences,
                        });
                    }
                }
                Err(source) => drifts.push(Drift::Unreplayable {
                    id: case.id.clone(),
                    message: source.to_string(),
                }),
            }
        }
        let replayed: BTreeSet<&str> = corpus.cases().iter().map(|case| case.id.as_str()).collect();
        for id in self.cases.keys() {
            if !replayed.contains(id.as_str()) {
                drifts.push(Drift::Stale { id: id.clone() });
            }
        }

        if drifts.is_empty() {
            return Ok(Check::Matched {
                vim_version: running,
                cases: self.cases.len(),
            });
        }

        Ok(Check::Drifted {
            recorded,
            running,
            drifts,
        })
    }
}

/// What checking a baseline found.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "verdict")]
pub enum Check {
    /// Every case ends where the baseline records it.
    Matched {
        /// The version of the vim the states were captured from.
        vim_version: VimVersion,

        /// The number of cases compared.
        cases: usize,
    },

    /// The recorded states were not compared, because the reference runs a vim of another release
    /// series than the one the baseline was recorded from.
    Skipped {
        /// The version the baseline was recorded from.
        recorded: VimVersion,

        /// The version the check ran against.
        running: VimVersion,
    },

    /// At least one case no longer ends where the baseline records it.
    Drifted {
        /// The version the baseline was recorded from.
        recorded: VimVersion,

        /// The version the check ran against.
        running: VimVersion,

        /// Every case that drifted, in corpus order.
        drifts: Vec<Drift>,
    },
}

impl Check {
    /// # Returns
    ///
    /// Whether the recorded states were compared and every one of them held.
    #[must_use]
    pub fn matched(&self) -> bool {
        matches!(self, Self::Matched { .. })
    }

    /// # Returns
    ///
    /// Whether the recorded states were compared and at least one of them moved.
    #[must_use]
    pub fn drifted(&self) -> bool {
        matches!(self, Self::Drifted { .. })
    }
}

impl Display for Check {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        match self {
            Self::Matched { vim_version, cases } => writeln!(
                formatter,
                "the baseline holds: {cases} cases captured from vim {vim_version} end where it \
                 records them"
            ),
            Self::Skipped { recorded, running } => {
                writeln!(formatter, "the recorded states were not compared")?;
                writeln!(
                    formatter,
                    "  the baseline was recorded from vim {recorded}, and this check ran against \
                     vim {running}"
                )?;
                writeln!(
                    formatter,
                    "  two vim releases end a case in different states by themselves, so \
                     comparing across them would report drift that is not drift"
                )?;
                writeln!(
                    formatter,
                    "  the schema version and the corpus hash were checked, and both are current"
                )?;

                writeln!(
                    formatter,
                    "  continuous integration pins vim {}.{} and is the authority for the \
                     recorded states: install that vim to check them here, and record the \
                     baseline again only from it",
                    recorded.major, recorded.minor
                )
            }
            Self::Drifted {
                recorded,
                running,
                drifts,
            } => {
                writeln!(
                    formatter,
                    "the baseline no longer holds: {} cases captured from vim {running} do not \
                     end where the baseline, recorded from vim {recorded}, records them",
                    drifts.len()
                )?;
                for drift in drifts {
                    writeln!(formatter)?;
                    write!(formatter, "{drift}")?;
                }

                Ok(())
            }
        }
    }
}

/// One case that no longer ends where the baseline records it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "drift")]
pub enum Drift {
    /// The corpus holds a case the baseline records no state for.
    Unrecorded {
        /// The identifier of the case.
        id: String,
    },

    /// The baseline records a state for a case the corpus no longer holds.
    Stale {
        /// The identifier of the case.
        id: String,
    },

    /// The reference could not replay a case the baseline records a state for.
    Unreplayable {
        /// The identifier of the case.
        id: String,

        /// What the reference reported.
        message: String,
    },

    /// A case's captured state is not the state the baseline records.
    Changed {
        /// The identifier of the case.
        id: String,

        /// The dimensions the case moved in, without repetition.
        dimensions: Vec<Dimension>,

        /// Every disagreement, with the recorded and the captured value.
        divergences: Vec<Divergence>,
    },
}

impl Drift {
    /// # Returns
    ///
    /// The identifier of the case that drifted.
    #[must_use]
    pub fn id(&self) -> &str {
        match self {
            Self::Unrecorded { id }
            | Self::Stale { id }
            | Self::Unreplayable { id, .. }
            | Self::Changed { id, .. } => id,
        }
    }
}

impl Display for Drift {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        match self {
            Self::Unrecorded { id } => writeln!(
                formatter,
                "case `{id}`: the corpus holds it and the baseline records no state for it"
            ),
            Self::Stale { id } => writeln!(
                formatter,
                "case `{id}`: the baseline records a state for it and the corpus no longer holds \
                 it"
            ),
            Self::Unreplayable { id, message } => {
                writeln!(formatter, "case `{id}`: it could not be replayed")?;
                writeln!(formatter, "  error: {message}")
            }
            Self::Changed {
                id,
                dimensions,
                divergences,
            } => {
                let names: Vec<String> = dimensions.iter().map(ToString::to_string).collect();
                writeln!(formatter, "case `{id}`: drifted in {}", names.join(", "))?;
                for divergence in divergences {
                    let (dimension, recorded, captured) = describe(divergence);
                    writeln!(formatter, "  {dimension}:")?;
                    writeln!(formatter, "    recorded : {recorded}")?;
                    writeln!(formatter, "    captured : {captured}")?;
                }

                Ok(())
            }
        }
    }
}

/// The ways a baseline can fail to be recorded, read, or compared against a corpus.
#[derive(Debug)]
pub enum Error {
    /// The baseline file could not be read or written.
    Io {
        /// The file the baseline is kept in.
        path: PathBuf,

        /// The underlying failure.
        source: io::Error,
    },

    /// The baseline file could not be decoded.
    Decode {
        /// The file the baseline is kept in.
        path: PathBuf,

        /// The underlying failure.
        source: serde_json::Error,
    },

    /// A baseline, or the corpus behind its hash, could not be serialized.
    Encode {
        /// The underlying failure.
        source: serde_json::Error,
    },

    /// The baseline is written in a schema this build does not read.
    SchemaVersion {
        /// The schema version the baseline is written in.
        recorded: u32,
    },

    /// The corpus is not the one the baseline was recorded over.
    CorpusChanged {
        /// The hash the baseline records.
        recorded: String,

        /// The hash the corpus has now.
        current: String,
    },

    /// The reference could not replay a case, so no state could be recorded for it.
    Unreplayable {
        /// The identifier of the case.
        id: String,

        /// What the reference reported.
        message: String,
    },
}

impl Display for Error {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        match self {
            Self::Io { path, source } => write!(
                formatter,
                "cannot access the baseline {}: {source}",
                path.display()
            ),
            Self::Decode { path, source } => write!(
                formatter,
                "the baseline {} cannot be decoded: {source}; a baseline written in another schema \
                 has to be recorded again",
                path.display()
            ),
            Self::Encode { source } => write!(formatter, "cannot serialize the baseline: {source}"),
            Self::SchemaVersion { recorded } => write!(
                formatter,
                "the baseline is written in schema version {recorded}, and this build reads schema \
                 version {SCHEMA_VERSION}; record the baseline again"
            ),
            Self::CorpusChanged { recorded, current } => write!(
                formatter,
                "the corpus is not the one the baseline was recorded over: the baseline records \
                 the corpus hash {recorded}, and the corpus hashes to {current}; a case was added, \
                 removed or edited, so record the baseline again once the change is meant"
            ),
            Self::Unreplayable { id, message } => write!(
                formatter,
                "the case `{id}` could not be replayed, so no state could be recorded for it: \
                 {message}"
            ),
        }
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Decode { source, .. } | Self::Encode { source } => Some(source),
            _ => None,
        }
    }
}

/// The file the repository's own baseline is kept in.
///
/// The path is resolved from the crate's source location, so it only exists in a checkout of the
/// repository.
///
/// # Returns
///
/// The path of the repository's baseline file.
#[must_use]
pub fn default_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("corpus")
        .join("baseline.json")
}

/// Hashes everything the corpus's cases declare, keyed by identifier, so an edit to what a case
/// declares is caught while moving a case from one section file to another is not.
///
/// # Returns
///
/// The hash, as lowercase hexadecimal, on success.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`Error::Encode`] if the cases could not be serialized.
pub fn corpus_hash(corpus: &Corpus) -> Result<String, Error> {
    let cases: BTreeMap<&str, &Case> = corpus
        .cases()
        .iter()
        .map(|case| (case.id.as_str(), case))
        .collect();
    let serialized = serde_json::to_vec(&cases).map_err(|source| Error::Encode { source })?;

    Ok(format!("{:x}", Sha256::digest(&serialized)))
}

/// # Returns
///
/// Whether the two vim versions are of the same release series, which is what a baseline's
/// recorded states carry across.
fn same_series(recorded: VimVersion, running: VimVersion) -> bool {
    recorded.major == running.major && recorded.minor == running.minor
}

/// # Returns
///
/// What a divergence is named by, the value the baseline records, and the value the check
/// captured, all rendered for a human.
fn describe(divergence: &Divergence) -> (String, String, String) {
    let (recorded, captured) = match divergence {
        Divergence::Buffer { left, right } => (format!("{left:?}"), format!("{right:?}")),
        Divergence::Cursor { left, right } => (
            format!("line {}, column {}", left.line, left.column),
            format!("line {}, column {}", right.line, right.column),
        ),
        Divergence::DisplayPosition { left, right } => (
            format!("screen row {}, screen column {}", left.row, left.column),
            format!("screen row {}, screen column {}", right.row, right.column),
        ),
        Divergence::Mode { left, right } => (format!("{left:?}"), format!("{right:?}")),
        Divergence::Register { left, right, .. } => (
            describe_register(left.as_ref()),
            describe_register(right.as_ref()),
        ),
    };
    let name = match divergence {
        Divergence::Register { name, .. } => format!("register `{name}`"),
        other => Dimension::from(other).to_string(),
    };

    (name, recorded, captured)
}
