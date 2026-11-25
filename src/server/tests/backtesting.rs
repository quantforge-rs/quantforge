//! What the provenance and rerun tests share: a store of three days of
//! candles, two of them packed and one of those corrected since, a worker
//! that backtests what it is handed and keeps it, and a job run over the
//! days to its end.

use std::sync::Mutex;

use super::jobs::{
    MANIFEST, bundle, data, jobs_of, queued, runtime, serve_jobs, submission, submit, upload,
};
use super::*;
use crate::engine::backtest::{BacktestConfig, BacktestEngine};
use crate::model::{BundleHash, JobId, JobOutcome, NewJob};
use crate::ports::JobStore;
use crate::protocol::bundle::Runtime;
use crate::protocol::bundle::{Bundle, BundleLimits};
use crate::protocol::{ErrorBody, ErrorCode};
use crate::sdk::BuiltInStrategyConfig;
use crate::server::results::BacktestOutput;

/// The first bar of the job's range: a hundred minutes into the first day.
pub(super) const START: TimestampMs = T + 100 * MINUTE;
/// The last bar of the job's range: 700 minutes into the third day.
pub(super) const END: TimestampMs = T + 2 * DAY + 700 * MINUTE;

/// A close that rises and falls, with its scale, so the SMA cross trades.
fn close(i: i64) -> String {
    format!("{}.{:02}", 100 + (i * 7) % 50, (i * 13) % 100)
}

/// Three UTC days of `1m` candles from `T`, as they were first stored.
pub(super) fn days() -> Vec<Candle> {
    (0..3 * 1440)
        .map(|i| candle(T + i * MINUTE, &close(i), u64::try_from(i).ok()))
        .collect()
}

/// The second day's correction: one bar, another close.
pub(super) fn correction() -> Candle {
    candle(T + DAY + 600 * MINUTE, "149.99", Some(9))
}

/// The bars as the job finds them: the second day corrected.
pub(super) fn found() -> Vec<Candle> {
    let fixed = correction();
    days()
        .into_iter()
        .map(|candle| {
            if candle.open_time_ms == fixed.open_time_ms {
                fixed.clone()
            } else {
                candle
            }
        })
        .collect()
}

/// The bed's store with three days of candles, the first two packed by a
/// refresh and the second corrected since: the plan of the job's range is
/// the first day's segment, then the rest from the candles.
pub(super) fn stored(bed: &Bed) {
    let market = market(Category::Spot);
    bed.store.upsert_candles(&market, &days()).expect("days");
    let refreshed = bed.store.refresh_segments(&market).expect("a refresh");
    assert_eq!(refreshed.built, 2, "the first two days");
    bed.store
        .upsert_candles(&market, &[correction()])
        .expect("the correction");
}

/// How a backtesting worker's runs after its first differ from it.
#[derive(Clone, Copy, Debug)]
pub(super) enum Later {
    /// Not at all.
    Same,
    /// The fee is a basis point higher.
    Costlier,
    /// The first equity row is written with one more decimal place: the
    /// same number, another text.
    Rescaled,
    /// The bundle raises.
    Failing,
}

/// A worker that backtests the built-in SMA cross on the candles it is
/// handed, and keeps them, run by run.
pub(super) struct Backtesting {
    pub(super) runtime: Runtime,
    pub(super) later: Later,
    pub(super) handed: Arc<Mutex<Vec<Vec<Candle>>>>,
}

impl Backtesting {
    pub(super) fn new(later: Later) -> Self {
        Self {
            runtime: runtime("3.12"),
            later,
            handed: Arc::default(),
        }
    }
}

impl JobWorker for Backtesting {
    fn runtime(&self) -> Option<Runtime> {
        Some(self.runtime.clone())
    }

    fn run(&self, input: JobInput, _cancel: &Cancel, _log: &JobLog) -> WorkerOutcome {
        let first = {
            let mut handed = self.handed.lock().expect("the runs");
            handed.push(input.candles.clone());
            handed.len() == 1
        };
        let mut fee_bps = input.submission.backtest.fee_bps.get();
        match self.later {
            Later::Costlier if !first => fee_bps += Decimal::ONE,
            Later::Failing if !first => {
                return WorkerOutcome::Failed(ErrorBody {
                    code: ErrorCode::InvalidRequest,
                    message: "the strategy raised on the rerun".to_string(),
                    supported: None,
                });
            }
            _ => {}
        }
        let mut strategy = BuiltInStrategyConfig::SmaCross { fast: 2, slow: 3 }
            .build()
            .expect("a strategy");
        let mut result = BacktestEngine::new(BacktestConfig {
            initial_cash: input.submission.backtest.initial_cash.get(),
            fee_bps,
            close_out_at_end: input.submission.backtest.close_out_at_end,
        })
        .run(&market(Category::Spot), &input.candles, strategy.as_mut())
        .expect("a backtest");
        if let (Later::Rescaled, false) = (self.later, first) {
            let point = &mut result.equity_curve[0];
            point.equity.rescale(point.equity.scale() + 1);
        }
        WorkerOutcome::Succeeded(BacktestOutput::of(&result).files().expect("the files"))
    }
}

/// A started queue over the bed's store, running jobs on `worker`, served.
pub(super) async fn running<W: JobWorker + 'static>(bed: &Bed, worker: W) -> (SocketAddr, Jobs) {
    let jobs = jobs_of(bed, worker, DEFAULT_MAX_SPAN_BARS);
    assert_eq!(jobs.start().await.expect("a started queue"), 0);
    (serve_jobs(bed, jobs.clone()).await, jobs)
}

/// The job's status once it has ended.
pub(super) async fn ended(address: SocketAddr, job: &str) -> Value {
    within("the job's end", async {
        loop {
            let status: Value = client()
                .get(format!("http://{address}/v1/backtests/{job}"))
                .send()
                .await
                .expect("a response")
                .json()
                .await
                .expect("a status");
            if ["succeeded", "failed", "cancelled"]
                .contains(&status["state"]["kind"].as_str().unwrap_or_default())
            {
                return status;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
}

/// Uploads the tests' bundle and runs a job over the range from `START` to
/// `END` to its end: its ID and its final status.
pub(super) async fn ran(address: SocketAddr) -> (String, Value) {
    let hash = upload(address, &bundle()).await;
    let response = submit(
        address,
        &submission(&hash, data(Some(Category::Spot), START, END)),
    )
    .await;
    let job = queued(response).await["job_id"]
        .as_str()
        .expect("a job id")
        .to_string();
    let status = ended(address, &job).await;
    (job, status)
}

/// `GET` of `path` on the server.
pub(super) async fn get(address: SocketAddr, path: &str) -> reqwest::Response {
    client()
        .get(format!("http://{address}{path}"))
        .send()
        .await
        .expect("a response")
}

/// A job another server ran to success, its end written straight into the
/// bed's store, with no provenance: as a build before provenance left it.
pub(super) fn succeeded_without_provenance(bed: &Bed) -> JobId {
    let bundle = bundle();
    let hash = BundleHash::from_digest(*bundle.code_hash().digest());
    bed.store
        .store_bundle(&hash, &bundle.canonical_tar(), T)
        .expect("the bundle");
    let job_id = JobId::generate();
    let request = submission(&bundle.code_hash(), data(Some(Category::Spot), START, END));
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
        results_json: "[]".to_string(),
        results: Vec::new(),
    };
    assert!(
        bed.store
            .finish_job(&job_id, &outcome, T + 2)
            .expect("an end")
    );
    job_id
}

/// The canonical tar of a bundle that is not the tests' one, though it
/// reads back as a bundle as well: its strategy holds something more. A
/// store that held it under the tests' bundle's name would run other code.
pub(super) fn other_code() -> Vec<u8> {
    Bundle::from_files(
        [
            ("quantforge.toml", MANIFEST),
            ("strategy.py", "class Hold:\n    other = 1\n"),
        ],
        &BundleLimits::default(),
    )
    .expect("another bundle")
    .canonical_tar()
}
