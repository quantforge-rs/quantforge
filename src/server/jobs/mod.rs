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
//! - `routes`: the five routes.
//! - `submission`: what an upload and a submission must pass.
//! - `queue`: the dispatcher, a job's run, and the stop.
//! - `status`: a job's record as the protocol shows it.
//! - `worker`: what runs a job, and the signal a cancel sends it.

mod queue;
mod routes;
mod status;
mod submission;
mod worker;

use std::collections::HashMap;
use std::fmt;
use std::io;
use std::sync::{Arc, Mutex, PoisonError};

use tokio::sync::{Notify, Semaphore, watch};
use tracing::warn;

use super::store::ServerStore;
use crate::model::{JobId, now_utc_ms};
use crate::protocol::bundle::BundleLimits;
use crate::protocol::{ErrorBody, ErrorCode};

pub(super) use routes::{cancel, job, list, submit, upload};
pub use worker::{Cancel, JobInput, JobWorker, NoRuntime, WorkerOutcome};

/// How many jobs one server runs at once, and how long a job's range may
/// be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JobLimits {
    /// How many jobs run at once; the others wait in the queue.
    pub concurrency: usize,
    /// The most bars one job's range may span: a job's candles are read
    /// into memory before it starts.
    pub max_span_bars: u64,
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
    /// The jobs running here, by ID, each with the signal that stops it.
    running: Mutex<HashMap<String, Cancel>>,
    /// `true` once the server stops: nothing more is claimed.
    stopping: watch::Sender<bool>,
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
    /// submit them again. Answers how many it stopped. A caller that cannot
    /// wait drops this future: the next start fails them.
    pub async fn stop(&self) -> usize {
        let stopped = {
            let running = self.running();
            self.inner.stopping.send_replace(true);
            running.values().for_each(Cancel::cancel);
            running.len()
        };
        let all = u32::try_from(self.inner.job_limits.concurrency).unwrap_or(u32::MAX);
        // Every job holds a slot until its end is written.
        let _ = self.inner.slots.acquire_many(all).await;
        stopped
    }

    /// How many jobs run at once.
    pub fn concurrency(&self) -> usize {
        self.inner.job_limits.concurrency
    }

    fn running(&self) -> std::sync::MutexGuard<'_, HashMap<String, Cancel>> {
        self.inner
            .running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Records a job as running here, and answers its signal: already sent
    /// when the server is stopping.
    fn register(&self, job_id: &JobId) -> Cancel {
        let cancel = Cancel::new();
        let mut running = self.running();
        if *self.inner.stopping.borrow() {
            cancel.cancel();
        }
        running.insert(job_id.to_string(), cancel.clone());
        cancel
    }

    fn unregister(&self, job_id: &JobId) {
        self.running().remove(job_id.as_str());
    }

    /// Sends a job's signal, when it runs here.
    fn signal(&self, job_id: &JobId) {
        if let Some(cancel) = self.running().get(job_id.as_str()) {
            cancel.cancel();
        }
    }
}

impl fmt::Debug for Jobs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Jobs")
            .field("limits", &self.inner.job_limits)
            .finish_non_exhaustive()
    }
}
