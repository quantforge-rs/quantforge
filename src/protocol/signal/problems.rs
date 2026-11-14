//! Why a body is not a signal: each problem has a code, which every
//! implementation of the format shares, the field it is about where it is
//! about one, and a message for the person fixing the alert.

use std::fmt;

use super::{SIGNAL_FIELDS, SIGNAL_VERSION};

/// What kind of problem a body has. The codes are part of the format: every
/// implementation reports the same codes for the same body, and the shared
/// test cases compare them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum SignalProblemCode {
    /// A body over [`MAX_SIGNAL_BYTES`](super::MAX_SIGNAL_BYTES), whatever
    /// it holds.
    TooLarge,
    /// A body that is not one JSON value in UTF-8, that nests deeper than
    /// [`MAX_SIGNAL_DEPTH`](super::MAX_SIGNAL_DEPTH), or that holds a
    /// number no double holds.
    NotJson,
    /// JSON that is not an object: a list of signals is not a signal.
    NotAnObject,
    /// A member written more than once.
    DuplicateField,
    /// A member the format does not define.
    UnknownField,
    /// An entry's field in an exit.
    MisplacedField,
    /// A field its kind of signal needs, and that is not there.
    MissingField,
    /// A value of the wrong JSON type: a price as a number, a stop as
    /// `null`, a version as text.
    WrongType,
    /// A version this format is not.
    UnsupportedVersion,
    /// A value of the right type that breaks its field's rule.
    InvalidValue,
    /// A bar time after the fire time.
    BarAfterFire,
}

impl SignalProblemCode {
    /// Every code, in the order the format lists them.
    pub const ALL: [Self; 11] = [
        Self::TooLarge,
        Self::NotJson,
        Self::NotAnObject,
        Self::DuplicateField,
        Self::UnknownField,
        Self::MisplacedField,
        Self::MissingField,
        Self::WrongType,
        Self::UnsupportedVersion,
        Self::InvalidValue,
        Self::BarAfterFire,
    ];

    /// The code as the format and the shared test cases write it, such as
    /// `unknown_field`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TooLarge => "too_large",
            Self::NotJson => "not_json",
            Self::NotAnObject => "not_an_object",
            Self::DuplicateField => "duplicate_field",
            Self::UnknownField => "unknown_field",
            Self::MisplacedField => "misplaced_field",
            Self::MissingField => "missing_field",
            Self::WrongType => "wrong_type",
            Self::UnsupportedVersion => "unsupported_version",
            Self::InvalidValue => "invalid_value",
            Self::BarAfterFire => "bar_after_fire",
        }
    }
}

impl fmt::Display for SignalProblemCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One thing wrong with a body: its code, the field it is about, and a
/// one-line message that says what to change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignalProblem {
    code: SignalProblemCode,
    field: Option<String>,
    message: String,
}

impl SignalProblem {
    /// A problem with the body as a whole.
    pub(super) fn body(code: SignalProblemCode, message: impl Into<String>) -> Self {
        Self {
            code,
            field: None,
            message: message.into(),
        }
    }

    /// A problem with the member named `field`.
    pub(super) fn field(code: SignalProblemCode, field: &str, message: impl Into<String>) -> Self {
        Self {
            code,
            field: Some(field.to_string()),
            message: message.into(),
        }
    }

    /// What kind of problem it is.
    pub fn code(&self) -> SignalProblemCode {
        self.code
    }

    /// The member the problem is about, by its name in the body; `None`
    /// for a problem with the body as a whole.
    pub fn field_name(&self) -> Option<&str> {
        self.field.as_deref()
    }

    /// What is wrong, and how to fix it.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Where the problem sorts: the body first, then the format's fields
    /// in their order, then every other member by its name; and within one
    /// member, by code.
    fn rank(&self) -> (usize, &str, SignalProblemCode) {
        let known = |name: &str| SIGNAL_FIELDS.iter().position(|field| *field == name);
        match self.field.as_deref() {
            None => (0, "", self.code),
            Some(name) => match known(name) {
                Some(index) => (index + 1, "", self.code),
                None => (SIGNAL_FIELDS.len() + 1, name, self.code),
            },
        }
    }
}

impl fmt::Display for SignalProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

/// Why a body is not a signal: every problem found, never an empty list.
/// They come in one order on every implementation: a problem with the body
/// as a whole, then the format's fields in the format's order, then every
/// other member in the order of their names.
///
/// Its text is one line, fit for the `message` of an
/// [`ErrorBody`](super::super::ErrorBody).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignalProblems(Vec<SignalProblem>);

impl SignalProblems {
    /// The problems in the format's order, each once.
    pub(super) fn sorted(mut problems: Vec<SignalProblem>) -> Self {
        problems.sort_by(|left, right| left.rank().cmp(&right.rank()));
        problems.dedup_by(|later, earlier| later.rank() == earlier.rank());
        Self(problems)
    }

    /// Every problem found.
    pub fn problems(&self) -> &[SignalProblem] {
        &self.0
    }
}

impl fmt::Display for SignalProblems {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the body is not a version {SIGNAL_VERSION} signal: ")?;
        for (index, problem) in self.0.iter().enumerate() {
            if index > 0 {
                f.write_str("; ")?;
            }
            write!(f, "{problem}")?;
        }
        Ok(())
    }
}

impl std::error::Error for SignalProblems {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn every_code_has_one_snake_case_name() {
        let names: BTreeSet<&str> = SignalProblemCode::ALL
            .iter()
            .map(|code| code.as_str())
            .collect();
        assert_eq!(names.len(), SignalProblemCode::ALL.len());
        for name in names {
            assert!(
                name.bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte == b'_'),
                "{name}"
            );
        }
        // The derived order is the listed one, which problems sort by.
        let mut sorted = SignalProblemCode::ALL;
        sorted.sort();
        assert_eq!(sorted, SignalProblemCode::ALL);
    }

    #[test]
    fn problems_sort_by_the_body_then_the_formats_fields_then_other_names() {
        let problem = |code, field: Option<&str>| match field {
            Some(field) => SignalProblem::field(code, field, "m"),
            None => SignalProblem::body(code, "m"),
        };
        let found = SignalProblems::sorted(vec![
            problem(SignalProblemCode::UnknownField, Some("zeta")),
            problem(SignalProblemCode::InvalidValue, Some("stop")),
            problem(SignalProblemCode::UnknownField, Some("Alpha")),
            problem(SignalProblemCode::MissingField, Some("version")),
            problem(SignalProblemCode::InvalidValue, Some("side")),
            problem(SignalProblemCode::InvalidValue, Some("side")),
            problem(SignalProblemCode::BarAfterFire, Some("bar_time")),
            problem(SignalProblemCode::InvalidValue, Some("bar_time")),
            problem(SignalProblemCode::NotJson, None),
        ]);
        let order: Vec<(&str, Option<&str>)> = found
            .problems()
            .iter()
            .map(|problem| (problem.code().as_str(), problem.field_name()))
            .collect();
        assert_eq!(
            order,
            [
                ("not_json", None),
                ("missing_field", Some("version")),
                ("invalid_value", Some("side")),
                ("invalid_value", Some("bar_time")),
                ("bar_after_fire", Some("bar_time")),
                ("invalid_value", Some("stop")),
                ("unknown_field", Some("Alpha")),
                ("unknown_field", Some("zeta")),
            ]
        );
    }

    #[test]
    fn the_problems_read_as_one_line() {
        let found = SignalProblems::sorted(vec![
            SignalProblem::field(SignalProblemCode::UnknownField, "qty", "\"qty\" is unknown"),
            SignalProblem::field(
                SignalProblemCode::MissingField,
                "side",
                "\"side\" is missing",
            ),
        ]);
        assert_eq!(
            found.to_string(),
            "the body is not a version 1 signal: \"side\" is missing; \"qty\" is unknown"
        );
    }
}
