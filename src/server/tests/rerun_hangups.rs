//! A rerun whose client hangs up while the store still reads for it: each
//! read keeps the rerun's job slot until it ends, so that neither another
//! job nor a stop takes the slot from under it.

use std::sync::mpsc;

use axum::extract::{Path, State};

use super::backtesting::{Backtesting, Later, ran, running, stored};
use super::*;

#[tokio::test]
async fn a_rerun_whose_client_hangs_up_keeps_its_slot_until_the_store_has_read_for_it() {
    let bed = Bed::new();
    stored(&bed);
    let worker = Backtesting::new(Later::Costlier);
    let handed = Arc::clone(&worker.handed);
    let (address, jobs) = running(&bed, worker).await;
    let (job, _) = ran(address).await;
    jobs.stop().await;

    // Each read a rerun asks of the store, held back on a server of one
    // slot: its bundle, its segments, and the job's copy of a result file
    // that differs from the rerun's, the worker's later runs being
    // costlier.
    for read in ["load_bundle", "load_segment", "load_result"] {
        let (began, begun) = mpsc::channel();
        let (release, released) = mpsc::channel::<()>();
        let store = bed.store.within(Duration::from_secs(2));
        let worker = Backtesting {
            handed: Arc::clone(&handed),
            ..Backtesting::new(Later::Costlier)
        };
        let jobs = Jobs::new(
            served(Faulty::holding_reads(store, read, began, released)),
            worker,
            JobLimits {
                concurrency: 1,
                max_span_bars: DEFAULT_MAX_SPAN_BARS,
                log: LogLimits::default(),
            },
        );
        let asked = tokio::spawn(crate::server::jobs::rerun(
            State(jobs.clone()),
            Ok(Path(job.clone())),
        ));
        let waited =
            tokio::task::spawn_blocking(move || begun.recv_timeout(Duration::from_secs(10)))
                .await
                .expect("the wait");
        assert!(waited.is_ok(), "no {read} began");

        // The client hangs up: its handler is dropped as the store reads.
        asked.abort();
        let dropped = asked.await;
        assert!(
            dropped
                .as_ref()
                .is_err_and(|failure| failure.is_cancelled()),
            "{read}: {dropped:?}"
        );
        // The read still holds the slot: a stop, which waits for every
        // slot, waits for it to end.
        let mut stop = Box::pin(jobs.stop());
        assert!(
            tokio::time::timeout(Duration::from_millis(200), &mut stop)
                .await
                .is_err(),
            "the slot was free while {read} still read"
        );
        drop(release);
        within("the stop", stop).await;
    }
}
