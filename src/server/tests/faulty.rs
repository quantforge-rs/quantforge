//! A store that fails, or stalls until released, or lists its markets
//! backwards, or finds itself out of time for its first candle reads, on
//! demand, and otherwise answers as the store it wraps: for what no real
//! store does on cue.

use std::sync::Mutex;
use std::sync::mpsc::{Receiver, Sender};
use std::time::Duration;

use crate::model::{
    BundleHash, Candle, JobId, JobOutcome, JobRecord, MarketCoverage, MarketId, NewJob, ResultHash,
    SegmentHash, SegmentPlan, SegmentRefresh, TimestampMs,
};
use crate::ports::{CandleQuery, CandleStore, JobStore, SegmentStore, StorageError};

/// A store that fails, stalls or lists backwards on demand; otherwise it
/// answers as the store it wraps.
pub(super) struct Faulty<S> {
    inner: S,
    failure: Option<fn() -> StorageError>,
    stall: Option<Stall>,
    backwards: bool,
    /// Candle reads still to find the store out of time.
    busy_loads: Mutex<usize>,
}

/// A stall: a call says it began, then waits until released.
struct Stall {
    began: Mutex<Sender<()>>,
    release: Mutex<Receiver<()>>,
}

impl<S> Faulty<S> {
    fn wrapping(inner: S) -> Self {
        Self {
            inner,
            failure: None,
            stall: None,
            backwards: false,
            busy_loads: Mutex::new(0),
        }
    }

    pub(super) fn failing(inner: S, failure: fn() -> StorageError) -> Self {
        Self {
            failure: Some(failure),
            ..Self::wrapping(inner)
        }
    }

    pub(super) fn stalling(inner: S, began: Sender<()>, release: Receiver<()>) -> Self {
        Self {
            stall: Some(Stall {
                began: Mutex::new(began),
                release: Mutex::new(release),
            }),
            ..Self::wrapping(inner)
        }
    }

    /// The port promises no order of markets: this one lists them backwards.
    pub(super) fn backwards(inner: S) -> Self {
        Self {
            backwards: true,
            ..Self::wrapping(inner)
        }
    }

    /// The first `count` candle reads find the store out of time.
    pub(super) fn busy_loads(inner: S, count: usize) -> Self {
        Self {
            busy_loads: Mutex::new(count),
            ..Self::wrapping(inner)
        }
    }

    fn fault(&self) -> Result<(), StorageError> {
        if let Some(stall) = &self.stall {
            let _ = stall.began.lock().expect("the stall").send(());
            let _ = stall
                .release
                .lock()
                .expect("the stall")
                .recv_timeout(Duration::from_secs(10));
        }
        match self.failure {
            Some(failure) => Err(failure()),
            None => Ok(()),
        }
    }
}

impl<S: CandleStore> CandleStore for Faulty<S> {
    fn init(&self) -> Result<(), StorageError> {
        self.fault()?;
        self.inner.init()
    }

    fn upsert_candles(&self, market: &MarketId, candles: &[Candle]) -> Result<usize, StorageError> {
        self.fault()?;
        self.inner.upsert_candles(market, candles)
    }

    fn load_candles(
        &self,
        market: &MarketId,
        query: CandleQuery,
    ) -> Result<Vec<Candle>, StorageError> {
        self.fault()?;
        {
            let mut left = self.busy_loads.lock().expect("the busy reads");
            if *left > 0 {
                *left -= 1;
                return Err(StorageError::Unavailable("busy".to_string()));
            }
        }
        self.inner.load_candles(market, query)
    }

    fn load_recent_candles(
        &self,
        market: &MarketId,
        limit: usize,
    ) -> Result<Vec<Candle>, StorageError> {
        self.fault()?;
        self.inner.load_recent_candles(market, limit)
    }

    fn max_open_time_ms(&self, market: &MarketId) -> Result<Option<TimestampMs>, StorageError> {
        self.fault()?;
        self.inner.max_open_time_ms(market)
    }

    fn market_coverage(&self) -> Result<Vec<MarketCoverage>, StorageError> {
        self.fault()?;
        let mut covered = self.inner.market_coverage()?;
        if self.backwards {
            covered.reverse();
        }
        Ok(covered)
    }
}

impl<S: SegmentStore> SegmentStore for Faulty<S> {
    fn refresh_segments(&self, market: &MarketId) -> Result<SegmentRefresh, StorageError> {
        self.fault()?;
        self.inner.refresh_segments(market)
    }

    fn resolve_segments(
        &self,
        market: &MarketId,
        first_open_ms: TimestampMs,
        last_open_ms: TimestampMs,
    ) -> Result<SegmentPlan, StorageError> {
        self.fault()?;
        self.inner
            .resolve_segments(market, first_open_ms, last_open_ms)
    }

    fn load_segment(&self, hash: &SegmentHash) -> Result<Option<Vec<u8>>, StorageError> {
        self.fault()?;
        self.inner.load_segment(hash)
    }
}

impl<S: JobStore> JobStore for Faulty<S> {
    fn store_bundle(
        &self,
        hash: &BundleHash,
        tar: &[u8],
        stored_at_ms: TimestampMs,
    ) -> Result<bool, StorageError> {
        self.fault()?;
        self.inner.store_bundle(hash, tar, stored_at_ms)
    }

    fn load_bundle(&self, hash: &BundleHash) -> Result<Option<Vec<u8>>, StorageError> {
        self.fault()?;
        self.inner.load_bundle(hash)
    }

    fn submit_job(&self, job: &NewJob) -> Result<(), StorageError> {
        self.fault()?;
        self.inner.submit_job(job)
    }

    fn job(&self, job_id: &JobId) -> Result<Option<JobRecord>, StorageError> {
        self.fault()?;
        self.inner.job(job_id)
    }

    fn jobs(&self, limit: usize) -> Result<Vec<JobRecord>, StorageError> {
        self.fault()?;
        self.inner.jobs(limit)
    }

    fn claim_job(&self, started_at_ms: TimestampMs) -> Result<Option<JobRecord>, StorageError> {
        self.fault()?;
        self.inner.claim_job(started_at_ms)
    }

    fn finish_job(
        &self,
        job_id: &JobId,
        outcome: &JobOutcome,
        finished_at_ms: TimestampMs,
    ) -> Result<bool, StorageError> {
        self.fault()?;
        self.inner.finish_job(job_id, outcome, finished_at_ms)
    }

    fn load_result(&self, hash: &ResultHash) -> Result<Option<Vec<u8>>, StorageError> {
        self.fault()?;
        self.inner.load_result(hash)
    }

    fn cancel_job(
        &self,
        job_id: &JobId,
        finished_at_ms: TimestampMs,
    ) -> Result<Option<JobRecord>, StorageError> {
        self.fault()?;
        self.inner.cancel_job(job_id, finished_at_ms)
    }

    fn fail_running_jobs(
        &self,
        failure_json: &str,
        finished_at_ms: TimestampMs,
    ) -> Result<usize, StorageError> {
        self.fault()?;
        self.inner.fail_running_jobs(failure_json, finished_at_ms)
    }
}
