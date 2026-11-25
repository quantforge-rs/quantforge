//! What a job that means harm is held to, by workloads that try it: no
//! file outside its scratch directory, by any path, link or second name;
//! no process but itself, to signal, to take the limits of, or to start;
//! and no bundle written through a link planted where it goes. Each guard
//! is tested where the workers say it holds, and nowhere else: a run that
//! was told the sandbox has to hold fails where it does not.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::support::{Pool, SANDBOX_REQUIRED, candles, holds, input, report, roomy, toml_string};

/// A directory outside every scratch directory, holding a canary.
fn outside() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("a directory");
    let canary = dir.path().join("canary.txt");
    std::fs::write(&canary, b"kept").expect("a canary");
    (dir, canary)
}

/// The names in `dir`, in order.
fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("a directory")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn nothing_a_job_writes_lands_outside_its_scratch_directory() {
    let (outside, canary) = outside();
    let pool = Pool::start(1, roomy());
    let params = format!(
        "workload = \"escape\"\noutside = {}",
        toml_string(&outside.path().to_string_lossy())
    );
    let tried = report(&pool.run(input(&params, candles(1))));
    let attempts = tried["attempts"].as_array().expect("the attempts");
    assert!(attempts.len() >= 3, "{tried}");
    if !holds(&pool.pool.posture().files, "file guard") {
        return;
    }
    for attempt in attempts {
        assert_eq!(attempt["allowed"], false, "{attempt}");
        assert!(
            attempt["outcome"]
                .as_str()
                .is_some_and(|outcome| outcome.starts_with("refused: ")),
            "{attempt}"
        );
    }
    assert_eq!(std::fs::read(&canary).expect("the canary"), b"kept");
    assert_eq!(names(outside.path()), ["canary.txt"]);
    // A path that climbs out of a scratch directory lands in the pool's
    // own, which holds its workers' directories and nothing else.
    for entry in std::fs::read_dir(pool.pool.scratch_root())
        .expect("the pool's directory")
        .flatten()
    {
        assert!(
            entry.path().is_dir(),
            "{} was written beside the scratch directories",
            entry.path().display()
        );
    }
}

#[cfg(unix)]
#[test]
fn a_job_reaches_no_process_but_itself_and_starts_none() {
    use serde_json::json;

    let pool = Pool::start(1, roomy());
    let network = pool.pool.posture().network.clone();
    let reached = report(&pool.run(input(r#"workload = "others""#, candles(1))));
    // Itself it may always signal, and a thread of its own it may start:
    // a filter that refused either would stop a worker's own runtime.
    assert_eq!(reached["signal_itself"], "allowed", "{reached}");
    assert_eq!(reached["thread"], "allowed", "{reached}");
    if !network.is_enforced() {
        assert_ne!(std::env::consts::OS, "linux", "{network:?}");
    }
    if !holds(&network, "seccomp filter") {
        return;
    }
    let refused = "refused: Permission denied (os error 13)";
    assert_eq!(
        reached,
        json!({
            "signal_itself": "allowed",
            "signal_server": refused,
            "limit_server": refused,
            "thread": "allowed",
            "process": refused,
            "socket_pair": refused,
        })
    );
}

#[cfg(unix)]
#[test]
fn a_link_planted_where_a_bundle_goes_takes_no_file_out_of_the_scratch_directory() {
    use quantforge::protocol::ErrorCode;
    use serde_json::{Value, json};

    use crate::support::{failure, until};

    let (outside, _canary) = outside();
    let pool = Pool::start(1, roomy());
    // The one worker the pool keeps ready waits in its scratch directory,
    // where its job's bundle is written as the job starts.
    let scratch = until("the ready worker's scratch directory", || {
        std::fs::read_dir(pool.pool.scratch_root())
            .ok()?
            .flatten()
            .map(|entry| entry.path())
            .next()
    });
    std::os::unix::fs::symlink(outside.path(), scratch.join("bundle")).expect("a link");
    let ran = pool.run(input(r#"workload = "inspect""#, candles(1)));
    assert_eq!(
        failure(&ran),
        (
            ErrorCode::Internal,
            "the server could not hand the job's bundle to its worker".to_string()
        )
    );
    assert_eq!(names(outside.path()), ["canary.txt"]);
    // The next job has a worker and a scratch directory of its own.
    let next: Value = report(&pool.run(input(r#"workload = "inspect""#, candles(1))));
    assert_eq!(
        next["scratch"],
        json!(["bundle/quantforge.toml", "bundle/strategy.py"])
    );
}

/// Where the sandbox has to hold, a test of a guard the workers do not
/// enforce fails, and anywhere else it skips: the test binary runs one of
/// its tests again, as a child, told each in turn.
#[test]
fn a_test_of_a_guard_that_does_not_hold_fails_only_where_the_sandbox_has_to() {
    const TEST: &str = "hostile::nothing_a_job_writes_lands_outside_its_scratch_directory";
    let run = |required: bool| {
        let mut child = Command::new(std::env::current_exe().expect("the test binary"));
        child
            .args([TEST, "--exact", "--nocapture"])
            .env_remove(SANDBOX_REQUIRED);
        if required {
            child.env(SANDBOX_REQUIRED, "1");
        }
        let output = child.output().expect("the test binary runs");
        let text = String::from_utf8_lossy(&output.stdout).into_owned()
            + &String::from_utf8_lossy(&output.stderr);
        (output.status.success(), text)
    };
    let guarded = Pool::start(1, roomy()).pool.posture().files.is_enforced();

    let (passed, text) = run(false);
    assert!(passed && text.contains("1 passed"), "{text}");
    assert_eq!(
        text.contains("skipping: the workers enforce no file guard here"),
        !guarded,
        "{text}"
    );

    let (passed, text) = run(true);
    if guarded {
        assert!(passed && text.contains("1 passed"), "{text}");
    } else {
        assert!(!passed && text.contains("1 failed"), "{text}");
        assert!(
            text.contains(&format!("{SANDBOX_REQUIRED} says the sandbox has to hold")),
            "{text}"
        );
    }
}
