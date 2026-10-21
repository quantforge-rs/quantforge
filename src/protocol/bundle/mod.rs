//! Strategy bundles: how a client packs a Python strategy for a remote
//! backtest, and how the server checks what it received.
//!
//! A bundle is explicit files and a manifest, never a pickled object. Its
//! files are Python source and data, and its manifest, [`MANIFEST_FILE`] at
//! the root, names the strategy to run ([`Entry`]), the engine and Python
//! versions it runs on, its parameters, and the packages it imports. The
//! full rules are the [bundle spec], which every client implements; this
//! module is its reference implementation, and the shared test cases in
//! `tests/fixtures/bundle_cases.json` hold every implementation to it.
//!
//! A bundle's files make exactly one byte sequence, its canonical tar
//! ([`Bundle::canonical_tar`]): a POSIX ustar archive with the files sorted
//! by path, every timestamp, owner, and mode fixed, and nothing else in it.
//! Its SHA-256 is the bundle's code hash ([`Bundle::code_hash`]), which a
//! [`JobSubmission`](super::jobs::JobSubmission) names the bundle by: the
//! same files hash the same on every machine, whatever their timestamps,
//! owners, or modes. The canonical tar is also what a client uploads, as
//! [`BUNDLE_MEDIA_TYPE`]; a server reads it back with
//! [`Bundle::from_canonical_tar`], which refuses any archive that is not
//! exactly canonical, and checks that the bundle can run on its workers with
//! [`Bundle::admit`].
//!
//! Unlike the wire's messages, a manifest refuses keys it does not know:
//! it is part of what the code hash names, and decides what runs, so a new
//! key comes with a new [`BUNDLE_FORMAT`].
//!
//! [bundle spec]: <https://github.com/quantforge-rs/quantforge/blob/main/docs/bundle-spec.md>
//!
//! ```
//! use quantforge::protocol::bundle::{Bundle, BundleLimits};
//!
//! let manifest = "format = 1\nentry = \"strategy.py:Hold\"\nengine = \">=0.3.0, <0.4.0\"\npython = \"3.12\"\n";
//! let strategy = "class Hold:\n    pass\n";
//! let limits = BundleLimits::default();
//! let bundle = Bundle::from_files(
//!     [("quantforge.toml", manifest), ("strategy.py", strategy)],
//!     &limits,
//! )?;
//! assert_eq!(bundle.manifest().entry().module(), "strategy");
//! assert_eq!(
//!     bundle.code_hash().to_string(),
//!     "sha256:59f8dcaea45590e10e16199eb6b856deb440a52844e69514fa372f909340d4f3"
//! );
//!
//! let tar = bundle.canonical_tar();
//! assert_eq!(Bundle::from_canonical_tar(&tar, &limits)?, bundle);
//! # Ok::<(), quantforge::protocol::bundle::BundleError>(())
//! ```

mod archive;
mod dir;
mod entry;
mod files;
mod manifest;
mod package;
mod params;
mod path;
mod problems;
mod reader;
mod requirement;
mod runtime;

use std::fmt;
use std::path::Path;

use super::ContentHash;

pub use entry::Entry;
pub use files::BundleLimits;
pub use manifest::{MANIFEST_KEYS, MAX_MANIFEST_DEPTH, Manifest};
pub use package::{PackageName, PackageVersion};
pub use params::Params;
pub use path::{BundleFile, BundlePath, MAX_PATH_BYTES, RESERVED_NAMES};
pub use problems::{BundleError, BundleProblem, ProblemCode};
pub use requirement::{EngineRequirement, EngineVersion, PythonVersion};
pub use runtime::Runtime;

/// The bundle format this build writes and reads, which a manifest names as
/// `format = 1`.
pub const BUNDLE_FORMAT: u32 = 1;

/// The manifest's path, at the bundle's root.
pub const MANIFEST_FILE: &str = "quantforge.toml";

/// The most bytes a manifest may hold, whatever the limits.
pub const MAX_MANIFEST_BYTES: u64 = 64 * 1024;

/// The extensions a bundle's files may have.
pub const ALLOWED_EXTENSIONS: [&str; 4] = ["py", "toml", "json", "txt"];

/// The media type of a bundle upload, whose body is the canonical tar.
pub const BUNDLE_MEDIA_TYPE: &str = "application/x-tar";

/// A checked bundle: its manifest and its files, sorted by path. It is
/// built only through the checks the spec defines, so every bundle has a
/// valid manifest, paths that extract anywhere, and files within the limits
/// it was built with.
#[derive(Clone, PartialEq, Eq)]
pub struct Bundle {
    manifest: Manifest,
    files: Vec<BundleFile>,
}

impl Bundle {
    /// Builds a bundle from files given in memory, as paths and their bytes,
    /// in any order. Every problem the spec defines is reported, in its
    /// order.
    pub fn from_files<P: Into<String>, C: Into<Vec<u8>>>(
        files: impl IntoIterator<Item = (P, C)>,
        limits: &BundleLimits,
    ) -> Result<Self, BundleError> {
        let candidates = files
            .into_iter()
            .map(|(path, contents)| files::Candidate::read(path.into(), contents.into()))
            .collect();
        Self::check(candidates, Vec::new(), limits)
    }

    /// Collects a bundle from a directory: every regular file below `root`,
    /// except entries whose name starts with `.` and `__pycache__`
    /// directories. A symlink or anything else that is not a regular file is
    /// refused, never followed.
    pub fn from_dir(root: &Path, limits: &BundleLimits) -> Result<Self, BundleError> {
        let collected = dir::collect(root, limits)?;
        Self::check(collected.candidates, collected.problems, limits)
    }

    /// Reads a bundle back from its canonical tar, as a server reads an
    /// upload. The archive must be exactly the canonical tar of its files,
    /// so its SHA-256 is the bundle's code hash.
    pub fn from_canonical_tar(tar: &[u8], limits: &BundleLimits) -> Result<Self, BundleError> {
        let candidates =
            reader::read(tar, limits).map_err(|problem| BundleError::Refused(vec![problem]))?;
        Self::check(candidates, Vec::new(), limits)
    }

    fn check(
        candidates: Vec<files::Candidate>,
        mut problems: Vec<BundleProblem>,
        limits: &BundleLimits,
    ) -> Result<Self, BundleError> {
        let manifest_bytes = candidates
            .iter()
            .find(|candidate| candidate.path == MANIFEST_FILE)
            .and_then(|candidate| candidate.contents.clone());
        let checked = files::check(candidates, limits);
        problems.extend(checked.problems);
        let manifest_path = BundlePath::new(MANIFEST_FILE);
        let has_manifest = manifest_path
            .as_ref()
            .is_ok_and(|path| checked.paths.contains(path));
        let passed = manifest_path
            .as_ref()
            .is_ok_and(|path| checked.files.iter().any(|file| file.path() == path));
        let manifest = match manifest_bytes.filter(|_| passed) {
            Some(bytes) => match manifest::parse(&bytes, &checked.paths) {
                Ok(manifest) => Some(manifest),
                Err(found) => {
                    problems.extend(found);
                    None
                }
            },
            // A manifest over the size limit is reported as such, and not parsed.
            None if has_manifest => None,
            None => {
                problems.push(BundleProblem::new(
                    ProblemCode::ManifestMissing,
                    format!(
                        "the bundle has no {MANIFEST_FILE} at its root; add one naming the entry, \
                         the engine, and the python version"
                    ),
                ));
                None
            }
        };
        match manifest {
            Some(manifest) if problems.is_empty() => Ok(Self {
                manifest,
                files: checked.files,
            }),
            _ => Err(BundleError::Refused(problems)),
        }
    }

    /// The checked manifest.
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// The files, sorted by path, the manifest among them.
    pub fn files(&self) -> &[BundleFile] {
        &self.files
    }

    /// The canonical tar: the bytes the code hash is taken over, and the body
    /// of an upload.
    pub fn canonical_tar(&self) -> Vec<u8> {
        archive::write(&self.files)
    }

    /// The SHA-256 of the canonical tar: the bundle's name, the same on every
    /// machine for the same files.
    pub fn code_hash(&self) -> ContentHash {
        archive::code_hash(&self.canonical_tar())
    }

    /// Checks that the bundle can run on `runtime`: the engine meets the
    /// manifest's requirement, the Python version is the one it was written
    /// for, and the runtime offers every dependency at its pinned version.
    /// Every reason it cannot is reported.
    pub fn admit(&self, runtime: &Runtime) -> Result<(), BundleError> {
        problems::refuse_unless_empty(runtime::admission(&self.manifest, runtime))
    }
}

// The files' bytes could be megabytes; their paths and sizes say enough.
impl fmt::Debug for Bundle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Bundle")
            .field("entry", &self.manifest.entry().to_string())
            .field("files", &self.files)
            .field("code_hash", &self.code_hash())
            .finish()
    }
}
