//! The differential runner: the corpus replayed against two engines, case by case.
//!
//! A run replays every case against both engines and compares every dimension of the state they
//! end in -- buffer, cursor, display position, mode and registers. Each case is reported as
//! agreeing, as diverging in named dimensions, or as one engine failing to replay it at all. A
//! report serializes to JSON for a continuous-integration job, and renders a per-case diff for a
//! human debugging a single failure.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error as StdError;
use std::fmt::{Display, Formatter, Result as FmtResult};

use serde::{Deserialize, Serialize};

use crate::corpus::{Case, Corpus, Tag};
use crate::state::{describe_register, Divergence, EditorState};
use crate::vim::{Error as VimError, VimDriver};

/// An engine a differential run replays cases against.
pub trait Engine {
    /// The reason a case could not be replayed.
    type Error: StdError;

    /// # Returns
    ///
    /// The name the engine is reported under, which tells the two sides of a run apart.
    fn name(&self) -> &str;

    /// Replays a case's keys against its starting buffer.
    ///
    /// # Parameters
    ///
    /// * `case` - The case to replay.
    ///
    /// # Returns
    ///
    /// The state the engine ends in on success.
    ///
    /// # Errors
    ///
    /// Returns an error if the engine could not replay the case.
    fn replay(&self, case: &Case) -> Result<EditorState, Self::Error>;
}

/// The driver replays a case's starting buffer and keys in the case's viewport and under its
/// display options, so a case whose outcome depends on them -- one moving by screen line, for
/// instance -- is replayed against the layout the case describes.
impl Engine for VimDriver {
    type Error = VimError;

    fn name(&self) -> &str {
        "vim"
    }

    fn replay(&self, case: &Case) -> Result<EditorState, Self::Error> {
        self.run_case(case)
    }
}

/// One of the dimensions two engines are compared in.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Dimension {
    /// The buffer's text.
    Buffer,

    /// The cursor's position.
    Cursor,

    /// Where the cursor is drawn in the viewport.
    DisplayPosition,

    /// The mode the engine is in.
    Mode,

    /// The text and type held by a register.
    Register,
}

impl Dimension {
    /// # Returns
    ///
    /// The dimensions the divergences fall in, without repetition and in the order the dimensions
    /// are declared in.
    #[must_use]
    pub fn of(divergences: &[Divergence]) -> Vec<Self> {
        divergences
            .iter()
            .map(Self::from)
            .collect::<BTreeSet<Self>>()
            .into_iter()
            .collect()
    }
}

impl Display for Dimension {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        let name = match self {
            Self::Buffer => "buffer",
            Self::Cursor => "cursor",
            Self::DisplayPosition => "display position",
            Self::Mode => "mode",
            Self::Register => "register",
        };

        formatter.write_str(name)
    }
}

impl From<&Divergence> for Dimension {
    fn from(divergence: &Divergence) -> Self {
        match divergence {
            Divergence::Buffer { .. } => Self::Buffer,
            Divergence::Cursor { .. } => Self::Cursor,
            Divergence::DisplayPosition { .. } => Self::DisplayPosition,
            Divergence::Mode { .. } => Self::Mode,
            Divergence::Register { .. } => Self::Register,
        }
    }
}

/// What replaying one case against both engines found.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "status")]
pub enum Outcome {
    /// Both engines ended in the same state.
    Agreed,

    /// The engines ended in different states.
    Diverged {
        /// The dimensions the two states disagree in, without repetition.
        dimensions: Vec<Dimension>,

        /// Every disagreement, with both engines' values.
        divergences: Vec<Divergence>,
    },

    /// One of the engines could not replay the case, so the two were never compared.
    Failed {
        /// The name of the engine that could not replay the case.
        engine: String,

        /// What that engine reported.
        message: String,
    },
}

impl Outcome {
    /// # Returns
    ///
    /// Whether the two engines were compared and agreed.
    #[must_use]
    pub fn agreed(&self) -> bool {
        matches!(self, Self::Agreed)
    }
}

/// One case's place in a run's report.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CaseReport {
    /// The identifier of the case that was replayed.
    pub id: String,

    /// The keys that were replayed, in vim's notation.
    pub keys: String,

    /// The tags the case carries, by which a run is sliced.
    pub tags: BTreeSet<Tag>,

    /// What the run found.
    #[serde(flatten)]
    pub outcome: Outcome,
}

/// How many cases of one slice of a run ended in each outcome.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Counts {
    /// The number of cases replayed.
    pub total: usize,

    /// The number of cases both engines agreed on.
    pub agreed: usize,

    /// The number of cases the engines disagreed on.
    pub diverged: usize,

    /// The number of cases an engine could not replay.
    pub failed: usize,
}

impl Counts {
    /// Adds one case's outcome to the counts.
    fn add(&mut self, outcome: &Outcome) {
        self.total += 1;
        match outcome {
            Outcome::Agreed => self.agreed += 1,
            Outcome::Diverged { .. } => self.diverged += 1,
            Outcome::Failed { .. } => self.failed += 1,
        }
    }
}

/// The counts a continuous-integration job checks a run by.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Summary {
    /// The counts over every case replayed.
    pub overall: Counts,

    /// The counts over the cases carrying each tag, with tags no replayed case carries left out.
    pub tags: BTreeMap<Tag, Counts>,
}

impl Display for Summary {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        writeln!(
            formatter,
            "{} cases: {} agreed, {} diverged, {} failed",
            self.overall.total, self.overall.agreed, self.overall.diverged, self.overall.failed
        )?;
        for (tag, counts) in &self.tags {
            writeln!(
                formatter,
                "  {:<14} {} cases, {} agreed, {} diverged, {} failed",
                format!("{tag:?}:"),
                counts.total,
                counts.agreed,
                counts.diverged,
                counts.failed
            )?;
        }

        Ok(())
    }
}

/// Everything one differential run found.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Report {
    /// The name of the engine taken as the reference, the left side of every comparison.
    pub reference: String,

    /// The name of the engine under test, the right side of every comparison.
    pub subject: String,

    /// One entry per case replayed, in the order the cases were given in.
    pub cases: Vec<CaseReport>,

    /// The counts over the whole run.
    pub summary: Summary,
}

impl Report {
    /// # Returns
    ///
    /// Whether every case was compared and agreed on.
    #[must_use]
    pub fn all_agreed(&self) -> bool {
        0 == self.summary.overall.diverged && 0 == self.summary.overall.failed
    }

    /// Serializes the run for a machine to read.
    ///
    /// # Returns
    ///
    /// The report as JSON on success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// * Forwards [`serde_json::to_string_pretty`]'s return values on failure.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Renders one case for a human debugging it, naming the case and every dimension it diverged
    /// in.
    ///
    /// # Returns
    ///
    /// The rendered case, or `None` if the run replayed no case with that identifier.
    #[must_use]
    pub fn render_case(&self, id: &str) -> Option<String> {
        self.cases
            .iter()
            .find(|case| case.id == id)
            .map(|case| self.diff(case).to_string())
    }

    /// # Returns
    ///
    /// A renderer for one of the run's cases.
    fn diff<'report_lifetime>(
        &'report_lifetime self,
        case: &'report_lifetime CaseReport,
    ) -> CaseDiff<'report_lifetime> {
        CaseDiff {
            case,
            reference: &self.reference,
            subject: &self.subject,
        }
    }
}

impl Display for Report {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        writeln!(
            formatter,
            "differential run: `{}` (reference) against `{}` (subject)",
            self.reference, self.subject
        )?;
        for case in &self.cases {
            match &case.outcome {
                Outcome::Agreed => writeln!(formatter, "pass {}", case.id)?,
                Outcome::Diverged { dimensions, .. } => writeln!(
                    formatter,
                    "FAIL {}: diverged in {}",
                    case.id,
                    join(dimensions)
                )?,
                Outcome::Failed { engine, .. } => {
                    writeln!(
                        formatter,
                        "FAIL {}: `{engine}` could not replay it",
                        case.id
                    )?;
                }
            }
        }
        for case in self.cases.iter().filter(|case| !case.outcome.agreed()) {
            writeln!(formatter)?;
            write!(formatter, "{}", self.diff(case))?;
        }
        writeln!(formatter)?;

        write!(formatter, "{}", self.summary)
    }
}

/// Replays every case of a corpus against both engines.
///
/// # Type Parameters
///
/// * `ReferenceEngineType` - The engine taken as the reference, the left side of every comparison.
/// * `SubjectEngineType` - The engine under test, the right side of every comparison.
///
/// # Returns
///
/// What the run found, with one entry per case in corpus order.
pub fn run_corpus<ReferenceEngineType: Engine, SubjectEngineType: Engine>(
    corpus: &Corpus,
    reference: &ReferenceEngineType,
    subject: &SubjectEngineType,
) -> Report {
    run_cases(corpus.cases(), reference, subject)
}

/// Replays the given cases against both engines.
///
/// # Type Parameters
///
/// * `ReferenceEngineType` - The engine taken as the reference, the left side of every comparison.
/// * `SubjectEngineType` - The engine under test, the right side of every comparison.
///
/// # Returns
///
/// What the run found, with one entry per case in the order the cases were given in.
pub fn run_cases<'case_lifetime, ReferenceEngineType: Engine, SubjectEngineType: Engine>(
    cases: impl IntoIterator<Item = &'case_lifetime Case>,
    reference: &ReferenceEngineType,
    subject: &SubjectEngineType,
) -> Report {
    let cases: Vec<CaseReport> = cases
        .into_iter()
        .map(|case| run_case(case, reference, subject))
        .collect();
    let summary = summarize(&cases);

    Report {
        reference: reference.name().to_owned(),
        subject: subject.name().to_owned(),
        cases,
        summary,
    }
}

/// Replays one case against both engines.
///
/// # Type Parameters
///
/// * `ReferenceEngineType` - The engine taken as the reference, the left side of every comparison.
/// * `SubjectEngineType` - The engine under test, the right side of every comparison.
///
/// # Returns
///
/// What the run found for that case.
pub fn run_case<ReferenceEngineType: Engine, SubjectEngineType: Engine>(
    case: &Case,
    reference: &ReferenceEngineType,
    subject: &SubjectEngineType,
) -> CaseReport {
    CaseReport {
        id: case.id.clone(),
        keys: case.keys.clone(),
        tags: case.tags.clone(),
        outcome: compare(case, reference, subject),
    }
}

/// One case's outcome rendered for a human.
struct CaseDiff<'report_lifetime> {
    case: &'report_lifetime CaseReport,
    reference: &'report_lifetime str,
    subject: &'report_lifetime str,
}

impl Display for CaseDiff<'_> {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        let reference = self.reference;
        let subject = self.subject;
        let width = reference.len().max(subject.len());
        match &self.case.outcome {
            Outcome::Agreed => writeln!(formatter, "case `{}` agreed", self.case.id)?,
            Outcome::Diverged { dimensions, .. } => writeln!(
                formatter,
                "case `{}` diverged in {}",
                self.case.id,
                join(dimensions)
            )?,
            Outcome::Failed { engine, .. } => writeln!(
                formatter,
                "case `{}` was never compared: `{engine}` could not replay it",
                self.case.id
            )?,
        }
        writeln!(formatter, "  keys: `{}`", self.case.keys)?;

        match &self.case.outcome {
            Outcome::Agreed => Ok(()),
            Outcome::Failed { message, .. } => writeln!(formatter, "  error: {message}"),
            Outcome::Diverged { divergences, .. } => {
                for divergence in divergences {
                    match divergence {
                        Divergence::Buffer { left, right } => {
                            writeln!(formatter, "  buffer:")?;
                            writeln!(formatter, "    {reference:<width$} : {left:?}")?;
                            writeln!(formatter, "    {subject:<width$} : {right:?}")?;
                            writeln!(
                                formatter,
                                "    they first differ at byte {}",
                                first_difference(left, right)
                            )?;
                        }
                        Divergence::Cursor { left, right } => {
                            writeln!(formatter, "  cursor:")?;
                            writeln!(
                                formatter,
                                "    {reference:<width$} : line {}, column {}",
                                left.line, left.column
                            )?;
                            writeln!(
                                formatter,
                                "    {subject:<width$} : line {}, column {}",
                                right.line, right.column
                            )?;
                        }
                        Divergence::DisplayPosition { left, right } => {
                            writeln!(formatter, "  display position:")?;
                            writeln!(
                                formatter,
                                "    {reference:<width$} : screen row {}, screen column {}",
                                left.row, left.column
                            )?;
                            writeln!(
                                formatter,
                                "    {subject:<width$} : screen row {}, screen column {}",
                                right.row, right.column
                            )?;
                        }
                        Divergence::Mode { left, right } => {
                            writeln!(formatter, "  mode:")?;
                            writeln!(formatter, "    {reference:<width$} : {left:?}")?;
                            writeln!(formatter, "    {subject:<width$} : {right:?}")?;
                        }
                        Divergence::Register { name, left, right } => {
                            writeln!(formatter, "  register `{name}`:")?;
                            writeln!(
                                formatter,
                                "    {reference:<width$} : {}",
                                describe_register(left.as_ref())
                            )?;
                            writeln!(
                                formatter,
                                "    {subject:<width$} : {}",
                                describe_register(right.as_ref())
                            )?;
                        }
                    }
                }

                Ok(())
            }
        }
    }
}

/// Replays one case against both engines and compares the states they end in.
///
/// # Type Parameters
///
/// * `ReferenceEngineType` - The engine taken as the reference, the left side of every comparison.
/// * `SubjectEngineType` - The engine under test, the right side of every comparison.
///
/// # Returns
///
/// The case's outcome, which reports the engine that could not replay the case when either side
/// failed to.
fn compare<ReferenceEngineType: Engine, SubjectEngineType: Engine>(
    case: &Case,
    reference: &ReferenceEngineType,
    subject: &SubjectEngineType,
) -> Outcome {
    let reference_state = match reference.replay(case) {
        Ok(state) => state,
        Err(error) => {
            return Outcome::Failed {
                engine: reference.name().to_owned(),
                message: error.to_string(),
            }
        }
    };
    let subject_state = match subject.replay(case) {
        Ok(state) => state,
        Err(error) => {
            return Outcome::Failed {
                engine: subject.name().to_owned(),
                message: error.to_string(),
            }
        }
    };

    let divergences = reference_state.diff(&subject_state);
    if divergences.is_empty() {
        return Outcome::Agreed;
    }

    Outcome::Diverged {
        dimensions: Dimension::of(&divergences),
        divergences,
    }
}

/// Counts the cases' outcomes, overall and per tag.
///
/// # Returns
///
/// The run's summary.
fn summarize(cases: &[CaseReport]) -> Summary {
    let mut summary = Summary::default();
    for case in cases {
        summary.overall.add(&case.outcome);
        for tag in &case.tags {
            summary.tags.entry(*tag).or_default().add(&case.outcome);
        }
    }

    summary
}

/// # Returns
///
/// The dimensions' names, separated by commas.
fn join(dimensions: &[Dimension]) -> String {
    dimensions
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<String>>()
        .join(", ")
}

/// # Returns
///
/// The offset of the first byte at which the two texts differ, which is the length of the shorter
/// one when it is a prefix of the other.
fn first_difference(left: &str, right: &str) -> usize {
    left.bytes()
        .zip(right.bytes())
        .position(|(left_byte, right_byte)| left_byte != right_byte)
        .unwrap_or_else(|| left.len().min(right.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_difference_of_a_prefix_is_its_length() {
        assert_eq!(first_difference("alpha", "alpha beta"), 5);
        assert_eq!(first_difference("alpha beta", "alpha"), 5);
        assert_eq!(first_difference("alpha", "alpha"), 5);
    }

    #[test]
    fn the_first_difference_is_reported_in_bytes() {
        assert_eq!(first_difference("", "a"), 0);
        assert_eq!(first_difference("alpha", "alpea"), 3);
        assert_eq!(first_difference("\u{4e2d}\u{6587}", "\u{4e2d}\u{6570}"), 4);
    }
}
