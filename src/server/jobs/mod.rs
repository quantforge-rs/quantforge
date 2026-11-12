//! The backtest job routes and the queue behind them. A submission is
//! checked and queued in the store; a dispatcher claims queued jobs in the
//! order they were submitted, as many at once as the server runs, reads
//! each one's input from the store, and hands it to the worker on a thread
//! of its own. The store holds every job's state, so a client, or this
//! server after a restart, sees what happened. One server runs a store's
//! queue: claims never collide, but a server that starts fails every job
//! its store holds as running, and a cancel stops only a job running in the
//! server that took it.
//!
//! - `routes`: the upload, submission, status, list and cancel routes.
//! - `results`: the routes that answer what a succeeded job produced.
//! - `follow`: the log route, a page of a job's log or the stream that
//!   follows it.
//! - `submission`: what an upload and a submission must pass.
//! - `queue`: the dispatcher, a job's run, and the stop.
//! - `status`: a job's record as the protocol shows it.
//! - `log`: a job's log, kept within its limits while the job runs.
//! - `worker`: what runs a job, and the signal a cancel sends it.

mod follow;
mod log;
mod queue;
mod results;
mod routes;
mod status;
mod submission;
mod worker;

use std::collections::HashMap;
use std::fmt;
use std::io;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::sync::{Notify, Semaphore, watch};
use tracing::warn;

use super::store::ServerStore;
use crate::model::{JobId, now_utc_ms};
use crate::protocol::bundle::BundleLimits;
use crate::protocol::{ErrorBody, ErrorCode};

pub(super) use follow::logs;
pub(in crate::server) use log::start_of;
pub use log::{DEFAULT_LOG_BYTES, DEFAULT_LOG_LINE_BYTES, DEFAULT_LOG_LINES, JobLog, LogLimits};
pub(super) use results::{result, result_file};
pub(super) use routes::{cancel, job, list, submit, upload};
pub(in crate::server) use worker::NO_RUNTIME;
pub use worker::{Cancel, JobInput, JobWorker, NoRuntime, WorkerOutcome};

/// How long a stopping server waits for the log streams it has open to say
/// their last and close.
const STREAMS_PATIENCE: Duration = Duration::from_secs(5);

/// How many jobs one server runs at once, how long a job's range may be,
/// and how much of a job's log it keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JobLimits {
    /// How many jobs run at once; the others wait in the queue.
    pub concurrency: usize,
    /// The most bars one job's range may span: a job's candles are read
    /// into memory before it starts.
    pub max_span_bars: u64,
    /// How much of each job's log is kept: what a job writes beyond it is
    /// dropped, oldest first, and counted.
    pub log: LogLimits,
}

/// A job running here: the signal that stops it, and its log.
struct Running {
    cancel: Cancel,
    log: JobLog,
}

/// The job routes and the queue: the server's store, its worker, and the
/// jobs running here.
#[derive(Clone)]
pub struct Jobs {
    inner: Arc<Inner>,
}

struct Inner {
    store: ServerStore,
    worker: Arc<dyn JobWorker>,
    /// The bundle spec's default limits: every upload is read against them.
    limits: BundleLimits,
    job_limits: JobLimits,
    /// One for each job that can run now.
    slots: Arc<Semaphore>,
    /// Told when a job is queued, so that the dispatcher looks at once.
    wake: Notify,
    /// The jobs running here, by ID, each with the signal that stops it
    /// and its log.
    running: Mutex<HashMap<String, Running>>,
    /// `true` once the server stops: nothing more is claimed.
    stopping: watch::Sender<bool>,
    /// Counts the jobs that started or were cancelled here, so that a log
    /// stream waiting for a queued job wakes when its job may have moved.
    moved: watch::Sender<u64>,
    /// How many log streams are open.
    streams: watch::Sender<usize>,
}

impl Jobs {
    /// The job routes over the server's store, running jobs on `worker`.
    /// Nothing runs before [`Jobs::start`].
    pub fn new<W: JobWorker + 'static>(store: ServerStore, worker: W, limits: JobLimits) -> Self {
        // A stop waits for every slot at once, in one u32 of permits.
        let most = Semaphore::MAX_PERMITS.min(u32::MAX as usize);
        let concurrency = limits.concurrency.clamp(1, most);
        Self {
            inner: Arc::new(Inner {
                store,
                worker: Arc::new(worker),
                limits: BundleLimits::default(),
                job_limits: JobLimits {
                    concurrency,
                    ..limits
                },
                slots: Arc::new(Semaphore::new(concurrency)),
                wake: Notify::new(),
                running: Mutex::new(HashMap::new()),
                stopping: watch::channel(false).0,
                moved: watch::channel(0).0,
                streams: watch::channel(0).0,
            }),
        }
    }

    /// Fails the jobs the store holds as running, which ran when the server
    /// on it was killed, and then starts running the queued ones. Answers
    /// how many jobs it failed. Call it once, before serving, and on a store
    /// no other server runs jobs from.
    pub async fn start(&self) -> io::Result<usize> {
        let failure = serde_json::to_string(&ErrorBody {
            code: ErrorCode::Internal,
            message: queue::STOPPED.to_string(),
            supported: None,
        })
        .map_err(io::Error::other)?;
        let failed = self
            .inner
            .store
            .call(move |store| {
                store
                    .fail_running_jobs(&failure, now_utc_ms())
                    .map_err(|failure| {
                        super::refusal::Refusal::storage("failing the jobs that ran", failure)
                    })
            })
            .await
            .map_err(|refusal| io::Error::other(refusal.message))?;
        if failed > 0 {
            warn!(
                failed,
                "failed the jobs that were running when the server last stopped"
            );
        }
        tokio::spawn(queue::dispatch(self.clone()));
        Ok(failed)
    }

    /// Stops claiming jobs, stops the running ones, and returns once each
    /// has ended, those it stopped failed so that their clients know to
    /// submit them again, and once the log streams it had open have said
    /// their last, or a few seconds have passed. Answers how many jobs it
    /// stopped. A caller that cannot wait drops this future: the next start
    /// fails them.
    pub async fn stop(&self) -> usize {
        let stopped = {
            let running = self.running();
            self.inner.stopping.send_replace(true);
            running.values().for_each(|job| job.cancel.cancel());
            running.len()
        };
        let all = u32::try_from(self.inner.job_limits.concurrency).unwrap_or(u32::MAX);
        // Every job holds a slot until its end and its log are written.
        let _ = self.inner.slots.acquire_many(all).await;
        // A stream ends with its job, or with the server: either way soon.
        let mut open = self.inner.streams.subscribe();
        let closed = open.wait_for(|open| *open == 0);
        let _ = tokio::time::timeout(STREAMS_PATIENCE, closed).await;
        stopped
    }

    /// How many jobs run at once.
    pub fn concurrency(&self) -> usize {
        self.inner.job_limits.concurrency
    }

    fn running(&self) -> std::sync::MutexGuard<'_, HashMap<String, Running>> {
        self.inner
            .running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Records a job as running here, and answers its signal, already sent
    /// when the server is stopping, and its log, empty.
    fn register(&self, job_id: &JobId) -> (Cancel, JobLog) {
        let job = Running {
            cancel: Cancel::new(),
            log: JobLog::new(self.inner.job_limits.log),
        };
        let handles = (job.cancel.clone(), job.log.clone());
        {
            let mut running = self.running();
            if *self.inner.stopping.borrow() {
                job.cancel.cancel();
            }
            running.insert(job_id.to_string(), job);
        }
        self.moved();
        handles
    }

    /// Wakes the log streams that wait for a queued job: one has started
    /// here, or was cancelled.
    fn moved(&self) {
        self.inner.moved.send_modify(|moved| *moved += 1);
    }

    fn unregister(&self, job_id: &JobId) {
        self.running().remove(job_id.as_str());
    }

    /// Sends a job's signal, when it runs here.
    fn signal(&self, job_id: &JobId) {
        if let Some(job) = self.running().get(job_id.as_str()) {
            job.cancel.cancel();
        }
    }

    /// The log of a job that runs here, or has just ended here and whose
    /// log is not stored yet.
    fn live_log(&self, job_id: &JobId) -> Option<JobLog> {
        self.running()
            .get(job_id.as_str())
            .map(|job| job.log.clone())
    }

    /// Counts a log stream as open until what this answers is dropped: a
    /// stopping server waits for the open ones to say their last.
    fn stream_opened(&self) -> OpenStream {
        self.inner.streams.send_modify(|open| *open += 1);
        OpenStream(self.clone())
    }
}

/// A log stream, counted as open while this lives.
struct OpenStream(Jobs);

impl Drop for OpenStream {
    fn drop(&mut self) {
        self.0
            .inner
            .streams
            .send_modify(|open| *open = open.saturating_sub(1));
    }
}

impl fmt::Debug for Jobs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Jobs")
            .field("limits", &self.inner.job_limits)
            .finish_non_exhaustive()
    }
}
