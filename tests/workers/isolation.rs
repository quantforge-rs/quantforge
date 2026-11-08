//! What a job can reach: its bundle in a scratch directory of its own,
//! which goes with its worker; nothing of another job's scratch directory,
//! of other files or of the network, where the worker enforces it; no
//! environment or descriptor of the server's; and nothing an earlier job
//! left, since every job gets a fresh worker. And the worker command
//! itself: a server of another build refused, and an end of its input
//! before any job ending it.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use quantforge::server::{Cancel, Guard, WORKER_COMMAND};
use serde_json::{Value, json};

use crate::support::{Pool, candles, given_env, input, report, roomy, until, worker_env};

/// The guard's reason, when it does not hold.
fn unenforced(guard: &Guard) -> Option<&str> {
    match guard {
        Guard::Enforced { .. } => None,
        Guard::Unenforced { why } => Some(why),
    }
}

#[test]
fn a_job_sees_its_bundle_alone_in_a_scratch_directory_that_goes_with_it() {
    let pool = Pool::start(1, roomy());
    let ran = pool.run(input(r#"workload = "inspect""#, candles(5)));
    let seen = report(&ran);
    assert_eq!(
        seen["scratch"],
        json!(["bundle/quantforge.toml", "bundle/strategy.py"])
    );
    let cwd = PathBuf::from(seen["cwd"].as_str().expect("a working directory"));
    let root = std::fs::canonicalize(pool.pool.scratch_root()).expect("the scratch root");
    assert!(
        cwd.starts_with(&root),
        "{} is not in {}",
        cwd.display(),
        root.display()
    );
    assert!(!cwd.exists(), "{} outlived its worker", cwd.display());
}

#[test]
fn a_worker_gets_no_environment_and_no_descriptor_of_the_server() {
    let pool = Pool::start(1, roomy());
    let seen = report(&pool.run(input(r#"workload = "inspect""#, candles(5))));
    assert_eq!(given_env(&seen), worker_env());
    if cfg!(unix) {
        assert_eq!(seen["descriptors"], json!([0, 1, 2]));
    }
}

#[test]
fn every_job_gets_a_fresh_worker_that_holds_nothing_of_the_last() {
    let pool = Pool::start(1, roomy());
    let planted = report(&pool.run(input(r#"workload = "plant""#, candles(5))));
    assert_eq!(planted["written"], true);
    assert_eq!(planted["seen"]["planted"], true);
    assert_eq!(planted["seen"]["planted_file"], true);

    let next = report(&pool.run(input(r#"workload = "inspect""#, candles(5))));
    assert_eq!(next["planted"], false, "{next}");
    assert_eq!(next["planted_file"], false, "{next}");
    assert_ne!(next["pid"], planted["seen"]["pid"]);
    assert_ne!(next["cwd"], planted["seen"]["cwd"]);
}

#[test]
fn a_job_reads_neither_another_jobs_scratch_directory_nor_any_file_outside_its_own() {
    let pool = Pool::start(2, roomy());
    if let Some(why) = unenforced(&pool.pool.posture().files) {
        assert!(
            std::env::consts::OS != "linux" || why == "this kernel has no Landlock",
            "{why}"
        );
        eprintln!("skipping: the workers enforce no file guard here: {why}");
        return;
    }
    let held = input(r#"workload = "hold""#, candles(5));
    let job = held.job_id.to_string();
    let cancel = Cancel::new();
    std::thread::scope(|scope| {
        let holding = scope.spawn(|| pool.pool.execute(held, &cancel));
        pool.worker_of(&job);
        let other = until("the held job's file", || {
            held_file(pool.pool.scratch_root(), &job)
        });

        let read = |path: &str| -> Value {
            let ran = pool.run(input(
                &format!("workload = \"read\"\npath = {}", toml_string(path)),
                candles(1),
            ));
            report(&ran)
        };
        let own = read("bundle/quantforge.toml");
        assert_eq!(own["allowed"], true, "{own}");
        for path in [
            other.to_string_lossy().into_owned(),
            pool.pool.scratch_root().to_string_lossy().into_owned(),
            "/etc/passwd".to_string(),
        ] {
            let refused = read(&path);
            assert_eq!(refused["allowed"], false, "{refused}");
            assert!(
                refused["outcome"]
                    .as_str()
                    .is_some_and(|outcome| outcome.starts_with("refused: ")),
                "{refused}"
            );
        }
        cancel.cancel();
        holding.join().expect("the held job's run");
    });
}

/// The `held` file of the running job `job`, in its scratch directory.
fn held_file(root: &Path, job: &str) -> Option<PathBuf> {
    std::fs::read_dir(root)
        .ok()?
        .flatten()
        .map(|entry| entry.path().join("held"))
        .find(|held| std::fs::read_to_string(held).is_ok_and(|text| text == job))
}

fn toml_string(text: &str) -> String {
    serde_json::to_string(text).expect("a string")
}

#[test]
fn a_job_reaches_no_network() {
    let pool = Pool::start(1, roomy());
    if let Some(why) = unenforced(&pool.pool.posture().network) {
        assert_ne!(std::env::consts::OS, "linux", "{why}");
        eprintln!("skipping: the workers enforce no network guard here: {why}");
        return;
    }
    let listener = TcpListener::bind("127.0.0.1:0").expect("a listener");
    let address = listener.local_addr().expect("its address").to_string();
    let reached = report(&pool.run(input(
        &format!(
            "workload = \"connect\"\naddress = {}",
            toml_string(&address)
        ),
        candles(1),
    )));
    assert_eq!(reached["allowed"], false, "{reached}");
    assert_eq!(
        reached["outcome"], "refused: Permission denied (os error 13)",
        "{reached}"
    );

    // A local service on a Unix socket, as a database may listen: Landlock
    // leaves a connection to one open, and only the refused socket stops it.
    #[cfg(unix)]
    {
        let dir = tempfile::tempdir().expect("a directory");
        let path = dir.path().join("service.sock");
        let _listener = std::os::unix::net::UnixListener::bind(&path).expect("a listener");
        let reached = report(&pool.run(input(
            &format!(
                "workload = \"connect_unix\"\npath = {}",
                toml_string(&path.to_string_lossy())
            ),
            candles(1),
        )));
        assert_eq!(reached["allowed"], false, "{reached}");
        assert_eq!(
            reached["outcome"], "refused: Permission denied (os error 13)",
            "{reached}"
        );
    }
}

#[cfg(unix)]
#[test]
fn a_program_a_job_starts_goes_with_its_worker_or_never_starts() {
    let pool = Pool::start(1, roomy());
    let started = report(&pool.run(input(
        "workload = \"spawn\"\nprogram = \"/bin/sleep\"\nargs = [\"3600\"]",
        candles(1),
    )));
    if pool.pool.posture().files.is_enforced() {
        // Landlock refuses to run anything outside the scratch directory.
        assert_eq!(started["allowed"], false, "{started}");
        return;
    }
    assert_eq!(started["allowed"], true, "{started}");
    let child = u32::try_from(started["pid"].as_u64().expect("a process")).expect("a pid");
    until("the job's own process to go with its worker", || {
        (!crate::support::alive(child)).then_some(())
    });
}

/// A frame as a server writes it: its length, then its JSON.
fn frame(json: &Value) -> Vec<u8> {
    let body = json.to_string().into_bytes();
    let mut bytes = u32::try_from(body.len())
        .expect("a length")
        .to_be_bytes()
        .to_vec();
    bytes.extend(body);
    bytes
}

fn worker() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_quantforge"));
    command
        .arg(WORKER_COMMAND)
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

#[test]
fn a_worker_refuses_a_server_of_another_build() {
    let mut child = worker().spawn().expect("a worker");
    let mut stdin = child.stdin.take().expect("its input");
    stdin
        .write_all(&frame(&json!({
            "type": "setup",
            "build": "quantforge 0.0.0 frames 0",
            "mode": {"kind": "no_runtime"},
            "cpu_secs": 60,
            "memory_bytes": 1_u64 << 31,
        })))
        .expect("the setup");
    drop(stdin);
    let output = child.wait_with_output().expect("its end");
    assert!(!output.status.success());
    let said = String::from_utf8_lossy(&output.stderr);
    assert!(
        said.contains(&format!(
            "the server is quantforge 0.0.0 frames 0, and this worker quantforge {} frames 1; a \
             worker runs only for a server of its own build",
            env!("CARGO_PKG_VERSION")
        )),
        "{said}"
    );
    assert!(
        output.stdout.is_empty(),
        "a refusing worker said it was ready"
    );
}

#[test]
fn a_worker_whose_server_goes_away_mid_job_exits() {
    let mut child = worker().spawn().expect("a worker");
    let mut stdin = child.stdin.take().expect("its input");
    let mut stdout = child.stdout.take().expect("its output");
    stdin
        .write_all(&frame(&json!({
            "type": "setup",
            "build": format!("quantforge {} frames 1", env!("CARGO_PKG_VERSION")),
            "mode": {"kind": "synthetic", "runtime": {"engine": "0.3.0", "python": "3.12", "packages": {}}},
            "cpu_secs": 60,
            "memory_bytes": 1_u64 << 31,
        })))
        .expect("the setup");
    let ready = read_frame(&mut stdout);
    assert_eq!(ready["type"], "ready", "{ready}");
    let job = input(r#"workload = "sleep""#, candles(1));
    stdin
        .write_all(&frame(&json!({
            "type": "job",
            "job_id": job.job_id.to_string(),
            "submission": job.submission,
            "entry": "strategy.py:Hold",
            "params": {"workload": "sleep"},
            "bars": 0,
        })))
        .expect("the job");
    // The job sleeps for an hour; the end of its input ends it at once.
    std::thread::sleep(Duration::from_millis(200));
    drop(stdin);
    let status = exited(&mut child);
    assert!(status.success(), "{status}");
}

/// The next frame a worker wrote.
fn read_frame(stdout: &mut impl Read) -> Value {
    let mut length = [0_u8; 4];
    stdout.read_exact(&mut length).expect("a frame's length");
    let mut body = vec![0; usize::try_from(u32::from_be_bytes(length)).expect("a length")];
    stdout.read_exact(&mut body).expect("a frame");
    serde_json::from_slice(&body).expect("a frame's JSON")
}

/// How the worker exited, within ten seconds.
fn exited(child: &mut std::process::Child) -> std::process::ExitStatus {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = child.try_wait().expect("its status") {
            return status;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the worker outlived its input"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_worker_given_no_job_exits_once_its_input_ends() {
    let mut child = worker().spawn().expect("a worker");
    let mut stdin = child.stdin.take().expect("its input");
    let mut stdout = child.stdout.take().expect("its output");
    stdin
        .write_all(&frame(&json!({
            "type": "setup",
            "build": format!("quantforge {} frames 1", env!("CARGO_PKG_VERSION")),
            "mode": {"kind": "no_runtime"},
            "cpu_secs": 60,
            "memory_bytes": 1_u64 << 31,
        })))
        .expect("the setup");
    let ready = read_frame(&mut stdout);
    assert_eq!(ready["type"], "ready", "{ready}");
    assert_eq!(ready["runtime"], Value::Null, "{ready}");
    drop(stdin);
    let status = exited(&mut child);
    assert!(status.success(), "{status}");
}
