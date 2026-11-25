//! The rerun route in process, over the store of `backtesting`: a job run
//! again from its provenance comes out the same however its candles were
//! corrected since; one that comes out otherwise is told where each file
//! first differs, a decimal's scale counting; a rerun whose bundle fails is
//! diverged; and a server that stops ends a rerun. `rerun_refusals` holds
//! the reruns refused. The helpers here serve it as well.

use std::sync::Mutex;

use super::backtesting::{Backtesting, Later, days, ended, get, ran, running, stored};
use super::*;
use crate::protocol::bundle::Runtime;
use crate::protocol::jobs::{Provenance, RerunReport, RerunVerdict};

/// `POST /v1/backtests/{job}/rerun`.
pub(super) async fn rerun(address: SocketAddr, job: &str) -> reqwest::Response {
    client()
        .post(format!("http://{address}/v1/backtests/{job}/rerun"))
        .send()
        .await
        .expect("a response")
}

/// The report of a rerun that ran.
pub(super) async fn reran(address: SocketAddr, job: &str) -> RerunReport {
    let response = rerun(address, job).await;
    assert_eq!(response.status(), 200);
    assert_eq!(header(&response, CACHE_CONTROL), Some("no-cache"));
    response.json().await.expect("a report")
}

pub(super) async fn provenance(address: SocketAddr, job: &str) -> Provenance {
    get(address, &format!("/v1/backtests/{job}/provenance"))
        .await
        .json()
        .await
        .expect("a provenance")
}

#[tokio::test]
async fn a_rerun_comes_out_the_same_on_the_same_bars_however_the_candles_were_corrected() {
    let bed = Bed::new();
    stored(&bed);
    let worker = Backtesting::new(Later::Same);
    let handed = Arc::clone(&worker.handed);
    let (address, _jobs) = running(&bed, worker).await;
    let (job, status) = ran(address).await;
    assert_eq!(status["state"]["kind"], "succeeded", "{status}");
    let recorded = provenance(address, &job).await;

    // Every bar of the range corrected, and the first two days packed again.
    let market = market(Category::Spot);
    let corrected: Vec<Candle> = days()
        .iter()
        .map(|bar| candle(bar.open_time_ms, "1.10", None))
        .collect();
    bed.store
        .upsert_candles(&market, &corrected)
        .expect("corrections");
    bed.store.refresh_segments(&market).expect("a refresh");

    let report = reran(address, &job).await;
    assert_eq!(report.verdict, RerunVerdict::Verified, "{report:?}");
    assert_eq!(report.identity, recorded.identity);
    assert_eq!(report.job_id.as_str(), job);
    assert_eq!(report.failure, None);
    let listed = recorded.results.expect("the results");
    assert_eq!(report.results.len(), 3);
    for (compared, file) in report.results.iter().zip(&listed) {
        assert_eq!(
            (compared.name.as_str(), compared.recorded, compared.rerun),
            (file.name.as_str(), file.hash, Some(file.hash))
        );
        assert_eq!(compared.divergence, None);
    }
    assert_eq!(report.runtime, recorded.inputs.runtime);
    // The rerun ran on the bars the job ran on, not on the corrected ones.
    let runs = handed.lock().expect("the runs");
    assert_eq!(runs.len(), 2);
    let texts = |run: &[Candle]| run.iter().map(text).collect::<Vec<_>>();
    assert_eq!(texts(&runs[1]), texts(&runs[0]));
}

#[tokio::test]
async fn a_rerun_that_comes_out_otherwise_names_where_each_file_first_differs() {
    let bed = Bed::new();
    stored(&bed);
    let (address, _jobs) = running(&bed, Backtesting::new(Later::Rescaled)).await;
    let (job, _) = ran(address).await;
    let listed = provenance(address, &job)
        .await
        .results
        .expect("the results");
    let equity_link = &listed[2];
    let equity = get(address, &format!("/v1/backtests/{job}/result/equity.arrow"))
        .await
        .bytes()
        .await
        .expect("the curve");
    let curve = crate::protocol::results::decode_equity(&equity).expect("a curve");
    let first = curve[0].equity.to_string();

    // The same number, written with one more place: another file.
    let report = reran(address, &job).await;
    assert_eq!(report.verdict, RerunVerdict::Diverged);
    let [metrics, trades, equity] = &report.results[..] else {
        panic!("three files: {report:?}");
    };
    assert_eq!(metrics.rerun, Some(metrics.recorded));
    assert_eq!(trades.rerun, Some(trades.recorded));
    assert_eq!(
        (metrics.divergence.as_ref(), trades.divergence.as_ref()),
        (None, None)
    );
    assert_eq!(equity.recorded, equity_link.hash);
    assert_ne!(equity.rerun, Some(equity.recorded));
    let divergence = equity.divergence.as_ref().expect("a divergence");
    assert_eq!(
        (divergence.row, divergence.field.as_str()),
        (Some(1), "equity")
    );
    let rescaled = if first.contains('.') {
        format!("{first}0")
    } else {
        format!("{first}.0")
    };
    assert_eq!(
        (divergence.recorded.as_deref(), divergence.rerun.as_deref()),
        (Some(first.as_str()), Some(rescaled.as_str()))
    );

    // A costlier rerun: every file differs, the metrics first in the final
    // equity, as the fee came out of it.
    let bed = Bed::new();
    stored(&bed);
    let (address, _jobs) = running(&bed, Backtesting::new(Later::Costlier)).await;
    let (job, _) = ran(address).await;
    let report = reran(address, &job).await;
    assert_eq!(report.verdict, RerunVerdict::Diverged);
    for compared in &report.results {
        assert_ne!(compared.rerun, Some(compared.recorded), "{}", compared.name);
        let divergence = compared.divergence.as_ref().expect("a divergence");
        assert_ne!(divergence.recorded, divergence.rerun, "{}", compared.name);
    }
    let metrics = report.results[0].divergence.as_ref().expect("a divergence");
    assert_eq!(
        (metrics.row, metrics.field.as_str()),
        (None, "final_equity")
    );
    assert!(
        report.results[1]
            .divergence
            .as_ref()
            .is_some_and(|first| first.row.is_some())
    );
}

#[tokio::test]
async fn a_rerun_whose_bundle_fails_is_diverged_with_why() {
    let bed = Bed::new();
    stored(&bed);
    let (address, _jobs) = running(&bed, Backtesting::new(Later::Failing)).await;
    let (job, _) = ran(address).await;
    let report = reran(address, &job).await;
    assert_eq!(report.verdict, RerunVerdict::Diverged);
    let failure = report.failure.expect("why");
    assert_eq!(
        (failure.code, failure.message.as_str()),
        (
            crate::protocol::ErrorCode::InvalidRequest,
            "the strategy raised on the rerun"
        )
    );
    assert!(
        report
            .results
            .iter()
            .all(|compared| compared.rerun.is_none() && compared.divergence.is_none())
    );
}

/// A worker whose later runs hold until they are cancelled, saying when
/// one has begun.
struct Holding {
    first: Backtesting,
    begun: Arc<Mutex<bool>>,
}

impl JobWorker for Holding {
    fn runtime(&self) -> Option<Runtime> {
        self.first.runtime()
    }

    fn run(&self, input: JobInput, cancel: &Cancel, log: &JobLog) -> WorkerOutcome {
        if self.first.handed.lock().expect("the runs").is_empty() {
            return self.first.run(input, cancel, log);
        }
        *self.begun.lock().expect("the flag") = true;
        while !cancel.wait(Duration::from_millis(50)) {}
        WorkerOutcome::Cancelled
    }
}

#[tokio::test]
async fn a_server_that_stops_while_a_rerun_runs_says_so() {
    let bed = Bed::new();
    stored(&bed);
    let begun = Arc::new(Mutex::new(false));
    let worker = Holding {
        first: Backtesting::new(Later::Same),
        begun: Arc::clone(&begun),
    };
    let (address, jobs) = running(&bed, worker).await;
    let (job, status) = ran(address).await;
    assert_eq!(status["state"]["kind"], "succeeded");
    let asking = job.clone();
    let asked = tokio::spawn(async move { rerun(address, &asking).await });
    within("the rerun's start", async {
        while !*begun.lock().expect("the flag") {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    // A rerun is stopped, and is no job.
    assert_eq!(within("the stop", jobs.stop()).await, 0);
    let answer = within("the rerun's answer", asked)
        .await
        .expect("the request");
    assert_eq!(
        refusal(answer).await,
        (
            503,
            "unavailable".to_string(),
            "the server stopped before the rerun could finish; ask for it again once the server \
             is back"
                .to_string()
        )
    );
    // The job is as it was.
    assert_eq!(ended(address, &job).await, status);
}
