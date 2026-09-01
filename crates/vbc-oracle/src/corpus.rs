//! The differential test corpus: the declarative cases that the harness replays against both
//! engines.
//!
//! A case fixes everything the two engines must agree on before a key is pressed -- the starting
//! buffer, the viewport width, and the display options -- together with the keys to replay and the
//! tags the corpus is sliced by. Cases are stored as TOML sections in a directory, and the loader
//! rejects a section that would otherwise contribute an unusable case.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error as StdError;
use std::fmt::{Display, Formatter, Result as FmtResult};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::string::FromUtf8Error;

use serde::{Deserialize, Serialize};

/// The extension of the files a corpus directory's sections are stored in.
pub const SECTION_EXTENSION: &str = "toml";

/// How characters of ambiguous East Asian width are measured, mirroring vim's `'ambiwidth'`.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AmbiWidth {
    /// Ambiguous characters occupy one cell.
    #[default]
    Single,

    /// Ambiguous characters occupy two cells.
    Double,
}

/// A label a case carries, by which the corpus is sliced into the areas it covers.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Tag {
    /// The case distinguishes the two `'ambiwidth'` settings.
    Ambiwidth,

    /// The case's text is plain ASCII.
    Ascii,

    /// The case is laid out with `'breakindent'` set.
    Breakindent,

    /// The case's text contains Chinese, Japanese, or Korean characters.
    Cjk,

    /// The case's text is source code rather than prose.
    Code,

    /// The case's text contains combining marks.
    Combining,

    /// The case's text contains emoji, including joined sequences.
    Emoji,

    /// The case's text contains regional-indicator flags.
    Flag,

    /// The case's text contains decomposed (NFD) clusters.
    Nfd,

    /// The case is laid out with `'wrap'` unset.
    Nowrap,

    /// The case is laid out with a `'showbreak'` marker.
    Showbreak,

    /// The case's text contains tabs.
    Tab,

    /// The case is laid out with `'wrap'` set.
    Wrap,

    /// The case exercises a word or WORD motion.
    WordMotion,
}

/// The display options an engine is configured with before a case is replayed.
///
/// A field left out of a case's TOML takes vim's own default.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Options {
    /// Whether a line too long for the viewport continues on the next screen line.
    pub wrap: bool,

    /// Whether continuation screen lines repeat the line's indent.
    pub breakindent: bool,

    /// The marker put in front of a continuation screen line, empty for none.
    pub showbreak: String,

    /// The number of cells a tab advances to.
    pub tabstop: u16,

    /// How characters of ambiguous East Asian width are measured.
    pub ambiwidth: AmbiWidth,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            wrap: true,
            breakindent: false,
            showbreak: String::new(),
            tabstop: 8,
            ambiwidth: AmbiWidth::Single,
        }
    }
}

/// A single corpus case: one starting state, one key sequence, one expectation of agreement.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Case {
    /// The identifier the case is reported under, unique across the whole corpus.
    pub id: String,

    /// What the case exercises, in one sentence.
    pub description: String,

    /// The buffer's text before the keys are replayed.
    pub buffer: String,

    /// The keys to replay, in vim's notation, for example `dw` or `iabc<Esc>`.
    pub keys: String,

    /// The width of the viewport the buffer is laid out in, in cells.
    pub viewport_width: u16,

    /// The areas the case covers.
    pub tags: BTreeSet<Tag>,

    /// The display options the engines are configured with.
    #[serde(default)]
    pub options: Options,
}

/// Every case the corpus holds, ordered by section file name and then by declaration order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Corpus {
    cases: Vec<Case>,
}

impl Corpus {
    /// Loads every case from the section files in the given directory.
    ///
    /// Files whose extension is not [`SECTION_EXTENSION`] are ignored, as are subdirectories.
    ///
    /// # Returns
    ///
    /// The loaded corpus on success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * [`Error::ReadDir`] if the directory cannot be listed.
    /// * [`Error::ReadEntry`] if one of the directory's entries cannot be inspected.
    /// * [`Error::ReadFile`] if a section file cannot be read.
    /// * [`Error::InvalidUtf8`] if a section file is not valid UTF-8.
    /// * [`Error::Parse`] if a section file is not the TOML a section is written in.
    /// * [`Error::EmptySection`] if a section file declares no case.
    /// * [`Error::EmptyCorpus`] if the directory holds no section file.
    /// * [`Error::DuplicateId`] if two cases share an identifier.
    /// * Forwards [`validate_case`]'s return values on failure.
    pub fn load_dir(dir: &Path) -> Result<Self, Error> {
        let mut section_paths = Vec::new();
        let entries = fs::read_dir(dir).map_err(|source| Error::ReadDir {
            dir: dir.to_path_buf(),
            source,
        })?;
        for entry in entries {
            let entry = entry.map_err(|source| Error::ReadEntry {
                dir: dir.to_path_buf(),
                source,
            })?;
            let path = entry.path();
            if path.is_file() && path.extension().is_some_and(|ext| ext == SECTION_EXTENSION) {
                section_paths.push(path);
            }
        }
        section_paths.sort();

        let mut cases = Vec::new();
        let mut id_origins: BTreeMap<String, PathBuf> = BTreeMap::new();
        for path in section_paths {
            for case in Self::load_section(&path)? {
                validate_case(&case, &path)?;
                if let Some(first_seen) = id_origins.insert(case.id.clone(), path.clone()) {
                    return Err(Error::DuplicateId {
                        id: case.id,
                        first_seen,
                        path,
                    });
                }
                cases.push(case);
            }
        }
        if cases.is_empty() {
            return Err(Error::EmptyCorpus {
                dir: dir.to_path_buf(),
            });
        }

        Ok(Self { cases })
    }

    #[must_use]
    pub fn cases(&self) -> &[Case] {
        &self.cases
    }

    /// Counts the cases carrying each tag.
    ///
    /// # Returns
    ///
    /// The number of cases per tag, with tags no case carries left out.
    #[must_use]
    pub fn tag_counts(&self) -> BTreeMap<Tag, usize> {
        let mut counts = BTreeMap::new();
        for case in &self.cases {
            for tag in &case.tags {
                *counts.entry(*tag).or_insert(0) += 1;
            }
        }

        counts
    }

    /// Selects the cases carrying the given tag.
    ///
    /// # Returns
    ///
    /// The matching cases, in corpus order.
    pub fn with_tag(&self, tag: Tag) -> impl Iterator<Item = &Case> + '_ {
        self.cases
            .iter()
            .filter(move |case| case.tags.contains(&tag))
    }

    /// Reads and parses one section file.
    ///
    /// # Returns
    ///
    /// The cases the section declares, in declaration order, on success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * [`Error::ReadFile`] if the file cannot be read.
    /// * [`Error::InvalidUtf8`] if the file is not valid UTF-8.
    /// * [`Error::Parse`] if the file is not the TOML a section is written in.
    /// * [`Error::EmptySection`] if the file declares no case.
    fn load_section(path: &Path) -> Result<Vec<Case>, Error> {
        let bytes = fs::read(path).map_err(|source| Error::ReadFile {
            path: path.to_path_buf(),
            source,
        })?;
        let text = String::from_utf8(bytes).map_err(|source| Error::InvalidUtf8 {
            path: path.to_path_buf(),
            source,
        })?;
        let section: Section = toml::from_str(&text).map_err(|source| Error::Parse {
            path: path.to_path_buf(),
            source,
        })?;
        if section.case.is_empty() {
            return Err(Error::EmptySection {
                path: path.to_path_buf(),
            });
        }

        Ok(section.case)
    }
}

/// The ways a corpus directory can fail to yield a usable set of cases.
///
/// Every variant names the file or directory the failure came from.
#[derive(Debug)]
pub enum Error {
    /// A corpus directory could not be listed.
    ReadDir {
        /// The directory that could not be listed.
        dir: PathBuf,

        /// The underlying failure.
        source: io::Error,
    },

    /// An entry of a corpus directory could not be inspected.
    ReadEntry {
        /// The directory being listed.
        dir: PathBuf,

        /// The underlying failure.
        source: io::Error,
    },

    /// A section file could not be read.
    ReadFile {
        /// The file that could not be read.
        path: PathBuf,

        /// The underlying failure.
        source: io::Error,
    },

    /// A section file's bytes are not valid UTF-8.
    InvalidUtf8 {
        /// The file holding the invalid bytes.
        path: PathBuf,

        /// The underlying failure.
        source: FromUtf8Error,
    },

    /// A section file is not the TOML a section is written in.
    Parse {
        /// The file that could not be parsed.
        path: PathBuf,

        /// The underlying failure.
        source: toml::de::Error,
    },

    /// A section file declares no case.
    EmptySection {
        /// The file declaring no case.
        path: PathBuf,
    },

    /// A corpus directory holds no case at all.
    EmptyCorpus {
        /// The directory holding no case.
        dir: PathBuf,
    },

    /// A case's identifier is empty.
    EmptyId {
        /// The file declaring the case.
        path: PathBuf,
    },

    /// Two cases share an identifier.
    DuplicateId {
        /// The shared identifier.
        id: String,

        /// The file declaring the case the identifier was first seen on.
        first_seen: PathBuf,

        /// The file declaring the case that repeats it.
        path: PathBuf,
    },

    /// A case's key sequence is empty, so replaying it would compare nothing.
    EmptyKeys {
        /// The file declaring the case.
        path: PathBuf,

        /// The case's identifier.
        id: String,
    },

    /// A case carries no tag, so no slice of the corpus would ever select it.
    NoTags {
        /// The file declaring the case.
        path: PathBuf,

        /// The case's identifier.
        id: String,
    },

    /// A case's viewport is zero cells wide.
    ZeroViewportWidth {
        /// The file declaring the case.
        path: PathBuf,

        /// The case's identifier.
        id: String,
    },

    /// A case's `'tabstop'` is zero, which vim rejects as well.
    ZeroTabstop {
        /// The file declaring the case.
        path: PathBuf,

        /// The case's identifier.
        id: String,
    },
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        match self {
            Self::ReadDir { dir, source } => {
                write!(
                    f,
                    "cannot list the corpus directory {}: {source}",
                    dir.display()
                )
            }
            Self::ReadEntry { dir, source } => write!(
                f,
                "cannot inspect an entry of the corpus directory {}: {source}",
                dir.display()
            ),
            Self::ReadFile { path, source } => {
                write!(f, "cannot read the section {}: {source}", path.display())
            }
            Self::InvalidUtf8 { path, source } => {
                write!(
                    f,
                    "the section {} is not valid UTF-8: {source}",
                    path.display()
                )
            }
            Self::Parse { path, source } => {
                write!(f, "cannot parse the section {}: {source}", path.display())
            }
            Self::EmptySection { path } => {
                write!(f, "the section {} declares no case", path.display())
            }
            Self::EmptyCorpus { dir } => {
                write!(f, "the corpus directory {} holds no case", dir.display())
            }
            Self::EmptyId { path } => {
                write!(
                    f,
                    "a case in the section {} has an empty id",
                    path.display()
                )
            }
            Self::DuplicateId {
                id,
                first_seen,
                path,
            } => write!(
                f,
                "the id `{id}` in the section {} is already used in {}",
                path.display(),
                first_seen.display()
            ),
            Self::EmptyKeys { path, id } => write!(
                f,
                "the case `{id}` in the section {} has an empty key sequence",
                path.display()
            ),
            Self::NoTags { path, id } => write!(
                f,
                "the case `{id}` in the section {} carries no tag",
                path.display()
            ),
            Self::ZeroViewportWidth { path, id } => write!(
                f,
                "the case `{id}` in the section {} has a zero-width viewport",
                path.display()
            ),
            Self::ZeroTabstop { path, id } => write!(
                f,
                "the case `{id}` in the section {} has a zero tabstop",
                path.display()
            ),
        }
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::ReadDir { source, .. }
            | Self::ReadEntry { source, .. }
            | Self::ReadFile { source, .. } => Some(source),
            Self::InvalidUtf8 { source, .. } => Some(source),
            Self::Parse { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// The directory the repository's own corpus is kept in.
///
/// The path is resolved from the crate's source location, so it only exists in a checkout of the
/// repository.
///
/// # Returns
///
/// The path of the repository's corpus directory.
#[must_use]
pub fn default_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("corpus")
}

/// One section file's contents.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Section {
    #[serde(default)]
    case: Vec<Case>,
}

/// Checks that a case is usable by the harness.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`Error::EmptyId`] if the case's identifier is empty.
/// * [`Error::EmptyKeys`] if the case's key sequence is empty.
/// * [`Error::NoTags`] if the case carries no tag.
/// * [`Error::ZeroViewportWidth`] if the case's viewport is zero cells wide.
/// * [`Error::ZeroTabstop`] if the case's `'tabstop'` is zero.
fn validate_case(case: &Case, path: &Path) -> Result<(), Error> {
    if case.id.is_empty() {
        return Err(Error::EmptyId {
            path: path.to_path_buf(),
        });
    }
    if case.keys.is_empty() {
        return Err(Error::EmptyKeys {
            path: path.to_path_buf(),
            id: case.id.clone(),
        });
    }
    if case.tags.is_empty() {
        return Err(Error::NoTags {
            path: path.to_path_buf(),
            id: case.id.clone(),
        });
    }
    if 0 == case.viewport_width {
        return Err(Error::ZeroViewportWidth {
            path: path.to_path_buf(),
            id: case.id.clone(),
        });
    }
    if 0 == case.options.tabstop {
        return Err(Error::ZeroTabstop {
            path: path.to_path_buf(),
            id: case.id.clone(),
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::env;
    use std::fs;
    use std::path::PathBuf;
    use std::process;
    use std::str;

    use super::{default_dir, AmbiWidth, Case, Corpus, Error, Options, Tag, SECTION_EXTENSION};

    /// The number of cases the repository's corpus holds.
    const TOTAL_CASE_COUNT: usize = 65;

    /// The number of cases carrying each tag, which is also what the pull request reports.
    const TAG_BREAKDOWN: [(Tag, usize); 14] = [
        (Tag::Ambiwidth, 2),
        (Tag::Ascii, 16),
        (Tag::Breakindent, 4),
        (Tag::Cjk, 14),
        (Tag::Code, 14),
        (Tag::Combining, 10),
        (Tag::Emoji, 13),
        (Tag::Flag, 3),
        (Tag::Nfd, 9),
        (Tag::Nowrap, 2),
        (Tag::Showbreak, 4),
        (Tag::Tab, 5),
        (Tag::Wrap, 17),
        (Tag::WordMotion, 30),
    ];

    /// The text shapes the word-motion grid crosses every motion with.
    const WORD_MOTION_SCENARIOS: [&str; 5] = [
        "cjk-latin",
        "zwj-family",
        "combining",
        "snake-case",
        "kebab-case",
    ];

    /// The word motions of the grid, each with the identifier prefix its cases are named by.
    const WORD_MOTIONS: [(&str, char); 6] = [
        ("word-w", 'w'),
        ("word-e", 'e'),
        ("word-b", 'b'),
        ("word-big-w", 'W'),
        ("word-big-e", 'E'),
        ("word-big-b", 'B'),
    ];

    /// A section holding one case that every field of which is valid.
    const VALID_SECTION: &str = r#"
[[case]]
id = "sample"
description = "A sample case."
buffer = "hello world\n"
keys = "dw"
viewport_width = 40
tags = ["ascii"]
"#;

    /// A corpus directory under the system's temporary directory, removed when the test ends.
    struct TempCorpus {
        dir: PathBuf,
    }

    impl TempCorpus {
        fn new(name: &str) -> Self {
            let dir = env::temp_dir().join(format!("vbc-corpus-{}-{name}", process::id()));
            fs::remove_dir_all(&dir).ok();
            fs::create_dir_all(&dir).expect("the temporary corpus directory must be creatable");

            Self { dir }
        }

        fn write(&self, file_name: &str, contents: &[u8]) {
            fs::write(self.dir.join(file_name), contents)
                .expect("the temporary section must be writable");
        }

        fn load(&self) -> Result<Corpus, Error> {
            Corpus::load_dir(&self.dir)
        }
    }

    impl Drop for TempCorpus {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.dir).ok();
        }
    }

    fn load_repository_corpus() -> Corpus {
        Corpus::load_dir(&default_dir()).expect("the repository's corpus must load")
    }

    #[test]
    fn repository_corpus_loads() {
        let corpus = load_repository_corpus();
        assert!(!corpus.cases().is_empty());
    }

    #[test]
    fn every_case_has_non_empty_keys_and_a_valid_utf8_buffer() {
        for case in load_repository_corpus().cases() {
            assert!(!case.keys.is_empty(), "case `{}` has no keys", case.id);
            assert_eq!(
                str::from_utf8(case.buffer.as_bytes()),
                Ok(case.buffer.as_str()),
                "case `{}` has a buffer that is not valid UTF-8",
                case.id
            );
            assert!(
                !case.buffer.contains(char::REPLACEMENT_CHARACTER),
                "case `{}` has a buffer holding a replacement character",
                case.id
            );
            assert!(
                !case.description.is_empty(),
                "case `{}` has no description",
                case.id
            );
            assert!(
                0 < case.viewport_width,
                "case `{}` has a zero-width viewport",
                case.id
            );
        }
    }

    #[test]
    fn every_section_file_is_valid_utf8() {
        let dir = default_dir();
        let entries = fs::read_dir(&dir).expect("the repository's corpus directory must be listed");
        let mut section_count = 0;
        for entry in entries {
            let path = entry.expect("the directory entry must be inspected").path();
            if !path.extension().is_some_and(|ext| ext == SECTION_EXTENSION) {
                continue;
            }
            let bytes = fs::read(&path).expect("the section must be readable");
            assert!(
                str::from_utf8(&bytes).is_ok(),
                "the section {} is not valid UTF-8",
                path.display()
            );
            section_count += 1;
        }
        assert!(0 < section_count);
    }

    #[test]
    fn no_two_cases_share_an_id() {
        let corpus = load_repository_corpus();
        let ids: BTreeSet<&str> = corpus.cases().iter().map(|case| case.id.as_str()).collect();
        assert_eq!(ids.len(), corpus.cases().len());
    }

    #[test]
    fn case_count_and_tag_breakdown_are_stable() {
        let corpus = load_repository_corpus();
        assert_eq!(corpus.cases().len(), TOTAL_CASE_COUNT);
        let expected: BTreeMap<Tag, usize> = TAG_BREAKDOWN.into_iter().collect();
        assert_eq!(corpus.tag_counts(), expected);
    }

    #[test]
    fn word_motion_grid_covers_every_motion_and_scenario() {
        let corpus = load_repository_corpus();
        let cases: BTreeMap<&str, &Case> = corpus
            .with_tag(Tag::WordMotion)
            .map(|case| (case.id.as_str(), case))
            .collect();
        assert_eq!(
            cases.len(),
            WORD_MOTIONS.len() * WORD_MOTION_SCENARIOS.len()
        );
        for scenario in WORD_MOTION_SCENARIOS {
            for (prefix, motion) in WORD_MOTIONS {
                let id = format!("{prefix}-{scenario}");
                let case = cases
                    .get(id.as_str())
                    .unwrap_or_else(|| panic!("the word-motion grid must hold the case `{id}`"));
                assert!(
                    case.keys.contains(motion),
                    "the case `{id}` does not replay `{motion}`"
                );
            }
        }
    }

    #[test]
    fn options_left_out_take_vim_defaults() {
        let temp = TempCorpus::new("defaults");
        temp.write("sample.toml", VALID_SECTION.as_bytes());
        let corpus = temp.load().expect("the section must load");
        assert_eq!(
            corpus.cases()[0].options,
            Options {
                wrap: true,
                breakindent: false,
                showbreak: String::new(),
                tabstop: 8,
                ambiwidth: AmbiWidth::Single,
            }
        );
    }

    #[test]
    fn a_case_round_trips_through_json() -> anyhow::Result<()> {
        let corpus = load_repository_corpus();
        for case in corpus.cases() {
            let encoded = serde_json::to_string(case)?;
            let decoded: Case = serde_json::from_str(&encoded)?;
            assert_eq!(&decoded, case);
        }

        Ok(())
    }

    #[test]
    fn non_section_files_are_ignored() {
        let temp = TempCorpus::new("ignored");
        temp.write("sample.toml", VALID_SECTION.as_bytes());
        temp.write("README.md", b"not a section");
        let corpus = temp.load().expect("the section must load");
        assert_eq!(corpus.cases().len(), 1);
    }

    #[test]
    fn a_directory_without_sections_is_rejected() {
        let temp = TempCorpus::new("no-sections");
        temp.write("README.md", b"not a section");
        let error = temp
            .load()
            .expect_err("a corpus without cases must be rejected");
        assert!(matches!(error, Error::EmptyCorpus { .. }), "{error:?}");
    }

    #[test]
    fn a_missing_directory_is_rejected() {
        let missing = env::temp_dir().join(format!("vbc-corpus-{}-missing", process::id()));
        let error =
            Corpus::load_dir(&missing).expect_err("a missing corpus directory must be rejected");
        assert!(matches!(error, Error::ReadDir { .. }), "{error:?}");
        assert!(error.to_string().contains("missing"), "{error}");
    }

    #[test]
    fn a_syntax_error_names_the_offending_file() {
        let temp = TempCorpus::new("syntax");
        temp.write("broken.toml", b"[[case]\nid = \"sample\"\n");
        let error = temp
            .load()
            .expect_err("a malformed section must be rejected");
        assert!(matches!(error, Error::Parse { .. }), "{error:?}");
        assert!(error.to_string().contains("broken.toml"), "{error}");
    }

    #[test]
    fn an_unknown_field_names_the_offending_file() {
        let temp = TempCorpus::new("unknown-field");
        let section = format!("{VALID_SECTION}colour = \"red\"\n");
        temp.write("broken.toml", section.as_bytes());
        let error = temp.load().expect_err("an unknown field must be rejected");
        assert!(matches!(error, Error::Parse { .. }), "{error:?}");
        assert!(error.to_string().contains("broken.toml"), "{error}");
    }

    #[test]
    fn an_unknown_tag_names_the_offending_file() {
        let temp = TempCorpus::new("unknown-tag");
        let section = VALID_SECTION.replace("\"ascii\"", "\"nonsense\"");
        temp.write("broken.toml", section.as_bytes());
        let error = temp.load().expect_err("an unknown tag must be rejected");
        assert!(matches!(error, Error::Parse { .. }), "{error:?}");
        assert!(error.to_string().contains("broken.toml"), "{error}");
    }

    #[test]
    fn a_missing_field_names_the_offending_file() {
        let temp = TempCorpus::new("missing-field");
        let section = VALID_SECTION.replace("keys = \"dw\"\n", "");
        temp.write("broken.toml", section.as_bytes());
        let error = temp.load().expect_err("a missing field must be rejected");
        assert!(matches!(error, Error::Parse { .. }), "{error:?}");
        assert!(error.to_string().contains("broken.toml"), "{error}");
    }

    #[test]
    fn invalid_utf8_names_the_offending_file() {
        let temp = TempCorpus::new("invalid-utf8");
        temp.write("broken.toml", b"id = \"\xff\xfe\"\n");
        let error = temp
            .load()
            .expect_err("a section that is not UTF-8 must be rejected");
        assert!(matches!(error, Error::InvalidUtf8 { .. }), "{error:?}");
        assert!(error.to_string().contains("broken.toml"), "{error}");
    }

    #[test]
    fn a_section_without_cases_is_rejected() {
        let temp = TempCorpus::new("empty-section");
        temp.write("broken.toml", b"# nothing here\n");
        let error = temp
            .load()
            .expect_err("a section without cases must be rejected");
        assert!(matches!(error, Error::EmptySection { .. }), "{error:?}");
        assert!(error.to_string().contains("broken.toml"), "{error}");
    }

    #[test]
    fn an_empty_key_sequence_is_rejected() {
        let temp = TempCorpus::new("empty-keys");
        let section = VALID_SECTION.replace("keys = \"dw\"", "keys = \"\"");
        temp.write("broken.toml", section.as_bytes());
        let error = temp
            .load()
            .expect_err("an empty key sequence must be rejected");
        assert!(matches!(error, Error::EmptyKeys { .. }), "{error:?}");
        assert!(error.to_string().contains("broken.toml"), "{error}");
    }

    #[test]
    fn an_untagged_case_is_rejected() {
        let temp = TempCorpus::new("no-tags");
        let section = VALID_SECTION.replace("tags = [\"ascii\"]", "tags = []");
        temp.write("broken.toml", section.as_bytes());
        let error = temp.load().expect_err("an untagged case must be rejected");
        assert!(matches!(error, Error::NoTags { .. }), "{error:?}");
        assert!(error.to_string().contains("broken.toml"), "{error}");
    }

    #[test]
    fn a_zero_width_viewport_is_rejected() {
        let temp = TempCorpus::new("zero-width");
        let section = VALID_SECTION.replace("viewport_width = 40", "viewport_width = 0");
        temp.write("broken.toml", section.as_bytes());
        let error = temp
            .load()
            .expect_err("a zero-width viewport must be rejected");
        assert!(
            matches!(error, Error::ZeroViewportWidth { .. }),
            "{error:?}"
        );
        assert!(error.to_string().contains("broken.toml"), "{error}");
    }

    #[test]
    fn a_zero_tabstop_is_rejected() {
        let temp = TempCorpus::new("zero-tabstop");
        let section = format!("{VALID_SECTION}options = {{ tabstop = 0 }}\n");
        temp.write("broken.toml", section.as_bytes());
        let error = temp.load().expect_err("a zero tabstop must be rejected");
        assert!(matches!(error, Error::ZeroTabstop { .. }), "{error:?}");
        assert!(error.to_string().contains("broken.toml"), "{error}");
    }

    #[test]
    fn an_empty_id_is_rejected() {
        let temp = TempCorpus::new("empty-id");
        let section = VALID_SECTION.replace("id = \"sample\"", "id = \"\"");
        temp.write("broken.toml", section.as_bytes());
        let error = temp.load().expect_err("an empty id must be rejected");
        assert!(matches!(error, Error::EmptyId { .. }), "{error:?}");
        assert!(error.to_string().contains("broken.toml"), "{error}");
    }

    #[test]
    fn a_duplicate_id_names_both_files() {
        let temp = TempCorpus::new("duplicate-id");
        temp.write("first.toml", VALID_SECTION.as_bytes());
        temp.write("second.toml", VALID_SECTION.as_bytes());
        let error = temp.load().expect_err("a repeated id must be rejected");
        assert!(matches!(error, Error::DuplicateId { .. }), "{error:?}");
        let message = error.to_string();
        assert!(message.contains("first.toml"), "{message}");
        assert!(message.contains("second.toml"), "{message}");
    }
}
