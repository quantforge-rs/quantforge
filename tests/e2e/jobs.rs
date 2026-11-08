//! `serve run`'s job routes as a process, on each backend: a bundle
//! uploaded and a job submitted, then failed as it starts, since this build
//! has no worker runtime; and a server that starts on jobs a killed one
//! left, failing the one that ran and running the queued one. On
//! PostgreSQL every scenario also proves that nothing was created beside
//! the store.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use quantforge::protocol::bundle::{Bundle, BundleLimits};
use quantforge::{
    BundleHash, Candle, CandleStore, ExchangeId, Interval, JobId, JobStore, MarketId, NewJob,
    Symbol,
};
use rust_decimal::Decimal;
use serde_json::{Value, json};

use crate::harness::{TestBed, on_each_backend};
use crate::history::{Fetched, request};
use crate::serve::Served;

/// 2026-10-20T00:00:00Z.
const T: i64 = 1_792_454_400_000;
const MINUTE: i64 = 60_000;

const NO_RUNTIME: &str = "this server cannot run Python bundles yet; a later QuantForge release brings the worker \
     runtime";
const STOPPED: &str = "the server stopped while the job ran; submit it again";

fn market() -> MarketId {
    MarketId::new(
        ExchangeId::BinanceSpot,
        Symbol::new("BTCUSDT").expect("a symbol"),
        Interval::M1,
    )
}

/// Ten `1m` bars from `T`.
fn seed(bed: &TestBed) {
    let candles: Vec<Candle> = (0..10)
        .map(|i| {
            let close = Decimal::new(100 + i, 0);
            Candle {
                open_time_ms: T + i * MINUTE,
                close_time_ms: T + (i + 1) * MINUTE - 1,
                open: close,
                high: close,
                low: close,
                close,
                volume: Decimal::ONE,
                trades: None,
            }
        })
        .collect();
    bed.store()
        .upsert_candles(&market(), &candles)
        .expect("the candles");
}

fn bundle() -> Bundle {
    Bundle::from_files(
        [
            (
                "quantforge.toml",
                "format = 1\nentry = \"strategy.py:Hold\"\nengine = \">=0.3.0, <0.4.0\"\npython \
                 = \"3.12\"\n",
            ),
            ("strategy.py", "class Hold:\n    pass\n"),
        ],
        &BundleLimits::default(),
    )
    .expect("a bundle")
}

/// A submission of the ten bars, running `bundle`.
fn submission(bundle: &Bundle) -> Value {
    json!({
        "bundle": bundle.code_hash().to_string(),
        "data": {"exchange": "binance", "category": "spot", "symbol": "BTCUSDT", "interval": "1m",
                 "start_open_ms": T, "end_open_ms": T + 9 * MINUTE},
        "backtest": {"initial_cash": "10000", "fee_bps": "10", "close_out_at_end": true},
    })
}

fn get(address: SocketAddr, path: &str) -> Fetched {
    request(address, "GET", path, &[], None)
}

/// The job's status once its state is `kind`, or a failed test after 30
/// seconds.
fn until_state(address: SocketAddr, job: &str, kind: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let fetched = get(address, &format!("/v1/backtests/{job}"));
        assert_eq!(fetched.status, 200, "{}", fetched.text());
        let status: Value = fetched.json();
        if status["state"]["kind"] == kind {
            return status;
        }
        assert!(
            Instant::now() < deadline,
            "job {job} is {}, not {kind}",
            status["state"]
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn serve_run_queues_a_job_and_fails_it_as_it_starts_without_a_worker_runtime(bed: &TestBed) {
    seed(bed);
    let served = Served::start(bed);
    let address = served.address;
    let bundle = bundle();
    let path = format!("/v1/bundles/{}", bundle.code_hash());
    let tar = bundle.canonical_tar();
    let tar_type = [("Content-Type", "application/x-tar")];
    let uploaded = request(address, "PUT", &path, &tar_type, Some(&tar));
    assert_eq!(uploaded.status, 201, "{}", uploaded.text());
    assert_eq!(
        uploaded.text(),
        format!(
            r#"{{"code_hash":"{}","size_bytes":{}}}"#,
            bundle.code_hash(),
            tar.len()
        )
    );
    let again = request(address, "PUT", &path, &tar_type, Some(&tar));
    assert_eq!(again.status, 200);

    let body = submission(&bundle).to_string();
    let json_type = [("Content-Type", "application/json")];
    let submitted = request(
        address,
        "POST",
        "/v1/backtests",
        &json_type,
        Some(body.as_bytes()),
    );
    assert_eq!(submitted.status, 201, "{}", submitted.text());
    let queued: Value = submitted.json();
    let job = queued["job_id"].as_str().expect("a job id").to_string();
    assert_eq!(
        submitted.header("location"),
        Some(format!("/v1/backtests/{job}").as_str())
    );
    assert_eq!(queued["submission"], submission(&bundle));

    let failed = until_state(address, &job, "failed");
    assert_eq!(
        failed["state"]["error"],
        json!({"code": "unavailable", "message": NO_RUNTIME})
    );
    assert!(
        failed["state"]["started_at_ms"].as_i64().is_some(),
        "{failed}"
    );
    let list: Value = get(address, "/v1/backtests").json();
    assert_eq!(list, json!({"jobs": [failed]}));
}

fn serve_run_fails_the_jobs_left_running_and_runs_the_queued_ones_as_it_starts(bed: &TestBed) {
    seed(bed);
    let store = bed.store();
    let bundle = bundle();
    let hash = BundleHash::from_digest(*bundle.code_hash().digest());
    store
        .store_bundle(&hash, &bundle.canonical_tar(), T)
        .expect("the bundle");
    let (ran, waited) = (JobId::generate(), JobId::generate());
    for job_id in [&ran, &waited] {
        store
            .submit_job(&NewJob {
                job_id: job_id.clone(),
                bundle: hash,
                request_json: submission(&bundle).to_string(),
                submitted_at_ms: T,
            })
            .expect("a job");
    }
    let claimed = store.claim_job(T + 1).expect("a claim").expect("a job");
    assert_eq!(claimed.job_id, ran);

    let served = Served::start(bed);
    let failed: Value = get(served.address, &format!("/v1/backtests/{ran}")).json();
    assert_eq!(
        failed["state"],
        json!({
            "kind": "failed",
            "started_at_ms": T + 1,
            "finished_at_ms": failed["state"]["finished_at_ms"],
            "error": {"code": "internal", "message": STOPPED},
        })
    );
    let next = until_state(served.address, waited.as_str(), "failed");
    assert_eq!(next["state"]["error"]["message"], NO_RUNTIME);
}

on_each_backend!(
    serve_run_queues_a_job_and_fails_it_as_it_starts_without_a_worker_runtime,
    serve_run_fails_the_jobs_left_running_and_runs_the_queued_ones_as_it_starts,
);
