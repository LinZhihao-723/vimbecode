//! Command-line entry point replaying the differential corpus against both engines.
//!
//! The vimbecode editor cannot replay a case yet, so the subject side of a run is held by a second
//! vim. A run therefore exercises the whole corpus end to end, and reports every case in which the
//! two engines disagree.

use std::env::args;
use std::path::PathBuf;
use std::process::ExitCode;

use vbc_oracle::corpus::{self, Case, Corpus, Tag};
use vbc_oracle::runner::{self, Engine, Report};
use vbc_oracle::state::EditorState;
use vbc_oracle::vim::{Error as VimError, VimDriver};

/// What the entry point prints when it is asked for help, or when it is given something it cannot
/// understand.
const USAGE: &str = "\
Usage: differential-run [OPTIONS]

Replays the differential corpus against both engines and reports every case in which they
disagree, naming the dimensions they disagree in.

Options:
  --corpus <DIR>  The corpus directory to replay, defaulting to the repository's own corpus.
  --tag <TAG>     Replay only the cases carrying this tag, for example `word-motion`.
  --case <ID>     Replay only the case with this identifier.
  --format <FMT>  `text`, the default, or `json`.
  --help          Print this message.

The exit status is zero exactly when every replayed case was compared and both engines agreed.";

/// How a run is printed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Format {
    /// The run is printed as JSON, for a continuous-integration job to read.
    Json,

    /// The run is printed for a human, with a diff of every case that failed.
    Text,
}

/// What the entry point was asked to do.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Arguments {
    corpus: PathBuf,
    tag: Option<Tag>,
    case: Option<String>,
    format: Format,
}

impl Arguments {
    /// Parses the command line.
    ///
    /// # Returns
    ///
    /// What the entry point was asked to do, or `None` if it was asked for help.
    ///
    /// # Errors
    ///
    /// Returns an error naming the option that could not be understood, the option whose value is
    /// missing, or the value that is not one the option takes.
    fn parse(arguments: impl IntoIterator<Item = String>) -> Result<Option<Self>, String> {
        let mut parsed = Self {
            corpus: corpus::default_dir(),
            tag: None,
            case: None,
            format: Format::Text,
        };
        let mut arguments = arguments.into_iter();
        while let Some(argument) = arguments.next() {
            let mut value = || {
                arguments
                    .next()
                    .ok_or_else(|| format!("The option `{argument}` takes a value"))
            };
            match argument.as_str() {
                "--help" | "-h" => return Ok(None),
                "--corpus" => parsed.corpus = PathBuf::from(value()?),
                "--tag" => parsed.tag = Some(parse_tag(&value()?)?),
                "--case" => parsed.case = Some(value()?),
                "--format" => {
                    parsed.format = match value()?.as_str() {
                        "json" => Format::Json,
                        "text" => Format::Text,
                        other => return Err(format!("`{other}` is not a format")),
                    };
                }
                other => return Err(format!("`{other}` is not an option")),
            }
        }

        Ok(Some(parsed))
    }

    /// # Returns
    ///
    /// Whether the case is one of those the entry point was asked to replay.
    fn selects(&self, case: &Case) -> bool {
        self.tag.is_none_or(|tag| case.tags.contains(&tag))
            && self.case.as_ref().is_none_or(|id| *id == case.id)
    }
}

/// The engine slot the vimbecode editor will fill, held by a second vim until the editor can
/// replay a case. A run against it therefore exercises the corpus and the runner, not the editor.
struct Placeholder {
    driver: VimDriver,
}

impl Engine for Placeholder {
    type Error = VimError;

    fn name(&self) -> &str {
        "vimbecode-placeholder"
    }

    fn replay(&self, case: &Case) -> Result<EditorState, Self::Error> {
        self.driver.replay(case)
    }
}

/// # Returns
///
/// [`ExitCode::SUCCESS`] if every replayed case was compared and both engines agreed, and
/// [`ExitCode::FAILURE`] otherwise.
fn main() -> ExitCode {
    let arguments = match Arguments::parse(args().skip(1)) {
        Ok(Some(arguments)) => arguments,
        Ok(None) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(message) => {
            eprintln!("{message}.");
            eprintln!("{USAGE}");
            return ExitCode::FAILURE;
        }
    };

    match run(&arguments) {
        Ok(exit_code) => exit_code,
        Err(message) => {
            eprintln!("{message}.");
            ExitCode::FAILURE
        }
    }
}

/// Loads the corpus, replays the selected cases against both engines, and prints the run.
///
/// # Returns
///
/// [`ExitCode::SUCCESS`] if every replayed case was compared and both engines agreed, and
/// [`ExitCode::FAILURE`] otherwise.
///
/// # Errors
///
/// Returns an error saying why the corpus could not be loaded, why vim could not be used, why the
/// selection matched no case, or why the run could not be printed.
fn run(arguments: &Arguments) -> Result<ExitCode, String> {
    let corpus = Corpus::load_dir(&arguments.corpus)
        .map_err(|error| format!("The corpus could not be loaded: {error}"))?;
    let cases: Vec<&Case> = corpus
        .cases()
        .iter()
        .filter(|case| arguments.selects(case))
        .collect();
    if cases.is_empty() {
        return Err("No case matched the selection".to_owned());
    }

    let reference = VimDriver::new().map_err(|error| format!("Vim is unusable: {error}"))?;
    let subject = Placeholder {
        driver: VimDriver::new().map_err(|error| format!("Vim is unusable: {error}"))?,
    };
    let report = runner::run_cases(cases, &reference, &subject);
    print(&report, arguments.format)?;

    Ok(if report.all_agreed() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// Prints a run in the requested format.
///
/// # Errors
///
/// Returns an error if the run could not be serialized as JSON.
fn print(report: &Report, format: Format) -> Result<(), String> {
    match format {
        Format::Text => println!("{report}"),
        Format::Json => {
            let json = report
                .to_json()
                .map_err(|error| format!("The report could not be serialized: {error}"))?;
            println!("{json}");
        }
    }

    Ok(())
}

/// # Returns
///
/// The tag the name stands for on success.
///
/// # Errors
///
/// Returns an error naming every tag a case may carry if the name is not one of them.
fn parse_tag(name: &str) -> Result<Tag, String> {
    serde_json::from_str(&format!("{name:?}"))
        .map_err(|error| format!("`{name}` is not a tag a case may carry: {error}"))
}

#[cfg(test)]
mod tests {
    use vbc_oracle::corpus::Options;

    use super::*;

    /// # Returns
    ///
    /// What the entry point makes of a command line it understands on success.
    ///
    /// # Errors
    ///
    /// Returns an error carrying the diagnostic the entry point rejected the command line with.
    fn accept(arguments: &[&str]) -> anyhow::Result<Option<Arguments>> {
        Arguments::parse(arguments.iter().map(|argument| (*argument).to_owned()))
            .map_err(anyhow::Error::msg)
    }

    /// # Returns
    ///
    /// The diagnostic the entry point rejects the command line with.
    ///
    /// # Panics
    ///
    /// Panics if the entry point understood the command line.
    fn reject(arguments: &[&str]) -> String {
        Arguments::parse(arguments.iter().map(|argument| (*argument).to_owned()))
            .expect_err("the command line cannot be understood")
    }

    /// # Returns
    ///
    /// A case the selection tests filter.
    fn case(id: &str, tags: &[Tag]) -> Case {
        Case {
            id: id.to_owned(),
            description: "A case the tests build.".to_owned(),
            buffer: "alpha beta\n".to_owned(),
            keys: "dw".to_owned(),
            viewport_width: 40,
            viewport_height: corpus::DEFAULT_VIEWPORT_HEIGHT,
            tags: tags.iter().copied().collect(),
            options: Options::default(),
        }
    }

    #[test]
    fn an_empty_command_line_replays_the_whole_repository_corpus_as_text() -> anyhow::Result<()> {
        let parsed = accept(&[])?.expect("an empty command line is not a request for help");

        assert_eq!(
            parsed,
            Arguments {
                corpus: corpus::default_dir(),
                tag: None,
                case: None,
                format: Format::Text,
            }
        );

        Ok(())
    }

    #[test]
    fn every_option_is_understood() -> anyhow::Result<()> {
        let parsed = accept(&[
            "--corpus",
            "/somewhere/else",
            "--tag",
            "word-motion",
            "--case",
            "word-w-cjk-latin",
            "--format",
            "json",
        ])?
        .expect("a full command line is not a request for help");

        assert_eq!(
            parsed,
            Arguments {
                corpus: PathBuf::from("/somewhere/else"),
                tag: Some(Tag::WordMotion),
                case: Some("word-w-cjk-latin".to_owned()),
                format: Format::Json,
            }
        );
        assert_eq!(
            accept(&["--format", "text"])?.map(|parsed| parsed.format),
            Some(Format::Text)
        );

        Ok(())
    }

    #[test]
    fn asking_for_help_asks_for_nothing_else() -> anyhow::Result<()> {
        assert_eq!(accept(&["--help"])?, None);
        assert_eq!(accept(&["-h"])?, None);
        assert_eq!(accept(&["--format", "json", "--help"])?, None);

        Ok(())
    }

    #[test]
    fn a_command_line_that_cannot_be_understood_is_rejected() {
        for arguments in [
            ["--nonsense"].as_slice(),
            ["--format", "yaml"].as_slice(),
            ["--tag", "not-a-tag"].as_slice(),
            ["--format"].as_slice(),
            ["--tag"].as_slice(),
            ["--case"].as_slice(),
            ["--corpus"].as_slice(),
        ] {
            assert!(
                !reject(arguments).is_empty(),
                "{arguments:?} was rejected silently"
            );
        }
    }

    #[test]
    fn a_rejection_names_what_it_could_not_understand() {
        assert!(reject(&["--nonsense"]).contains("--nonsense"));
        assert!(reject(&["--format", "yaml"]).contains("yaml"));
        assert!(reject(&["--tag", "not-a-tag"]).contains("not-a-tag"));
        assert!(reject(&["--format"]).contains("--format"));
    }

    #[test]
    fn a_selection_narrows_the_corpus_to_what_it_names() -> anyhow::Result<()> {
        let ascii_word_motion = case("ascii-word-motion", &[Tag::Ascii, Tag::WordMotion]);
        let cjk = case("cjk", &[Tag::Cjk]);

        let everything = accept(&[])?.expect("an empty command line selects every case");
        assert!(everything.selects(&ascii_word_motion));
        assert!(everything.selects(&cjk));

        let by_tag = accept(&["--tag", "cjk"])?.expect("a tag selects the cases carrying it");
        assert!(!by_tag.selects(&ascii_word_motion));
        assert!(by_tag.selects(&cjk));

        let by_case = accept(&["--case", "cjk"])?.expect("an identifier selects one case");
        assert!(!by_case.selects(&ascii_word_motion));
        assert!(by_case.selects(&cjk));

        let by_both = accept(&["--tag", "ascii", "--case", "cjk"])?
            .expect("a tag and an identifier select the cases matching both");
        assert!(!by_both.selects(&ascii_word_motion));
        assert!(!by_both.selects(&cjk));

        Ok(())
    }
}
