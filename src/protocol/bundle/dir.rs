//! Collecting a bundle from a directory: which files go in, and how they are
//! read. Every builder collects the same files from the same directory, so
//! that the same source gives the same bundle.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use super::MANIFEST_FILE;
use super::files::{BundleLimits, Candidate};
use super::problems::{BundleError, BundleProblem, ProblemCode};

/// The files found below a bundle's root, and the entries that cannot be in
/// a bundle at all.
#[derive(Debug, Default)]
pub(super) struct Collected {
    pub(super) candidates: Vec<Candidate>,
    pub(super) problems: Vec<BundleProblem>,
}

/// Every regular file below `root`, except entries whose name starts with
/// `.` and directories named `__pycache__`. A symlink, a junction, or any
/// other entry that is not a regular file is a problem, and is never
/// followed. A file is read only when it fits the limits, and the walk stops
/// once it has found more files than a bundle may hold.
pub(super) fn collect(root: &Path, limits: &BundleLimits) -> Result<Collected, BundleError> {
    let mut collected = Collected::default();
    let mut total: u64 = 0;
    let max_files = usize::try_from(limits.max_files).unwrap_or(usize::MAX);
    let mut pending: Vec<(PathBuf, String)> = vec![(root.to_path_buf(), String::new())];
    while let Some((dir, prefix)) = pending.pop() {
        let mut subdirectories = Vec::new();
        for (name, path) in sorted_entries(&dir)? {
            if name.starts_with('.') {
                continue;
            }
            let relative = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            // Never follows a symlink, so a link is seen as one.
            let metadata = fs::symlink_metadata(&path).map_err(|error| io_error(&path, error))?;
            let file_type = metadata.file_type();
            if file_type.is_symlink() {
                collected
                    .problems
                    .push(not_regular(&relative, "a symlink or junction"));
            } else if file_type.is_dir() {
                if name != "__pycache__" {
                    subdirectories.push((path, relative));
                }
            } else if !file_type.is_file() {
                collected
                    .problems
                    .push(not_regular(&relative, "not a regular file"));
            } else if collected.candidates.len() >= max_files.saturating_add(1) {
                // Enough to know the bundle holds too many files.
                return Ok(collected);
            } else {
                let size = metadata.len();
                // The manifest is read whatever the total, so that its problems
                // are reported as they would be for the same files given in
                // memory.
                let fits = size <= limits.file_limit()
                    && (relative == MANIFEST_FILE
                        || total.saturating_add(size) <= limits.max_total_bytes);
                total = total.saturating_add(size);
                let candidate = if fits {
                    let contents = read_at_most(&path, limits.file_limit())?;
                    Candidate::read(relative, contents)
                } else {
                    Candidate {
                        path: relative,
                        size,
                        contents: None,
                    }
                };
                collected.candidates.push(candidate);
            }
        }
        // Popped in name order.
        pending.extend(subdirectories.into_iter().rev());
    }
    Ok(collected)
}

/// The entries of `dir` sorted by name bytes, with names that are not UTF-8
/// kept in a lossy form for the path rules to refuse.
fn sorted_entries(dir: &Path) -> Result<Vec<(String, PathBuf)>, BundleError> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(dir).map_err(|error| io_error(dir, error))? {
        let entry = entry.map_err(|error| io_error(dir, error))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        entries.push((name, entry.path()));
    }
    entries.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    Ok(entries)
}

/// At most `limit` + 1 bytes of `path`, so that a file that grew since its
/// size was checked is still caught by the limit.
fn read_at_most(path: &Path, limit: u64) -> Result<Vec<u8>, BundleError> {
    let file = fs::File::open(path).map_err(|error| io_error(path, error))?;
    let mut contents = Vec::new();
    file.take(limit.saturating_add(1))
        .read_to_end(&mut contents)
        .map_err(|error| io_error(path, error))?;
    Ok(contents)
}

fn not_regular(relative: &str, what: &str) -> BundleProblem {
    BundleProblem::new(
        ProblemCode::NotARegularFile,
        format!(
            "{relative:?} is {what}; a bundle holds regular files only, so copy the file in or \
             leave it out"
        ),
    )
}

fn io_error(path: &Path, error: std::io::Error) -> BundleError {
    BundleError::Io {
        path: path.display().to_string(),
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, relative: &str, contents: &[u8]) {
        let path = relative
            .split('/')
            .fold(root.to_path_buf(), |path, part| path.join(part));
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create the directory");
        }
        fs::write(path, contents).expect("write the file");
    }

    fn paths(collected: &Collected) -> Vec<&str> {
        let mut paths: Vec<&str> = collected
            .candidates
            .iter()
            .map(|c| c.path.as_str())
            .collect();
        paths.sort_unstable();
        paths
    }

    #[test]
    fn hidden_entries_and_python_caches_are_left_out() {
        let root = tempfile::tempdir().expect("a temporary directory");
        for relative in [
            "quantforge.toml",
            "strategy.py",
            "pkg/__init__.py",
            "pkg/deep/levels.json",
            ".git/config",
            ".DS_Store",
            "pkg/.hidden.py",
            "__pycache__/strategy.cpython-312.pyc",
            "pkg/__pycache__/x.pyc",
        ] {
            write(root.path(), relative, b"x");
        }
        let collected = collect(root.path(), &BundleLimits::default()).expect("collected");
        assert!(collected.problems.is_empty(), "{:?}", collected.problems);
        assert_eq!(
            paths(&collected),
            [
                "pkg/__init__.py",
                "pkg/deep/levels.json",
                "quantforge.toml",
                "strategy.py"
            ]
        );
    }

    #[test]
    fn a_file_is_read_only_while_it_fits_the_limits_except_the_manifest() {
        let root = tempfile::tempdir().expect("a temporary directory");
        write(root.path(), "a.py", &[b'a'; 6]);
        write(root.path(), "big.py", &[b'b'; 20]);
        write(root.path(), "c.py", &[b'c'; 6]);
        write(root.path(), "quantforge.toml", &[b'q'; 6]);
        let limits = BundleLimits {
            max_file_bytes: 10,
            max_total_bytes: 12,
            max_files: 10,
        };
        let collected = collect(root.path(), &limits).expect("collected");
        let read: Vec<(&str, u64, bool)> = collected
            .candidates
            .iter()
            .map(|c| (c.path.as_str(), c.size, c.contents.is_some()))
            .collect();
        assert_eq!(
            read,
            [
                ("a.py", 6, true),
                ("big.py", 20, false),
                ("c.py", 6, false),
                ("quantforge.toml", 6, true),
            ]
        );
    }

    #[test]
    fn the_walk_stops_once_it_has_found_more_files_than_the_limit() {
        let root = tempfile::tempdir().expect("a temporary directory");
        for name in ["a.py", "b.py", "c.py", "d.py"] {
            write(root.path(), name, b"x");
        }
        let limits = BundleLimits {
            max_files: 2,
            ..BundleLimits::default()
        };
        let collected = collect(root.path(), &limits).expect("collected");
        assert_eq!(paths(&collected), ["a.py", "b.py", "c.py"]);
    }

    #[test]
    fn a_missing_directory_is_an_io_error() {
        let root = tempfile::tempdir().expect("a temporary directory");
        let missing = root.path().join("nowhere");
        let error = collect(&missing, &BundleLimits::default()).expect_err("no directory");
        assert!(matches!(error, BundleError::Io { .. }), "{error:?}");
    }
}
