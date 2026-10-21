//! Why a bundle is refused: each problem has a code, which every
//! implementation of the spec shares, and a message for the person fixing
//! the bundle.

use std::fmt;

/// What kind of problem a bundle has. The codes are part of the spec: every
/// implementation reports the same codes for the same bundle, and the shared
/// test cases compare them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum ProblemCode {
    /// A path that breaks the path rules.
    PathInvalid,
    /// Two paths a case-insensitive filesystem cannot tell apart, or a file
    /// that is also a directory.
    PathCollision,
    /// A file whose extension is not allowed.
    ExtensionNotAllowed,
    /// A file larger than the limit for one file.
    FileTooLarge,
    /// Files that add up to more than the limit, or an archive too large for
    /// the limits.
    BundleTooLarge,
    /// More files than the limit.
    TooManyFiles,
    /// A symlink, junction, socket, or anything else that is not a regular
    /// file.
    NotARegularFile,
    /// No manifest at the bundle's root.
    ManifestMissing,
    /// A manifest that is not bundle TOML, or a key the format does not
    /// define.
    ManifestInvalid,
    /// A manifest written for a bundle format this build does not read.
    FormatUnsupported,
    /// An `entry` that does not name an importable module and a class.
    EntryInvalid,
    /// An `entry` naming a file the bundle does not hold.
    EntryNotFound,
    /// An `engine` that is not a version requirement.
    EngineInvalid,
    /// A `python` that is not a Python version.
    PythonInvalid,
    /// A param that JSON cannot hold.
    ParamNotJson,
    /// A dependency whose name or version breaks the rules.
    DependencyInvalid,
    /// A dependency the server does not offer.
    DependencyNotAllowed,
    /// A dependency version the server does not offer.
    DependencyVersionUnavailable,
    /// An engine requirement the server's engine does not meet.
    EngineMismatch,
    /// A Python version other than the server's.
    PythonMismatch,
    /// An archive that is not exactly the canonical tar of its files.
    ArchiveNotCanonical,
}

impl ProblemCode {
    /// Every code, in the order the spec lists them.
    pub const ALL: [ProblemCode; 21] = [
        Self::PathInvalid,
        Self::PathCollision,
        Self::ExtensionNotAllowed,
        Self::FileTooLarge,
        Self::BundleTooLarge,
        Self::TooManyFiles,
        Self::NotARegularFile,
        Self::ManifestMissing,
        Self::ManifestInvalid,
        Self::FormatUnsupported,
        Self::EntryInvalid,
        Self::EntryNotFound,
        Self::EngineInvalid,
        Self::PythonInvalid,
        Self::ParamNotJson,
        Self::DependencyInvalid,
        Self::DependencyNotAllowed,
        Self::DependencyVersionUnavailable,
        Self::EngineMismatch,
        Self::PythonMismatch,
        Self::ArchiveNotCanonical,
    ];

    /// The code as the spec and the shared test cases write it, such as
    /// `path_invalid`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PathInvalid => "path_invalid",
            Self::PathCollision => "path_collision",
            Self::ExtensionNotAllowed => "extension_not_allowed",
            Self::FileTooLarge => "file_too_large",
            Self::BundleTooLarge => "bundle_too_large",
            Self::TooManyFiles => "too_many_files",
            Self::NotARegularFile => "not_a_regular_file",
            Self::ManifestMissing => "manifest_missing",
            Self::ManifestInvalid => "manifest_invalid",
            Self::FormatUnsupported => "format_unsupported",
            Self::EntryInvalid => "entry_invalid",
            Self::EntryNotFound => "entry_not_found",
            Self::EngineInvalid => "engine_invalid",
            Self::PythonInvalid => "python_invalid",
            Self::ParamNotJson => "param_not_json",
            Self::DependencyInvalid => "dependency_invalid",
            Self::DependencyNotAllowed => "dependency_not_allowed",
            Self::DependencyVersionUnavailable => "dependency_version_unavailable",
            Self::EngineMismatch => "engine_mismatch",
            Self::PythonMismatch => "python_mismatch",
            Self::ArchiveNotCanonical => "archive_not_canonical",
        }
    }
}

/// One thing wrong with a bundle: its code, and a one-line message that says
/// what to change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BundleProblem {
    code: ProblemCode,
    message: String,
}

impl BundleProblem {
    pub(super) fn new(code: ProblemCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// What kind of problem it is.
    pub fn code(&self) -> ProblemCode {
        self.code
    }

    /// What is wrong, and how to fix it.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for BundleProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

/// Why a bundle could not be built, read, or run.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum BundleError {
    /// A file or directory could not be read.
    Io {
        /// The file or directory.
        path: String,
        /// What the operating system reported.
        message: String,
    },
    /// The bundle breaks the spec, or the server cannot run it: every
    /// problem found, in the order the spec checks them.
    Refused(Vec<BundleProblem>),
}

impl BundleError {
    /// The problems, if the bundle was refused.
    pub fn problems(&self) -> &[BundleProblem] {
        match self {
            Self::Refused(problems) => problems,
            Self::Io { .. } => &[],
        }
    }
}

impl fmt::Display for BundleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, message } => write!(f, "cannot read {path}: {message}"),
            Self::Refused(problems) => match problems.as_slice() {
                [only] => write!(f, "the bundle is refused: {only}"),
                many => {
                    write!(f, "the bundle is refused for {} reasons:", many.len())?;
                    many.iter()
                        .try_for_each(|problem| write!(f, "\n  - {problem}"))
                }
            },
        }
    }
}

impl std::error::Error for BundleError {}

/// `Err(Refused(problems))` unless there are none.
pub(super) fn refuse_unless_empty(problems: Vec<BundleProblem>) -> Result<(), BundleError> {
    if problems.is_empty() {
        Ok(())
    } else {
        Err(BundleError::Refused(problems))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn every_code_is_listed_once_with_its_own_name() {
        for code in ProblemCode::ALL {
            // Exhaustive on purpose: a new code stops this module from
            // compiling until it is added to ALL.
            match code {
                ProblemCode::PathInvalid
                | ProblemCode::PathCollision
                | ProblemCode::ExtensionNotAllowed
                | ProblemCode::FileTooLarge
                | ProblemCode::BundleTooLarge
                | ProblemCode::TooManyFiles
                | ProblemCode::NotARegularFile
                | ProblemCode::ManifestMissing
                | ProblemCode::ManifestInvalid
                | ProblemCode::FormatUnsupported
                | ProblemCode::EntryInvalid
                | ProblemCode::EntryNotFound
                | ProblemCode::EngineInvalid
                | ProblemCode::PythonInvalid
                | ProblemCode::ParamNotJson
                | ProblemCode::DependencyInvalid
                | ProblemCode::DependencyNotAllowed
                | ProblemCode::DependencyVersionUnavailable
                | ProblemCode::EngineMismatch
                | ProblemCode::PythonMismatch
                | ProblemCode::ArchiveNotCanonical => {}
            }
            assert!(
                code.as_str()
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte == b'_'),
                "{code:?}"
            );
        }
        let codes: HashSet<ProblemCode> = ProblemCode::ALL.into_iter().collect();
        let names: HashSet<&str> = ProblemCode::ALL.iter().map(|code| code.as_str()).collect();
        assert_eq!(codes.len(), ProblemCode::ALL.len());
        assert_eq!(names.len(), ProblemCode::ALL.len());
    }

    #[test]
    fn a_refusal_lists_every_problem() {
        let one = BundleError::Refused(vec![BundleProblem::new(
            ProblemCode::ManifestMissing,
            "no manifest",
        )]);
        assert_eq!(one.to_string(), "the bundle is refused: no manifest");
        let two = BundleError::Refused(vec![
            BundleProblem::new(ProblemCode::PathInvalid, "first"),
            BundleProblem::new(ProblemCode::FileTooLarge, "second"),
        ]);
        assert_eq!(
            two.to_string(),
            "the bundle is refused for 2 reasons:\n  - first\n  - second"
        );
        assert_eq!(two.problems().len(), 2);
        assert_eq!(refuse_unless_empty(Vec::new()), Ok(()));
    }
}
