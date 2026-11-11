//! Black-box tests for the backtest job queue, driven through the server's
//! public API with a hand-written worker, on SQLite and, where
//! `QUANTFORGE_TEST_POSTGRES_URL` names a server, on PostgreSQL: a job's
//! whole life, a cancel that frees its worker, the order and number of jobs
//! at once, a restart that fails the jobs that ran and runs the queued
//! ones, a stop, a server with no runtime, and a succeeded job's result,
//! downloaded file by file from the server that ran it and from the next
//! one on its store. Each scenario is one generic body, run once per
//! backend.

use quantforge::protocol::bundle::{Bundle, BundleLimits, Runtime};
use quantforge::protocol::jobs::{JobResult, ResultRef};
use quantforge::protocol::results::{
    ARROW_FILE_MEDIA_TYPE, Metrics, RESULT_CACHE_CONTROL, decode_equity, decode_trades,
};
use quantforge::protocol::{ContentHash, ErrorBody, ErrorCode};
use quantforge::server::{
    self, BacktestOutput, Cancel, History, HistoryLimits, JobInput, JobLimits, JobWorker, Jobs,
    NoRuntime, ResultFile, ServedStore, ServerStore, StoreLimits, WorkerOutcome,
};
use quantforge::{
    BacktestConfig, BacktestEngine, BacktestResult, BuiltInStrategyConfig, BundleHash, Candle,
    CandleQuery, CandleStore, ExchangeId, Interval, JobId, JobStore, MarketId, NewJob, SqliteStore,
    Symbol, TimestampMs,
};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use tempfile::TempDir;
use tokio::net::TcpListener;

/// 2026-10-20T00:00:00Z.
const T: TimestampMs = 1_792_454_400_000;
const MINUTE: TimestampMs = 60_000;

/// Why a job that ran when its server stopped failed.
const STOPPED: &str = "the server stopped while the job ran; submit it again";

const MANIFEST: &str =
    "format = 1\nentry = \"strategy.py:Hold\"\nengine = \">=0.3.0, <0.4.0\"\npython = \"3.12\"\n";

/// A store of one backend, as many handles on it as a test asks for.
trait Backend {
    type Store: ServedStore + 'static;

    /// A handle on the store, each call bounded as `serve run` bounds them.
    fn store(&self) -> Self::Store;
}

struct Sqlite {
    _dir: TempDir,
    path: std::path::PathBuf,
}

impl Sqlite {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("a directory");
        let path = dir.path().join("market.sqlite");
        CandleStore::init(&SqliteStore::new(&path)).expect("init");
        Self { _dir: dir, path }
    }
}

impl Backend for Sqlite {
    type Store = SqliteStore;

    fn store(&self) -> SqliteStore {
        SqliteStore::new(&self.path).within(Duration::from_secs(5))
    }
}

fn market() -> MarketId {
    MarketId::new(
        ExchangeId::BinanceSpot,
        Symbol::new("BTCUSDT").expect("a symbol"),
        Interval::M1,
    )
}

/// `count` `1m` candles from `T`, each with its own close.
fn candles(count: i64) -> Vec<Candle> {
    (0..count)
        .map(|i| {
            let close = Decimal::new(10_000 + i, 2);
            Candle {
                open_time_ms: T + i * MINUTE,
                close_time_ms: T + (i + 1) * MINUTE - 1,
                open: close,
                high: close,
                low: close,
                close,
                volume: Decimal::new(50_000_000, 8),
                trades: Some(u64::try_from(i).expect("a count")),
            }
        })
        .collect()
}

/// `count` `1m` candles from `T` whose closes rise and fall in waves, so
/// that an SMA cross trades them several times, every close with two
/// decimal places.
fn waves(count: i64) -> Vec<Candle> {
    let close = |i: i64| {
        let phase = i % 24;
        let height = if phase < 12 { phase } else { 24 - phase };
        Decimal::new(10_000 + 137 * height + i, 2)
    };
    (0..count)
        .map(|i| {
            let (open, close) = (close((i - 1).max(0)), close(i));
            Candle {
                open_time_ms: T + i * MINUTE,
                close_time_ms: T + (i + 1) * MINUTE - 1,
                open,
                high: open.max(close),
                low: open.min(close),
                close,
                volume: Decimal::new(50_000_000, 8),
                trades: Some(u64::try_from(i).expect("a count")),
            }
        })
        .collect()
}

/// What the engine answers on `candles` here, in this process: the SMA
/// cross built into QuantForge, under the settings every job here submits.
fn local_run(candles: &[Candle]) -> BacktestResult {
    let mut strategy = BuiltInStrategyConfig::SmaCross { fast: 2, slow: 3 }
        .build()
        .expect("a strategy");
    BacktestEngine::new(BacktestConfig {
        initial_cash: Decimal::from(10_000),
        fee_bps: Decimal::from(10),
        close_out_at_end: true,
    })
    .run(&market(), candles, strategy.as_mut())
    .expect("a backtest")
}

/// A result as the three files a job of it is stored as.
fn files(result: &BacktestResult) -> Vec<ResultFile> {
    BacktestOutput::of(result).files().expect("the files")
}

fn bundle() -> Bundle {
    Bundle::from_files(
        [
            ("quantforge.toml", MANIFEST),
            ("strategy.py", "class Hold:\n    pass\n"),
        ],
        &BundleLimits::default(),
    )
    .expect("a bundle")
}

fn runtime() -> Runtime {
    serde_json::from_str(r#"{"engine":"0.3.0","python":"3.12","packages":{}}"#).expect("a runtime")
}

/// What the worker was asked to run, and what each job answers once told.
#[derive(Default)]
struct Script {
    /// Each run in the order it began: the job and the candles it got.
    began: Vec<(String, Vec<Candle>)>,
    /// What each job answers once told, by job.
    told: HashMap<String, WorkerOutcome>,
    /// Runs that have not returned yet, and the most there ever were.
    running: usize,
    most: usize,
    /// Set as a test ends, passed or failed: every run still going returns.
    released: bool,
}

/// A worker that runs each job until it is told how the job ends, or until
/// its cancel comes.
#[derive(Clone, Default)]
struct Scripted(Arc<(Mutex<Script>, Condvar)>);

/// Releases every run still going when dropped: a test's runtime waits for
/// each to return before it ends, so a failed assertion would otherwise
/// hang the test instead of failing it.
struct Release(Scripted);

impl Drop for Release {
    fn drop(&mut self) {
        if let Ok(mut script) = self.0.0.0.lock() {
            script.released = true;
        }
    }
}

impl Scripted {
    /// The worker, and what releases its runs as the test ends.
    fn new() -> (Self, Release) {
        let worker = Self::default();
        (worker.clone(), Release(worker))
    }

    fn script(&self) -> std::sync::MutexGuard<'_, Script> {
        self.0.0.lock().expect("the script")
    }

    fn tell(&self, job: &str, outcome: WorkerOutcome) {
        self.script().told.insert(job.to_string(), outcome);
    }

    fn began(&self) -> Vec<String> {
        self.script()
            .began
            .iter()
            .map(|(job, _)| job.clone())
            .collect()
    }

    /// Waits until `count` runs have begun, and answers their jobs.
    async fn until_begun(&self, count: usize) -> Vec<String> {
        until(&format!("{count} runs"), || {
            let began = self.began();
            (began.len() >= count).then_some(began)
        })
        .await
    }
}

impl JobWorker for Scripted {
    fn runtime(&self) -> Option<Runtime> {
        Some(runtime())
    }

    fn run(&self, input: JobInput, cancel: &Cancel) -> WorkerOutcome {
        let job = input.job_id.to_string();
        {
            let mut script = self.script();
            script.began.push((job.clone(), input.candles));
            script.running += 1;
            script.most = script.most.max(script.running);
        }
        let outcome = loop {
            if cancel.is_cancelled() || self.script().released {
                break WorkerOutcome::Cancelled;
            }
            if let Some(outcome) = self.script().told.remove(&job) {
                break outcome;
            }
            cancel.wait(Duration::from_millis(10));
        };
        self.script().running -= 1;
        outcome
    }
}

/// `check`'s answer once it has one, or a failed test after ten seconds.
async fn until<T>(what: &str, mut check: impl FnMut() -> Option<T>) -> T {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(found) = check() {
            return found;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no {what} within ten seconds"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// The job routes over `backend`, running `concurrency` jobs at once on
/// `worker`, started.
async fn jobs<B: Backend, W: JobWorker + 'static>(
    backend: &B,
    worker: W,
    concurrency: usize,
) -> (ServerStore, Jobs) {
    let store = ServerStore::new(backend.store(), StoreLimits::default());
    let jobs = Jobs::new(
        store.clone(),
        worker,
        JobLimits {
            concurrency,
            max_span_bars: server::DEFAULT_MAX_SPAN_BARS,
        },
    );
    jobs.start().await.expect("a started queue");
    (store, jobs)
}

/// Serves the history routes and `jobs` on a free loopback port.
async fn serve(store: ServerStore, jobs: Jobs) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("a listener");
    let address = listener.local_addr().expect("its address");
    let history = History::new(store, HistoryLimits::default());
    tokio::spawn(server::serve(
        listener,
        history,
        jobs,
        std::future::pending(),
    ));
    address
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("an HTTP client")
}

/// Uploads the bundle, as a client does.
async fn upload(address: SocketAddr) -> ContentHash {
    let bundle = bundle();
    let response = client()
        .put(format!(
            "http://{address}/v1/bundles/{}",
            bundle.code_hash()
        ))
        .header("content-type", "application/x-tar")
        .body(bundle.canonical_tar())
        .send()
        .await
        .expect("a response");
    assert!(response.status().is_success(), "{}", response.status());
    bundle.code_hash()
}

/// Submits a job over the bars from `T` to `T + last` minutes, and answers
/// its ID.
async fn submit(address: SocketAddr, hash: &ContentHash, last: i64) -> String {
    let body = json!({
        "bundle": hash.to_string(),
        "data": {
            "exchange": "binance",
            "category": "spot",
            "symbol": "BTCUSDT",
            "interval": "1m",
            "start_open_ms": T,
            "end_open_ms": T + last * MINUTE,
        },
        "backtest": {"initial_cash": "10000", "fee_bps": "10", "close_out_at_end": true},
    });
    let response = client()
        .post(format!("http://{address}/v1/backtests"))
        .json(&body)
        .send()
        .await
        .expect("a response");
    assert_eq!(response.status(), 201);
    let status: Value = response.json().await.expect("a status");
    status["job_id"].as_str().expect("a job id").to_string()
}

/// The job's state, as `GET /v1/backtests/{job}` answers it.
async fn state(address: SocketAddr, job: &str) -> Value {
    let response = client()
        .get(format!("http://{address}/v1/backtests/{job}"))
        .send()
        .await
        .expect("a response");
    assert_eq!(response.status(), 200);
    let status: Value = response.json().await.expect("a status");
    status["state"].clone()
}

/// The job's state once it is `kind`.
async fn until_state(address: SocketAddr, job: &str, kind: &str) -> Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let state = state(address, job).await;
        if state["kind"] == kind {
            return state;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "job {job} is {state}, not {kind}, after ten seconds"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn cancel(address: SocketAddr, job: &str) -> reqwest::Response {
    client()
        .post(format!("http://{address}/v1/backtests/{job}/cancel"))
        .send()
        .await
        .expect("a response")
}

fn failure(code: ErrorCode, message: &str) -> WorkerOutcome {
    WorkerOutcome::Failed(ErrorBody {
        code,
        message: message.to_string(),
        supported: None,
    })
}

/// A job's whole life: queued, running on exactly the candles the store
/// holds for its range, then succeeded with its results; and another that
/// fails with the worker's reason.
async fn a_job_runs_on_its_candles_and_its_end_is_read_back<B: Backend>(backend: &B) {
    backend
        .store()
        .upsert_candles(&market(), &candles(120))
        .expect("the candles");
    let (worker, _release) = Scripted::new();
    let (store, jobs) = jobs(backend, worker.clone(), 1).await;
    let address = serve(store, jobs).await;
    let hash = upload(address).await;

    let job = submit(address, &hash, 99).await;
    assert_eq!(worker.until_begun(1).await, std::slice::from_ref(&job));
    let running = state(address, &job).await;
    assert_eq!(running["kind"], "running");
    let started = running["started_at_ms"].as_i64().expect("a start");
    let expected = backend
        .store()
        .load_candles(
            &market(),
            CandleQuery {
                start_time_ms: Some(T),
                end_time_ms: Some(T + 99 * MINUTE),
                limit: None,
            },
        )
        .expect("the range");
    let text = |candles: &[Candle]| format!("{candles:?}");
    assert_eq!(expected.len(), 100);
    assert_eq!(text(&worker.script().began[0].1), text(&expected));

    let files = files(&local_run(&expected));
    let results: Vec<ResultRef> = files.iter().map(|file| file.reference.clone()).collect();
    worker.tell(&job, WorkerOutcome::Succeeded(files));
    let succeeded = until_state(address, &job, "succeeded").await;
    assert_eq!(succeeded["started_at_ms"], started);
    assert!(succeeded["finished_at_ms"].as_i64() >= Some(started));
    assert_eq!(
        succeeded["results"],
        serde_json::to_value(&results).expect("json")
    );
    let names: Vec<&str> = results.iter().map(|result| result.name.as_str()).collect();
    assert_eq!(names, ["metrics", "trades", "equity"]);

    let failing = submit(address, &hash, 0).await;
    worker.until_begun(2).await;
    worker.tell(
        &failing,
        failure(ErrorCode::InvalidRequest, "the strategy raised ValueError"),
    );
    let failed = until_state(address, &failing, "failed").await;
    assert_eq!(
        failed["error"],
        json!({"code": "invalid_request", "message": "the strategy raised ValueError"})
    );
    assert!(failed["started_at_ms"].as_i64().is_some(), "{failed}");
}

/// One response: its status, the headers a result is served with, and its
/// body.
struct Fetched {
    status: u16,
    content_type: String,
    etag: String,
    cache_control: String,
    body: Vec<u8>,
}

impl Fetched {
    fn refusal(&self) -> (u16, String, String) {
        let envelope: Value = serde_json::from_slice(&self.body).expect("an error envelope");
        let text = |field: &str| envelope["error"][field].as_str().unwrap_or("").to_string();
        (self.status, text("code"), text("message"))
    }
}

async fn fetch(address: SocketAddr, path: &str, if_none_match: Option<&str>) -> Fetched {
    let mut request = client().get(format!("http://{address}{path}"));
    if let Some(tag) = if_none_match {
        request = request.header("if-none-match", tag);
    }
    let response = request.send().await.expect("a response");
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_string()
    };
    Fetched {
        status: response.status().as_u16(),
        content_type: header("content-type"),
        etag: header("etag"),
        cache_control: header("cache-control"),
        body: response.bytes().await.expect("a body").to_vec(),
    }
}

/// Every field of a result's rows as text, so that equal texts mean equal
/// digits, where `==` on decimals would take `1.10` for `1.1`.
fn digits(result: &BacktestResult) -> (Vec<String>, Vec<String>) {
    let trades = result
        .trades
        .iter()
        .map(|trade| {
            format!(
                "{} {} {} {} {} {} {}",
                trade.symbol,
                trade.entry_time_ms,
                trade.exit_time_ms,
                trade.entry_price,
                trade.exit_price,
                trade.qty,
                trade.gross_quote_pnl
            )
        })
        .collect();
    let equity = result
        .equity_curve
        .iter()
        .map(|point| format!("{} {}", point.open_time_ms, point.equity))
        .collect();
    (trades, equity)
}

/// Everything a client reads of a succeeded job's result, each file checked
/// against its reference as it is fetched: the files, by name.
async fn download(address: SocketAddr, job: &str, expected: &[ResultFile]) -> Vec<Vec<u8>> {
    let answered = fetch(address, &format!("/v1/backtests/{job}/result"), None).await;
    assert_eq!(answered.status, 200);
    assert_eq!(answered.cache_control, "no-cache");
    let result: JobResult = serde_json::from_slice(&answered.body).expect("a job's result");
    assert_eq!(result.job_id.as_str(), job);
    assert_eq!(result.format, 1);
    assert_eq!(
        result.metrics,
        Metrics::decode(&expected[0].bytes).expect("metrics")
    );
    let listed: Vec<(&str, &str, String)> = result
        .results
        .iter()
        .map(|link| {
            (
                link.name.as_str(),
                link.media_type.as_str(),
                link.path.clone(),
            )
        })
        .collect();
    let at = |file: &str| format!("/v1/backtests/{job}/result/{file}");
    assert_eq!(
        listed,
        [
            ("metrics", "application/json", at("metrics.json")),
            ("trades", ARROW_FILE_MEDIA_TYPE, at("trades.arrow")),
            ("equity", ARROW_FILE_MEDIA_TYPE, at("equity.arrow")),
        ]
    );
    let mut files = Vec::new();
    for (link, file) in result.results.iter().zip(expected) {
        assert_eq!(
            (link.hash, link.size_bytes),
            (file.reference.hash, file.reference.size_bytes)
        );
        let fetched = fetch(address, &link.path, None).await;
        assert_eq!(fetched.status, 200, "{}", link.path);
        assert_eq!(fetched.content_type, link.media_type);
        assert_eq!(fetched.etag, format!("\"{}\"", link.hash));
        assert_eq!(fetched.cache_control, RESULT_CACHE_CONTROL);
        assert_eq!(fetched.body, file.bytes, "{}: every byte", link.path);

        // A client that holds the file is told so, and sent nothing.
        for tag in [
            fetched.etag.clone(),
            format!("W/{}", fetched.etag),
            "*".to_string(),
        ] {
            let cached = fetch(address, &link.path, Some(&tag)).await;
            assert_eq!((cached.status, cached.body.len()), (304, 0), "{tag}");
            assert_eq!(cached.etag, fetched.etag);
            assert_eq!(cached.cache_control, RESULT_CACHE_CONTROL);
        }
        let other = fetch(address, &link.path, Some("\"sha256:other\"")).await;
        assert_eq!(other.status, 200);
        let head = client()
            .head(format!("http://{address}{}", link.path))
            .send()
            .await
            .expect("a response");
        assert_eq!(head.status(), 200);
        let length = head.headers().get("content-length");
        assert_eq!(
            length.and_then(|value| value.to_str().ok()),
            Some(link.size_bytes.to_string().as_str())
        );
        files.push(fetched.body);
    }
    files
}

/// A succeeded job's result is its three files, downloaded byte for byte,
/// read back to the digits the engine computed, and still served by the
/// next server on the store. A job that has not succeeded has none.
async fn a_succeeded_jobs_result_is_downloaded_and_outlives_its_server<B: Backend>(backend: &B) {
    let stored = waves(240);
    backend
        .store()
        .upsert_candles(&market(), &stored)
        .expect("the candles");
    let (worker, _release) = Scripted::new();
    let (store, queue) = jobs(backend, worker.clone(), 1).await;
    let address = serve(store, queue.clone()).await;
    let hash = upload(address).await;
    let job = submit(address, &hash, 239).await;
    worker.until_begun(1).await;

    // No result yet, whichever of its routes is asked.
    for path in ["result", "result/trades.arrow", "result/metrics.json"] {
        let early = fetch(address, &format!("/v1/backtests/{job}/{path}"), None).await;
        assert_eq!(
            early.refusal(),
            (
                409,
                "conflict".to_string(),
                format!("job {job} is still running; only a job that has succeeded has a result")
            ),
            "{path}"
        );
        assert_eq!(early.cache_control, "no-store");
    }

    let local = local_run(&stored);
    assert!(
        local.trade_count >= 3,
        "the fixture trades: {}",
        local.trade_count
    );
    let expected = files(&local);
    worker.tell(&job, WorkerOutcome::Succeeded(expected.clone()));
    until_state(address, &job, "succeeded").await;
    let fetched = download(address, &job, &expected).await;

    // What a client decodes is what the engine computed, digit for digit.
    let trades = decode_trades(&fetched[1]).expect("the trades");
    let curve = decode_equity(&fetched[2]).expect("the curve");
    let read = (
        trades
            .iter()
            .map(|trade| {
                format!(
                    "{} {} {} {} {} {} {}",
                    trade.symbol,
                    trade.entry_time_ms,
                    trade.exit_time_ms,
                    trade.entry_price,
                    trade.exit_price,
                    trade.qty,
                    trade.gross_quote_pnl
                )
            })
            .collect::<Vec<_>>(),
        curve
            .iter()
            .map(|row| format!("{} {}", row.open_time_ms, row.equity))
            .collect::<Vec<_>>(),
    );
    assert_eq!(read, digits(&local));
    assert_eq!(curve.len(), stored.len(), "a row per bar");

    // A file of another name, another job, and jobs that never succeeded.
    let unknown = fetch(
        address,
        &format!("/v1/backtests/{job}/result/trades.json"),
        None,
    )
    .await;
    assert_eq!(
        unknown.refusal(),
        (
            404,
            "not_found".to_string(),
            "a job's result has no file trades.json; its files are metrics.json, trades.arrow \
             and equity.arrow"
                .to_string()
        )
    );
    let nobody = JobId::generate();
    let missing = fetch(address, &format!("/v1/backtests/{nobody}/result"), None).await;
    assert_eq!(
        missing.refusal(),
        (
            404,
            "not_found".to_string(),
            format!("the server holds no job {nobody}")
        )
    );
    let failing = submit(address, &hash, 9).await;
    worker.until_begun(2).await;
    worker.tell(
        &failing,
        failure(ErrorCode::InvalidRequest, "the strategy raised ValueError"),
    );
    until_state(address, &failing, "failed").await;
    let cancelled = submit(address, &hash, 9).await;
    worker.until_begun(3).await;
    assert_eq!(cancel(address, &cancelled).await.status(), 200);
    for (job, stands) in [(&failing, "failed"), (&cancelled, "was cancelled")] {
        let none = fetch(
            address,
            &format!("/v1/backtests/{job}/result/equity.arrow"),
            None,
        )
        .await;
        assert_eq!(
            none.refusal(),
            (
                409,
                "conflict".to_string(),
                format!("job {job} {stands}; only a job that has succeeded has a result")
            )
        );
    }

    // The server stops; the next one on the store serves the same result.
    queue.stop().await;
    let (next_worker, _next_release) = Scripted::new();
    let (store, next) = jobs(backend, next_worker, 1).await;
    let restarted = serve(store, next).await;
    assert_ne!(restarted, address);
    assert_eq!(download(restarted, &job, &expected).await, fetched);
}

/// A running job cancelled: its worker stops, the job stays cancelled, and
/// the next job takes its slot; a queued one cancelled never starts.
async fn a_cancel_stops_a_running_job_and_frees_its_slot<B: Backend>(backend: &B) {
    backend
        .store()
        .upsert_candles(&market(), &candles(10))
        .expect("the candles");
    let (worker, _release) = Scripted::new();
    let (store, jobs) = jobs(backend, worker.clone(), 1).await;
    let address = serve(store, jobs).await;
    let hash = upload(address).await;
    let first = submit(address, &hash, 9).await;
    worker.until_begun(1).await;
    let second = submit(address, &hash, 9).await;
    let third = submit(address, &hash, 9).await;
    assert_eq!(state(address, &second).await, json!({"kind": "queued"}));

    let response = cancel(address, &first).await;
    assert_eq!(response.status(), 200);
    let cancelled: Value = response.json().await.expect("a status");
    assert_eq!(cancelled["state"]["kind"], "cancelled");
    assert!(cancelled["state"]["started_at_ms"].as_i64().is_some());
    assert_eq!(worker.until_begun(2).await, [first.clone(), second.clone()]);
    until("the first run to return", || {
        (worker.script().running == 1).then_some(())
    })
    .await;
    assert_eq!(state(address, &first).await, cancelled["state"]);

    let response = cancel(address, &third).await;
    let queued_then: Value = response.json().await.expect("a status");
    assert!(
        queued_then["state"].get("started_at_ms").is_none(),
        "{queued_then}"
    );
    worker.tell(&second, WorkerOutcome::Succeeded(Vec::new()));
    until_state(address, &second, "succeeded").await;
    let fourth = submit(address, &hash, 9).await;
    assert_eq!(worker.until_begun(3).await, [first, second, fourth.clone()]);
    // A run left going would hold up the runtime's end.
    worker.tell(&fourth, WorkerOutcome::Succeeded(Vec::new()));
    until_state(address, &fourth, "succeeded").await;
}

/// Jobs start in the order they were submitted, never more at once than
/// the server runs.
async fn jobs_start_first_submitted_first_and_no_more_at_once_than_allowed<B: Backend>(
    backend: &B,
) {
    backend
        .store()
        .upsert_candles(&market(), &candles(10))
        .expect("the candles");
    let (worker, _release) = Scripted::new();
    let (store, jobs) = jobs(backend, worker.clone(), 2).await;
    let address = serve(store, jobs).await;
    let hash = upload(address).await;
    let mut submitted = Vec::new();
    for _ in 0..5 {
        submitted.push(submit(address, &hash, 9).await);
    }
    let mut first_two = worker.until_begun(2).await;
    first_two.sort();
    let mut expected = submitted[..2].to_vec();
    expected.sort();
    assert_eq!(first_two, expected);
    for (finished, next) in [(0, 2), (1, 3), (2, 4)] {
        worker.tell(&submitted[finished], WorkerOutcome::Succeeded(Vec::new()));
        let began = worker.until_begun(next + 1).await;
        assert_eq!(
            began[next], submitted[next],
            "the job started after {finished}"
        );
    }
    for job in &submitted[3..] {
        worker.tell(job, WorkerOutcome::Succeeded(Vec::new()));
    }
    for job in &submitted {
        until_state(address, job, "succeeded").await;
    }
    assert_eq!(worker.script().most, 2);
}

/// A server that starts on a store where a job was left running, as one
/// killed while it ran leaves it, fails that job with the reason and runs
/// the queued one.
async fn a_restart_fails_the_jobs_that_ran_and_runs_the_queued_ones<B: Backend>(backend: &B) {
    let store = backend.store();
    store
        .upsert_candles(&market(), &candles(10))
        .expect("the candles");
    let bundle = bundle();
    let hash = BundleHash::from_digest(*bundle.code_hash().digest());
    store
        .store_bundle(&hash, &bundle.canonical_tar(), T)
        .expect("the bundle");
    let request = json!({
        "bundle": bundle.code_hash().to_string(),
        "data": {"exchange": "binance", "category": "spot", "symbol": "BTCUSDT", "interval": "1m",
                 "start_open_ms": T, "end_open_ms": T + 9 * MINUTE},
        "backtest": {"initial_cash": "10000", "fee_bps": "10", "close_out_at_end": true},
    })
    .to_string();
    let (ran, waited) = (JobId::generate(), JobId::generate());
    for job_id in [&ran, &waited] {
        store
            .submit_job(&NewJob {
                job_id: job_id.clone(),
                bundle: hash,
                request_json: request.clone(),
                submitted_at_ms: T,
            })
            .expect("a job");
    }
    let claimed = store.claim_job(T + 1).expect("a claim").expect("a job");
    assert_eq!(claimed.job_id, ran);

    let (worker, _release) = Scripted::new();
    let store = ServerStore::new(backend.store(), StoreLimits::default());
    let jobs = Jobs::new(
        store.clone(),
        worker.clone(),
        JobLimits {
            concurrency: 1,
            max_span_bars: server::DEFAULT_MAX_SPAN_BARS,
        },
    );
    assert_eq!(jobs.start().await.expect("a started queue"), 1);
    let address = serve(store, jobs).await;
    let failed = state(address, ran.as_str()).await;
    assert_eq!(failed["kind"], "failed");
    assert_eq!(failed["started_at_ms"], T + 1);
    assert_eq!(
        failed["error"],
        json!({"code": "internal", "message": STOPPED})
    );
    assert_eq!(worker.until_begun(1).await, [waited.to_string()]);
    worker.tell(waited.as_str(), WorkerOutcome::Succeeded(Vec::new()));
    until_state(address, waited.as_str(), "succeeded").await;
}

/// A server that stops fails the job it runs, so that its client submits it
/// again, leaves the queued one queued, and the next server runs that.
async fn a_stop_fails_the_running_jobs_and_leaves_the_queued_ones<B: Backend>(backend: &B) {
    backend
        .store()
        .upsert_candles(&market(), &candles(10))
        .expect("the candles");
    let (worker, _release) = Scripted::new();
    let (store, jobs) = jobs(backend, worker.clone(), 1).await;
    let address = serve(store, jobs.clone()).await;
    let hash = upload(address).await;
    let running = submit(address, &hash, 9).await;
    worker.until_begun(1).await;
    let queued = submit(address, &hash, 9).await;

    let stopped = tokio::time::timeout(Duration::from_secs(10), jobs.stop())
        .await
        .expect("a stop within ten seconds");
    assert_eq!(stopped, 1);
    let failed = state(address, &running).await;
    assert_eq!(failed["kind"], "failed");
    assert_eq!(
        failed["error"],
        json!({"code": "internal", "message": STOPPED})
    );
    assert_eq!(state(address, &queued).await, json!({"kind": "queued"}));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        worker.began(),
        std::slice::from_ref(&running),
        "a stopped server ran a job"
    );

    let (next, _release_next) = Scripted::new();
    let (store, jobs) = self::jobs(backend, next.clone(), 1).await;
    let address = serve(store, jobs).await;
    assert_eq!(next.until_begun(1).await, std::slice::from_ref(&queued));
    next.tell(&queued, WorkerOutcome::Succeeded(Vec::new()));
    until_state(address, &queued, "succeeded").await;
}

/// A server with no worker runtime fails each job as it starts.
async fn a_server_without_a_runtime_fails_each_job_as_it_starts<B: Backend>(backend: &B) {
    backend
        .store()
        .upsert_candles(&market(), &candles(10))
        .expect("the candles");
    let (store, jobs) = jobs(backend, NoRuntime, 1).await;
    let address = serve(store, jobs).await;
    let hash = upload(address).await;
    let job = submit(address, &hash, 9).await;
    let failed = until_state(address, &job, "failed").await;
    assert_eq!(
        failed["error"],
        json!({
            "code": "unavailable",
            "message": "this server cannot run Python bundles yet; a later QuantForge release \
                        brings the worker runtime",
        })
    );
    assert!(failed["started_at_ms"].as_i64().is_some(), "{failed}");
}

/// Each scenario as a test per backend: `<scenario>::sqlite`, and
/// `<scenario>::postgres` with the feature.
macro_rules! on_each_backend {
    ($($scenario:ident),* $(,)?) => {$(
        mod $scenario {
            #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
            async fn sqlite() {
                super::$scenario(&super::Sqlite::new()).await;
            }

            #[cfg(feature = "postgres")]
            #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
            async fn postgres() {
                if let Some(backend) = super::postgres::Postgres::new() {
                    super::$scenario(&backend).await;
                }
            }
        }
    )*};
}

on_each_backend!(
    a_job_runs_on_its_candles_and_its_end_is_read_back,
    a_cancel_stops_a_running_job_and_frees_its_slot,
    jobs_start_first_submitted_first_and_no_more_at_once_than_allowed,
    a_restart_fails_the_jobs_that_ran_and_runs_the_queued_ones,
    a_stop_fails_the_running_jobs_and_leaves_the_queued_ones,
    a_server_without_a_runtime_fails_each_job_as_it_starts,
    a_succeeded_jobs_result_is_downloaded_and_outlives_its_server,
);

#[cfg(feature = "postgres")]
#[allow(dead_code, reason = "the storage tests use the rest of it")]
#[path = "storage/postgres/server.rs"]
mod server_helpers;

/// The store on a database of its own on the server
/// `QUANTFORGE_TEST_POSTGRES_URL` names, with the server's pool.
#[cfg(feature = "postgres")]
mod postgres {
    use super::server_helpers::{IsolatedDatabase, VARIABLE, server_url, settings};
    use super::*;
    use quantforge::storage::postgres::PostgresStore;

    pub(super) struct Postgres {
        // Dropped before the database.
        store: PostgresStore,
        _database: IsolatedDatabase,
    }

    impl Postgres {
        pub(super) fn new() -> Option<Self> {
            let Some(server) = server_url() else {
                eprintln!("skipping: {VARIABLE} is unset");
                return None;
            };
            let database = IsolatedDatabase::create(&server);
            let mut settings = settings(&database.url);
            settings.pool_size = 8;
            let store = PostgresStore::connect(&settings).expect("a store");
            CandleStore::init(&store).expect("init");
            Some(Self {
                store,
                _database: database,
            })
        }
    }

    impl Backend for Postgres {
        type Store = PostgresStore;

        fn store(&self) -> PostgresStore {
            self.store.within(Duration::from_secs(5))
        }
    }
}
