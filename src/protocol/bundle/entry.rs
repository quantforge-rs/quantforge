//! A bundle's entry: the module and class the worker runs.

use std::{fmt, str::FromStr};

use super::super::ProtocolError;
use super::path::BundlePath;

/// Python's hard keywords, which cannot name a module or a class.
pub(super) const PYTHON_KEYWORDS: [&str; 35] = [
    "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class", "continue",
    "def", "del", "elif", "else", "except", "finally", "for", "from", "global", "if", "import",
    "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise", "return", "try", "while",
    "with", "yield",
];

/// The strategy a bundle runs: a Python file in the bundle and the class in
/// it, written `path/to/module.py:ClassName`.
///
/// The file must be importable: each directory and the file's name without
/// `.py` are Python identifiers in ASCII, and none is a keyword. A package's
/// `__init__.py` names the package itself.
///
/// ```
/// use quantforge::protocol::bundle::Entry;
///
/// let entry: Entry = "pkg/sma.py:SmaCross".parse()?;
/// assert_eq!(entry.file().as_str(), "pkg/sma.py");
/// assert_eq!(entry.module(), "pkg.sma");
/// assert_eq!(entry.class(), "SmaCross");
///
/// let package: Entry = "pkg/__init__.py:SmaCross".parse()?;
/// assert_eq!(package.module(), "pkg");
/// # Ok::<(), quantforge::protocol::ProtocolError>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    file: BundlePath,
    module: String,
    class: String,
}

impl Entry {
    /// The Python file that defines the class.
    pub fn file(&self) -> &BundlePath {
        &self.file
    }

    /// The module the worker imports, such as `pkg.sma`.
    pub fn module(&self) -> &str {
        &self.module
    }

    /// The class the worker runs.
    pub fn class(&self) -> &str {
        &self.class
    }
}

impl FromStr for Entry {
    type Err = ProtocolError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let refuse = |reason: String| ProtocolError::InvalidEntry {
            input: text.to_string(),
            reason,
        };
        let Some((file, class)) = text.split_once(':') else {
            return Err(refuse(
                "it must name a file and a class, as in \"strategy.py:MyStrategy\"".to_string(),
            ));
        };
        let file =
            BundlePath::new(file).map_err(|problem| refuse(problem.message().to_string()))?;
        let Some(stem) = file.as_str().strip_suffix(".py") else {
            return Err(refuse(format!(
                "{:?} is not a Python file; the entry names a .py file",
                file.as_str()
            )));
        };
        let mut parts: Vec<&str> = stem.split('/').collect();
        if parts.last() == Some(&"__init__") {
            parts.pop();
        }
        if parts.is_empty() {
            return Err(refuse(
                "a root __init__.py is not a module; put the class in a named module".to_string(),
            ));
        }
        if let Some(part) = parts.iter().find(|part| identifier_fault(part).is_some()) {
            return Err(refuse(format!(
                "{:?} cannot be imported: {}",
                file.as_str(),
                identifier_fault(part).unwrap_or_default()
            )));
        }
        if let Some(fault) = identifier_fault(class) {
            return Err(refuse(format!("the class {fault}")));
        }
        Ok(Self {
            module: parts.join("."),
            class: class.to_string(),
            file,
        })
    }
}

impl fmt::Display for Entry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.file, self.class)
    }
}

/// Why `name` is not an ASCII Python identifier, or `None` when it is one.
fn identifier_fault(name: &str) -> Option<String> {
    let bytes = name.as_bytes();
    let starts = bytes
        .first()
        .is_some_and(|&byte| byte.is_ascii_alphabetic() || byte == b'_');
    if !starts
        || !bytes
            .iter()
            .all(|&byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        Some(format!(
            "{name:?} is not a Python identifier: letters, digits, and _, not starting with a digit"
        ))
    } else if PYTHON_KEYWORDS.contains(&name) {
        Some(format!("{name:?} is a Python keyword"))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_entry_names_an_importable_module_and_a_class() {
        for (text, module, class) in [
            ("strategy.py:MyStrategy", "strategy", "MyStrategy"),
            ("pkg/sub/sma.py:Sma_2", "pkg.sub.sma", "Sma_2"),
            ("pkg/__init__.py:_Hidden", "pkg", "_Hidden"),
            ("match.py:case", "match", "case"),
        ] {
            let entry: Entry = text.parse().expect("an entry");
            assert_eq!((entry.module(), entry.class()), (module, class));
            assert_eq!(entry.to_string(), text);
        }
    }

    #[test]
    fn an_entry_that_cannot_be_imported_says_why() {
        for (text, reason) in [
            (
                "strategy.py",
                "it must name a file and a class, as in \"strategy.py:MyStrategy\"",
            ),
            (
                "strategy.txt:X",
                "\"strategy.txt\" is not a Python file; the entry names a .py file",
            ),
            (
                "../x.py:X",
                "\"../x.py\" is not a bundle path: it has a . or .. component",
            ),
            (
                "my-strategy.py:X",
                "\"my-strategy.py\" cannot be imported: \"my-strategy\" is not a Python identifier",
            ),
            (
                "2fast.py:X",
                "\"2fast.py\" cannot be imported: \"2fast\" is not a Python identifier",
            ),
            (
                "pkg.v2/x.py:X",
                "\"pkg.v2/x.py\" cannot be imported: \"pkg.v2\" is not a Python identifier",
            ),
            (
                "class.py:X",
                "\"class.py\" cannot be imported: \"class\" is a Python keyword",
            ),
            ("__init__.py:X", "a root __init__.py is not a module"),
            ("strategy.py:", "the class \"\" is not a Python identifier"),
            (
                "strategy.py:1X",
                "the class \"1X\" is not a Python identifier",
            ),
            (
                "strategy.py:Outer.Inner",
                "the class \"Outer.Inner\" is not a Python identifier",
            ),
            ("strategy.py:None", "the class \"None\" is a Python keyword"),
            (
                "strategy.py:Ünïcode",
                "the class \"Ünïcode\" is not a Python identifier",
            ),
            ("a.py:B:C", "the class \"B:C\" is not a Python identifier"),
        ] {
            let error = text.parse::<Entry>().expect_err("refused");
            assert!(
                error
                    .to_string()
                    .starts_with(&format!("invalid entry {text:?}: {reason}")),
                "{text:?}: {error}"
            );
        }
    }

    #[test]
    fn the_keywords_are_pythons_thirty_five() {
        let mut sorted = PYTHON_KEYWORDS.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 35);
        // Soft keywords stay identifiers.
        for soft in ["match", "case", "type", "_"] {
            assert_eq!(identifier_fault(soft), None, "{soft}");
        }
    }
}
