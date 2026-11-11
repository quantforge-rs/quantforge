//! The result routes over a store whose copy of a result is not what its
//! job lists: nothing damaged is ever sent under a result's name, and a
//! job that lists no metrics has no result to answer.

use sha2::{Digest, Sha256};

use super::jobs::{Admitting, bundle, data, jobs_of, runtime, serve_jobs, submission};
use super::*;
use crate::engine::backtest::{BacktestResult, EquityPoint};
use crate::model::{BundleHash, JobId, JobOutcome, NewJob, StoredResult};
use crate::ports::JobStore;
use crate::protocol::jobs::ResultRef;
use crate::server::results::{BacktestOutput, ResultFile};

/// A result of no trades over two bars.
fn files() -> Vec<ResultFile> {
    let result = BacktestResult {
        initial_cash: decimal("10000"),
        final_equity: decimal("10000.00"),
        total_return_pct: decimal("0"),
        trade_count: 0,
        max_drawdown_pct: decimal("0"),
        trades: Vec::new(),
        equity_curve: [(T, "10000"), (T + MINUTE, "10000.00")]
            .map(|(open_time_ms, equity)| EquityPoint {
                open_time_ms,
                equity: decimal(equity),
            })
            .to_vec(),
    };
    BacktestOutput::of(&result).files().expect("the files")
}

/// A job another server ran to its end, succeeded with `listed` as its
/// results and `stored` kept with it.
fn succeeded(bed: &Bed, listed: &[&ResultRef], stored: Vec<StoredResult>) -> JobId {
    let bundle = bundle();
    let hash = BundleHash::from_digest(*bundle.code_hash().digest());
    bed.store
        .store_bundle(&hash, &bundle.canonical_tar(), T)
        .expect("the bundle");
    let job_id = JobId::generate();
    let request = submission(
        &bundle.code_hash(),
        data(Some(Category::Spot), T, T + MINUTE),
    );
    bed.store
        .submit_job(&NewJob {
            job_id: job_id.clone(),
            bundle: hash,
            request_json: request.to_string(),
            submitted_at_ms: T,
        })
        .expect("a job");
    bed.store
        .claim_job(T + 1)
        .expect("a claim")
        .expect("the job");
    let outcome = JobOutcome::Succeeded {
        results_json: serde_json::to_string(listed).expect("json"),
        results: stored,
    };
    assert!(
        bed.store
            .finish_job(&job_id, &outcome, T + 2)
            .expect("an end")
    );
    job_id
}

async fn get(address: SocketAddr, path: &str, tag: Option<&str>) -> reqwest::Response {
    let mut request = client().get(format!("http://{address}{path}"));
    if let Some(tag) = tag {
        request = request.header("if-none-match", tag);
    }
    request.send().await.expect("a response")
}

#[tokio::test]
async fn a_stored_result_that_does_not_match_its_name_is_never_sent() {
    let bed = Bed::new();
    let files = files();
    let listed: Vec<&ResultRef> = files.iter().map(|file| &file.reference).collect();
    let stored: Vec<StoredResult> = files
        .iter()
        .map(|file| file.stored().expect("packed"))
        .collect();
    let job = succeeded(&bed, &listed, stored.clone());
    let address = serve_jobs(&bed, jobs_of(&bed, Admitting(runtime("3.12")), 100)).await;

    // As stored, every file is the file.
    for (file, name) in files
        .iter()
        .zip(["metrics.json", "trades.arrow", "equity.arrow"])
    {
        let sent = get(address, &format!("/v1/backtests/{job}/result/{name}"), None).await;
        assert_eq!(sent.status(), 200, "{name}");
        let body = sent.bytes().await.expect("a body");
        let digest: [u8; 32] = Sha256::digest(&body).into();
        assert_eq!(body.as_ref(), file.bytes.as_slice(), "{name}");
        assert_eq!(digest, *file.reference.hash.digest(), "{name}");
    }

    let (trades, equity) = (&stored[1], &stored[2]);
    let path = format!("/v1/backtests/{job}/result/trades.arrow");
    let tag = format!("\"{}\"", files[1].reference.hash);
    let damage: [Vec<u8>; 4] = [
        // Another file's bytes, half of its own, its own left unpacked,
        // and nothing a zstd frame starts with.
        equity.data.clone(),
        trades.data[..trades.data.len() / 2].to_vec(),
        files[1].bytes.clone(),
        b"not zstd".to_vec(),
    ];
    for bytes in damage {
        bed.connection()
            .execute(
                "UPDATE job_results SET data = ?1 WHERE hash = ?2",
                rusqlite::params![bytes, trades.hash.to_string()],
            )
            .expect("the damage");
        let (status, code, message) = refusal(get(address, &path, None).await).await;
        assert_eq!((status, code.as_str()), (500, "internal"));
        assert_eq!(
            message,
            format!("the server's copy of job {job}'s trades result is damaged")
        );
        // A client that holds the file already is told so: nothing is read.
        assert_eq!(get(address, &path, Some(&tag)).await.status(), 304);
    }

    // A result the store no longer holds at all, and the summary's too.
    bed.connection()
        .execute("DELETE FROM job_results", [])
        .expect("the loss");
    let (status, code, message) = refusal(get(address, &path, None).await).await;
    assert_eq!((status, code.as_str()), (500, "internal"));
    assert_eq!(
        message,
        format!("the server's copy of job {job}'s trades result is damaged")
    );
    let (status, _, message) =
        refusal(get(address, &format!("/v1/backtests/{job}/result"), None).await).await;
    assert_eq!(status, 500);
    assert_eq!(
        message,
        format!("the server's copy of job {job}'s metrics result is damaged")
    );
}

#[tokio::test]
async fn a_metrics_file_that_is_not_the_one_written_is_never_answered() {
    let bed = Bed::new();
    // Metrics with a space in them: the object reads, and is another file.
    let spaced = ResultFile::new(
        "metrics",
        "application/json",
        br#"{"initial_cash":"1", "final_equity":"1","total_return_pct":"0","max_drawdown_pct":"0","trade_count":0}"#.to_vec(),
    );
    let job = succeeded(
        &bed,
        &[&spaced.reference],
        vec![spaced.stored().expect("packed")],
    );
    let address = serve_jobs(&bed, jobs_of(&bed, Admitting(runtime("3.12")), 100)).await;
    let (status, code, message) =
        refusal(get(address, &format!("/v1/backtests/{job}/result"), None).await).await;
    assert_eq!((status, code.as_str()), (500, "internal"));
    assert_eq!(
        message,
        format!("the server's copy of job {job}'s metrics result is damaged")
    );
}

#[tokio::test]
async fn a_succeeded_job_that_lists_no_such_result_has_none_to_send() {
    let bed = Bed::new();
    // What a build before result files left: a list naming a result no
    // store kept, and no metrics.
    let report = ResultFile::new("report", "application/json", b"{}".to_vec());
    let job = succeeded(&bed, &[&report.reference], Vec::new());
    let address = serve_jobs(&bed, jobs_of(&bed, Admitting(runtime("3.12")), 100)).await;
    for (path, result) in [
        ("result", "metrics"),
        ("result/metrics.json", "metrics"),
        ("result/equity.arrow", "equity"),
    ] {
        let (status, code, message) =
            refusal(get(address, &format!("/v1/backtests/{job}/{path}"), None).await).await;
        assert_eq!((status, code.as_str()), (500, "internal"), "{path}");
        assert_eq!(
            message,
            format!("the server holds no {result} result of job {job}"),
            "{path}"
        );
    }
    // The job's status still reads, with the list as it was kept.
    let status: Value = get(address, &format!("/v1/backtests/{job}"), None)
        .await
        .json()
        .await
        .expect("a status");
    assert_eq!(status["state"]["results"][0]["name"], "report");
}

#[tokio::test]
async fn a_result_route_reads_its_job_as_every_job_route_does_and_takes_only_get() {
    let bed = Bed::new();
    let address = serve_jobs(&bed, jobs_of(&bed, Admitting(runtime("3.12")), 100)).await;
    for path in ["result", "result/trades.arrow"] {
        let (status, code, message) =
            refusal(get(address, &format!("/v1/backtests/j%20x/{path}"), None).await).await;
        assert_eq!((status, code.as_str()), (400, "invalid_request"), "{path}");
        assert!(message.starts_with("invalid job id"), "{message}");
        let (status, code, message) =
            refusal(get(address, &format!("/v1/backtests/job-1/{path}"), None).await).await;
        assert_eq!((status, code.as_str()), (404, "not_found"), "{path}");
        assert_eq!(message, "the server holds no job job-1");

        let posted = client()
            .post(format!("http://{address}/v1/backtests/job-1/{path}"))
            .send()
            .await
            .expect("a response");
        assert_eq!(header(&posted, ALLOW), Some("GET,HEAD"), "{path}");
        let (status, code, _) = refusal(posted).await;
        assert_eq!(
            (status, code.as_str()),
            (405, "method_not_allowed"),
            "{path}"
        );
    }
    // A deeper path is no route at all.
    let (status, code, _) =
        refusal(get(address, "/v1/backtests/job-1/result/trades.arrow/x", None).await).await;
    assert_eq!((status, code.as_str()), (404, "not_found"));
}
