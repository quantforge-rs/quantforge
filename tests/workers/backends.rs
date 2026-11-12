//! Jobs through the server's queue to real workers, with the store on
//! SQLite and, where `QUANTFORGE_TEST_POSTGRES_URL` names a server, on
//! PostgreSQL: a job runs on exactly the candles its server read for it, a
//! crash fails only its own job, every job gets a worker of its own, a
//! worker sees nothing of the store its server holds open, a backtest run
//! in a worker is downloaded as the files a local run writes, and what a
//! job writes is followed while its worker runs and replayed from the
//! store afterwards. Each scenario is one generic body, run once per
//! backend.

use std::any::Any;
use std::net::SocketAddr;
use std::time::Duration;

use futures_util::StreamExt;
use quantforge::protocol::ContentHash;
use quantforge::protocol::bundle::Bundle;
use quantforge::server::{
    self, History, HistoryLimits, JobLimits, Jobs, LogLimits, ServedStore, ServerStore,
    StoreLimits, WorkerPool,
};
use quantforge::{CandleStore, ExchangeId, Interval, MarketId, SqliteStore, Symbol};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use crate::support::{
    MINUTE, Pool, T, bundle, candles, digest, given_env, input, report, roomy, until, worker_env,
};

/// A store of one backend.
trait Backend {
    type Store: ServedStore + 'static;

    /// A handle on the store, each call bounded as `serve run` bounds them.
    fn store(&self) -> Self::Store;

    /// What a server holds open of its store: a connection, with its
    /// descriptor, until dropped.
    fn hold_open(&self) -> Box<dyn Any>;
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

    fn hold_open(&self) -> Box<dyn Any> {
        let connection = rusqlite::Connection::open(&self.path).expect("a connection");
        connection
            .query_row("SELECT count(*) FROM candles", [], |row| {
                row.get::<_, i64>(0)
            })
            .expect("a read");
        Box::new(connection)
    }
}

fn market() -> MarketId {
    MarketId::new(
        ExchangeId::BinanceSpot,
        Symbol::new("BTCUSDT").expect("a symbol"),
        Interval::M1,
    )
}

/// The job routes over `backend`, running `concurrency` jobs at once on
/// `pool`, started, and served on a free loopback port.
async fn serve<B: Backend>(backend: &B, pool: &WorkerPool, concurrency: usize) -> SocketAddr {
    let store = ServerStore::new(backend.store(), StoreLimits::default());
    let jobs = Jobs::new(
        store.clone(),
        pool.clone(),
        JobLimits {
            concurrency,
            max_span_bars: server::DEFAULT_MAX_SPAN_BARS,
            log: LogLimits::default(),
        },
    );
    jobs.start().await.expect("a started queue");
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

/// Uploads `bundle`, as a client does.
async fn upload(address: SocketAddr, bundle: &Bundle) -> ContentHash {
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

/// The job's state once it is final.
async fn ended(address: SocketAddr, job: &str) -> Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let response = client()
            .get(format!("http://{address}/v1/backtests/{job}"))
            .send()
            .await
            .expect("a response");
        let status: Value = response.json().await.expect("a status");
        let state = status["state"].clone();
        if ["succeeded", "failed", "cancelled"].contains(&state["kind"].as_str().unwrap_or("")) {
            return state;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "job {job} is {state} after thirty seconds"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// A job that succeeds only on exactly `candles`.
fn expecting(candles: &[quantforge::Candle]) -> Bundle {
    bundle(&format!(
        "workload = \"expect\"\ndigest = \"{}\"",
        digest(candles)
    ))
}

/// A job runs on exactly the candles its server read for it from the store,
/// every decimal with its scale.
async fn a_job_runs_on_the_exact_candles_its_server_read<B: Backend>(backend: &B) {
    let stored = candles(25_000);
    backend
        .store()
        .upsert_candles(&market(), &stored)
        .expect("the candles");
    let pool = Pool::start(1, roomy());
    let address = serve(backend, &pool.pool, 1).await;

    let hash = upload(address, &expecting(&stored)).await;
    let job = submit(address, &hash, 24_999).await;
    let state = ended(address, &job).await;
    assert_eq!(state["kind"], "succeeded", "{state}");

    // The same bundle on a range one bar shorter gets other candles.
    let shorter = submit(address, &hash, 24_998).await;
    let state = ended(address, &shorter).await;
    assert_eq!(state["kind"], "failed", "{state}");
    assert_eq!(state["error"]["code"], "invalid_request");
    assert_eq!(
        state["error"]["message"],
        format!(
            "the 24999 candles hash to {}, not {}",
            digest(&stored[..24_999]),
            digest(&stored)
        )
    );
}

/// A worker that crashes fails its own job, and the queue runs the next on a
/// fresh worker while the server answers throughout.
async fn a_crash_fails_only_its_own_job<B: Backend>(backend: &B) {
    let stored = candles(100);
    backend
        .store()
        .upsert_candles(&market(), &stored)
        .expect("the candles");
    let pool = Pool::start(2, roomy());
    let address = serve(backend, &pool.pool, 2).await;
    let crash = upload(address, &bundle(r#"workload = "crash""#)).await;
    let expect = upload(address, &expecting(&stored)).await;

    let crashed = submit(address, &crash, 99).await;
    let ran = submit(address, &expect, 99).await;
    let state = ended(address, &crashed).await;
    assert_eq!(state["kind"], "failed", "{state}");
    assert_eq!(state["error"]["code"], "internal");
    let message = state["error"]["message"].as_str().expect("a message");
    assert!(
        message.starts_with("the job's worker stopped before the job finished: "),
        "{message}"
    );
    assert_eq!(ended(address, &ran).await["kind"], "succeeded");

    let health = client()
        .get(format!("http://{address}/healthz"))
        .send()
        .await
        .expect("a response");
    assert_eq!(health.status(), 200);
    let next = submit(address, &expect, 99).await;
    assert_eq!(ended(address, &next).await["kind"], "succeeded");
}

/// Every job gets a worker of its own: a cancelled job's worker is gone,
/// and the next job runs on another.
async fn every_job_gets_a_worker_of_its_own<B: Backend>(backend: &B) {
    backend
        .store()
        .upsert_candles(&market(), &candles(10))
        .expect("the candles");
    let pool = Pool::start(1, roomy());
    let address = serve(backend, &pool.pool, 1).await;
    let hold = upload(address, &bundle(r#"workload = "hold""#)).await;

    let mut workers = Vec::new();
    for _ in 0..2 {
        let job = submit(address, &hold, 9).await;
        let finder = pool.pool.clone();
        let wanted = job.clone();
        let pid = tokio::task::spawn_blocking(move || {
            until("the job's worker", || {
                finder
                    .running()
                    .into_iter()
                    .find(|running| running.job_id == wanted)
                    .map(|running| running.pid)
            })
        })
        .await
        .expect("the worker");
        workers.push(pid);
        let cancelled = client()
            .post(format!("http://{address}/v1/backtests/{job}/cancel"))
            .send()
            .await
            .expect("a response");
        assert_eq!(cancelled.status(), 200);
        assert_eq!(ended(address, &job).await["kind"], "cancelled");
    }
    assert_ne!(workers[0], workers[1]);
    #[cfg(unix)]
    {
        let first = workers[0];
        let gone = tokio::task::spawn_blocking(move || {
            until("the first worker's end", || {
                (!crate::support::alive(first)).then_some(())
            })
        })
        .await;
        assert!(gone.is_ok(), "the cancelled job's worker outlived it");
    }
}

/// A backtest run in a worker, through the queue: the job succeeds with
/// three results, and each downloads as the file a run of the same engine
/// in this process writes, byte for byte, whichever store keeps it.
async fn a_backtest_run_in_a_worker_downloads_as_a_local_runs_files<B: Backend>(backend: &B) {
    let stored = candles(12_000);
    backend
        .store()
        .upsert_candles(&market(), &stored)
        .expect("the candles");
    let pool = Pool::start(1, roomy());
    let address = serve(backend, &pool.pool, 1).await;
    let hash = upload(address, &bundle(r#"workload = "backtest""#)).await;
    let job = submit(address, &hash, 11_999).await;
    let state = ended(address, &job).await;
    assert_eq!(state["kind"], "succeeded", "{state}");

    let expected = crate::results::local_files(&stored, 2, 3);
    let listed: Vec<Value> = expected
        .iter()
        .map(|file| serde_json::to_value(&file.reference).expect("json"))
        .collect();
    assert_eq!(state["results"], Value::Array(listed));

    let answered: Value = client()
        .get(format!("http://{address}/v1/backtests/{job}/result"))
        .send()
        .await
        .expect("a response")
        .json()
        .await
        .expect("a result");
    assert_eq!(
        answered["metrics"],
        serde_json::from_slice::<Value>(&expected[0].bytes).expect("metrics")
    );
    for (link, file) in answered["results"]
        .as_array()
        .expect("the links")
        .iter()
        .zip(&expected)
    {
        let path = link["path"].as_str().expect("a path");
        let response = client()
            .get(format!("http://{address}{path}"))
            .send()
            .await
            .expect("a response");
        assert_eq!(response.status(), 200, "{path}");
        let body = response.bytes().await.expect("a body");
        assert!(body.as_ref() == file.bytes.as_slice(), "{path}: every byte");
    }
}

type LogStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Opens the job's log stream, from the line after `after`.
async fn follow(address: SocketAddr, job: &str, after: u64) -> LogStream {
    let url = format!("ws://{address}/v1/backtests/{job}/logs?after={after}");
    let (stream, _) = tokio_tungstenite::connect_async(url)
        .await
        .expect("a log stream");
    stream
}

/// The stream's next frame, or `None` once the server closed it as it
/// closes the stream of a job that has ended.
async fn next_frame(stream: &mut LogStream) -> Option<Value> {
    loop {
        let message = tokio::time::timeout(Duration::from_secs(30), stream.next())
            .await
            .expect("the stream says something within thirty seconds");
        match message {
            Some(Ok(Message::Text(text))) => {
                return Some(serde_json::from_str(text.as_str()).expect("a frame"));
            }
            Some(Ok(Message::Close(close))) => {
                assert_eq!(close.map(|close| close.code), Some(CloseCode::Normal));
                return None;
            }
            Some(Ok(_)) => {}
            other => panic!("a log stream ended with {other:?}"),
        }
    }
}

/// Every frame of a stream to its close.
async fn frames(mut stream: LogStream) -> Vec<Value> {
    let mut frames = Vec::new();
    while let Some(frame) = next_frame(&mut stream).await {
        frames.push(frame);
    }
    frames
}

/// One page of the job's log.
async fn page(address: SocketAddr, job: &str, query: &str) -> Value {
    let response = client()
        .get(format!("http://{address}/v1/backtests/{job}/logs{query}"))
        .send()
        .await
        .expect("a response");
    assert_eq!(response.status(), 200, "{query}");
    response.json().await.expect("a page")
}

/// What a job writes in a real worker is followed while the worker runs,
/// line by line with the job's progress, and replayed from the store once
/// the job has ended, by the server that ran it and by another on its
/// store; a job that writes more than a log keeps leaves the newest lines
/// and a count of the rest in the store.
async fn a_jobs_log_is_followed_while_its_worker_runs_and_replayed_from_the_store<B: Backend>(
    backend: &B,
) {
    backend
        .store()
        .upsert_candles(&market(), &candles(20))
        .expect("the candles");
    let pool = Pool::start(1, roomy());
    let address = serve(backend, &pool.pool, 1).await;
    let all_done = json!({"type": "progress", "bars_done": 20, "bars_total": 20});

    // A job that writes twelve lines, a moment apart, and then holds until
    // it is stopped: whatever its stream sends, it sends while the job runs.
    let talk = "workload = \"log\"\nlines = 12\npause_ms = 25\nhold = true";
    let job = submit(address, &upload(address, &bundle(talk)).await, 19).await;
    let mut stream = follow(address, &job, 0).await;
    let (mut lines, mut progress) = (Vec::new(), Vec::new());
    while lines.len() < 12 || progress.last() != Some(&all_done) {
        let frame = next_frame(&mut stream).await.expect("a frame of a job");
        match frame["type"].as_str() {
            Some("line") => lines.push(frame),
            Some("progress") => progress.push(frame),
            _ => panic!("a running job's stream sent {frame}"),
        }
    }
    let running: Value = client()
        .get(format!("http://{address}/v1/backtests/{job}"))
        .send()
        .await
        .expect("a response")
        .json()
        .await
        .expect("a status");
    assert_eq!(running["state"]["kind"], "running", "followed live");
    for (line, n) in lines.iter().zip(1_u64..) {
        let level = ["debug", "info", "warning", "error"][(n as usize - 1) % 4];
        assert_eq!(
            (&line["seq"], &line["level"], &line["message"]),
            (&json!(n), &json!(level), &json!(format!("line {n} of 12")))
        );
    }
    let done: Vec<u64> = progress
        .iter()
        .map(|frame| frame["bars_done"].as_u64().expect("a count"))
        .collect();
    assert!(done.windows(2).all(|pair| pair[0] < pair[1]), "{done:?}");

    // Stopped by its client: the stream ends with the job, and what the
    // job wrote is kept with it.
    let cancelled = client()
        .post(format!("http://{address}/v1/backtests/{job}/cancel"))
        .send()
        .await
        .expect("a response");
    assert_eq!(cancelled.status(), 200);
    assert_eq!(next_frame(&mut stream).await, Some(json!({"type": "end"})));
    assert_eq!(next_frame(&mut stream).await, None);
    assert_eq!(ended(address, &job).await["kind"], "cancelled");

    // Replayed afterwards, each line as it was sent while the job ran,
    // from the server that ran the job and from another on its store.
    let mut expected = lines.clone();
    expected.extend([all_done.clone(), json!({"type": "end"})]);
    let listed: Vec<Value> = lines
        .iter()
        .map(|line| {
            let mut line = line.clone();
            line.as_object_mut().expect("a line").remove("type");
            line
        })
        .collect();
    let other = serve(backend, &pool.pool, 1).await;
    for address in [address, other] {
        assert_eq!(frames(follow(address, &job, 0).await).await, expected);
        assert_eq!(
            frames(follow(address, &job, 9).await).await,
            expected[9..],
            "from a cursor"
        );
        let read = page(address, &job, "").await;
        assert_eq!(read["lines"], Value::Array(listed.clone()));
        assert_eq!(read["progress"], json!({"bars_done": 20, "bars_total": 20}));
        assert_eq!(
            (&read["next_after"], &read["complete"]),
            (&json!(12), &json!(true))
        );
    }

    // A job that writes three times what a log keeps, and succeeds.
    let flood = "workload = \"log\"\nlines = 30000\nbytes = 100";
    let job = submit(address, &upload(address, &bundle(flood)).await, 19).await;
    let state = ended(address, &job).await;
    assert_eq!(state["kind"], "succeeded", "{state}");
    let first = page(other, &job, "?limit=1").await;
    assert_eq!(first["dropped_lines"], 20_000);
    assert_eq!(first["lines"][0]["seq"], 20_001);
    assert_eq!(first["complete"], false);
    let sent = frames(follow(other, &job, 29_998).await).await;
    assert_eq!(sent.len(), 4, "{sent:?}");
    assert_eq!(
        (&sent[0]["seq"], &sent[1]["seq"]),
        (&json!(29_999), &json!(30_000))
    );
    let last = sent[1]["message"].as_str().expect("text");
    assert!(last.starts_with("line 30000 of 30000..."), "{last}");
    assert_eq!(last.len(), 100);
    assert_eq!(sent[2..], [all_done, json!({"type": "end"})]);
    let whole = frames(follow(address, &job, 0).await).await;
    assert_eq!(
        whole.len(),
        10_003,
        "what was dropped, the lines kept, and the end"
    );
    assert_eq!(
        whole[0],
        json!({"type": "truncated", "dropped_lines": 20_000})
    );
}

/// A worker gets no environment and no descriptor of the server's while
/// the server holds its store open.
async fn a_worker_sees_nothing_of_the_store_its_server_holds_open<B: Backend>(backend: &B) {
    let _open = backend.hold_open();
    let pool = Pool::start(1, roomy());
    let seen = tokio::task::spawn_blocking(move || {
        report(&pool.run(input(r#"workload = "inspect""#, candles(5))))
    })
    .await
    .expect("the job");
    assert_eq!(given_env(&seen), worker_env());
    if cfg!(unix) {
        assert_eq!(seen["descriptors"], json!([0, 1, 2]));
    }
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
    a_job_runs_on_the_exact_candles_its_server_read,
    a_crash_fails_only_its_own_job,
    every_job_gets_a_worker_of_its_own,
    a_worker_sees_nothing_of_the_store_its_server_holds_open,
    a_backtest_run_in_a_worker_downloads_as_a_local_runs_files,
    a_jobs_log_is_followed_while_its_worker_runs_and_replayed_from_the_store,
);

#[cfg(feature = "postgres")]
#[allow(dead_code, reason = "the storage tests use the rest of it")]
#[path = "../storage/postgres/server.rs"]
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

        fn hold_open(&self) -> Box<dyn Any> {
            let store = self.store();
            store.market_coverage().expect("a read");
            Box::new(store)
        }
    }
}
