//! Each limit run into and named: CPU time and memory where the worker can
//! enforce them, wall-clock time everywhere; a crash, an outside kill and a
//! cancel, each of which ends only its own job and leaves the pool running
//! the next; and a pool that stops as a job's worker starts, which runs
//! nothing more.

use std::time::{Duration, Instant};

use quantforge::protocol::ErrorCode;
use quantforge::server::{Cancel, WorkerLimits, WorkerOutcome};

use crate::support::{Pool, candles, digest, failure, input, kill, report, roomy, unread};

/// How long a job that runs into a limit may take beyond it to end, however
/// busy the machine.
const SLACK: Duration = Duration::from_secs(20);

#[test]
fn a_job_that_uses_up_its_cpu_time_is_stopped_and_named() {
    let pool = Pool::start(
        1,
        WorkerLimits {
            cpu_secs: 1,
            wall_secs: 30,
            ..roomy()
        },
    );
    if !pool.pool.posture().cpu.is_enforced() {
        assert_eq!(std::env::consts::OS, "windows", "{:?}", pool.pool.posture());
        return;
    }
    let began = Instant::now();
    let ran = pool.run(input(r#"workload = "spin""#, candles(10)));
    assert_eq!(
        failure(&ran),
        (
            ErrorCode::InvalidRequest,
            "the job used the 1 s of CPU time it may use, and was stopped".to_string()
        )
    );
    assert!(began.elapsed() < SLACK, "{:?}", began.elapsed());
}

#[test]
fn a_job_that_takes_more_memory_than_it_may_is_stopped_and_named() {
    let pool = Pool::start(
        1,
        WorkerLimits {
            memory_mib: 512,
            ..roomy()
        },
    );
    if !pool.pool.posture().memory.is_enforced() {
        // A hog with no limit would take the machine's memory instead.
        assert_ne!(std::env::consts::OS, "linux", "{:?}", pool.pool.posture());
        return;
    }
    let ran = pool.run(input(
        "workload = \"allocate\"\nchunk_mib = 32\nmost_mib = 2048",
        candles(10),
    ));
    assert_eq!(
        failure(&ran),
        (
            ErrorCode::InvalidRequest,
            "the job needed more than the 512 MiB of memory it may use, and was stopped"
                .to_string()
        )
    );
}

#[test]
fn a_job_past_its_wall_clock_time_is_killed_and_named() {
    let pool = Pool::start(
        1,
        WorkerLimits {
            wall_secs: 1,
            ..roomy()
        },
    );
    let began = Instant::now();
    let ran = pool.run(input(r#"workload = "sleep""#, candles(10)));
    let took = began.elapsed();
    assert_eq!(
        failure(&ran),
        (
            ErrorCode::InvalidRequest,
            "the job ran for the 1 s of wall-clock time it may run, and was stopped".to_string()
        )
    );
    assert!(took >= Duration::from_secs(1) && took < SLACK, "{took:?}");
    assert!(pool.pool.running().is_empty());
}

#[test]
fn a_wall_clock_limit_longer_than_any_clock_counts_lets_a_job_finish() {
    // No instant lies that far ahead: the limit is measured, never added.
    let pool = Pool::start(
        1,
        WorkerLimits {
            wall_secs: u64::MAX,
            ..roomy()
        },
    );
    let ran = pool.run(input(r#"workload = "report""#, candles(10)));
    assert_eq!(report(&ran)["bars"], 10);
    assert!(pool.pool.running().is_empty());
}

#[test]
fn a_job_that_crashes_its_worker_fails_and_the_next_job_runs() {
    let pool = Pool::start(1, roomy());
    let ran = pool.run(input(r#"workload = "crash""#, candles(10)));
    let (code, message) = failure(&ran);
    assert_eq!(code, ErrorCode::Internal);
    #[cfg(unix)]
    assert_eq!(
        message,
        "the job's worker stopped before the job finished: it was stopped by signal 6 (SIGABRT)"
    );
    #[cfg(windows)]
    assert!(
        message
            .starts_with("the job's worker stopped before the job finished: it exited with status"),
        "{message}"
    );

    let exited = pool.run(input("workload = \"exit\"\ncode = 3", candles(10)));
    assert_eq!(
        failure(&exited),
        (
            ErrorCode::Internal,
            "the job's worker stopped before the job finished: it exited with status 3".to_string()
        )
    );

    let next = candles(100);
    let ran = pool.run(input(r#"workload = "report""#, next.clone()));
    assert_eq!(report(&ran)["digest"], digest(&next));
}

#[test]
fn a_worker_killed_from_outside_fails_only_its_own_job() {
    let pool = Pool::start(2, roomy());
    let held = input(r#"workload = "sleep""#, candles(10));
    let held_job = held.job_id.to_string();
    let busy = candles(30_000);
    std::thread::scope(|scope| {
        let killed = scope.spawn(|| pool.run(held));
        let other = scope.spawn(|| pool.run(input(r#"workload = "report""#, busy.clone())));
        kill(pool.worker_of(&held_job));
        let killed = killed.join().expect("the killed job's run");
        let (code, message) = failure(&killed);
        assert_eq!(code, ErrorCode::Internal);
        #[cfg(unix)]
        assert_eq!(
            message,
            "the job's worker stopped before the job finished: it was stopped by signal 9 (SIGKILL)"
        );
        #[cfg(windows)]
        assert!(message.contains("it exited with status"), "{message}");
        let other = other.join().expect("the other job's run");
        assert_eq!(report(&other)["digest"], digest(&busy));
    });
    let after = candles(10);
    let ran = pool.run(input(r#"workload = "report""#, after.clone()));
    assert_eq!(report(&ran)["digest"], digest(&after));
}

#[test]
fn a_cancel_kills_the_worker_and_ends_the_run_at_once() {
    let pool = Pool::start(1, roomy());
    let held = input(r#"workload = "sleep""#, candles(10));
    let job = held.job_id.to_string();
    let cancel = Cancel::new();
    std::thread::scope(|scope| {
        let run = scope.spawn(|| pool.pool.execute(held, &cancel, &unread()));
        let pid = pool.worker_of(&job);
        let sent = Instant::now();
        cancel.cancel();
        let ran = run.join().expect("the run");
        assert_eq!(ran.outcome, WorkerOutcome::Cancelled);
        assert!(
            sent.elapsed() < Duration::from_secs(5),
            "{:?}",
            sent.elapsed()
        );
        assert_eq!(ran.pid, Some(pid));
        #[cfg(unix)]
        assert!(
            !crate::support::alive(pid),
            "worker {pid} outlived its cancel"
        );
    });
    assert!(pool.pool.running().is_empty());
}

#[test]
fn a_job_that_fails_by_itself_keeps_its_own_reason() {
    let pool = Pool::start(1, roomy());
    let ran = pool.run(input(
        "workload = \"fail\"\nmessage = \"the strategy refused these candles\"",
        candles(10),
    ));
    assert_eq!(
        failure(&ran),
        (
            ErrorCode::InvalidRequest,
            "the strategy refused these candles".to_string()
        )
    );
    let unknown = pool.run(input(r#"workload = "nothing-like-it""#, candles(1)));
    assert_eq!(
        failure(&unknown),
        (
            ErrorCode::InvalidRequest,
            r#"no synthetic workload is named Some("nothing-like-it")"#.to_string()
        )
    );
}

#[test]
fn a_worker_that_floods_its_standard_error_holds_nothing_up() {
    let pool = Pool::start(1, roomy());
    let ran = pool.run(input("workload = \"chatter\"\nkib = 16384", candles(10)));
    assert_eq!(report(&ran)["written"], true);
}

#[cfg(unix)]
#[test]
fn a_worker_that_starts_for_a_job_as_its_pool_stops_runs_nothing() {
    use quantforge::server::{PoolConfig, WorkerMode, WorkerPool, WorkerProgram};

    use crate::support::{runtime, until};

    const STOPPING: &str = "the server is stopping; submit the job again";
    // Workers that start only once `go` exists, and that leave `go.waiting`
    // when they find it missing, so that a job's own worker can be held on
    // its way while the pool begins to stop.
    const HELD: &str = r#"
        [ -e "$0" ] || : > "$0.waiting"
        while [ ! -e "$0" ]; do /bin/sleep 0.05; done
        exec "$1" worker
    "#;
    let dir = tempfile::tempdir().expect("a directory");
    let go = dir.path().join("go");
    let waiting = dir.path().join("go.waiting");
    std::fs::write(&go, b"").expect("go");
    let parent = tempfile::tempdir().expect("a directory");
    let pool = WorkerPool::start(PoolConfig {
        program: WorkerProgram {
            path: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                HELD.into(),
                go.clone().into_os_string(),
                env!("CARGO_BIN_EXE_quantforge").into(),
            ],
        },
        size: 1,
        limits: roomy(),
        mode: WorkerMode::Synthetic { runtime: runtime() },
        scratch_parent: parent.path().to_path_buf(),
    })
    .expect("a started pool");
    std::fs::remove_file(&go).expect("workers held from now on");
    let held = input(r#"workload = "sleep""#, candles(1));
    let held_job = held.job_id.to_string();
    let cancel = Cancel::new();
    let (first, second, third) = std::thread::scope(|scope| {
        // The first job takes the only worker, so the second starts one of
        // its own, which waits.
        let first = scope.spawn(|| pool.execute(held, &cancel, &unread()));
        until("the first job to run", || {
            let running = pool.running();
            running
                .iter()
                .any(|job| job.job_id == held_job)
                .then_some(())
        });
        let second = scope.spawn(|| {
            pool.execute(
                input(r#"workload = "report""#, candles(1)),
                &Cancel::new(),
                &unread(),
            )
        });
        until("the second job's worker to wait", || {
            waiting.exists().then_some(())
        });
        let stopped = scope.spawn(|| pool.stop());
        // A third job is answered only once the pool is stopping.
        let third = pool.execute(
            input(r#"workload = "report""#, candles(1)),
            &Cancel::new(),
            &unread(),
        );
        std::fs::write(&go, b"").expect("go");
        let second = second.join().expect("the second job's run");
        cancel.cancel();
        let first = first.join().expect("the first job's run");
        stopped.join().expect("the stop");
        (first, second, third)
    });
    assert_eq!(
        failure(&third),
        (ErrorCode::Unavailable, STOPPING.to_string())
    );
    assert_eq!(
        failure(&second),
        (ErrorCode::Unavailable, STOPPING.to_string())
    );
    assert_eq!(first.outcome, WorkerOutcome::Cancelled);
    assert!(pool.running().is_empty());
    assert!(!pool.scratch_root().exists());
}
