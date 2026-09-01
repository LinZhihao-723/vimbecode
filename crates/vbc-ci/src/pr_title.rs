//! Validation of pull request titles against the repository's Conventional Commits convention.
//!
//! A conforming title reads `type: Subject sentence.`, which the repository's squash merges turn
//! directly into a commit subject.

use std::error::Error as StdError;
use std::fmt::{Display, Formatter, Result as FmtResult};

/// The maximum number of characters a title may contain.
pub const MAX_TITLE_LEN: usize = 71;

/// The commit types a title may be prefixed with, sorted alphabetically.
pub const ALLOWED_TYPES: [&str; 9] = [
    "build", "chore", "ci", "docs", "feat", "fix", "perf", "refactor", "test",
];

/// The ways in which a pull request title can violate the convention.
#[derive(Debug, Eq, PartialEq)]
pub enum Error {
    /// The title is longer than [`MAX_TITLE_LEN`] characters, holding the actual length.
    TooLong(usize),

    /// The title has no `type:` prefix.
    MissingType,

    /// The prefix is not one of [`ALLOWED_TYPES`], holding the prefix found.
    UnknownType(String),

    /// The colon after the type is not followed by a space.
    MissingSpaceAfterType,

    /// Nothing follows the `type:` prefix.
    EmptySubject,

    /// The subject starts with a lowercase letter.
    UncapitalizedSubject,

    /// The subject does not end with a period.
    MissingPeriod,
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        match self {
            Self::TooLong(len) => write!(
                f,
                "title is {len} characters long, but must be at most {MAX_TITLE_LEN}"
            ),
            Self::MissingType => write!(
                f,
                "title has no `type:` prefix; expected one of {}",
                ALLOWED_TYPES.join(", ")
            ),
            Self::UnknownType(found) => write!(
                f,
                "`{found}` is not a valid type; expected one of {}",
                ALLOWED_TYPES.join(", ")
            ),
            Self::MissingSpaceAfterType => {
                write!(f, "the colon after the type must be followed by a space")
            }
            Self::EmptySubject => write!(f, "the subject is empty"),
            Self::UncapitalizedSubject => write!(f, "the subject must start with a capital letter"),
            Self::MissingPeriod => write!(f, "the subject must end with a period"),
        }
    }
}

impl StdError for Error {}

/// Validates a pull request title against the repository's Conventional Commits convention.
///
/// # Errors
///
/// Returns an error if:
///
/// * [`Error::TooLong`] if the title exceeds [`MAX_TITLE_LEN`] characters.
/// * [`Error::MissingType`] if the title has no `type:` prefix.
/// * [`Error::UnknownType`] if the prefix is not one of [`ALLOWED_TYPES`].
/// * [`Error::MissingSpaceAfterType`] if no space separates the colon from the subject.
/// * [`Error::EmptySubject`] if the subject is empty.
/// * [`Error::UncapitalizedSubject`] if the subject starts with a lowercase letter.
/// * [`Error::MissingPeriod`] if the subject does not end with a period.
pub fn validate(title: &str) -> Result<(), Error> {
    let len = title.chars().count();
    if MAX_TITLE_LEN < len {
        return Err(Error::TooLong(len));
    }

    let Some((commit_type, remainder)) = title.split_once(':') else {
        return Err(Error::MissingType);
    };
    if !ALLOWED_TYPES.contains(&commit_type) {
        return Err(Error::UnknownType(commit_type.to_owned()));
    }

    let Some(subject) = remainder.strip_prefix(' ') else {
        return Err(Error::MissingSpaceAfterType);
    };
    let Some(first_char) = subject.chars().next() else {
        return Err(Error::EmptySubject);
    };
    if first_char.is_lowercase() {
        return Err(Error::UncapitalizedSubject);
    }
    if !subject.ends_with('.') {
        return Err(Error::MissingPeriod);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{validate, Error, ALLOWED_TYPES, MAX_TITLE_LEN};

    /// The number of characters a title spends on everything but the subject's own words.
    const OVERHEAD_LEN: usize = "feat: .".len();

    #[test]
    fn every_allowed_type_passes() {
        for commit_type in ALLOWED_TYPES {
            let title = format!("{commit_type}: Set up the Cargo workspace.");
            assert_eq!(validate(&title), Ok(()));
        }
    }

    #[test]
    fn title_at_length_limit_passes() {
        let title = format!("feat: {}.", "A".repeat(MAX_TITLE_LEN - OVERHEAD_LEN));
        assert_eq!(title.chars().count(), MAX_TITLE_LEN);
        assert_eq!(validate(&title), Ok(()));
    }

    #[test]
    fn over_length_title_is_rejected() {
        let title = format!("feat: {}.", "A".repeat(MAX_TITLE_LEN - OVERHEAD_LEN + 1));
        assert_eq!(validate(&title), Err(Error::TooLong(MAX_TITLE_LEN + 1)));
    }

    #[test]
    fn missing_type_is_rejected() {
        assert_eq!(
            validate("Set up the Cargo workspace."),
            Err(Error::MissingType)
        );
    }

    #[test]
    fn unknown_type_is_rejected() {
        assert_eq!(
            validate("feature: Set up the Cargo workspace."),
            Err(Error::UnknownType("feature".to_owned()))
        );
    }

    #[test]
    fn missing_space_after_type_is_rejected() {
        assert_eq!(
            validate("build:Set up the Cargo workspace."),
            Err(Error::MissingSpaceAfterType)
        );
    }

    #[test]
    fn empty_subject_is_rejected() {
        assert_eq!(validate("build: "), Err(Error::EmptySubject));
    }

    #[test]
    fn lowercase_subject_is_rejected() {
        assert_eq!(
            validate("build: set up the Cargo workspace."),
            Err(Error::UncapitalizedSubject)
        );
    }

    #[test]
    fn missing_period_is_rejected() {
        assert_eq!(
            validate("build: Set up the Cargo workspace"),
            Err(Error::MissingPeriod)
        );
    }
}
