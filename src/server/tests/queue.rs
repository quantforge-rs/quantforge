//! The queue in process: a job whose store is out of time as it starts
//! waits for the store rather than fail. `tests/backtest_jobs.rs` runs the
//! rest of a job's life on both backends.

use std::sync::Mutex;

use super::jobs::{bundle, runtime};
use super::*;
use crate::model::{BundleHash, JobId, JobPhase, NewJob};
use crate::ports::JobStore;
use crate::protocol::bundle::Runtime;

/// A worker that notes how many candles each job got, and succeeds.
#[derive(Default)]
struct Counting(Arc<Mutex<Vec<usize>>>);

impl JobWorker for Counting {
    fn runtime(&self) -> Option<Runtime> {
        Some(runtime("3.12"))
    }

    fn run(&self, input: JobInput, _cancel: &Cancel) -> WorkerOutcome {
        self.0.lock().expect("the counts").push(input.candles.len());
        WorkerOutcome::Succeeded(Vec::new())
    }
}

#[tokio::test]
async fn a_job_whose_store_is_out_of_time_as_it_starts_waits_for_it() {
    let bed = Bed::new();
    let candles: Vec<Candle> = (0..10).map(|i| candle(T + i * MINUTE, "1", None)).collect();
    bed.store
        .upsert_candles(&market(Category::Spot), &candles)
        .expect("the candles");
    let bundle = bundle();
    let hash = BundleHash::from_digest(*bundle.code_hash().digest());
    bed.store
        .store_bundle(&hash, &bundle.canonical_tar(), T)
        .expect("the bundle");
    let request = serde_json::json!({
        "bundle": bundle.code_hash().to_string(),
        "data": {"exchange": VENUE.as_str(), "category": "spot", "symbol": "BTCUSDT",
                 "interval": "1m", "start_open_ms": T, "end_open_ms": T + 9 * MINUTE},
        "backtest": {"initial_cash": "10000", "fee_bps": "10", "close_out_at_end": true},
    });
    let job_id = JobId::generate();
    bed.store
        .submit_job(&NewJob {
            job_id: job_id.clone(),
            bundle: hash,
            request_json: request.to_string(),
            submitted_at_ms: T,
        })
        .expect("a job");

    let counts = Arc::new(Mutex::new(Vec::new()));
    let busy = Faulty::busy_loads(bed.store.within(Duration::from_secs(2)), 2);
    let jobs = Jobs::new(
        served(busy),
        Counting(Arc::clone(&counts)),
        JobLimits {
            concurrency: 1,
            max_span_bars: DEFAULT_MAX_SPAN_BARS,
        },
    );
    assert_eq!(jobs.start().await.expect("a started queue"), 0);
    let ended = within("the job's end", async {
        loop {
            let job = bed.store.job(&job_id).expect("a read").expect("the job");
            if job.phase.is_final() {
                return job;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    assert_eq!(ended.phase, JobPhase::Succeeded, "{ended:?}");
    assert_eq!(*counts.lock().expect("the counts"), [10]);
}
