//! The limits a bundle stays within, and the checks each file and the whole
//! set of files pass, however the bundle arrived.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::path::{BundleFile, BundlePath};
use super::problems::{BundleProblem, ProblemCode};

/// The largest file a canonical tar holds: what 11 octal digits count to.
pub(super) const MAX_TAR_FILE_BYTES: u64 = 0o77_777_777_777;

/// How large a bundle may be. A server sets its own; a client checks against
/// the defaults, or against the limits the server advertises.
///
/// ```
/// use quantforge::protocol::bundle::BundleLimits;
///
/// let json = r#"{"max_file_bytes":1048576,"max_total_bytes":8388608,"max_files":256}"#;
/// assert_eq!(serde_json::to_string(&BundleLimits::default())?, json);
/// assert_eq!(serde_json::from_str::<BundleLimits>(json)?, BundleLimits::default());
/// # Ok::<(), serde_json::Error>(())
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleLimits {
    /// The most bytes one file may hold.
    pub max_file_bytes: u64,
    /// The most bytes all the files may hold together.
    pub max_total_bytes: u64,
    /// The most files a bundle may hold.
    pub max_files: u32,
}

impl Default for BundleLimits {
    /// 1 MiB per file, 8 MiB in all, and 256 files.
    fn default() -> Self {
        Self {
            max_file_bytes: 1024 * 1024,
            max_total_bytes: 8 * 1024 * 1024,
            max_files: 256,
        }
    }
}

impl BundleLimits {
    /// The largest canonical tar a bundle within these limits can take: a
    /// 512-byte header and up to 511 bytes of padding per file, the files'
    /// bytes, and the closing 1024 bytes. A server refuses a larger upload
    /// before reading it.
    pub fn max_archive_bytes(&self) -> u64 {
        u64::from(self.max_files)
            .saturating_mul(1023)
            .saturating_add(self.max_total_bytes)
            .saturating_add(1024)
    }

    /// The per-file limit, capped at what a tar header can record.
    pub(super) fn file_limit(&self) -> u64 {
        self.max_file_bytes.min(MAX_TAR_FILE_BYTES)
    }
}

/// A file as it arrived, before any rule is checked. `contents` is `None`
/// only when reading it was skipped because a limit was already broken.
#[derive(Debug)]
pub(super) struct Candidate {
    pub(super) path: String,
    pub(super) size: u64,
    pub(super) contents: Option<Vec<u8>>,
}

impl Candidate {
    pub(super) fn read(path: String, contents: Vec<u8>) -> Self {
        Self {
            path,
            size: u64::try_from(contents.len()).unwrap_or(u64::MAX),
            contents: Some(contents),
        }
    }
}

/// What the per-file and whole-set checks found.
#[derive(Debug, Default)]
pub(super) struct Checked {
    /// The files that passed every per-file check, sorted by path.
    pub(super) files: Vec<BundleFile>,
    /// Every path that passed the path rules, including files over the size
    /// limit: what the manifest's entry may name.
    pub(super) paths: BTreeSet<BundlePath>,
    pub(super) problems: Vec<BundleProblem>,
}

/// The spec's per-file checks — the path rules, then the size of each file —
/// and its whole-set checks: collisions, the number of files, and their
/// total size.
pub(super) fn check(mut candidates: Vec<Candidate>, limits: &BundleLimits) -> Checked {
    candidates.sort_by(|a, b| a.path.as_bytes().cmp(b.path.as_bytes()));
    let mut checked = Checked::default();
    let mut total: u64 = 0;
    let mut valid = Vec::new();
    for candidate in &candidates {
        total = total.saturating_add(candidate.size);
        let path = match BundlePath::new(candidate.path.as_str()) {
            Ok(path) => path,
            Err(problem) => {
                checked.problems.push(problem);
                continue;
            }
        };
        valid.push(path.clone());
        checked.paths.insert(path.clone());
        if candidate.size > limits.file_limit() {
            checked.problems.push(BundleProblem::new(
                ProblemCode::FileTooLarge,
                format!(
                    "{:?} is {} bytes, over the limit of {} bytes for one file",
                    path.as_str(),
                    candidate.size,
                    limits.file_limit()
                ),
            ));
        } else if let Some(contents) = &candidate.contents {
            checked.files.push(BundleFile::new(path, contents.clone()));
        }
    }
    checked.problems.extend(collisions(&valid));
    if candidates.len() > usize::try_from(limits.max_files).unwrap_or(usize::MAX) {
        checked.problems.push(BundleProblem::new(
            ProblemCode::TooManyFiles,
            format!(
                "the bundle holds more than the limit of {} files; leave out what the strategy \
                 does not need",
                limits.max_files
            ),
        ));
    }
    if total > limits.max_total_bytes {
        checked.problems.push(BundleProblem::new(
            ProblemCode::BundleTooLarge,
            format!(
                "the bundle's files hold {total} bytes, over the limit of {} bytes in all",
                limits.max_total_bytes
            ),
        ));
    }
    checked
}

/// Paths a case-insensitive filesystem could not hold side by side: two
/// paths that differ only in case, a path listed twice, or a file whose name
/// another path needs for a directory.
fn collisions(paths: &[BundlePath]) -> Vec<BundleProblem> {
    let mut problems = Vec::new();
    let mut files: BTreeMap<String, &BundlePath> = BTreeMap::new();
    for path in paths {
        let folded = path.as_str().to_ascii_lowercase();
        match files.get(&folded) {
            Some(&first) if first == path => problems.push(BundleProblem::new(
                ProblemCode::PathCollision,
                format!("{:?} is listed twice", path.as_str()),
            )),
            Some(&first) => problems.push(BundleProblem::new(
                ProblemCode::PathCollision,
                format!(
                    "{:?} and {:?} differ only in case, which a case-insensitive filesystem \
                     cannot hold; rename one",
                    first.as_str(),
                    path.as_str()
                ),
            )),
            None => {
                files.insert(folded, path);
            }
        }
    }
    for path in paths {
        let folded = path.as_str().to_ascii_lowercase();
        let directories = folded.match_indices('/').map(|(at, _)| &folded[..at]);
        for directory in directories {
            if let Some(file) = files.get(directory) {
                problems.push(BundleProblem::new(
                    ProblemCode::PathCollision,
                    format!(
                        "{:?} is a file, but {:?} needs a directory of that name; rename one",
                        file.as_str(),
                        path.as_str()
                    ),
                ));
            }
        }
    }
    // Directories whose names differ only in case merge on a case-insensitive filesystem.
    let mut spellings: BTreeMap<String, &str> = BTreeMap::new();
    let mut reported: BTreeSet<String> = BTreeSet::new();
    for path in paths {
        let text = path.as_str();
        for (at, _) in text.match_indices('/') {
            let directory = &text[..at];
            let folded = directory.to_ascii_lowercase();
            match spellings.get(&folded) {
                Some(&first) if first != directory && reported.insert(folded.clone()) => {
                    problems.push(BundleProblem::new(
                        ProblemCode::PathCollision,
                        format!(
                            "the directories {first:?} and {directory:?} differ only in case, \
                             which a case-insensitive filesystem cannot hold; rename one"
                        ),
                    ));
                }
                Some(_) => {}
                None => {
                    spellings.insert(folded, directory);
                }
            }
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidates(paths: &[(&str, usize)]) -> Vec<Candidate> {
        paths
            .iter()
            .map(|&(path, size)| Candidate::read(path.to_string(), vec![b'x'; size]))
            .collect()
    }

    fn codes(checked: &Checked) -> Vec<&'static str> {
        checked.problems.iter().map(|p| p.code().as_str()).collect()
    }

    #[test]
    fn files_within_the_limits_pass_sorted_by_path_bytes() {
        let checked = check(
            candidates(&[
                ("b.py", 1),
                ("a_b.py", 1),
                ("a/b.py", 1),
                ("a.py", 1),
                ("C.py", 1),
            ]),
            &BundleLimits::default(),
        );
        assert!(checked.problems.is_empty(), "{:?}", checked.problems);
        let order: Vec<&str> = checked.files.iter().map(|f| f.path().as_str()).collect();
        assert_eq!(order, ["C.py", "a.py", "a/b.py", "a_b.py", "b.py"]);
    }

    #[test]
    fn each_limit_is_checked_with_its_own_problem() {
        let limits = BundleLimits {
            max_file_bytes: 10,
            max_total_bytes: 25,
            max_files: 2,
        };
        let checked = check(
            candidates(&[("a.py", 11), ("b.py", 10), ("c.py", 5)]),
            &limits,
        );
        assert_eq!(
            codes(&checked),
            ["file_too_large", "too_many_files", "bundle_too_large"]
        );
        assert_eq!(
            checked.problems[0].message(),
            "\"a.py\" is 11 bytes, over the limit of 10 bytes for one file"
        );
        assert_eq!(
            checked.problems[2].message(),
            "the bundle's files hold 26 bytes, over the limit of 25 bytes in all"
        );
        // A file over the limit is still a path the entry may name.
        assert!(checked.paths.iter().any(|path| path.as_str() == "a.py"));
        assert_eq!(checked.files.len(), 2);
    }

    #[test]
    fn paths_a_case_insensitive_filesystem_cannot_hold_collide() {
        let checked = check(
            candidates(&[
                ("Strategy.py", 1),
                ("strategy.py", 1),
                ("lib.py", 1),
                ("LIB.py/x.py", 1),
                ("once.py", 1),
                ("once.py", 1),
            ]),
            &BundleLimits::default(),
        );
        let messages: Vec<&str> = checked.problems.iter().map(|p| p.message()).collect();
        // In path-byte order: uppercase sorts before lowercase.
        assert_eq!(
            messages,
            [
                "\"once.py\" is listed twice",
                "\"Strategy.py\" and \"strategy.py\" differ only in case, which a \
                 case-insensitive filesystem cannot hold; rename one",
                "\"lib.py\" is a file, but \"LIB.py/x.py\" needs a directory of that name; \
                 rename one",
            ]
        );
    }

    #[test]
    fn a_path_that_breaks_a_rule_is_reported_once_and_checked_no_further() {
        let checked = check(
            candidates(&[("../x.py", 99), ("notes.md", 1)]),
            &BundleLimits {
                max_file_bytes: 10,
                ..BundleLimits::default()
            },
        );
        assert_eq!(codes(&checked), ["path_invalid", "extension_not_allowed"]);
        assert!(checked.paths.is_empty());
    }

    #[test]
    fn the_archive_limit_counts_headers_padding_and_the_end() {
        assert_eq!(BundleLimits::default().max_archive_bytes(), 8_651_520);
        let huge = BundleLimits {
            max_file_bytes: u64::MAX,
            max_total_bytes: u64::MAX,
            max_files: u32::MAX,
        };
        assert_eq!(huge.max_archive_bytes(), u64::MAX);
        assert_eq!(huge.file_limit(), MAX_TAR_FILE_BYTES);
    }
}
