//! The engine and Python versions a bundle asks for, and the versions a
//! server runs.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

use super::super::ProtocolError;
use super::package::number;

/// A QuantForge engine version, `MAJOR.MINOR.PATCH` without leading zeros,
/// such as `0.3.0`: what a server runs bundles on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct EngineVersion {
    major: u32,
    minor: u32,
    patch: u32,
}

impl EngineVersion {
    /// The version `major.minor.patch`.
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }
}

impl FromStr for EngineVersion {
    type Err = ProtocolError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match numbers(text, 3).as_deref() {
            Some(&[major, minor, patch]) => Ok(Self::new(major, minor, patch)),
            _ => Err(ProtocolError::InvalidEngineVersion {
                input: text.to_string(),
            }),
        }
    }
}

impl TryFrom<String> for EngineVersion {
    type Error = ProtocolError;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        text.parse()
    }
}

impl From<EngineVersion> for String {
    fn from(version: EngineVersion) -> Self {
        version.to_string()
    }
}

impl fmt::Display for EngineVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// A Python version, `MAJOR.MINOR` without leading zeros, such as `3.12`.
/// A bundle names the one it was written for, and runs only where the server
/// runs that version.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct PythonVersion {
    major: u32,
    minor: u32,
}

impl PythonVersion {
    /// The version `major.minor`.
    pub const fn new(major: u32, minor: u32) -> Self {
        Self { major, minor }
    }
}

impl FromStr for PythonVersion {
    type Err = ProtocolError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match numbers(text, 2).as_deref() {
            Some(&[major, minor]) => Ok(Self::new(major, minor)),
            _ => Err(ProtocolError::InvalidPythonVersion {
                input: text.to_string(),
            }),
        }
    }
}

impl TryFrom<String> for PythonVersion {
    type Error = ProtocolError;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        text.parse()
    }
}

impl From<PythonVersion> for String {
    fn from(version: PythonVersion) -> Self {
        version.to_string()
    }
}

impl fmt::Display for PythonVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

/// Exactly `count` numbers without leading zeros, separated by `.`.
fn numbers(text: &str, count: usize) -> Option<Vec<u32>> {
    let parts: Vec<&str> = text.split('.').collect();
    if parts.len() != count {
        return None;
    }
    parts
        .into_iter()
        .map(|part| {
            let canonical = number(part.as_bytes(), 0) == Some(part.len());
            canonical.then(|| part.parse().ok()).flatten()
        })
        .collect()
}

/// The engine versions a bundle runs on: comparators separated by commas,
/// all of which a version must meet, such as `>=0.3.0, <0.4.0`. The
/// operators are `>=`, `>`, `<=`, `<`, and `==`, each followed by a
/// `MAJOR.MINOR.PATCH` version. Spaces around a comparator are allowed.
///
/// ```
/// use quantforge::protocol::bundle::{EngineRequirement, EngineVersion};
///
/// let requirement: EngineRequirement = ">= 0.3.0,<0.4.0".parse()?;
/// assert_eq!(requirement.to_string(), ">=0.3.0, <0.4.0");
/// assert!(requirement.matches(EngineVersion::new(0, 3, 7)));
/// assert!(!requirement.matches(EngineVersion::new(0, 4, 0)));
/// # Ok::<(), quantforge::protocol::ProtocolError>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EngineRequirement {
    comparators: Vec<(Operator, EngineVersion)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Operator {
    AtLeast,
    Above,
    AtMost,
    Below,
    Exactly,
}

impl Operator {
    /// Longest first, so that `>=` is not read as `>`.
    const ALL: [(Operator, &'static str); 5] = [
        (Self::AtLeast, ">="),
        (Self::AtMost, "<="),
        (Self::Exactly, "=="),
        (Self::Above, ">"),
        (Self::Below, "<"),
    ];

    fn symbol(self) -> &'static str {
        match self {
            Self::AtLeast => ">=",
            Self::Above => ">",
            Self::AtMost => "<=",
            Self::Below => "<",
            Self::Exactly => "==",
        }
    }

    fn admits(self, version: EngineVersion, bound: EngineVersion) -> bool {
        match self {
            Self::AtLeast => version >= bound,
            Self::Above => version > bound,
            Self::AtMost => version <= bound,
            Self::Below => version < bound,
            Self::Exactly => version == bound,
        }
    }
}

impl EngineRequirement {
    /// Whether `version` meets every comparator.
    pub fn matches(&self, version: EngineVersion) -> bool {
        self.comparators
            .iter()
            .all(|&(operator, bound)| operator.admits(version, bound))
    }
}

impl FromStr for EngineRequirement {
    type Err = ProtocolError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let refuse = |reason: String| ProtocolError::InvalidEngineRequirement {
            input: text.to_string(),
            reason,
        };
        if text.trim_matches(' ').is_empty() {
            return Err(refuse("it is empty".to_string()));
        }
        let mut comparators = Vec::new();
        for part in text.split(',') {
            let comparator = part.trim_matches(' ');
            if comparator.is_empty() {
                return Err(refuse("a comma leaves an empty comparator".to_string()));
            }
            let Some((operator, rest)) = Operator::ALL.iter().find_map(|&(operator, symbol)| {
                comparator
                    .strip_prefix(symbol)
                    .map(|rest| (operator, rest.trim_start_matches(' ')))
            }) else {
                return Err(refuse(format!(
                    "{comparator:?} starts with no operator; use >=, >, <=, <, or =="
                )));
            };
            let bound = rest
                .parse()
                .map_err(|_| refuse(format!("{rest:?} is not a MAJOR.MINOR.PATCH version")))?;
            comparators.push((operator, bound));
        }
        Ok(Self { comparators })
    }
}

impl fmt::Display for EngineRequirement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, (operator, bound)) in self.comparators.iter().enumerate() {
            let separator = if index == 0 { "" } else { ", " };
            write!(f, "{separator}{}{bound}", operator.symbol())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_are_numbers_without_leading_zeros() {
        assert_eq!("0.3.0".parse(), Ok(EngineVersion::new(0, 3, 0)));
        assert_eq!("10.20.30".parse(), Ok(EngineVersion::new(10, 20, 30)));
        for text in [
            "",
            "0.3",
            "0.3.0.1",
            "00.3.0",
            "0.03.0",
            "v0.3.0",
            "0.3.0-rc.1",
            " 0.3.0",
            "0.3.x",
            "4294967296.0.0",
        ] {
            assert_eq!(
                text.parse::<EngineVersion>(),
                Err(ProtocolError::InvalidEngineVersion {
                    input: text.to_string()
                }),
                "{text:?}"
            );
        }
        assert_eq!("3.12".parse(), Ok(PythonVersion::new(3, 12)));
        for text in ["3", "3.12.1", "03.12", "3.012", "py3.12", "3.12 ", ""] {
            assert!(text.parse::<PythonVersion>().is_err(), "{text:?}");
        }
    }

    #[test]
    fn versions_are_json_strings() {
        let engine: EngineVersion = serde_json::from_str(r#""0.3.0""#).expect("version");
        assert_eq!(
            serde_json::to_string(&engine).expect("serialize"),
            r#""0.3.0""#
        );
        let python: PythonVersion = serde_json::from_str(r#""3.12""#).expect("version");
        assert_eq!(
            serde_json::to_string(&python).expect("serialize"),
            r#""3.12""#
        );
        assert!(serde_json::from_str::<PythonVersion>("3.12").is_err());
    }

    #[test]
    fn a_requirement_holds_when_every_comparator_does() {
        let requirement: EngineRequirement = ">=0.3.0, <0.4.0".parse().expect("requirement");
        let admitted: Vec<String> = [(0, 2, 9), (0, 3, 0), (0, 3, 12), (0, 4, 0), (1, 0, 0)]
            .into_iter()
            .map(|(major, minor, patch)| EngineVersion::new(major, minor, patch))
            .filter(|&version| requirement.matches(version))
            .map(|version| version.to_string())
            .collect();
        assert_eq!(admitted, ["0.3.0", "0.3.12"]);
        for (text, version, holds) in [
            (">0.3.0", (0, 3, 0), false),
            (">0.3.0", (0, 3, 1), true),
            ("<=0.3.0", (0, 3, 0), true),
            ("<0.3.0", (0, 3, 0), false),
            ("==0.3.1", (0, 3, 1), true),
            ("==0.3.1", (0, 3, 2), false),
            (">=0.4.0, <0.3.0", (0, 3, 5), false),
        ] {
            let requirement: EngineRequirement = text.parse().expect("requirement");
            let (major, minor, patch) = version;
            assert_eq!(
                requirement.matches(EngineVersion::new(major, minor, patch)),
                holds,
                "{text} with {version:?}"
            );
        }
    }

    #[test]
    fn a_requirement_that_does_not_parse_says_why() {
        for (text, reason) in [
            ("", "it is empty"),
            ("  ", "it is empty"),
            (">=0.3.0,", "a comma leaves an empty comparator"),
            (">=0.3.0,,<0.4.0", "a comma leaves an empty comparator"),
            (
                "0.3.0",
                "\"0.3.0\" starts with no operator; use >=, >, <=, <, or ==",
            ),
            (
                "=0.3.0",
                "\"=0.3.0\" starts with no operator; use >=, >, <=, <, or ==",
            ),
            (
                "^0.3.0",
                "\"^0.3.0\" starts with no operator; use >=, >, <=, <, or ==",
            ),
            (
                "~=0.3",
                "\"~=0.3\" starts with no operator; use >=, >, <=, <, or ==",
            ),
            (">=0.3", "\"0.3\" is not a MAJOR.MINOR.PATCH version"),
            (
                ">=0.3.0 <0.4.0",
                "\"0.3.0 <0.4.0\" is not a MAJOR.MINOR.PATCH version",
            ),
            (">=*", "\"*\" is not a MAJOR.MINOR.PATCH version"),
        ] {
            let error = text.parse::<EngineRequirement>().expect_err("refused");
            assert_eq!(
                error.to_string(),
                format!(
                    "invalid engine requirement {text:?}: {reason}; write comparators on \
                     MAJOR.MINOR.PATCH versions, such as \">=0.3.0, <0.4.0\""
                )
            );
        }
    }
}
