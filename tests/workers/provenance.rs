//! A job's provenance and its rerun with real workers, through the queue,
//! on three days of candles: the first two packed, the second corrected
//! since, the third packed by no refresh. A job records every segment that
//! holds its bars, and a rerun in a worker of its own comes out the same
//! however the candles were corrected since, on SQLite and, where
//! `QUANTFORGE_TEST_POSTGRES_URL` names a server, on PostgreSQL. With the
//! server, the same job on both stores has the same inputs, identity and
//! result, and a store copied from SQLite into PostgreSQL, or restored from
//! a copy of its file, reruns the jobs it holds to the same result.

use std::net::SocketAddr;
use std::time::Duration;

use quantforge::protocol::ContentHash;
use quantforge::protocol::jobs::{Provenance, RerunReport, RerunVerdict};
use quantforge::server::{
    self, History, HistoryLimits, JobLimits, Jobs, LogLimits, ServerStore, StoreLimits,
};
use quantforge::{Candle, CandleStore, SegmentStore};
use serde_json::{Value, json};
use tokio::net::TcpListener;

use crate::backends::{Backend, Sqlite, client, ended, market, upload};
use crate::support::{MINUTE, Pool, T, bundle, candles, roomy};

const DAY: i64 = 86_400_000;
/// The job's range: a hundred minutes into the first day to 700 minutes
/// into the third.
const START: i64 = T + 100 * MINUTE;
const END: i64 = T + 2 * DAY + 700 * MINUTE;

/// The second day's correction: one bar, another close.
fn correction() -> Candle {
    let mut bar = candles(3 * 1440)[1440 + 600].clone();
    bar.close = rust_decimal::Decimal::new(1, 0);
    bar
}

/// The store as the job finds it: three days, the first two packed, then
/// the second corrected, which leaves it to be packed again.
fn stored<B: Backend>(backend: &B) {
    let store = backend.store();
    store
        .upsert_candles(&market(), &candles(3 * 1440))
        .expect("the days");
    assert_eq!(
        store.refresh_segments(&market()).expect("a refresh").built,
        2
    );
    store
        .upsert_candles(&market(), &[correction()])
        .expect("the correction");
}

/// Every bar corrected, and the closed days packed again.
fn corrected<B: Backend>(backend: &B) {
    let store = backend.store();
    let mut bars = candles(3 * 1440);
    for bar in &mut bars {
        bar.close = rust_decimal::Decimal::new(7, 1);
    }
    store.upsert_candles(&market(), &bars).expect("corrections");
    store.refresh_segments(&market()).expect("a refresh");
}

/// A started queue over `store` on `pool`, served, and its jobs, which a
/// test stops before it copies the store.
async fn serve(store: ServerStore, pool: &Pool) -> (SocketAddr, Jobs) {
    let jobs = Jobs::new(
        store.clone(),
        pool.pool.clone(),
        JobLimits {
            concurrency: 1,
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
        jobs.clone(),
        std::future::pending(),
    ));
    (address, jobs)
}

fn served<B: Backend>(backend: &B) -> ServerStore {
    ServerStore::new(backend.store(), StoreLimits::default())
}

/// Runs the `backtest` workload over the range to its end, and answers
/// the job's ID once it has succeeded.
async fn ran(address: SocketAddr) -> String {
    let hash: ContentHash = upload(address, &bundle(r#"workload = "backtest""#)).await;
    let body = json!({
        "bundle": hash.to_string(),
        "data": {"exchange": "binance", "category": "spot", "symbol": "BTCUSDT",
                 "interval": "1m", "start_open_ms": START, "end_open_ms": END},
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
    let job = status["job_id"].as_str().expect("a job id").to_string();
    let state = ended(address, &job).await;
    assert_eq!(state["kind"], "succeeded", "{state}");
    job
}

async fn provenance(address: SocketAddr, job: &str) -> Provenance {
    let response = client()
        .get(format!("http://{address}/v1/backtests/{job}/provenance"))
        .send()
        .await
        .expect("a response");
    assert_eq!(response.status(), 200);
    response.json().await.expect("a provenance")
}

async fn rerun(address: SocketAddr, job: &str) -> RerunReport {
    let response = client()
        .post(format!("http://{address}/v1/backtests/{job}/rerun"))
        .timeout(Duration::from_secs(60))
        .send()
        .await
        .expect("a response");
    assert_eq!(response.status(), 200);
    response.json().await.expect("a report")
}

/// Asserts that a rerun of `job` came out the same as `recorded` lists.
async fn verified(address: SocketAddr, job: &str, recorded: &Provenance) {
    let report = rerun(address, job).await;
    assert_eq!(report.verdict, RerunVerdict::Verified, "{report:?}");
    assert_eq!(report.identity, recorded.identity);
    let listed = recorded.results.as_ref().expect("the results");
    let rerun: Vec<(&str, ContentHash)> = report
        .results
        .iter()
        .map(|compared| (compared.name.as_str(), compared.rerun.expect("a file")))
        .collect();
    let recorded: Vec<(&str, ContentHash)> = listed
        .iter()
        .map(|file| (file.name.as_str(), file.hash))
        .collect();
    assert_eq!(rerun, recorded);
}

/// A job's provenance names the first day's segment and the two packed for
/// it; a rerun in a worker of its own, after every bar was corrected,
/// comes out the same.
async fn a_job_records_what_it_ran_on_and_reruns_to_the_same_result<B: Backend>(backend: &B) {
    stored(backend);
    let pool = Pool::start(1, roomy());
    let (address, _jobs) = serve(served(backend), &pool).await;
    let job = ran(address).await;
    let recorded = provenance(address, &job).await;
    let data = &recorded.inputs.data;
    let spans: Vec<(i64, i64, u32)> = data
        .segments
        .iter()
        .map(|segment| {
            (
                segment.first_open_ms,
                segment.last_open_ms,
                segment.bar_count,
            )
        })
        .collect();
    assert_eq!(
        spans,
        [
            (T, T + DAY - MINUTE, 1440),
            (T + DAY, T + 2 * DAY - MINUTE, 1440),
            (T + 2 * DAY, END, 701)
        ]
    );
    assert_eq!(data.bar_count, 1340 + 1440 + 701);
    assert_eq!(
        recorded.identity,
        recorded.inputs.identity().expect("an identity")
    );
    assert_eq!(recorded.recorded.backend, B::NAME);
    // The job ran on the corrected day: its result is a local run's on it.
    let mut bars = candles(3 * 1440);
    bars[1440 + 600] = correction();
    bars.retain(|bar| (START..=END).contains(&bar.open_time_ms));
    let local: Vec<ContentHash> = crate::results::local_files(&bars, 2, 3)
        .iter()
        .map(|file| file.reference.hash)
        .collect();
    let listed: Vec<ContentHash> = recorded
        .results
        .iter()
        .flatten()
        .map(|file| file.hash)
        .collect();
    assert_eq!(listed, local);

    corrected(backend);
    verified(address, &job, &recorded).await;
}

/// The scenario as a test per backend: `sqlite`, and `postgres` with the
/// feature.
mod a_job_records_what_it_ran_on_and_reruns_to_the_same_result {
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sqlite() {
        super::a_job_records_what_it_ran_on_and_reruns_to_the_same_result(&super::Sqlite::new())
            .await;
    }

    #[cfg(feature = "postgres")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn postgres() {
        if let Some(backend) = crate::backends::postgres::Postgres::new() {
            super::a_job_records_what_it_ran_on_and_reruns_to_the_same_result(&backend).await;
        }
    }
}

/// The same job on SQLite and on PostgreSQL: the same inputs, identity,
/// strategy and result; only where it was recorded differs.
#[cfg(feature = "postgres")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_same_job_on_either_store_has_the_same_inputs_identity_and_result() {
    let Some(postgres) = crate::backends::postgres::Postgres::new() else {
        return;
    };
    let sqlite = Sqlite::new();
    stored(&sqlite);
    stored(&postgres);
    let pool = Pool::start(1, roomy());
    let (on_sqlite, _sqlite_jobs) = serve(served(&sqlite), &pool).await;
    let (on_postgres, _postgres_jobs) = serve(served(&postgres), &pool).await;
    let (first, second) = (ran(on_sqlite).await, ran(on_postgres).await);
    let (one, other) = (
        provenance(on_sqlite, &first).await,
        provenance(on_postgres, &second).await,
    );
    assert_eq!(one.inputs, other.inputs);
    assert_eq!(one.identity, other.identity);
    assert_eq!(one.strategy, other.strategy);
    assert_eq!(one.results, other.results);
    assert_eq!(
        (
            one.recorded.backend.as_str(),
            other.recorded.backend.as_str()
        ),
        ("sqlite", "postgres")
    );
    assert_eq!(one.recorded.schema_version, other.recorded.schema_version);
    verified(on_sqlite, &first, &one).await;
    verified(on_postgres, &second, &other).await;
}

/// A store copied from SQLite into PostgreSQL holds every job's provenance
/// as it was recorded, and reruns the job to the same result, before and
/// after a correction on the copy; so does a copy of the SQLite file, as a
/// restore leaves it.
#[cfg(feature = "postgres")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_store_copied_or_restored_reruns_its_jobs_to_the_same_result() {
    use quantforge::storage::sqlite::SqliteSnapshot;
    use quantforge::{SqliteStore, TransferConfig, TransferEngine};

    let Some(postgres) = crate::backends::postgres::Postgres::new() else {
        return;
    };
    let sqlite = Sqlite::new();
    stored(&sqlite);
    let pool = Pool::start(1, roomy());
    let (address, jobs) = serve(served(&sqlite), &pool).await;
    let job = ran(address).await;
    let recorded = provenance(address, &job).await;
    jobs.stop().await;

    let target = postgres.store();
    let copied = tokio::task::spawn_blocking({
        let path = sqlite.path.clone();
        move || {
            let snapshot = SqliteSnapshot::open(&path).expect("the source");
            TransferEngine::new(&snapshot, &target, TransferConfig::default())
                .copy()
                .map(|summary| summary.tables.len())
        }
    })
    .await
    .expect("the copy's thread");
    assert!(copied.expect("the copy") > 0);
    let (on_postgres, _jobs) = serve(served(&postgres), &pool).await;
    assert_eq!(provenance(on_postgres, &job).await, recorded);
    verified(on_postgres, &job, &recorded).await;
    corrected(&postgres);
    verified(on_postgres, &job, &recorded).await;

    let restored = tempfile::tempdir().expect("a directory");
    let path = restored.path().join("market.sqlite");
    std::fs::copy(&sqlite.path, &path).expect("the file copied");
    let store = SqliteStore::new(&path).within(Duration::from_secs(5));
    CandleStore::init(&store).expect("init");
    let (on_restored, _jobs) = serve(ServerStore::new(store, StoreLimits::default()), &pool).await;
    assert_eq!(provenance(on_restored, &job).await, recorded);
    verified(on_restored, &job, &recorded).await;
}
