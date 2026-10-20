//! The same source hashes the same on any machine. A tree written in another
//! order, with other timestamps and modes, and carrying the junk a checkout
//! does, makes the same canonical tar, whose hash is the one an independent
//! Python implementation computed for the shared case. CI runs this on
//! Linux, macOS, and Windows.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use quantforge::protocol::bundle::{Bundle, BundleLimits};

use crate::fixture;

const CASE: &str = "package with params and deps";

fn place(root: &Path, relative: &str) -> PathBuf {
    relative
        .split('/')
        .fold(root.to_path_buf(), |path, part| path.join(part))
}

fn write(root: &Path, relative: &str, contents: &[u8]) {
    let path = place(root, relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create the directory");
    }
    fs::write(path, contents).expect("write the file");
}

/// Sets a file's modification time. Windows needs write access to change it.
fn touch(root: &Path, relative: &str, seconds: u64) {
    let file = fs::File::options()
        .write(true)
        .open(place(root, relative))
        .expect("open the file");
    file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(seconds))
        .expect("set the modification time");
}

fn collect(root: &Path) -> Bundle {
    Bundle::from_dir(root, &BundleLimits::default()).expect("a bundle")
}

#[test]
fn a_tree_hashes_the_same_whatever_its_order_times_and_junk() {
    let case = fixture::case(CASE);
    let files = case.files();
    let first = tempfile::tempdir().expect("a temporary directory");
    for (path, contents) in &files {
        write(first.path(), path, contents);
    }
    let second = tempfile::tempdir().expect("a temporary directory");
    for (index, (path, contents)) in files.iter().rev().enumerate() {
        write(second.path(), path, contents);
        let day = u64::try_from(index).expect("a small index") * 86_400;
        touch(second.path(), path, 1_000_000_000 + day);
    }
    for junk in [
        ".git/HEAD",
        ".DS_Store",
        ".venv/lib/site.py",
        "__pycache__/strategy.cpython-312.pyc",
        "pkg/__pycache__/breakout.cpython-312.pyc",
        "pkg/.ipynb_checkpoints/breakout.py",
    ] {
        write(second.path(), junk, b"junk");
    }
    let (first, second) = (collect(first.path()), collect(second.path()));
    assert_eq!(first.canonical_tar(), second.canonical_tar());
    assert_eq!(first.code_hash(), second.code_hash());
    assert_eq!(Some(first.code_hash().to_string()), case.code_hash);
}

#[test]
fn a_bundle_written_out_and_collected_again_is_the_same_bundle() {
    let bundle = Bundle::from_files(fixture::case(CASE).files(), &BundleLimits::default())
        .expect("a bundle");
    let root = tempfile::tempdir().expect("a temporary directory");
    for file in bundle.files() {
        write(root.path(), file.path().as_str(), file.contents());
    }
    assert_eq!(collect(root.path()), bundle);
}

#[cfg(unix)]
mod unix {
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::os::unix::net::UnixListener;

    use quantforge::protocol::bundle::ProblemCode;

    use super::*;

    fn codes_and_messages(root: &Path) -> Vec<(ProblemCode, String)> {
        let error = Bundle::from_dir(root, &BundleLimits::default()).expect_err("refused");
        error
            .problems()
            .iter()
            .map(|problem| (problem.code(), problem.message().to_string()))
            .collect()
    }

    #[test]
    fn modes_do_not_change_the_hash() {
        let files = fixture::case(CASE).files();
        let mut hashes = Vec::new();
        for mode in [0o600, 0o755] {
            let root = tempfile::tempdir().expect("a temporary directory");
            for (path, contents) in &files {
                write(root.path(), path, contents);
                fs::set_permissions(place(root.path(), path), fs::Permissions::from_mode(mode))
                    .expect("set the mode");
            }
            hashes.push(collect(root.path()).code_hash());
        }
        assert_eq!(hashes[0], hashes[1]);
    }

    #[test]
    fn a_symlink_is_refused_and_never_followed() {
        let root = tempfile::tempdir().expect("a temporary directory");
        for (path, contents) in fixture::case("minimal").files() {
            write(root.path(), &path, &contents);
        }
        let outside = tempfile::tempdir().expect("a temporary directory");
        write(outside.path(), "secret.py", b"key = 1\n");
        symlink(
            outside.path().join("secret.py"),
            root.path().join("linked.py"),
        )
        .expect("link a file");
        symlink(outside.path(), root.path().join("linked_dir")).expect("link a directory");
        let found = codes_and_messages(root.path());
        assert_eq!(
            found,
            [
                (
                    ProblemCode::NotARegularFile,
                    "\"linked.py\" is a symlink or junction; a bundle holds regular files only, \
                     so copy the file in or leave it out"
                        .to_string()
                ),
                (
                    ProblemCode::NotARegularFile,
                    "\"linked_dir\" is a symlink or junction; a bundle holds regular files only, \
                     so copy the file in or leave it out"
                        .to_string()
                ),
            ]
        );
    }

    #[test]
    fn a_socket_is_refused() {
        let root = tempfile::tempdir().expect("a temporary directory");
        for (path, contents) in fixture::case("minimal").files() {
            write(root.path(), &path, &contents);
        }
        let _listener = UnixListener::bind(root.path().join("s.sock")).expect("bind a socket");
        let found = codes_and_messages(root.path());
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].0, ProblemCode::NotARegularFile);
        assert!(
            found[0].1.starts_with("\"s.sock\" is not a regular file"),
            "{found:?}"
        );
    }
}
