//! A canonical tar is an ordinary archive: the system's tar lists it without
//! a warning, though it ends right after its two zero blocks. CI has GNU tar
//! on Linux and bsdtar on macOS and Windows.

use std::io::Write;
use std::process::{Command, Stdio};

use quantforge::protocol::bundle::{Bundle, BundleLimits};

use crate::fixture;

#[test]
fn the_system_tar_lists_a_canonical_tar_without_complaint() {
    let bundle = Bundle::from_files(
        fixture::case("package with params and deps").files(),
        &BundleLimits::default(),
    )
    .expect("a bundle");
    let spawned = Command::new("tar")
        .args(["-tf", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let Ok(mut tar) = spawned else {
        eprintln!("skipped: no tar on PATH");
        return;
    };
    tar.stdin
        .take()
        .expect("tar's stdin")
        .write_all(&bundle.canonical_tar())
        .expect("write the archive to tar");
    let output = tar.wait_with_output().expect("tar runs");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && stderr.trim().is_empty(),
        "tar exited {} with: {stderr}",
        output.status
    );
    let listed: Vec<&str> = std::str::from_utf8(&output.stdout)
        .expect("tar lists UTF-8 names")
        .lines()
        .collect();
    let paths: Vec<&str> = bundle
        .files()
        .iter()
        .map(|file| file.path().as_str())
        .collect();
    assert_eq!(listed, paths);
}
