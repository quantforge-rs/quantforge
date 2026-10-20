//! Bundle paths: the rules that make a bundle extract the same way on
//! every operating system, and the files they name.

use std::fmt;

use super::ALLOWED_EXTENSIONS;
use super::problems::{BundleProblem, ProblemCode};

/// The longest path, in bytes: what a tar header's name field holds.
pub const MAX_PATH_BYTES: usize = 100;

/// The names Windows reserves for devices, whatever their extension, which
/// no path component may have: matched without regard to case on the text
/// before the component's first `.`.
pub const RESERVED_NAMES: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Where a file sits in a bundle, such as `pkg/strategy.py`.
///
/// A path is ASCII, relative, and at most 100 bytes, with `/` between its
/// components. Each component starts with a letter, a digit, or `_`, holds
/// only letters, digits, `.`, `_`, and `-`, and does not end in `.`. No
/// component is `__pycache__` or a name Windows reserves for a device, such
/// as `aux.py`. The extension is `py`, `toml`, `json`, or `txt`, in
/// lowercase.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BundlePath(String);

impl BundlePath {
    /// Checks a path against the rules.
    pub fn new(path: impl Into<String>) -> Result<Self, BundleProblem> {
        let path = path.into();
        if let Some(reason) = path_fault(&path) {
            return Err(BundleProblem::new(
                ProblemCode::PathInvalid,
                format!("{path:?} is not a bundle path: {reason}"),
            ));
        }
        let last = path.rsplit('/').next().unwrap_or_default();
        let allowed = last
            .rsplit_once('.')
            .is_some_and(|(_, extension)| ALLOWED_EXTENSIONS.contains(&extension));
        if !allowed {
            return Err(BundleProblem::new(
                ProblemCode::ExtensionNotAllowed,
                format!(
                    "{path:?} cannot be in a bundle: a bundle holds only .py, .toml, .json, and \
                     .txt files, so leave it out or convert it"
                ),
            ));
        }
        Ok(Self(path))
    }

    /// The path as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The path's components, outermost first.
    pub fn components(&self) -> impl Iterator<Item = &str> {
        self.0.split('/')
    }
}

impl fmt::Display for BundlePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for BundlePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

/// Why `path` breaks a path rule, or `None` when it keeps them all.
fn path_fault(path: &str) -> Option<String> {
    if path.is_empty() {
        return Some("it is empty".to_string());
    }
    if !path.is_ascii() {
        return Some("it must be ASCII".to_string());
    }
    if path.len() > MAX_PATH_BYTES {
        return Some(format!(
            "it is {} bytes, over the {MAX_PATH_BYTES} a tar header holds",
            path.len()
        ));
    }
    if path.starts_with('/') {
        return Some("it must be relative to the bundle, without a leading /".to_string());
    }
    if path.contains('\\') {
        return Some("separate directories with /, not \\".to_string());
    }
    path.split('/').find_map(component_fault)
}

fn component_fault(component: &str) -> Option<String> {
    let Some(first) = component.bytes().next() else {
        return Some("it has an empty component, as in a//b or a trailing /".to_string());
    };
    if component == "." || component == ".." {
        return Some("it has a . or .. component; a path stays inside the bundle".to_string());
    }
    if !(first.is_ascii_alphanumeric() || first == b'_') {
        return Some(format!(
            "{component:?} starts with {:?}; a component starts with a letter, a digit, or _",
            char::from(first)
        ));
    }
    let allowed = |byte: u8| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-');
    if let Some(byte) = component.bytes().find(|&byte| !allowed(byte)) {
        return Some(format!(
            "{component:?} holds {:?}; a component holds only letters, digits, ., _, and -",
            char::from(byte)
        ));
    }
    if component.ends_with('.') {
        return Some(format!(
            "{component:?} ends in ., which Windows drops from file names"
        ));
    }
    if component == "__pycache__" {
        return Some("__pycache__ is Python's cache, never part of a bundle".to_string());
    }
    let stem = component.split('.').next().unwrap_or_default();
    if RESERVED_NAMES
        .iter()
        .any(|name| name.eq_ignore_ascii_case(stem))
    {
        return Some(format!(
            "{component:?} is named {}, which Windows reserves for a device",
            stem.to_ascii_uppercase()
        ));
    }
    None
}

/// A file in a bundle: its path and its bytes, exactly as they are hashed.
#[derive(Clone, PartialEq, Eq)]
pub struct BundleFile {
    path: BundlePath,
    contents: Vec<u8>,
}

impl BundleFile {
    pub(super) fn new(path: BundlePath, contents: Vec<u8>) -> Self {
        Self { path, contents }
    }

    /// Where the file sits in the bundle.
    pub fn path(&self) -> &BundlePath {
        &self.path
    }

    /// The file's bytes.
    pub fn contents(&self) -> &[u8] {
        &self.contents
    }
}

// The bytes can be megabytes of source; their length says enough.
impl fmt::Debug for BundleFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BundleFile")
            .field("path", &self.path)
            .field("bytes", &self.contents.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_that_extract_anywhere_are_accepted() {
        let longest = format!("{}.py", "a".repeat(MAX_PATH_BYTES - 3));
        for path in [
            "strategy.py",
            "quantforge.toml",
            "pkg/__init__.py",
            "pkg/sub_pkg/model-v2.json",
            "data/levels.2024.txt",
            "_private.py",
            "9lives.py",
            "console.py",
            "com10.py",
            "auxiliary.py",
            &longest,
        ] {
            let checked = BundlePath::new(path).expect("a valid path");
            assert_eq!(checked.as_str(), path);
        }
    }

    #[test]
    fn each_path_rule_refuses_with_the_rule_it_breaks() {
        let too_long = format!("{}.py", "a".repeat(MAX_PATH_BYTES - 2));
        for (path, reason) in [
            ("", "it is empty"),
            ("stratégie.py", "it must be ASCII"),
            (
                too_long.as_str(),
                "it is 101 bytes, over the 100 a tar header holds",
            ),
            (
                "/abs.py",
                "it must be relative to the bundle, without a leading /",
            ),
            ("pkg\\x.py", "separate directories with /, not \\"),
            ("a//b.py", "it has an empty component"),
            ("pkg/", "it has an empty component"),
            ("../x.py", "it has a . or .. component"),
            ("a/./b.py", "it has a . or .. component"),
            (".env.py", "\".env.py\" starts with '.'"),
            ("-x.py", "\"-x.py\" starts with '-'"),
            ("a b.py", "\"a b.py\" holds ' '"),
            ("a\nb.py", "\"a\\nb.py\" holds '\\n'"),
            ("x:y.py", "\"x:y.py\" holds ':'"),
            ("dir./x.py", "\"dir.\" ends in ., which Windows drops"),
            ("__pycache__/x.py", "__pycache__ is Python's cache"),
            ("aux.py", "\"aux.py\" is named AUX, which Windows reserves"),
            ("pkg/Con.json", "\"Con.json\" is named CON"),
            ("lpt9/x.py", "\"lpt9\" is named LPT9"),
            ("nul.tar.txt", "\"nul.tar.txt\" is named NUL"),
        ] {
            let problem = BundlePath::new(path).expect_err("refused");
            assert_eq!(problem.code(), ProblemCode::PathInvalid, "{path:?}");
            assert!(
                problem.message().contains(reason),
                "{path:?}: {}",
                problem.message()
            );
            assert!(
                problem
                    .message()
                    .starts_with(&format!("{path:?} is not a bundle path: ")),
                "{}",
                problem.message()
            );
            assert!(!problem.message().contains('\n'), "{}", problem.message());
        }
    }

    #[test]
    fn only_lowercase_py_toml_json_and_txt_files_are_allowed() {
        for path in [
            "notes.md",
            "Makefile",
            "x.PY",
            "model.pkl",
            "data.npy",
            "a.json.bak",
            "py",
        ] {
            let problem = BundlePath::new(path).expect_err("refused");
            assert_eq!(problem.code(), ProblemCode::ExtensionNotAllowed, "{path:?}");
            assert_eq!(
                problem.message(),
                format!(
                    "{path:?} cannot be in a bundle: a bundle holds only .py, .toml, .json, and \
                     .txt files, so leave it out or convert it"
                )
            );
        }
    }

    #[test]
    fn a_file_shows_its_size_never_its_bytes() {
        let file = BundleFile::new(
            BundlePath::new("strategy.py").expect("path"),
            b"secret = 1".to_vec(),
        );
        assert_eq!(
            format!("{file:?}"),
            r#"BundleFile { path: "strategy.py", bytes: 10 }"#
        );
    }
}
