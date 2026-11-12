//! What every test here builds: a pool of real workers in the synthetic
//! mode, bundles whose parameters name a workload, the jobs that run them,
//! and what they answer.

use std::time::Duration;

use quantforge::protocol::bundle::{Bundle, BundleLimits, Runtime};
use quantforge::protocol::jobs::{self, JobSubmission};
use quantforge::protocol::{ErrorBody, ErrorCode};
use quantforge::server::{
    Cancel, Execution, JobInput, JobLog, LogLimits, PoolConfig, WORKER_COMMAND, WorkerLimits,
    WorkerMode, WorkerOutcome, WorkerPool, WorkerProgram,
};
use quantforge::{Candle, JobId, TimestampMs};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

/// 2026-10-20T00:00:00Z.
pub(crate) const T: TimestampMs = 1_792_454_400_000;
pub(crate) const MINUTE: TimestampMs = 60_000;

/// The binary under test, as a worker.
pub(crate) fn program() -> WorkerProgram {
    WorkerProgram {
        path: env!("CARGO_BIN_EXE_quantforge").into(),
        args: vec![WORKER_COMMAND.into()],
    }
}

/// The runtime the synthetic workers say they offer, which admits the
/// tests' bundles.
pub(crate) fn runtime() -> Runtime {
    serde_json::from_str(r#"{"engine":"0.3.0","python":"3.12","packages":{}}"#).expect("a runtime")
}

/// Limits a job runs into only when it means to.
pub(crate) fn roomy() -> WorkerLimits {
    WorkerLimits {
        cpu_secs: 120,
        memory_mib: 2048,
        wall_secs: 120,
    }
}

/// A started pool of workers in the synthetic mode, its scratch directories
/// in a directory of the test's own, stopped when dropped.
pub(crate) struct Pool {
    pub(crate) pool: WorkerPool,
    _parent: TempDir,
}

impl Pool {
    pub(crate) fn start(size: usize, limits: WorkerLimits) -> Self {
        let parent = tempfile::tempdir().expect("a directory");
        let pool = WorkerPool::start(PoolConfig {
            program: program(),
            size,
            limits,
            mode: WorkerMode::Synthetic { runtime: runtime() },
            scratch_parent: parent.path().to_path_buf(),
        })
        .expect("a started pool");
        Self {
            pool,
            _parent: parent,
        }
    }

    /// Runs `input` to its end.
    pub(crate) fn run(&self, input: JobInput) -> Execution {
        self.pool.execute(input, &Cancel::new(), &unread())
    }

    /// Runs `input` to its end, and answers what it wrote to a log that
    /// keeps `limits` of it.
    pub(crate) fn run_logged(&self, input: JobInput, limits: LogLimits) -> (Execution, JobLog) {
        let log = JobLog::new(limits);
        let execution = self.pool.execute(input, &Cancel::new(), &log);
        (execution, log)
    }

    /// The job's worker, once it runs one, waiting up to ten seconds.
    pub(crate) fn worker_of(&self, job: &str) -> u32 {
        until(&format!("a worker running {job}"), || {
            self.pool
                .running()
                .into_iter()
                .find(|running| running.job_id == job)
                .map(|running| running.pid)
        })
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        self.pool.stop();
    }
}

/// A log for a job whose log no test reads.
pub(crate) fn unread() -> JobLog {
    JobLog::new(LogLimits::default())
}

/// `check`'s answer once it has one, or a failed test after ten seconds.
pub(crate) fn until<T>(what: &str, mut check: impl FnMut() -> Option<T>) -> T {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(found) = check() {
            return found;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no {what} within ten seconds"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// A bundle whose strategy is the synthetic workload `params`, a TOML
/// table body such as `workload = "spin"`, names.
pub(crate) fn bundle(params: &str) -> Bundle {
    let manifest = format!(
        "format = 1\nentry = \"strategy.py:Hold\"\nengine = \">=0.3.0, <0.4.0\"\n\
         python = \"3.12\"\n\n[params]\n{params}\n"
    );
    Bundle::from_files(
        [
            ("quantforge.toml", manifest.as_str()),
            ("strategy.py", "class Hold:\n    pass\n"),
        ],
        &BundleLimits::default(),
    )
    .expect("a bundle")
}

/// `count` `1m` candles from `T`, their decimals of several scales.
pub(crate) fn candles(count: i64) -> Vec<Candle> {
    (0..count)
        .map(|i| {
            let close = Decimal::new(100_000 + i, u32::try_from(i % 4).expect("a scale"));
            Candle {
                open_time_ms: T + i * MINUTE,
                close_time_ms: T + (i + 1) * MINUTE - 1,
                open: close,
                high: close,
                low: close,
                close,
                volume: Decimal::new(50_000_000, 8),
                trades: (i % 3 != 0).then(|| u64::try_from(i).expect("a count")),
            }
        })
        .collect()
}

/// The SHA-256 of the candles as text, one line each, every decimal with
/// its scale, as the `report` workload takes it.
pub(crate) fn digest(candles: &[Candle]) -> String {
    let mut hash = Sha256::new();
    for candle in candles {
        hash.update(format!(
            "{} {} {} {} {} {} {} {:?}\n",
            candle.open_time_ms,
            candle.close_time_ms,
            candle.open,
            candle.high,
            candle.low,
            candle.close,
            candle.volume,
            candle.trades
        ));
    }
    hex::encode(hash.finalize())
}

/// A job running the workload `params` names on `candles`.
pub(crate) fn input(params: &str, candles: Vec<Candle>) -> JobInput {
    let bundle = bundle(params);
    let last = candles.last().map_or(T, |candle| candle.open_time_ms);
    let submission: JobSubmission = serde_json::from_value(json!({
        "bundle": bundle.code_hash().to_string(),
        "data": {
            "exchange": "binance",
            "category": "spot",
            "symbol": "BTCUSDT",
            "interval": "1m",
            "start_open_ms": T,
            "end_open_ms": last,
        },
        "backtest": {"initial_cash": "10000", "fee_bps": "10", "close_out_at_end": true},
    }))
    .expect("a submission");
    JobInput {
        job_id: jobs::JobId::new(JobId::generate().as_str()).expect("a job id"),
        submission,
        bundle,
        candles,
    }
}

/// The report a job answered.
pub(crate) fn report(execution: &Execution) -> Value {
    let WorkerOutcome::Succeeded(results) = &execution.outcome else {
        panic!("a succeeded job, not {:?}", execution.outcome);
    };
    let [result] = results.as_slice() else {
        panic!("one result, not {results:?}");
    };
    assert_eq!(result.reference.name, "report");
    assert_eq!(result.reference.media_type, "application/json");
    assert_eq!(
        result.reference.size_bytes,
        u64::try_from(result.bytes.len()).expect("a size")
    );
    let digest: [u8; 32] = Sha256::digest(&result.bytes).into();
    assert_eq!(
        result.reference.hash.to_string(),
        format!("sha256:{}", hex::encode(digest))
    );
    serde_json::from_slice(&result.bytes).expect("a JSON report")
}

/// How a job that failed failed.
pub(crate) fn failure(execution: &Execution) -> (ErrorCode, String) {
    match &execution.outcome {
        WorkerOutcome::Failed(ErrorBody { code, message, .. }) => (*code, message.clone()),
        other => panic!("a failed job, not {other:?}"),
    }
}

/// The names of the environment a worker gets: its scratch directory as its
/// temporary directory.
pub(crate) fn worker_env() -> Vec<&'static str> {
    if cfg!(windows) {
        vec!["TEMP", "TMP"]
    } else {
        vec!["TMPDIR"]
    }
}

/// The names of the environment an `inspect` report saw, but the one
/// CoreFoundation sets as a macOS process that links it starts, which holds
/// the user's text encoding and was given by no one.
pub(crate) fn given_env(seen: &Value) -> Vec<&str> {
    seen["env"]
        .as_array()
        .expect("the names")
        .iter()
        .filter_map(Value::as_str)
        .filter(|name| !(cfg!(target_os = "macos") && *name == "__CF_USER_TEXT_ENCODING"))
        .collect()
}

/// Kills the process `pid` from outside, as an operator or the system
/// might.
pub(crate) fn kill(pid: u32) {
    let pid = pid.to_string();
    #[cfg(unix)]
    let status = std::process::Command::new("kill")
        .args(["-s", "KILL", &pid])
        .status();
    #[cfg(windows)]
    let status = std::process::Command::new("taskkill")
        .args(["/F", "/PID", &pid])
        .status();
    assert!(
        status.expect("a kill").success(),
        "process {pid} was not killed"
    );
}

/// Whether the process `pid` still runs.
#[cfg(unix)]
pub(crate) fn alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}
