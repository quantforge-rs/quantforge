//! The scratch directories: one directory of the server's own, made when
//! its pool starts and removed when it stops, and inside it one directory
//! for each worker, which is the worker's working and temporary directory
//! and is removed once the worker has stopped. Each is readable by its
//! owner only, and named at random.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use tracing::warn;
use uuid::Uuid;

use crate::protocol::bundle::Bundle;

/// The directory the bundle's files go in, inside a worker's scratch
/// directory.
pub(super) const BUNDLE_DIR: &str = "bundle";

/// The server's own directory, holding its workers' scratch directories.
#[derive(Debug)]
pub(super) struct ScratchRoot {
    path: PathBuf,
}

impl ScratchRoot {
    /// A new directory in `parent`.
    pub(super) fn create(parent: &Path) -> io::Result<Self> {
        let path = parent.join(format!("quantforge-jobs-{}", Uuid::new_v4().simple()));
        private_dir(&path)?;
        Ok(Self { path })
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    /// A new scratch directory for one worker.
    pub(super) fn new_dir(&self) -> io::Result<PathBuf> {
        let path = self.path.join(Uuid::new_v4().simple().to_string());
        private_dir(&path)?;
        Ok(path)
    }

    /// Removes it, and everything in it.
    pub(super) fn remove(&self) {
        wipe(&self.path);
    }
}

#[cfg(unix)]
fn private_dir(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new().mode(0o700).create(path)
}

#[cfg(not(unix))]
fn private_dir(path: &Path) -> io::Result<()> {
    fs::create_dir(path)
}

/// Removes `dir` and everything in it, without following a link a job left
/// there.
pub(super) fn wipe(dir: &Path) {
    match fs::remove_dir_all(dir) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            warn!(dir = %dir.display(), %error, "a scratch directory could not be removed")
        }
    }
}

/// Writes the bundle's files into `bundle/` in the scratch directory `dir`,
/// each at its path in the bundle, which the bundle spec keeps inside it.
pub(super) fn write_bundle(dir: &Path, bundle: &Bundle) -> io::Result<()> {
    let root = dir.join(BUNDLE_DIR);
    for file in bundle.files() {
        let path = root.join(file.path().as_str());
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        File::create_new(&path)?.write_all(file.contents())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::bundle::BundleLimits;

    #[test]
    fn a_root_holds_a_directory_per_worker_and_removing_it_leaves_nothing() {
        let parent = tempfile::tempdir().expect("a directory");
        let root = ScratchRoot::create(parent.path()).expect("a root");
        assert!(root.path().starts_with(parent.path()));
        let first = root.new_dir().expect("a scratch directory");
        let second = root.new_dir().expect("another");
        assert_ne!(first, second);
        assert_eq!(first.parent(), Some(root.path()));
        fs::write(first.join("left"), b"x").expect("a file");
        wipe(&first);
        assert!(!first.exists() && second.exists());
        root.remove();
        assert!(!root.path().exists());
        wipe(&first);
    }

    #[cfg(unix)]
    #[test]
    fn scratch_directories_are_their_owners_only() {
        use std::os::unix::fs::PermissionsExt;
        let parent = tempfile::tempdir().expect("a directory");
        let root = ScratchRoot::create(parent.path()).expect("a root");
        let dir = root.new_dir().expect("a scratch directory");
        for path in [root.path(), dir.as_path()] {
            let mode = fs::metadata(path)
                .expect("its metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o700, "{}", path.display());
        }
    }

    #[test]
    fn a_bundle_is_written_file_by_file_under_bundle() {
        let dir = tempfile::tempdir().expect("a directory");
        let bundle = Bundle::from_files(
            [
                (
                    "quantforge.toml",
                    "format = 1\nentry = \"pkg/strategy.py:Hold\"\nengine = \">=0.3.0\"\npython = \"3.12\"\n",
                ),
                ("pkg/strategy.py", "class Hold:\n    pass\n"),
            ],
            &BundleLimits::default(),
        )
        .expect("a bundle");
        write_bundle(dir.path(), &bundle).expect("written");
        let written = |path: &str| fs::read_to_string(dir.path().join(BUNDLE_DIR).join(path));
        assert_eq!(
            written("pkg/strategy.py").expect("the strategy"),
            "class Hold:\n    pass\n"
        );
        assert!(
            written("quantforge.toml")
                .expect("the manifest")
                .starts_with("format = 1")
        );
        assert!(
            write_bundle(dir.path(), &bundle).is_err(),
            "a file already there is never overwritten"
        );
    }
}
