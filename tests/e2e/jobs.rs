//! `serve run`'s job routes as a process, on each backend: a bundle
//! uploaded and a job submitted, then failed as it starts, since this build
//! has no worker runtime; a server that starts on jobs a killed one left,
//! failing the one that ran and running the queued one; a succeeded job's
//! result, served file by file by every server that starts on its store;
//! and an ended job's log, served a page at a time and as a WebSocket
//! stream, the same by each. On PostgreSQL every scenario also proves that
//! nothing was created beside the store.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use quantforge::protocol::bundle::{Bundle, BundleLimits};
use quantforge::protocol::jobs::{LogLevel, ResultRef};
use quantforge::server::{BacktestOutput, JobLog, LogLimits, ResultFile};
use quantforge::{
    BacktestConfig, BacktestEngine, BuiltInStrategyConfig, BundleHash, Candle, CandleQuery,
    CandleStore, ExchangeId, Interval, JobId, JobOutcome, JobStore, MarketId, NewJob, StoredResult,
    Symbol,
};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use tungstenite::Message;
use tungstenite::stream::MaybeTlsStream;

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

/// The three files of an SMA cross over the bed's ten bars, as a server
/// that ran the job would have written them.
fn result_files(bed: &TestBed) -> Vec<ResultFile> {
    let candles = bed
        .store()
        .load_candles(
            &market(),
            CandleQuery {
                start_time_ms: None,
                end_time_ms: None,
                limit: None,
            },
        )
        .expect("the candles");
    let mut strategy = BuiltInStrategyConfig::SmaCross { fast: 2, slow: 3 }
        .build()
        .expect("a strategy");
    let result = BacktestEngine::new(BacktestConfig {
        initial_cash: Decimal::from(10_000),
        fee_bps: Decimal::from(10),
        close_out_at_end: true,
    })
    .run(&market(), &candles, strategy.as_mut())
    .expect("a backtest");
    assert_eq!(
        result.trade_count, 1,
        "the bars rise: one trade, closed out"
    );
    BacktestOutput::of(&result).files().expect("the files")
}

/// Everything a client reads of the job's result from the server at
/// `address`, each file held to its reference.
fn fetch_result(address: SocketAddr, job: &JobId, files: &[ResultFile]) -> Vec<Vec<u8>> {
    let answered = get(address, &format!("/v1/backtests/{job}/result"));
    assert_eq!(answered.status, 200, "{}", answered.text());
    let result: Value = answered.json();
    assert_eq!(result["job_id"], job.as_str());
    assert_eq!(result["format"], 1);
    assert_eq!(
        result["metrics"],
        serde_json::from_slice::<Value>(&files[0].bytes).expect("metrics")
    );
    let links = result["results"].as_array().expect("the links");
    let names: Vec<&str> = links
        .iter()
        .filter_map(|link| link["name"].as_str())
        .collect();
    assert_eq!(names, ["metrics", "trades", "equity"]);
    links
        .iter()
        .zip(files)
        .map(|(link, file)| {
            let path = link["path"].as_str().expect("a path");
            let tag = format!("\"{}\"", file.reference.hash);
            let fetched = get(address, path);
            assert_eq!(fetched.status, 200, "{path}: {}", fetched.text());
            assert_eq!(
                fetched.header("content-type"),
                Some(file.reference.media_type.as_str())
            );
            assert_eq!(fetched.header("etag"), Some(tag.as_str()));
            assert_eq!(
                fetched.header("cache-control"),
                Some("private, max-age=31536000, immutable")
            );
            assert_eq!(
                fetched.header("content-length"),
                Some(file.reference.size_bytes.to_string().as_str())
            );
            assert!(
                fetched.body() == file.bytes.as_slice(),
                "{path}: every byte"
            );
            let cached = request(address, "GET", path, &[("If-None-Match", &tag)], None);
            assert_eq!((cached.status, cached.body().len()), (304, 0), "{path}");
            fetched.body().to_vec()
        })
        .collect()
}

fn serve_run_serves_a_succeeded_jobs_result_from_its_store_start_after_start(bed: &TestBed) {
    seed(bed);
    let files = result_files(bed);
    let store = bed.store();
    let bundle = bundle();
    let hash = BundleHash::from_digest(*bundle.code_hash().digest());
    store
        .store_bundle(&hash, &bundle.canonical_tar(), T)
        .expect("the bundle");
    let (job, pending) = (JobId::generate(), JobId::generate());
    for job_id in [&job, &pending] {
        store
            .submit_job(&NewJob {
                job_id: job_id.clone(),
                bundle: hash,
                request_json: submission(&bundle).to_string(),
                submitted_at_ms: T,
            })
            .expect("a job");
    }
    // The first job as a server that ran it to its end leaves it.
    let claimed = store.claim_job(T + 1).expect("a claim").expect("a job");
    assert_eq!(claimed.job_id, job);
    let listed: Vec<&ResultRef> = files.iter().map(|file| &file.reference).collect();
    let stored: Vec<StoredResult> = files
        .iter()
        .map(|file| file.stored().expect("a stored result"))
        .collect();
    let outcome = JobOutcome::Succeeded {
        results_json: serde_json::to_string(&listed).expect("json"),
        results: stored,
    };
    assert!(store.finish_job(&job, &outcome, T + 2).expect("an end"));
    drop(store);

    let first = {
        let served = Served::start(bed);
        let status: Value = get(served.address, &format!("/v1/backtests/{job}")).json();
        assert_eq!(status["state"]["kind"], "succeeded", "{status}");
        assert_eq!(
            status["state"]["results"],
            serde_json::to_value(&listed).expect("json")
        );
        // The other job fails as it starts, and has no result.
        until_state(served.address, pending.as_str(), "failed");
        let none = get(
            served.address,
            &format!("/v1/backtests/{pending}/result/equity.arrow"),
        );
        assert_eq!(none.status, 409, "{}", none.text());
        assert_eq!(
            none.json::<Value>()["error"]["message"],
            format!("job {pending} failed; only a job that has succeeded has a result")
        );
        fetch_result(served.address, &job, &files)
    };
    // That server is gone; the next one on the store serves the same bytes.
    let served = Served::start(bed);
    assert_eq!(fetch_result(served.address, &job, &files), first);
}

/// A job the bed's store holds as running that no server runs: claimed
/// here, behind the back of the server on the store. That server claims
/// any job it finds queued, and fails it for want of a runtime, so this
/// queues one after another until its own claim is the one that took it.
#[cfg(unix)]
pub(crate) fn left_running(bed: &TestBed) -> JobId {
    let store = bed.store();
    let bundle = bundle();
    let hash = BundleHash::from_digest(*bundle.code_hash().digest());
    store
        .store_bundle(&hash, &bundle.canonical_tar(), T)
        .expect("the bundle");
    for _ in 0..50 {
        let job_id = JobId::generate();
        store
            .submit_job(&NewJob {
                job_id: job_id.clone(),
                bundle: hash,
                request_json: submission(&bundle).to_string(),
                submitted_at_ms: T,
            })
            .expect("a job");
        let claimed = store.claim_job(T + 1).expect("a claim");
        if claimed.is_some_and(|claimed| claimed.job_id == job_id) {
            return job_id;
        }
    }
    panic!("the server claimed fifty jobs before the test could");
}

/// What a log stream of the job sent from the line after `after` until the
/// server closed it: each frame, and the close's code.
fn stream(address: SocketAddr, job: &str, after: u64) -> (Vec<Value>, u16) {
    let url = format!("ws://{address}/v1/backtests/{job}/logs?after={after}");
    let (mut socket, response) = tungstenite::connect(url).expect("a log stream");
    assert_eq!(response.status(), 101);
    if let MaybeTlsStream::Plain(tcp) = socket.get_mut() {
        tcp.set_read_timeout(Some(Duration::from_secs(30)))
            .expect("a bound on each read");
    }
    let mut frames = Vec::new();
    loop {
        match socket.read().expect("a message, or the close") {
            Message::Text(text) => {
                frames.push(serde_json::from_str(text.as_str()).expect("a frame"));
            }
            Message::Close(close) => {
                let code = close.expect("a close that says why").code;
                return (frames, code.into());
            }
            _ => {}
        }
    }
}

fn serve_run_serves_an_ended_jobs_log_by_pages_and_as_a_stream_start_after_start(bed: &TestBed) {
    seed(bed);
    let store = bed.store();
    let bundle = bundle();
    let hash = BundleHash::from_digest(*bundle.code_hash().digest());
    store
        .store_bundle(&hash, &bundle.canonical_tar(), T)
        .expect("the bundle");
    let (job, pending) = (JobId::generate(), JobId::generate());
    for job_id in [&job, &pending] {
        store
            .submit_job(&NewJob {
                job_id: job_id.clone(),
                bundle: hash,
                request_json: submission(&bundle).to_string(),
                submitted_at_ms: T,
            })
            .expect("a job");
    }
    // The first job as a server that ran it to its end leaves it, with the
    // log of a job that wrote more than a log keeps: nine lines, the last
    // of them long, into a log of five lines of forty bytes.
    let log = JobLog::new(LogLimits {
        max_lines: 5,
        max_bytes: 1 << 10,
        max_line_bytes: 40,
    });
    for n in 1..=8 {
        log.line(LogLevel::Info, &format!("line {n}"));
    }
    log.line(LogLevel::Error, &"long ".repeat(20));
    log.progress(10, 10);
    let log_json = log.stored_json().expect("a log to store");
    let kept: Value = serde_json::from_str(&log_json).expect("json");
    let lines = kept["lines"].as_array().expect("lines").clone();
    assert_eq!((lines.len(), &kept["dropped_lines"]), (5, &json!(4)));
    assert_eq!(lines[4]["dropped_bytes"], 60);
    let claimed = store.claim_job(T + 1).expect("a claim").expect("a job");
    assert_eq!(claimed.job_id, job);
    assert!(store.store_job_log(&job, &log_json, T + 2).expect("a log"));
    let outcome = JobOutcome::Failed {
        failure_json: r#"{"code":"invalid_request","message":"the strategy raised"}"#.to_string(),
    };
    assert!(store.finish_job(&job, &outcome, T + 3).expect("an end"));
    drop(store);

    let progress = json!({"bars_done": 10, "bars_total": 10});
    let path = format!("/v1/backtests/{job}/logs");
    let read = |served: &Served| {
        let address = served.address;
        let whole = get(address, &path);
        assert_eq!(whole.status, 200, "{}", whole.text());
        assert_eq!(whole.header("content-type"), Some("application/json"));
        assert_eq!(whole.header("cache-control"), Some("no-cache"));
        assert_eq!(
            whole.json::<Value>(),
            json!({
                "job_id": job.as_str(),
                "lines": lines,
                "dropped_lines": 4,
                "next_after": 9,
                "progress": progress,
                "complete": true,
            })
        );
        // A page at a time, with the cursor each page hands on.
        let mut after = 0;
        for (dropped, numbers, complete) in [
            (4, [5, 6].as_slice(), false),
            (0, [7, 8].as_slice(), false),
            (0, [9].as_slice(), true),
        ] {
            let page: Value = get(address, &format!("{path}?after={after}&limit=2")).json();
            let read: Vec<u64> = page["lines"]
                .as_array()
                .expect("lines")
                .iter()
                .filter_map(|line| line["seq"].as_u64())
                .collect();
            assert_eq!(read, numbers, "after {after}");
            assert_eq!(page["dropped_lines"], dropped, "after {after}");
            assert_eq!(page["complete"], complete, "after {after}");
            after = page["next_after"].as_u64().expect("a cursor");
        }
        assert_eq!(after, 9);

        // As a stream: what was dropped, each line kept, the progress, and
        // the end, after which the server closes as it does when done.
        let (frames, code) = stream(address, job.as_str(), 0);
        let mut expected = vec![json!({"type": "truncated", "dropped_lines": 4})];
        expected.extend(lines.iter().map(|line| {
            let mut frame = line.clone();
            frame["type"] = json!("line");
            frame
        }));
        let mut last = progress.clone();
        last["type"] = json!("progress");
        expected.extend([last, json!({"type": "end"})]);
        assert_eq!((&frames, code), (&expected, 1000));
        let (tail, code) = stream(address, job.as_str(), 7);
        assert_eq!(
            (tail.as_slice(), code),
            (&expected[4..], 1000),
            "from a cursor"
        );
        (whole.text(), frames)
    };

    let first = {
        let served = Served::start(bed);
        // The other job fails as it starts, having written nothing: its
        // log is empty, and ends at once.
        until_state(served.address, pending.as_str(), "failed");
        let empty: Value = get(served.address, &format!("/v1/backtests/{pending}/logs")).json();
        assert_eq!(
            empty,
            json!({"job_id": pending.as_str(), "lines": [], "dropped_lines": 0, "next_after": 0,
                   "complete": true})
        );
        let (frames, code) = stream(served.address, pending.as_str(), 0);
        assert_eq!((frames, code), (vec![json!({"type": "end"})], 1000));

        // Refused as on every job route, for a page and for a stream.
        let unknown = "0".repeat(32);
        let refused = get(served.address, &format!("/v1/backtests/{unknown}/logs"));
        assert_eq!(refused.status, 404, "{}", refused.text());
        let url = format!("ws://{}/v1/backtests/{unknown}/logs", served.address);
        match tungstenite::connect(url) {
            Err(tungstenite::Error::Http(response)) => assert_eq!(response.status(), 404),
            other => panic!(
                "a stream of no job: {:?}",
                other.map(|(_, response)| response)
            ),
        }
        let refused = get(served.address, &format!("{path}?limit=0"));
        assert_eq!(refused.status, 400, "{}", refused.text());
        assert_eq!(
            refused.json::<Value>()["error"]["message"],
            "limit 0 is not between 1 and 1000"
        );
        read(&served)
    };
    // That server is gone; the next one on the store answers the same.
    let served = Served::start(bed);
    assert_eq!(read(&served), first);
}

on_each_backend!(
    serve_run_queues_a_job_and_fails_it_as_it_starts_without_a_worker_runtime,
    serve_run_fails_the_jobs_left_running_and_runs_the_queued_ones_as_it_starts,
    serve_run_serves_a_succeeded_jobs_result_from_its_store_start_after_start,
    serve_run_serves_an_ended_jobs_log_by_pages_and_as_a_stream_start_after_start,
);
