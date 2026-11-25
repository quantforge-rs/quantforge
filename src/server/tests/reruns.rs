//! The rerun route in process, over the store of `backtesting`: a job run
//! again from its provenance comes out the same however its candles were
//! corrected since; one that comes out otherwise is told where each file
//! first differs, a decimal's scale counting, and one that produces a file
//! the job does not list is diverged too; a rerun whose bundle fails is
//! diverged; a worker that stops a rerun unasked fails it as the server's
//! own; a rerun reads nothing before a slot is free; and a server that
//! stops ends a rerun. `rerun_refusals` holds the reruns refused. The
//! helpers here serve it as well.

use std::sync::Mutex;

use super::backtesting::{
    Backtesting, Later, START, days, ended, get, notes, ran, running, stored,
};
use super::jobs::{bundle, data, queued, submission, submit};
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
            (file.name.as_str(), Some(file.hash), Some(file.hash))
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
    assert_eq!(metrics.rerun, metrics.recorded);
    assert_eq!(trades.rerun, trades.recorded);
    assert_eq!(
        (metrics.divergence.as_ref(), trades.divergence.as_ref()),
        (None, None)
    );
    assert_eq!(equity.recorded, Some(equity_link.hash));
    assert_ne!(equity.rerun, equity.recorded);
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
        assert_ne!(compared.rerun, compared.recorded, "{}", compared.name);
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
async fn a_rerun_that_produces_a_file_the_job_does_not_list_is_diverged() {
    let bed = Bed::new();
    stored(&bed);
    let (address, _jobs) = running(&bed, Backtesting::new(Later::Extra)).await;
    let (job, _) = ran(address).await;
    let listed = provenance(address, &job)
        .await
        .results
        .expect("the results");
    let report = reran(address, &job).await;
    assert_eq!(report.verdict, RerunVerdict::Diverged, "{report:?}");
    let [same @ .., extra] = &report.results[..] else {
        panic!("files: {report:?}");
    };
    assert_eq!(same.len(), listed.len());
    for (compared, file) in same.iter().zip(&listed) {
        assert_eq!(
            (compared.name.as_str(), compared.recorded, compared.rerun),
            (file.name.as_str(), Some(file.hash), Some(file.hash))
        );
        assert_eq!(compared.divergence, None);
    }
    // Listed last, with nothing recorded to compare it with.
    assert_eq!(
        (extra.name.as_str(), extra.recorded, extra.rerun),
        ("notes", None, Some(notes().reference.hash))
    );
    assert_eq!(extra.divergence, None);
}

#[tokio::test]
async fn a_worker_that_stops_a_rerun_unasked_fails_it_as_the_servers_own() {
    let bed = Bed::new();
    stored(&bed);
    let (address, _jobs) = running(&bed, Backtesting::new(Later::Quitting)).await;
    let (job, status) = ran(address).await;
    assert_eq!(status["state"]["kind"], "succeeded", "{status}");
    assert_eq!(
        refusal(rerun(address, &job).await).await,
        (
            500,
            "internal".to_string(),
            "the rerun could not finish: the worker stopped it without being asked to".to_string()
        )
    );
    // The job is as it was.
    assert_eq!(ended(address, &job).await, status);
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

/// A worker whose second run holds until it is cancelled or released,
/// saying when it has begun; it runs the others as `Backtesting` does.
struct Holding {
    first: Backtesting,
    begun: Arc<Mutex<bool>>,
    released: Cancel,
    runs: Mutex<usize>,
}

/// Releases a `Holding` worker's held run once dropped, so that a test
/// that fails while a run holds still ends: its runtime waits for the run.
struct Release(Cancel);

impl Drop for Release {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

impl Holding {
    /// The worker, and what releases its held run.
    fn new(begun: &Arc<Mutex<bool>>) -> (Self, Release) {
        let released = Cancel::new();
        let worker = Self {
            first: Backtesting::new(Later::Same),
            begun: Arc::clone(begun),
            released: released.clone(),
            runs: Mutex::new(0),
        };
        (worker, Release(released))
    }
}

impl JobWorker for Holding {
    fn runtime(&self) -> Option<Runtime> {
        self.first.runtime()
    }

    fn run(&self, input: JobInput, cancel: &Cancel, log: &JobLog) -> WorkerOutcome {
        let run = {
            let mut runs = self.runs.lock().expect("the runs");
            *runs += 1;
            *runs
        };
        if run != 2 {
            return self.first.run(input, cancel, log);
        }
        *self.begun.lock().expect("the flag") = true;
        while !cancel.wait(Duration::from_millis(50)) && !self.released.is_cancelled() {}
        WorkerOutcome::Cancelled
    }
}

/// Once the second run of a `Holding` worker has begun.
async fn begun(flag: &Mutex<bool>) {
    within("the second run's start", async {
        while !*flag.lock().expect("the flag") {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
}

#[tokio::test]
async fn a_rerun_reads_nothing_the_job_ran_on_before_a_slot_is_free() {
    let bed = Bed::new();
    stored(&bed);
    let flag = Arc::new(Mutex::new(false));
    let (worker, _release) = Holding::new(&flag);
    let (address, jobs) = running(&bed, worker).await;
    let (job, _) = ran(address).await;

    // The last segment the job ran on is lost while another job, over an
    // hour of the first day, holds the server's one slot.
    let recorded = provenance(address, &job).await;
    let last = recorded.inputs.data.segments.last().expect("a segment");
    let last = last.hash.to_string();
    let connection = bed.connection();
    let (payload_len, stored_bytes): (i64, Vec<u8>) = connection
        .query_row(
            "SELECT payload_len, data FROM segments WHERE hash = ?1",
            [&last],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("the segment");
    connection
        .execute("DELETE FROM segments WHERE hash = ?1", [&last])
        .expect("a lost segment");
    let hour = data(Some(Category::Spot), START, START + 59 * MINUTE);
    let other = queued(submit(address, &submission(&bundle().code_hash(), hour)).await).await;
    let other = other["job_id"].as_str().expect("a job id").to_string();
    begun(&flag).await;

    // The rerun waits for the slot, registered as it starts to, having read
    // nothing: had it read the segments, it would have been refused.
    let asking = job.clone();
    let asked = tokio::spawn(async move { rerun(address, &asking).await });
    within("the rerun's wait for a slot", async {
        while jobs.reruns() == 0 {
            assert!(!asked.is_finished(), "the rerun answered before it waited");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    connection
        .execute(
            "INSERT INTO segments(hash, format, payload_len, data) VALUES (?1, 1, ?2, ?3)",
            rusqlite::params![last, payload_len, stored_bytes],
        )
        .expect("the segment stored again");
    let cancelled = client()
        .post(format!("http://{address}/v1/backtests/{other}/cancel"))
        .send()
        .await
        .expect("a response");
    assert_eq!(cancelled.status(), 200);
    let answer = within("the rerun's answer", asked)
        .await
        .expect("the request");
    assert_eq!(answer.status(), 200);
    let report: RerunReport = answer.json().await.expect("a report");
    assert_eq!(report.verdict, RerunVerdict::Verified, "{report:?}");
}

#[tokio::test]
async fn a_server_that_stops_while_a_rerun_runs_says_so() {
    let bed = Bed::new();
    stored(&bed);
    let flag = Arc::new(Mutex::new(false));
    let (worker, _release) = Holding::new(&flag);
    let (address, jobs) = running(&bed, worker).await;
    let (job, status) = ran(address).await;
    assert_eq!(status["state"]["kind"], "succeeded");
    let asking = job.clone();
    let asked = tokio::spawn(async move { rerun(address, &asking).await });
    begun(&flag).await;
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
