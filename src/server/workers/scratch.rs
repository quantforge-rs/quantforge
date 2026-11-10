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
/// there, and even when a job took its owner's rights to a directory away.
pub(super) fn wipe(dir: &Path) {
    let removed = match fs::remove_dir_all(dir) {
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            open_up(dir);
            fs::remove_dir_all(dir)
        }
        removed => removed,
    };
    match removed {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            warn!(dir = %dir.display(), %error, "a scratch directory could not be removed")
        }
    }
}

/// Gives the owner every right to `dir` and to each directory under it,
/// never through a link: a job runs as its server's user, so it can take
/// them away, and no sandbox here stops it.
#[cfg(unix)]
fn open_up(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut pending = vec![dir.to_path_buf()];
    while let Some(next) = pending.pop() {
        if !fs::symlink_metadata(&next).is_ok_and(|found| found.is_dir()) {
            continue;
        }
        let _ = fs::set_permissions(&next, fs::Permissions::from_mode(0o700));
        if let Ok(entries) = fs::read_dir(&next) {
            pending.extend(entries.flatten().map(|entry| entry.path()));
        }
    }
}

#[cfg(not(unix))]
fn open_up(_dir: &Path) {}

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

    #[cfg(unix)]
    fn set_mode(path: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("a mode");
    }

    #[cfg(unix)]
    #[test]
    fn a_scratch_directory_goes_even_where_its_job_took_the_rights_away() {
        let parent = tempfile::tempdir().expect("a directory");
        let dir = parent.path().join("scratch");
        fs::create_dir_all(dir.join("closed/inner")).expect("directories");
        fs::write(dir.join("closed/inner/file"), b"x").expect("a file");
        fs::create_dir(dir.join("read-only")).expect("a directory");
        fs::write(dir.join("read-only/file"), b"x").expect("a file");
        set_mode(&dir.join("closed/inner"), 0o000);
        set_mode(&dir.join("closed"), 0o000);
        set_mode(&dir.join("read-only"), 0o500);
        set_mode(&dir, 0o500);
        wipe(&dir);
        assert!(!dir.exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_link_out_of_a_scratch_directory_is_never_followed_to_give_rights() {
        use std::os::unix::fs::PermissionsExt;
        let parent = tempfile::tempdir().expect("a directory");
        let outside = parent.path().join("outside");
        fs::create_dir(&outside).expect("a directory");
        fs::write(outside.join("kept"), b"x").expect("a file");
        let dir = parent.path().join("scratch");
        fs::create_dir_all(dir.join("closed")).expect("directories");
        std::os::unix::fs::symlink(&outside, dir.join("link")).expect("a link");
        set_mode(&outside, 0o500);
        set_mode(&dir.join("closed"), 0o000);
        wipe(&dir);
        assert!(!dir.exists());
        let mode = fs::metadata(&outside)
            .expect("still there")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o500);
        assert!(outside.join("kept").exists());
        set_mode(&outside, 0o700);
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
