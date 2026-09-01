//! Command-line entry point replaying the differential corpus against both engines.
//!
//! The vimbecode editor cannot replay a case yet, so the subject side of a run is held by a second
//! vim. A run therefore exercises the whole corpus end to end, and reports every case in which the
//! two engines disagree.
//!
//! The same entry point records and checks the corpus baseline, which guards the reference side of
//! a run: what a run compares says nothing about vim itself ending a case somewhere else than it
//! used to.

use std::env::args;
use std::path::PathBuf;
use std::process::ExitCode;

use vbc_oracle::baseline::{self, Baseline, Check};
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

Given `--record-baseline` or `--check-baseline` it works on the corpus baseline instead: the state
vim ends every case in, recorded once and checked afterwards, which is what guards the reference
side of a run against silent drift.

Options:
  --corpus <DIR>        The corpus directory to replay, defaulting to the repository's own corpus.
  --tag <TAG>           Replay only the cases carrying this tag, for example `word-motion`.
  --case <ID>           Replay only the case with this identifier.
  --format <FMT>        `text`, the default, or `json`.
  --baseline <FILE>     The baseline to record or check, defaulting to the repository's own.
  --record-baseline     Capture the state vim ends every corpus case in, and write the baseline.
  --check-baseline      Report every case that no longer ends where the baseline records it.
  --strict-vim-version  Fail a check that runs against another vim than the baseline was recorded
                        from, instead of reporting that the recorded states were not compared.
  --help                Print this message.

The exit status is zero exactly when every replayed case was compared and both engines agreed, or,
for a baseline check, when the baseline still holds.";

/// How a run is printed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Format {
    /// The run is printed as JSON, for a continuous-integration job to read.
    Json,

    /// The run is printed for a human, with a diff of every case that failed.
    Text,
}

/// What the entry point works on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Mode {
    /// The corpus is replayed against both engines.
    Compare,

    /// The state vim ends every corpus case in is captured and written as the baseline.
    RecordBaseline,

    /// The corpus is replayed against vim and compared with the baseline.
    CheckBaseline,
}

/// What the entry point was asked to do.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Arguments {
    corpus: PathBuf,
    baseline: PathBuf,
    mode: Mode,
    tag: Option<Tag>,
    case: Option<String>,
    format: Format,
    strict_vim_version: bool,
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
    /// missing, the value that is not one the option takes, or the options that contradict each
    /// other.
    fn parse(arguments: impl IntoIterator<Item = String>) -> Result<Option<Self>, String> {
        let mut parsed = Self {
            corpus: corpus::default_dir(),
            baseline: baseline::default_path(),
            mode: Mode::Compare,
            tag: None,
            case: None,
            format: Format::Text,
            strict_vim_version: false,
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
                "--baseline" => parsed.baseline = PathBuf::from(value()?),
                "--record-baseline" => parsed.set_mode(Mode::RecordBaseline)?,
                "--check-baseline" => parsed.set_mode(Mode::CheckBaseline)?,
                "--strict-vim-version" => parsed.strict_vim_version = true,
                other => return Err(format!("`{other}` is not an option")),
            }
        }
        if Mode::Compare != parsed.mode && (parsed.tag.is_some() || parsed.case.is_some()) {
            return Err(
                "`--tag` and `--case` replay part of the corpus, and a baseline covers all of it"
                    .to_owned(),
            );
        }
        if Mode::CheckBaseline != parsed.mode && parsed.strict_vim_version {
            return Err("`--strict-vim-version` applies to `--check-baseline`".to_owned());
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

    /// Puts the entry point in the given mode.
    ///
    /// # Errors
    ///
    /// Returns an error if the entry point was already put in another mode.
    fn set_mode(&mut self, mode: Mode) -> Result<(), String> {
        if Mode::Compare != self.mode && mode != self.mode {
            return Err(
                "`--record-baseline` and `--check-baseline` cannot both be given".to_owned(),
            );
        }
        self.mode = mode;

        Ok(())
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

/// Loads the corpus and does what the entry point was asked to do with it.
///
/// # Returns
///
/// [`ExitCode::SUCCESS`] if the run or the check the entry point was asked for passed, and
/// [`ExitCode::FAILURE`] otherwise.
///
/// # Errors
///
/// Returns an error saying why the corpus could not be loaded, or forwards what the requested work
/// failed with.
fn run(arguments: &Arguments) -> Result<ExitCode, String> {
    let corpus = Corpus::load_dir(&arguments.corpus)
        .map_err(|error| format!("The corpus could not be loaded: {error}"))?;

    match arguments.mode {
        Mode::Compare => compare(arguments, &corpus),
        Mode::RecordBaseline => record_baseline(arguments, &corpus),
        Mode::CheckBaseline => check_baseline(arguments, &corpus),
    }
}

/// Replays the selected cases against both engines, and prints the run.
///
/// # Returns
///
/// [`ExitCode::SUCCESS`] if every replayed case was compared and both engines agreed, and
/// [`ExitCode::FAILURE`] otherwise.
///
/// # Errors
///
/// Returns an error saying why vim could not be used, why the selection matched no case, or why
/// the run could not be printed.
fn compare(arguments: &Arguments, corpus: &Corpus) -> Result<ExitCode, String> {
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

/// Captures the state vim ends every corpus case in, and writes it as the baseline.
///
/// # Returns
///
/// [`ExitCode::SUCCESS`], since a baseline that could not be recorded is reported as an error.
///
/// # Errors
///
/// Returns an error saying why vim could not be used, why a case could not be replayed, or why the
/// baseline could not be written.
fn record_baseline(arguments: &Arguments, corpus: &Corpus) -> Result<ExitCode, String> {
    let reference = VimDriver::new().map_err(|error| format!("Vim is unusable: {error}"))?;
    let baseline = Baseline::record(corpus, &reference)
        .map_err(|error| format!("The baseline could not be recorded: {error}"))?;
    baseline
        .write(&arguments.baseline)
        .map_err(|error| format!("The baseline could not be written: {error}"))?;
    println!(
        "recorded {} cases from vim {} into {}",
        baseline.cases.len(),
        baseline.header.vim_version,
        arguments.baseline.display()
    );

    Ok(ExitCode::SUCCESS)
}

/// Replays the corpus against vim and reports every case that no longer ends where the baseline
/// records it.
///
/// # Returns
///
/// [`ExitCode::SUCCESS`] if the baseline still holds, or if the recorded states were not compared
/// and the command line allows that, and [`ExitCode::FAILURE`] otherwise.
///
/// # Errors
///
/// Returns an error saying why the baseline could not be read, why vim could not be used, why the
/// baseline could not be checked at all, why the check could not be printed, or that the check ran
/// against another vim than the baseline was recorded from.
fn check_baseline(arguments: &Arguments, corpus: &Corpus) -> Result<ExitCode, String> {
    let baseline = Baseline::read(&arguments.baseline)
        .map_err(|error| format!("The baseline could not be read: {error}"))?;
    let reference = VimDriver::new().map_err(|error| format!("Vim is unusable: {error}"))?;
    let check = baseline
        .check(corpus, &reference)
        .map_err(|error| format!("The baseline could not be checked: {error}"))?;
    match arguments.format {
        Format::Text => print!("{check}"),
        Format::Json => {
            let json = serde_json::to_string_pretty(&check)
                .map_err(|error| format!("The check could not be serialized: {error}"))?;
            println!("{json}");
        }
    }

    if let Check::Skipped { recorded, running } = &check {
        if arguments.strict_vim_version {
            return Err(format!(
                "The baseline was recorded from vim {recorded} and this check ran against vim \
                 {running}, which `--strict-vim-version` forbids: continuous integration pins vim \
                 {}.{} and records the baseline there, so the recorded states can be checked \
                 nowhere else",
                recorded.major, recorded.minor
            ));
        }
        eprintln!(
            "warning: the recorded states were not compared; pass `--strict-vim-version` to make \
             that a failure."
        );
    }

    Ok(if check.drifted() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
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
                baseline: baseline::default_path(),
                mode: Mode::Compare,
                tag: None,
                case: None,
                format: Format::Text,
                strict_vim_version: false,
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
                baseline: baseline::default_path(),
                mode: Mode::Compare,
                tag: Some(Tag::WordMotion),
                case: Some("word-w-cjk-latin".to_owned()),
                format: Format::Json,
                strict_vim_version: false,
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
    fn the_baseline_options_are_understood() -> anyhow::Result<()> {
        let recording = accept(&["--record-baseline", "--baseline", "/somewhere/else.json"])?
            .expect("a request to record a baseline is not a request for help");
        assert_eq!(recording.mode, Mode::RecordBaseline);
        assert_eq!(recording.baseline, PathBuf::from("/somewhere/else.json"));
        assert!(!recording.strict_vim_version);

        let checking = accept(&["--check-baseline", "--strict-vim-version"])?
            .expect("a request to check a baseline is not a request for help");
        assert_eq!(checking.mode, Mode::CheckBaseline);
        assert_eq!(checking.baseline, baseline::default_path());
        assert!(checking.strict_vim_version);

        Ok(())
    }

    #[test]
    fn a_command_line_that_contradicts_itself_is_rejected() {
        assert!(reject(&["--record-baseline", "--check-baseline"]).contains("--check-baseline"));
        assert!(reject(&["--check-baseline", "--tag", "ascii"]).contains("--tag"));
        assert!(reject(&["--record-baseline", "--case", "cjk"]).contains("--case"));
        assert!(reject(&["--strict-vim-version"]).contains("--check-baseline"));
        assert!(
            reject(&["--record-baseline", "--strict-vim-version"]).contains("--strict-vim-version")
        );
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
