//! Python packages, as a manifest pins them and a server offers them.

use std::fmt;

use serde::{Deserialize, Serialize};

use super::super::ProtocolError;

/// A Python package's name in its normalized form (PEP 503): lowercase ASCII
/// letters and digits, with single `-` between them, such as `numpy` or
/// `scikit-learn`.
///
/// Other spellings of a valid name, such as `PyYAML` or `typing_extensions`,
/// are refused with the normalized form to write instead, so that a name has
/// one spelling to compare.
///
/// ```
/// use quantforge::protocol::bundle::PackageName;
///
/// assert_eq!(PackageName::new("scikit-learn")?.as_str(), "scikit-learn");
/// let refused = PackageName::new("PyYAML").expect_err("not normalized");
/// assert!(refused.to_string().contains("write it as \"pyyaml\""));
/// # Ok::<(), quantforge::protocol::ProtocolError>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String")]
pub struct PackageName(String);

impl PackageName {
    /// Checks a name.
    pub fn new(name: impl Into<String>) -> Result<Self, ProtocolError> {
        let name = name.into();
        if is_normalized(&name) {
            return Ok(Self(name));
        }
        let reason = if is_valid(&name) {
            format!(
                "names are compared in their normalized form, so write it as {:?}",
                normalize(&name)
            )
        } else {
            "expected a package name, such as \"numpy\" or \"scikit-learn\"".to_string()
        };
        Err(ProtocolError::InvalidPackageName {
            input: name,
            reason,
        })
    }

    /// The name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for PackageName {
    type Error = ProtocolError;

    fn try_from(name: String) -> Result<Self, Self::Error> {
        Self::new(name)
    }
}

impl fmt::Display for PackageName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A name as PEP 508 allows it: ASCII letters, digits, `.`, `_`, and `-`,
/// starting and ending with a letter or digit.
fn is_valid(name: &str) -> bool {
    let bytes = name.as_bytes();
    let edge = |byte: Option<&u8>| byte.is_some_and(u8::is_ascii_alphanumeric);
    edge(bytes.first())
        && edge(bytes.last())
        && bytes
            .iter()
            .all(|&byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

/// The PEP 503 form: lowercase, with each run of `.`, `_`, and `-` as one
/// `-`.
fn normalize(name: &str) -> String {
    let mut normalized = String::with_capacity(name.len());
    for c in name.chars() {
        if matches!(c, '.' | '_' | '-') {
            if !normalized.ends_with('-') {
                normalized.push('-');
            }
        } else {
            normalized.push(c.to_ascii_lowercase());
        }
    }
    normalized
}

fn is_normalized(name: &str) -> bool {
    is_valid(name)
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && !name.contains("--")
}

/// A package version in its canonical form (PEP 440): release numbers
/// without leading zeros, then optionally a pre-release (`a`, `b`, or `rc`
/// and a number), a post-release (`.post` and a number), and a development
/// release (`.dev` and a number), such as `2.1.3`, `1.0rc1`, or
/// `2.0.post1`. Epochs and local versions are refused.
///
/// A pin is compared as text, not by version order, so `1.0` and `1.0.0`
/// are different pins.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String")]
pub struct PackageVersion(String);

impl PackageVersion {
    /// Checks a version.
    pub fn new(version: impl Into<String>) -> Result<Self, ProtocolError> {
        let version = version.into();
        if is_canonical_version(version.as_bytes()) {
            Ok(Self(version))
        } else {
            Err(ProtocolError::InvalidPackageVersion { input: version })
        }
    }

    /// The version.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for PackageVersion {
    type Error = ProtocolError;

    fn try_from(version: String) -> Result<Self, Self::Error> {
        Self::new(version)
    }
}

impl fmt::Display for PackageVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// `0` or a digit string without a leading zero, starting at `at`: where it
/// ends.
pub(super) fn number(bytes: &[u8], at: usize) -> Option<usize> {
    match bytes.get(at)? {
        b'0' => Some(at + 1),
        b'1'..=b'9' => {
            let digits = bytes[at..]
                .iter()
                .take_while(|byte| byte.is_ascii_digit())
                .count();
            Some(at + digits)
        }
        _ => None,
    }
}

fn is_canonical_version(bytes: &[u8]) -> bool {
    let Some(mut at) = number(bytes, 0) else {
        return false;
    };
    while bytes.get(at) == Some(&b'.') && bytes.get(at + 1).is_some_and(u8::is_ascii_digit) {
        match number(bytes, at + 1) {
            Some(end) => at = end,
            None => return false,
        }
    }
    let pre_release = [&b"rc"[..], b"a", b"b"]
        .into_iter()
        .find(|prefix| bytes[at..].starts_with(prefix));
    if let Some(prefix) = pre_release {
        match number(bytes, at + prefix.len()) {
            Some(end) => at = end,
            None => return false,
        }
    }
    for suffix in [&b".post"[..], b".dev"] {
        if bytes[at..].starts_with(suffix) {
            match number(bytes, at + suffix.len()) {
                Some(end) => at = end,
                None => return false,
            }
        }
    }
    at == bytes.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_normalized_name_is_accepted_and_any_other_spelling_names_the_normalized_one() {
        for name in [
            "numpy",
            "scikit-learn",
            "py3",
            "a",
            "zope-interface",
            "x1-2",
        ] {
            assert_eq!(PackageName::new(name).expect("normalized").as_str(), name);
        }
        for (name, normalized) in [
            ("PyYAML", "pyyaml"),
            ("typing_extensions", "typing-extensions"),
            ("zope.interface", "zope-interface"),
            ("A--B", "a-b"),
            ("a._-b", "a-b"),
        ] {
            let error = PackageName::new(name).expect_err("not normalized");
            assert_eq!(
                error.to_string(),
                format!(
                    "invalid package name {name:?}: names are compared in their normalized form, \
                     so write it as {normalized:?}"
                )
            );
        }
        for name in ["", "-numpy", "numpy-", "num py", "nümpy", "numpy==1", ".x"] {
            let error = PackageName::new(name).expect_err("not a name");
            assert!(
                error
                    .to_string()
                    .ends_with("expected a package name, such as \"numpy\" or \"scikit-learn\""),
                "{name:?}: {error}"
            );
        }
    }

    #[test]
    fn only_canonical_versions_are_pins() {
        for version in [
            "0",
            "1",
            "2.1.3",
            "10.0.0",
            "1.0rc1",
            "1.0a0",
            "1.0b2",
            "2.0.post1",
            "1.0.dev3",
            "1.0rc1.post2.dev3",
            "2024.10.1",
            "0.0.0",
        ] {
            assert_eq!(
                PackageVersion::new(version).expect("canonical").as_str(),
                version
            );
        }
        for version in [
            "",
            "v1.0",
            "01",
            "1.01",
            "1.0.",
            ".1",
            "1..0",
            "1.0-rc1",
            "1.0RC1",
            "1.0c1",
            "1.0rc",
            "1.0.rc1",
            "1.0post1",
            "1.0-1",
            "1!2.0",
            "1.0+local",
            "1.0 ",
            " 1.0",
            "==1.0",
            "1.0rc1a1",
            "1.0.dev1.post1",
            "1.0a01",
            "1.0.post01",
        ] {
            let error = PackageVersion::new(version).expect_err("not canonical");
            assert!(
                error
                    .to_string()
                    .starts_with(&format!("invalid package version {version:?}")),
                "{error}"
            );
        }
    }

    #[test]
    fn names_and_versions_are_json_strings() {
        let name: PackageName = serde_json::from_str(r#""numpy""#).expect("name");
        assert_eq!(
            serde_json::to_string(&name).expect("serialize"),
            r#""numpy""#
        );
        assert!(serde_json::from_str::<PackageName>(r#""NumPy""#).is_err());
        let version: PackageVersion = serde_json::from_str(r#""2.1.3""#).expect("version");
        assert_eq!(
            serde_json::to_string(&version).expect("serialize"),
            r#""2.1.3""#
        );
        assert!(serde_json::from_str::<PackageVersion>("2.1").is_err());
    }
}
