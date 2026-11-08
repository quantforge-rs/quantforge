//! The queue in process: a job whose store is out of time as it starts
//! waits for the store rather than fail, and a stop that races the
//! dispatcher leaves the queued job queued. The races are stepped on one
//! thread, which runs a spawned task only when the test yields to it.
//! `tests/backtest_jobs.rs` runs the rest of a job's life on both backends.

use std::future::poll_fn;
use std::pin::Pin;
use std::sync::{Mutex, mpsc};
use std::task::Poll;

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

/// Queues a backtest of the tests' bundle on ten `1m` candles from `T`,
/// straight into the bed's store, and answers its ID.
fn queued(bed: &Bed) -> JobId {
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
    job_id
}

/// One job at a time over the bed's store, which takes one call at a time
/// and lets a call wait long for its turn.
fn one_at_a_time(bed: &Bed) -> (ServerStore, Jobs) {
    let store = ServerStore::new(
        bed.store.within(Duration::from_secs(2)),
        StoreLimits {
            concurrency: 1,
            wait: Duration::from_secs(10),
        },
    );
    let jobs = Jobs::new(
        store.clone(),
        NoRuntime,
        JobLimits {
            concurrency: 1,
            max_span_bars: DEFAULT_MAX_SPAN_BARS,
        },
    );
    (store, jobs)
}

/// `future` polled once, on this task and without yielding: its output, or
/// `None` while it waits.
async fn polled_once<F: Future + Unpin>(future: &mut F) -> Option<F::Output> {
    poll_fn(|context| match Pin::new(&mut *future).poll(context) {
        Poll::Ready(output) => Poll::Ready(Some(output)),
        Poll::Pending => Poll::Ready(None),
    })
    .await
}

/// Takes the store's only turn before any task on this thread runs again,
/// with a call that keeps it until the answered sender is dropped.
async fn take_the_turn(store: &ServerStore) -> mpsc::Sender<()> {
    let (began, begun) = mpsc::channel();
    let (release, released) = mpsc::channel::<()>();
    let mut call = Box::pin(store.call(move |_| {
        let _ = began.send(());
        let _ = released.recv_timeout(Duration::from_secs(10));
        Ok(())
    }));
    assert!(
        polled_once(&mut call).await.is_none(),
        "the call keeps the turn"
    );
    // Blocking this thread holds back every task on it until the call has
    // the turn.
    begun
        .recv_timeout(Duration::from_secs(10))
        .expect("the call took the turn");
    release
}

#[tokio::test]
async fn a_job_whose_store_is_out_of_time_as_it_starts_waits_for_it() {
    let bed = Bed::new();
    let job_id = queued(&bed);
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

#[tokio::test(flavor = "current_thread")]
async fn a_stop_that_begins_while_a_claim_waits_for_its_turn_at_the_store_claims_nothing() {
    let bed = Bed::new();
    let job_id = queued(&bed);
    let (store, jobs) = one_at_a_time(&bed);
    assert_eq!(jobs.start().await.expect("a started queue"), 0);
    // The dispatcher is spawned, not yet run: it finds the turn taken.
    let release = take_the_turn(&store).await;
    tokio::task::yield_now().await;
    // It took the slot, and waits for the turn to claim the job.
    let mut stop = Box::pin(jobs.stop());
    assert_eq!(
        polled_once(&mut stop).await,
        None,
        "the claim holds the slot"
    );
    drop(release);
    assert_eq!(within("the stop", stop).await, 0);
    let job = bed.store.job(&job_id).expect("a read").expect("the job");
    assert_eq!(job.phase, JobPhase::Queued, "{job:?}");
}

#[tokio::test(flavor = "current_thread")]
async fn a_dispatcher_that_finds_its_server_stopping_takes_no_slot_even_a_free_one() {
    let bed = Bed::new();
    let job_id = queued(&bed);
    // A select among ready branches picks one at random: enough rounds that
    // one of them would take the slot.
    for round in 0..16 {
        let (store, jobs) = one_at_a_time(&bed);
        assert_eq!(jobs.start().await.expect("a started queue"), 0);
        // Before the dispatcher first runs, the server stops with the slot
        // free, and the turn is taken: a claim would wait for it, holding
        // the slot.
        assert_eq!(jobs.stop().await, 0);
        let release = take_the_turn(&store).await;
        tokio::task::yield_now().await;
        let again = polled_once(&mut Box::pin(jobs.stop())).await;
        assert_eq!(
            again,
            Some(0),
            "round {round}: the dispatcher took the slot"
        );
        drop(release);
    }
    let job = bed.store.job(&job_id).expect("a read").expect("the job");
    assert_eq!(job.phase, JobPhase::Queued, "{job:?}");
}
