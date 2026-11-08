//! What runs a job: a worker the server hands each job's input, read for it
//! from the store, and the signal a cancel sends it. A worker never sees the
//! store, its URL or its credentials: the server parent reads and writes
//! everything a job needs.

use std::fmt;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use crate::model::Candle;
use crate::protocol::bundle::{Bundle, Runtime};
use crate::protocol::jobs::{JobId, JobSubmission, ResultRef};
use crate::protocol::{ErrorBody, ErrorCode};

/// Why a server with no worker runtime fails each job as it starts.
pub(super) const NO_RUNTIME: &str = "this server cannot run Python bundles yet; a later \
     QuantForge release brings the worker runtime";

/// Everything one job runs on, read for it by the server as it started.
#[derive(Debug)]
pub struct JobInput {
    /// The job.
    pub job_id: JobId,
    /// What was asked, its market's category named.
    pub submission: JobSubmission,
    /// The bundle, checked against its name and the spec.
    pub bundle: Bundle,
    /// The range's candles, oldest first, every decimal with its scale.
    pub candles: Vec<Candle>,
}

/// How a run ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkerOutcome {
    /// Finished, with what it produced.
    Succeeded(Vec<ResultRef>),
    /// Finished without results, with why: `invalid_request` when the bundle
    /// or its data is at fault, `internal` or `unavailable` when the server
    /// is.
    Failed(ErrorBody),
    /// Stopped because a cancel said so.
    Cancelled,
}

/// The signal a cancel sends a running job. Cloned, it is the same signal.
#[derive(Clone, Default)]
pub struct Cancel(Arc<(Mutex<bool>, Condvar)>);

impl Cancel {
    /// A signal not sent yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sends it: every clone sees it, and a wait ends.
    pub fn cancel(&self) {
        let (sent, changed) = &*self.0;
        if let Ok(mut sent) = sent.lock() {
            *sent = true;
        }
        changed.notify_all();
    }

    /// Whether it was sent.
    pub fn is_cancelled(&self) -> bool {
        self.0.0.lock().map(|sent| *sent).unwrap_or(true)
    }

    /// Waits up to `timeout` for it, and answers whether it came.
    pub fn wait(&self, timeout: Duration) -> bool {
        let (sent, changed) = &*self.0;
        let Ok(guard) = sent.lock() else {
            return true;
        };
        match changed.wait_timeout_while(guard, timeout, |sent| !*sent) {
            Ok((sent, _)) => *sent,
            Err(_) => true,
        }
    }
}

impl fmt::Debug for Cancel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Cancel")
            .field("cancelled", &self.is_cancelled())
            .finish()
    }
}

/// What runs jobs.
pub trait JobWorker: Send + Sync {
    /// What the worker runs bundles on, which every bundle is admitted
    /// against, or `None` when it can run none: the server then fails each
    /// job as it starts, with `unavailable`.
    fn runtime(&self) -> Option<Runtime>;

    /// Runs one job to its end, on a thread of its own, and returns soon
    /// after `cancel` is sent: a server that stops sends it to every job
    /// and waits for each run to return, and so does the process as it
    /// exits.
    fn run(&self, input: JobInput, cancel: &Cancel) -> WorkerOutcome;
}

/// The worker of a server that cannot run Python bundles yet: it offers no
/// runtime, so every job fails as it starts.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoRuntime;

impl JobWorker for NoRuntime {
    fn runtime(&self) -> Option<Runtime> {
        None
    }

    fn run(&self, _input: JobInput, _cancel: &Cancel) -> WorkerOutcome {
        WorkerOutcome::Failed(ErrorBody {
            code: ErrorCode::Unavailable,
            message: NO_RUNTIME.to_string(),
            supported: None,
        })
    }
}
