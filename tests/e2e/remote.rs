//! `backtest --remote` against a real server, hosted in this process on a
//! loopback port over the bed's store, on each backend. Its worker runs the
//! engine's own SMA cross with the bundle's parameters in place of Python,
//! so that every leg of CI holds the client to what matters here: the
//! goldens' bundle, uploaded under the hash the client computed, runs on
//! the candles a local run reads, its log shows as it runs, and its report
//! is the local report and the goldens' result, line for line but the one
//! that names the strategy. `--no-wait` and `backtest jobs status` pick a
//! job up later, Ctrl-C leaves it running, a refusal is passed on word for
//! word, and a remote run opens no store of its own. The Python suite runs
//! the same bundle on a real Python worker (`test_worker_jobs.py`).

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use quantforge::protocol::bundle::Runtime;
use quantforge::protocol::jobs::LogLevel;
use quantforge::server::{
    self, BacktestOutput, Cancel, History, HistoryLimits, JobInput, JobLimits, JobLog, JobWorker,
    Jobs, LogLimits, ServerStore, StoreLimits, WorkerOutcome,
};
use quantforge::{
    BacktestConfig, BacktestEngine, BuiltInStrategyConfig, BundleHash, Candle, CandleStore,
    ENGINE_VERSION, ExchangeId, Interval, JobStore, MarketId, Symbol,
};
use rust_decimal::Decimal;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::harness::{TestBed, cli_cmd, isolated_cmd, on_each_backend};

/// The goldens, from the repository's root.
const GOLDEN: &str = "tests/fixtures/golden";

/// What the goldens' strategy writes to its log, here.
const SAID: &str = "sma cross over the frozen day";

fn golden_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(GOLDEN)
        .join(name)
}

fn market() -> MarketId {
    MarketId::new(
        ExchangeId::BinanceSpot,
        Symbol::new("BTCUSDT").expect("a symbol"),
        Interval::M1,
    )
}

/// The goldens' frozen day of candles.
fn golden_candles() -> Vec<Candle> {
    let text = std::fs::read_to_string(golden_path("candles.csv")).expect("the candles");
    text.lines()
        .skip(1)
        .map(|line| {
            let fields: Vec<&str> = line.split(',').collect();
            let decimal = |at: usize| fields[at].parse::<Decimal>().expect("a decimal");
            Candle {
                open_time_ms: fields[0].parse().expect("a time"),
                close_time_ms: fields[1].parse().expect("a time"),
                open: decimal(2),
                high: decimal(3),
                low: decimal(4),
                close: decimal(5),
                volume: decimal(6),
                trades: Some(fields[7].parse().expect("a count")),
            }
        })
        .collect()
}

/// What the goldens say the SMA cross made of the day: the report a run of
/// it prints, after its `strategy:` line.
fn golden_report() -> Vec<String> {
    let text = std::fs::read_to_string(golden_path("sma_cross.json")).expect("the goldens");
    let golden: Value = serde_json::from_str(&text).expect("json");
    let result = &golden["result"];
    let text_of = |value: &Value| match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    let mut report = vec![
        format!("final_equity: {}", text_of(&result["final_equity"])),
        format!("total_return_pct: {}", text_of(&result["total_return_pct"])),
        format!("max_drawdown_pct: {}", text_of(&result["max_drawdown_pct"])),
        format!("trade_count: {}", text_of(&result["trade_count"])),
    ];
    let trades = result["trades"].as_array().expect("trades");
    for trade in &trades[trades.len() - 5..] {
        report.push(format!(
            "trade: entry={} @ {} exit={} @ {} qty={} gross_pnl={}",
            text_of(&trade["entry_time_ms"]),
            text_of(&trade["entry_price"]),
            text_of(&trade["exit_time_ms"]),
            text_of(&trade["exit_price"]),
            text_of(&trade["qty"]),
            text_of(&trade["gross_quote_pnl"])
        ));
    }
    report
}

fn runtime() -> Runtime {
    serde_json::from_str(&format!(
        r#"{{"engine":"{ENGINE_VERSION}","python":"3.11","packages":{{}}}}"#
    ))
    .expect("a runtime")
}

/// A worker that runs the engine's own SMA cross, with the windows the
/// bundle's parameters name, where a server runs the bundle's Python.
struct Engine;

impl JobWorker for Engine {
    fn runtime(&self) -> Option<Runtime> {
        Some(runtime())
    }

    fn run(&self, input: JobInput, _cancel: &Cancel, log: &JobLog) -> WorkerOutcome {
        let params = input.bundle.manifest().params().as_map();
        let window = |name: &str| {
            params
                .get(name)
                .and_then(Value::as_u64)
                .and_then(|bars| usize::try_from(bars).ok())
                .expect("a window")
        };
        let mut strategy = BuiltInStrategyConfig::SmaCross {
            fast: window("fast"),
            slow: window("slow"),
        }
        .build()
        .expect("a strategy");
        log.line(LogLevel::Info, SAID);
        log.line(LogLevel::Warning, "a warning, too");
        let bars = u64::try_from(input.candles.len()).expect("a count");
        log.progress(bars / 2, bars);
        let backtest = &input.submission.backtest;
        let result = BacktestEngine::new(BacktestConfig {
            initial_cash: backtest.initial_cash.get(),
            fee_bps: backtest.fee_bps.get(),
            close_out_at_end: backtest.close_out_at_end,
        })
        .run(&market(), &input.candles, strategy.as_mut())
        .expect("a backtest");
        log.progress(bars, bars);
        WorkerOutcome::Succeeded(BacktestOutput::of(&result).files().expect("the files"))
    }
}

/// A worker whose jobs write a line and run until they are cancelled.
#[cfg(unix)]
struct Holding;

#[cfg(unix)]
impl JobWorker for Holding {
    fn runtime(&self) -> Option<Runtime> {
        Some(runtime())
    }

    fn run(&self, _input: JobInput, cancel: &Cancel, log: &JobLog) -> WorkerOutcome {
        log.line(LogLevel::Info, SAID);
        while !cancel.wait(Duration::from_millis(50)) {}
        WorkerOutcome::Cancelled
    }
}

/// A server over the bed's store, in this process, running `worker`'s jobs
/// one at a time; it stops, cancelling what still runs, when dropped.
struct Remote {
    runtime: tokio::runtime::Runtime,
    jobs: Jobs,
    address: SocketAddr,
}

impl Remote {
    fn start(bed: &TestBed, worker: impl JobWorker + 'static, max_span_bars: u64) -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("a runtime");
        let store = ServerStore::new(bed.store(), StoreLimits::default());
        let (jobs, address) = runtime.block_on(async {
            let jobs = Jobs::new(
                store.clone(),
                worker,
                JobLimits {
                    concurrency: 1,
                    max_span_bars,
                    log: LogLimits::default(),
                },
            );
            jobs.start().await.expect("a started queue");
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("a listener");
            let address = listener.local_addr().expect("its address");
            let history = History::new(store, HistoryLimits::default());
            tokio::spawn(server::serve(
                listener,
                history,
                jobs.clone(),
                std::future::pending(),
            ));
            (jobs, address)
        });
        Self {
            runtime,
            jobs,
            address,
        }
    }

    fn url(&self) -> String {
        format!("http://{}", self.address)
    }

    /// `GET path` on the server, as JSON.
    fn get(&self, path: &str) -> Value {
        let url = format!("{}{path}", self.url());
        self.runtime.block_on(async {
            reqwest::get(url)
                .await
                .expect("a response")
                .json()
                .await
                .expect("json")
        })
    }
}

impl Drop for Remote {
    fn drop(&mut self) {
        self.runtime.block_on(self.jobs.stop());
    }
}

fn seed(bed: &TestBed) {
    bed.store()
        .upsert_candles(&market(), &golden_candles())
        .expect("the candles");
}

/// `backtest --remote` of the goldens' bundle, run in `dir`, with `extra`
/// flags after the bundle's.
fn remote_cmd(remote: &Remote, dir: &Path, extra: &[&str]) -> assert_cmd::Command {
    let mut cmd = isolated_cmd();
    cmd.current_dir(dir)
        .args(["--log-level", "error", "backtest", "--symbol", "BTCUSDT"])
        .args(["--remote", &remote.url()])
        .arg("--strategy")
        .arg(golden_path("sma_cross.py"))
        .args([
            "--entry",
            "sma_cross.py:SmaCross",
            "--params",
            "fast=20,slow=50",
        ])
        .args(extra);
    cmd
}

fn stdout_lines(output: &std::process::Output) -> Vec<String> {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_string)
        .collect()
}

/// The report's lines: from `strategy:` on.
fn report(lines: &[String]) -> Vec<String> {
    let start = lines
        .iter()
        .position(|line| line.starts_with("strategy: "))
        .unwrap_or_else(|| panic!("no report in {lines:#?}"));
    lines[start..].to_vec()
}

fn value<'a>(lines: &'a [String], key: &str) -> &'a str {
    let prefix = format!("{key}: ");
    lines
        .iter()
        .find_map(|line| line.strip_prefix(&prefix))
        .unwrap_or_else(|| panic!("no {key}: in {lines:#?}"))
}

fn the_goldens_bundle_runs_remotely_and_prints_the_local_report(bed: &TestBed) {
    seed(bed);
    let local = cli_cmd(bed, "error")
        .args(["backtest", "--symbol", "BTCUSDT"])
        .output()
        .expect("a local run");
    assert!(local.status.success(), "{local:?}");
    let local = report(&stdout_lines(&local));

    let remote = Remote::start(bed, Engine, server::DEFAULT_MAX_SPAN_BARS);
    let elsewhere = tempfile::tempdir().expect("a directory");
    let output = remote_cmd(&remote, elsewhere.path(), &[])
        .output()
        .expect("a remote run");
    assert!(output.status.success(), "{output:?}");
    let lines = stdout_lines(&output);

    // The report: the local one's but the strategy's name, and the goldens'.
    let printed = report(&lines);
    assert_eq!(printed[0], "strategy: sma_cross.py:SmaCross");
    assert_eq!(local[0], "strategy: sma_cross");
    assert_eq!(printed[1..], local[1..]);
    assert_eq!(printed[1..], golden_report());

    // What came before it: the server, the bundle under the client's hash,
    // the range a local run reads, the job, and its log as it ran.
    assert_eq!(
        value(&lines, "remote"),
        format!(
            "{}/ (QuantForge {ENGINE_VERSION}, protocol v1)",
            remote.url()
        )
    );
    assert_eq!(
        value(&lines, "range"),
        "2026-01-01T12:00:00Z to 2026-01-02T11:59:00Z"
    );
    let bundle = value(&lines, "bundle");
    let hash = bundle.split(' ').next().expect("a hash");
    assert!(bundle.ends_with(" bytes, stored)"), "{bundle}");
    let job = value(&lines, "job_id");
    let status = remote.get(&format!("/v1/backtests/{job}"));
    assert_eq!(status["submission"]["bundle"], hash);
    let stored = bed
        .store()
        .load_bundle(&hash.parse::<BundleHash>().expect("a bundle hash"))
        .expect("a read")
        .expect("the bundle the server holds");
    assert_eq!(
        format!("sha256:{}", hex::encode(Sha256::digest(&stored))),
        hash
    );
    let at = |line: &str| {
        lines
            .iter()
            .position(|each| each == line)
            .unwrap_or_else(|| panic!("no {line:?} in {lines:#?}"))
    };
    assert!(at(&format!("log: {SAID}")) < at("log: warning: a warning, too"));
    assert!(at("log: warning: a warning, too") < at("progress: 1440 of 1440 bars"));
    assert!(at("progress: 1440 of 1440 bars") < at("state: succeeded"));

    // The same bundle again is held already, under the same hash.
    let again = remote_cmd(&remote, elsewhere.path(), &["--no-wait"])
        .output()
        .expect("a remote run");
    assert!(again.status.success(), "{again:?}");
    let again = stdout_lines(&again);
    assert_eq!(
        value(&again, "bundle"),
        bundle.replace("stored", "already held")
    );

    // A remote run opens no store of its own.
    assert_eq!(
        std::fs::read_dir(elsewhere.path())
            .expect("the directory")
            .count(),
        0
    );
}

fn a_job_left_with_no_wait_is_reported_by_jobs_status(bed: &TestBed) {
    seed(bed);
    let remote = Remote::start(bed, Engine, server::DEFAULT_MAX_SPAN_BARS);
    let dir = tempfile::tempdir().expect("a directory");
    let output = remote_cmd(
        &remote,
        dir.path(),
        &["--no-wait", "--start", "2026-01-02T00:00:00Z"],
    )
    .output()
    .expect("a remote run");
    assert!(output.status.success(), "{output:?}");
    let lines = stdout_lines(&output);
    assert!(
        !lines.iter().any(|line| line.starts_with("strategy: ")),
        "{lines:#?}"
    );
    assert_eq!(
        value(&lines, "range"),
        "2026-01-02T00:00:00Z to 2026-01-02T11:59:00Z"
    );
    let job = value(&lines, "job_id").to_string();
    let hash = value(&lines, "bundle")
        .split(' ')
        .next()
        .expect("a hash")
        .to_string();

    let status = |extra: &[&str]| {
        let mut cmd = isolated_cmd();
        cmd.current_dir(dir.path())
            .args(["--log-level", "error", "backtest", "jobs", "status", &job])
            .args(["--remote", &remote.url()])
            .args(extra);
        cmd.output().expect("a status")
    };
    let followed = status(&["--follow"]);
    assert!(followed.status.success(), "{followed:?}");
    let followed = stdout_lines(&followed);
    assert_eq!(value(&followed, "job_id"), job);
    assert!(followed.contains(&format!("log: {SAID}")), "{followed:#?}");
    assert_eq!(value(&followed, "state"), "succeeded");
    let printed = report(&followed);
    assert_eq!(printed[0], format!("strategy: {hash}"));

    // Asked again, without following, it reports the same.
    let asked = status(&[]);
    assert!(asked.status.success(), "{asked:?}");
    let asked = stdout_lines(&asked);
    assert!(
        !asked.iter().any(|line| line.starts_with("log: ")),
        "{asked:#?}"
    );
    assert_eq!(report(&asked), printed);
}

fn a_refusal_is_the_servers_message_word_for_word(bed: &TestBed) {
    seed(bed);
    let remote = Remote::start(bed, Engine, 100);
    let dir = tempfile::tempdir().expect("a directory");
    let output = remote_cmd(&remote, dir.path(), &[])
        .output()
        .expect("a remote run");
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(&format!(
            "the server at {}/ refused the job: the range spans 1440 bars of 1m, more than the \
             100 this server runs one job on; ask for a shorter range",
            remote.url()
        )),
        "{stderr}"
    );
    let outside = remote_cmd(&remote, dir.path(), &["--end", "2025-12-31T00:00:00Z"])
        .output()
        .expect("a remote run");
    assert_eq!(outside.status.code(), Some(1), "{outside:?}");
    let stderr = String::from_utf8_lossy(&outside.stderr);
    assert!(
        stderr.contains(&format!(
            "the server at {}/ stores binance spot BTCUSDT 1m from 2026-01-01T12:00:00Z to \
             2026-01-02T11:59:00Z, outside the window from 2026-01-01T12:00:00Z to \
             2025-12-31T00:00:00Z",
            remote.url()
        )),
        "{stderr}"
    );
}

on_each_backend!(
    the_goldens_bundle_runs_remotely_and_prints_the_local_report,
    a_job_left_with_no_wait_is_reported_by_jobs_status,
    a_refusal_is_the_servers_message_word_for_word,
);

#[test]
fn every_flag_is_checked_before_any_server_is_asked() {
    let dir = tempfile::tempdir().expect("a directory");
    let strategy = golden_path("sma_cross.py");
    let strategy = strategy.to_str().expect("a path");
    for (args, said) in [
        (
            vec![
                "--remote",
                "localhost:9",
                "--strategy",
                strategy,
                "--entry",
                "sma_cross.py:SmaCross",
            ],
            "--remote: invalid server URL",
        ),
        (
            vec![
                "--remote",
                "http://127.0.0.1:9",
                "--strategy",
                strategy,
                "--entry",
                "sma_cross.py:SmaCross",
                "--params",
                "fast",
            ],
            "--params must be TOML key = value pairs",
        ),
        (
            vec![
                "--remote",
                "http://127.0.0.1:9",
                "--strategy",
                strategy,
                "--entry",
                "sma_cross:SmaCross",
            ],
            "--entry: invalid entry",
        ),
        (
            vec!["--remote", "http://127.0.0.1:9", "--strategy", strategy],
            "--entry must name the strategy, as in --entry sma_cross.py:MyStrategy",
        ),
        (
            vec![
                "--remote",
                "http://127.0.0.1:9",
                "--strategy",
                strategy,
                "--entry",
                "sma_cross.py:SmaCross",
                "--cash",
                "0",
            ],
            "--cash must be greater than 0",
        ),
        (
            vec![
                "--remote",
                "http://127.0.0.1:9",
                "--strategy",
                "no-such.py",
                "--entry",
                "no-such.py:X",
            ],
            "cannot read --strategy no-such.py",
        ),
    ] {
        let output = isolated_cmd()
            .current_dir(dir.path())
            .args(["--log-level", "error", "backtest", "--symbol", "BTCUSDT"])
            .args(&args)
            .output()
            .expect("a run");
        assert_eq!(output.status.code(), Some(1), "{args:?}: {output:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(said), "{args:?}: {stderr}");
        assert!(!stderr.contains("could not reach"), "{args:?}: {stderr}");
    }
    // Then the server, which is not there.
    let output = isolated_cmd()
        .current_dir(dir.path())
        .args(["--log-level", "error", "backtest", "--symbol", "BTCUSDT"])
        .args(["--remote", "http://127.0.0.1:9", "--strategy", strategy])
        .args(["--entry", "sma_cross.py:SmaCross"])
        .output()
        .expect("a run");
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("could not reach the server at http://127.0.0.1:9/: ")
            && stderr.contains("; check --remote, and that `quantforge serve run` listens there"),
        "{stderr}"
    );
    assert_eq!(
        std::fs::read_dir(dir.path())
            .expect("the directory")
            .count(),
        0
    );
}

#[test]
fn the_built_in_strategy_and_a_python_one_do_not_mix() {
    for (args, said) in [
        (
            vec!["--strategy", "s.py"],
            "the following required arguments were not provided",
        ),
        (
            vec![
                "--remote",
                "http://127.0.0.1:9",
                "--strategy",
                "s.py",
                "--fast",
                "5",
            ],
            "the argument '--remote <REMOTE>' cannot be used with '--fast <FAST>'",
        ),
        (
            vec!["--remote", "http://127.0.0.1:9"],
            "--strategy <STRATEGY>",
        ),
    ] {
        let output = isolated_cmd()
            .args(["backtest", "--symbol", "BTCUSDT"])
            .args(&args)
            .output()
            .expect("a run");
        assert_eq!(output.status.code(), Some(2), "{args:?}: {output:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(said), "{args:?}: {stderr}");
    }
}

#[cfg(unix)]
#[test]
fn ctrl_c_leaves_the_job_running_and_says_how_to_pick_it_up() {
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;

    let bed = TestBed::new();
    seed(&bed);
    let remote = Remote::start(&bed, Holding, server::DEFAULT_MAX_SPAN_BARS);
    let dir = tempfile::tempdir().expect("a directory");
    let mut cmd = crate::harness::isolated_std_cmd();
    cmd.current_dir(dir.path())
        .args(["--log-level", "error", "backtest", "--symbol", "BTCUSDT"])
        .args(["--remote", &remote.url()])
        .arg("--strategy")
        .arg(golden_path("sma_cross.py"))
        .args([
            "--entry",
            "sma_cross.py:SmaCross",
            "--params",
            "fast=20,slow=50",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("a remote run");
    let mut stdout = BufReader::new(child.stdout.take().expect("its stdout")).lines();
    let mut job = String::new();
    for line in stdout.by_ref() {
        let line = line.expect("a line");
        if let Some(id) = line.strip_prefix("job_id: ") {
            job = id.to_string();
        }
        if line == format!("log: {SAID}") {
            break;
        }
    }
    assert!(!job.is_empty(), "no job_id: line");
    let signalled = std::process::Command::new("kill")
        .args(["-s", "INT", &child.id().to_string()])
        .status()
        .expect("kill");
    assert!(signalled.success());
    let rest: Vec<String> = stdout.map(|line| line.expect("a line")).collect();
    let status = child.wait().expect("an exit");
    assert!(status.success(), "{status:?}: {rest:#?}");
    assert_eq!(
        rest,
        [format!(
            "detached: job {job} keeps running on the server; `quantforge backtest jobs status \
             {job} --remote {}/ --follow` picks it up again",
            remote.url()
        )]
    );
    let state = remote.get(&format!("/v1/backtests/{job}"));
    assert_eq!(state["state"]["kind"], "running", "{state}");
}

#[test]
fn a_server_that_runs_no_bundles_is_refused_before_anything_is_uploaded() {
    let bed = TestBed::new();
    seed(&bed);
    let served = crate::serve::Served::start(&bed);
    let dir = tempfile::tempdir().expect("a directory");
    let output = isolated_cmd()
        .current_dir(dir.path())
        .args(["--log-level", "error", "backtest", "--symbol", "BTCUSDT"])
        .args(["--remote", &format!("http://{}", served.address)])
        .arg("--strategy")
        .arg(golden_path("sma_cross.py"))
        .args(["--entry", "sma_cross.py:SmaCross"])
        .output()
        .expect("a remote run");
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(&format!(
            "the server at http://{}/ runs no Python bundles: it was started without --job-python",
            served.address
        )),
        "{stderr}"
    );
    let jobs: Value = bed
        .store()
        .jobs(10)
        .map(|jobs| Value::from(jobs.len()))
        .expect("the jobs");
    assert_eq!(jobs, 0);
}
