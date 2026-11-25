//! The dispatcher and a job's run. The dispatcher claims the job submitted
//! first whenever a slot is free; a run reads the job's input from the
//! store and records its provenance (`inputs`), hands the input to the
//! worker on a thread of its own, and writes how it ended, a succeeded
//! job's result files in the same change, and then what is kept of its
//! log. A cancelled job's end is the cancel: the store already says so.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::OwnedSemaphorePermit;
use tracing::{error, info, warn};

use super::Jobs;
use super::inputs::read_input;
use super::log::JobLog;
use super::worker::{Cancel, NO_RUNTIME, WorkerOutcome};
use crate::model::{JobId, JobOutcome, JobRecord, StoredResult, now_utc_ms};
use crate::protocol::jobs::ResultRef;
use crate::protocol::{ErrorBody, ErrorCode};
use crate::server::refusal::Refusal;
use crate::server::results::ResultFile;
use crate::server::store::ServedStore;

/// Why a job that ran when its server stopped failed.
pub(super) const STOPPED: &str = "the server stopped while the job ran; submit it again";

/// How long the dispatcher waits, with nothing queued or the store not
/// answering, before it looks again. A submission wakes it at once.
const IDLE: Duration = Duration::from_secs(5);

/// How long a job's store call waits before it is made again, when the
/// store could not answer in time.
const RETRY: Duration = Duration::from_secs(1);

/// How many times a job's store call is made while the store cannot answer
/// in time.
const ATTEMPTS: u32 = 10;

/// Claims jobs, the first submitted first, while a slot is free, until the
/// server stops.
pub(super) async fn dispatch(jobs: Jobs) {
    let mut stopping = jobs.inner.stopping.subscribe();
    loop {
        // A stop comes first: a stopping server takes no slot, even a free
        // one.
        let slot = tokio::select! {
            biased;
            _ = stopping.wait_for(|stopping| *stopping) => return,
            slot = Arc::clone(&jobs.inner.slots).acquire_owned() => match slot {
                Ok(slot) => slot,
                Err(_) => return,
            },
        };
        let claimer = jobs.clone();
        let claimed = jobs
            .inner
            .store
            .call(move |store| {
                // A stop that began while the claim waited for its turn
                // claims nothing; the job a claim in progress takes is
                // stopped like any running one.
                if *claimer.inner.stopping.borrow() {
                    return Ok(None);
                }
                store
                    .claim_job(now_utc_ms())
                    .map_err(|failure| Refusal::storage("claiming a job", failure))
            })
            .await;
        match claimed {
            Ok(Some(record)) => {
                tokio::spawn(run(jobs.clone(), record, slot));
                continue;
            }
            Ok(None) => drop(slot),
            Err(refusal) => {
                drop(slot);
                warn!(error = %refusal.message, "no job could be claimed; trying again");
            }
        }
        tokio::select! {
            () = jobs.inner.wake.notified() => {}
            () = tokio::time::sleep(IDLE) => {}
            _ = stopping.wait_for(|stopping| *stopping) => return,
        }
    }
}

/// How a run ended.
pub(super) enum Ended {
    /// With an outcome to write.
    Finished(JobOutcome),
    /// Stopped by its signal, before or while the worker ran it.
    Stopped,
}

/// Runs one claimed job to its end, holding `slot` until that and its log
/// are written. The job's log stays here, for whoever follows it, until
/// the store holds it.
async fn run(jobs: Jobs, record: JobRecord, slot: OwnedSemaphorePermit) {
    let job_id = record.job_id.clone();
    let (cancel, log) = jobs.register(&job_id);
    let outcome = match execute(&jobs, record, &cancel, &log).await {
        Ended::Finished(outcome) => Some(outcome),
        // A client's cancel is written already; the server's stop is not.
        Ended::Stopped if *jobs.inner.stopping.borrow() => {
            Some(failed(ErrorCode::Internal, STOPPED))
        }
        Ended::Stopped => None,
    };
    // The job's run is over, and its log takes no more. The log is stored
    // before the job's end is, so that whoever reads the end from the store
    // finds the log there too; and whoever follows the log here is told
    // that it has ended only once the store holds it and says how the job
    // did.
    log.close();
    keep_log(&jobs, &job_id, &log).await;
    if let Some(outcome) = outcome {
        finish(&jobs, &job_id, outcome).await;
    }
    log.end();
    jobs.unregister(&job_id);
    drop(slot);
}

/// Stores what is kept of a job's log, when anything was written to it. A
/// log that cannot be stored is lost with the server's memory of it.
async fn keep_log(jobs: &Jobs, job_id: &JobId, log: &JobLog) {
    let Some(log_json) = log.stored_json() else {
        return;
    };
    let (id, log_json) = (job_id.clone(), Arc::new(log_json));
    let stored = patiently(jobs, move |store| {
        store
            .store_job_log(&id, &log_json, now_utc_ms())
            .map_err(|failure| Refusal::storage("storing a job's log", failure))
    })
    .await;
    if let Err(refusal) = stored {
        error!(job = %job_id, error = %refusal.message, "a job's log could not be stored, and is lost");
    }
}

/// Reads the job's input and runs it on the worker, which writes what the
/// job says into `log`.
async fn execute(jobs: &Jobs, record: JobRecord, cancel: &Cancel, log: &JobLog) -> Ended {
    let Some(runtime) = jobs.inner.worker.runtime() else {
        return Ended::Finished(failed(ErrorCode::Unavailable, NO_RUNTIME));
    };
    let input = match read_input(jobs, &record, &runtime, cancel).await {
        Ok(Some(input)) => input,
        Ok(None) => return Ended::Stopped,
        Err(ended) => return ended,
    };
    let worker = Arc::clone(&jobs.inner.worker);
    let (signal, said) = (cancel.clone(), log.clone());
    // The files are packed for the store on the worker's thread too.
    let ran = tokio::task::spawn_blocking(move || match worker.run(input, &signal, &said) {
        WorkerOutcome::Succeeded(files) => Ran::Succeeded(succeeded(&files)),
        WorkerOutcome::Failed(error) => Ran::Failed(error),
        WorkerOutcome::Cancelled => Ran::Cancelled,
    })
    .await;
    match ran {
        Ok(Ran::Succeeded(Ok(outcome))) => Ended::Finished(outcome),
        Ok(Ran::Succeeded(Err(failure))) => {
            error!(job = %record.job_id, %failure, "a job's results cannot be written");
            Ended::Finished(failed(
                ErrorCode::Internal,
                "the server cannot write the job's results",
            ))
        }
        Ok(Ran::Failed(error)) => Ended::Finished(written(error)),
        Ok(Ran::Cancelled) if cancel.is_cancelled() => Ended::Stopped,
        Ok(Ran::Cancelled) => Ended::Finished(failed(
            ErrorCode::Internal,
            "the worker stopped the job without being asked to",
        )),
        Err(failure) => {
            error!(job = %record.job_id, %failure, "the worker failed while it ran a job");
            Ended::Finished(failed(
                ErrorCode::Internal,
                "the worker failed while it ran the job",
            ))
        }
    }
}

/// How a worker's run ended, a succeeded one's files already packed.
enum Ran {
    Succeeded(std::io::Result<JobOutcome>),
    Failed(ErrorBody),
    Cancelled,
}

/// A succeeded job's outcome: the list its record shows, and each file as
/// the store keeps it.
fn succeeded(files: &[ResultFile]) -> std::io::Result<JobOutcome> {
    let listed: Vec<&ResultRef> = files.iter().map(|file| &file.reference).collect();
    let results_json = serde_json::to_string(&listed)?;
    let results: Vec<StoredResult> = files
        .iter()
        .map(ResultFile::stored)
        .collect::<Result<_, _>>()?;
    Ok(JobOutcome::Succeeded {
        results_json,
        results,
    })
}

/// Writes a job's end unless it has ended already. A job whose end is never
/// written stays running until the next start fails it.
async fn finish(jobs: &Jobs, job_id: &JobId, outcome: JobOutcome) {
    let name = outcome_name(&outcome);
    // Shared with every attempt: an outcome holds the job's result files.
    let (id, end) = (job_id.clone(), Arc::new(outcome));
    let written = patiently(jobs, move |store| {
        store
            .finish_job(&id, &end, now_utc_ms())
            .map_err(|failure| Refusal::storage("ending a job", failure))
    })
    .await;
    match written {
        Ok(true) => info!(job = %job_id, outcome = name, "a job ended"),
        Ok(false) => info!(job = %job_id, "a job ended after it was cancelled"),
        Err(refusal) => error!(
            job = %job_id,
            error = %refusal.message,
            "a job's end could not be written; the server fails it when it next starts"
        ),
    }
}

/// Makes `work`'s store call, and makes it again while the store cannot
/// answer in time, up to `ATTEMPTS` times: a job waits out a busy store
/// rather than fail.
pub(super) async fn patiently<T, F>(jobs: &Jobs, work: F) -> Result<T, Refusal>
where
    T: Send + 'static,
    F: Fn(&dyn ServedStore) -> Result<T, Refusal> + Clone + Send + 'static,
{
    for _ in 1..ATTEMPTS {
        match jobs.inner.store.call(work.clone()).await {
            Err(refusal) if refusal.code == ErrorCode::Unavailable => {
                tokio::time::sleep(RETRY).await;
            }
            done => return done,
        }
    }
    jobs.inner.store.call(work).await
}

fn outcome_name(outcome: &JobOutcome) -> &'static str {
    match outcome {
        JobOutcome::Succeeded { .. } => "succeeded",
        JobOutcome::Failed { .. } => "failed",
    }
}

/// A failed outcome with `code` and `message`.
pub(super) fn failed(code: ErrorCode, message: impl Into<String>) -> JobOutcome {
    written(ErrorBody {
        code,
        message: message.into(),
        supported: None,
    })
}

/// A failed outcome with `error`, as JSON.
fn written(error: ErrorBody) -> JobOutcome {
    let failure_json = serde_json::to_string(&error).unwrap_or_else(|_| {
        r#"{"code":"internal","message":"the server cannot write why the job failed"}"#.to_string()
    });
    JobOutcome::Failed { failure_json }
}
