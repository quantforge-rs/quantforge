//! The dispatcher and a job's run. The dispatcher claims the job submitted
//! first whenever a slot is free; a run reads the job's input from the
//! store, hands it to the worker on a thread of its own, and writes how it
//! ended. A cancelled job's end is the cancel: the store already says so.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::OwnedSemaphorePermit;
use tracing::{error, info, warn};

use super::Jobs;
use super::worker::{Cancel, JobInput, NO_RUNTIME, WorkerOutcome};
use crate::model::{Candle, JobId, JobOutcome, JobPhase, JobRecord, now_utc_ms};
use crate::ports::CandleQuery;
use crate::protocol::bundle::{Bundle, Runtime};
use crate::protocol::jobs::{self, JobSubmission};
use crate::protocol::{ErrorBody, ErrorCode};
use crate::server::refusal::Refusal;
use crate::server::selection::Selection;
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

/// Why a job failed whose store could not answer in time as it started.
const BUSY: &str =
    "the server's store could not answer in time as the job started; submit it again";

/// How many candles one store call reads for a job.
const CHUNK: usize = 50_000;

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
enum Ended {
    /// With an outcome to write.
    Finished(JobOutcome),
    /// Stopped by its signal, before or while the worker ran it.
    Stopped,
}

/// Runs one claimed job to its end, holding `slot` until that is written.
async fn run(jobs: Jobs, record: JobRecord, slot: OwnedSemaphorePermit) {
    let job_id = record.job_id.clone();
    let cancel = jobs.register(&job_id);
    let outcome = match execute(&jobs, record, &cancel).await {
        Ended::Finished(outcome) => Some(outcome),
        // A client's cancel is written already; the server's stop is not.
        Ended::Stopped if *jobs.inner.stopping.borrow() => {
            Some(failed(ErrorCode::Internal, STOPPED))
        }
        Ended::Stopped => None,
    };
    if let Some(outcome) = outcome {
        finish(&jobs, &job_id, outcome).await;
    }
    jobs.unregister(&job_id);
    drop(slot);
}

/// Reads the job's input and runs it on the worker.
async fn execute(jobs: &Jobs, record: JobRecord, cancel: &Cancel) -> Ended {
    let Some(runtime) = jobs.inner.worker.runtime() else {
        return Ended::Finished(failed(ErrorCode::Unavailable, NO_RUNTIME));
    };
    let input = match read_input(jobs, &record, &runtime, cancel).await {
        Ok(Some(input)) => input,
        Ok(None) => return Ended::Stopped,
        Err(ended) => return ended,
    };
    let worker = Arc::clone(&jobs.inner.worker);
    let signal = cancel.clone();
    let ran = tokio::task::spawn_blocking(move || worker.run(input, &signal)).await;
    match ran {
        Ok(WorkerOutcome::Succeeded(results)) => match serde_json::to_string(&results) {
            Ok(results_json) => Ended::Finished(JobOutcome::Succeeded { results_json }),
            Err(failure) => {
                error!(job = %record.job_id, %failure, "a job's results cannot be written");
                Ended::Finished(failed(
                    ErrorCode::Internal,
                    "the server cannot write the job's results",
                ))
            }
        },
        Ok(WorkerOutcome::Failed(error)) => Ended::Finished(written(error)),
        Ok(WorkerOutcome::Cancelled) if cancel.is_cancelled() => Ended::Stopped,
        Ok(WorkerOutcome::Cancelled) => Ended::Finished(failed(
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

/// The job's input: its submission, its bundle admitted again against the
/// runtime, and its candles, read a chunk per store call. `None` when the
/// job was cancelled meanwhile. The job is looked up once it is registered,
/// so that a cancel either finds it registered or is seen here.
async fn read_input(
    jobs: &Jobs,
    record: &JobRecord,
    runtime: &Runtime,
    cancel: &Cancel,
) -> Result<Option<JobInput>, Ended> {
    let unreadable = |what: &str, why: String| {
        error!(job = %record.job_id, %why, "a job's {what} cannot be read");
        Ended::Finished(failed(
            ErrorCode::Internal,
            format!("the server cannot read the job's {what}"),
        ))
    };
    // A store still out of time after every attempt is worth a new
    // submission; any other refusal is not.
    let refused = |what: &str, refusal: Refusal| {
        if refusal.code == ErrorCode::Unavailable {
            Ended::Finished(failed(ErrorCode::Unavailable, BUSY))
        } else {
            unreadable(what, refusal.message)
        }
    };
    let job_id = jobs::JobId::new(record.job_id.as_str())
        .map_err(|failure| unreadable("id", failure.to_string()))?;
    let submission: JobSubmission = serde_json::from_str(&record.request_json)
        .map_err(|failure| unreadable("submission", failure.to_string()))?;
    let selection = Selection::read_data(&submission.data)
        .map_err(|refusal| unreadable("submission", refusal.message))?;
    let Some(category) = selection.category else {
        return Err(unreadable("submission", "it names no category".to_string()));
    };
    let market = selection.market(category);
    let (id, hash, limits) = (record.job_id.clone(), record.bundle, jobs.inner.limits);
    let looked_up = patiently(jobs, move |store| {
        let running = store
            .job(&id)
            .map_err(|failure| Refusal::storage("reading a job", failure))?
            .is_some_and(|job| job.phase == JobPhase::Running);
        if !running {
            return Ok(None);
        }
        let tar = store
            .load_bundle(&hash)
            .map_err(|failure| Refusal::storage("loading a bundle", failure))?
            .ok_or_else(|| Refusal::internal(format!("the server holds no bundle {hash}")))?;
        Bundle::from_canonical_tar(&tar, &limits)
            .map(Some)
            .map_err(|refused| Refusal::internal(refused.to_string()))
    })
    .await;
    let bundle = match looked_up {
        Ok(Some(bundle)) => bundle,
        Ok(None) => return Ok(None),
        Err(refusal) => return Err(refused("bundle", refusal)),
    };
    if let Err(refusal) = bundle.admit(runtime) {
        return Err(Ended::Finished(failed(
            ErrorCode::InvalidRequest,
            refusal.to_string(),
        )));
    }
    let mut candles: Vec<Candle> = Vec::new();
    let mut next = selection.start_open_ms;
    loop {
        if cancel.is_cancelled() {
            return Ok(None);
        }
        let (market, end) = (market.clone(), selection.end_open_ms);
        let chunk = patiently(jobs, move |store| {
            store
                .load_candles(
                    &market,
                    CandleQuery {
                        start_time_ms: Some(next),
                        end_time_ms: Some(end),
                        limit: Some(CHUNK),
                    },
                )
                .map_err(|failure| Refusal::storage("reading a job's candles", failure))
        })
        .await
        .map_err(|refusal| refused("candles", refusal))?;
        let last = chunk.last().map(|candle| candle.open_time_ms);
        let full = chunk.len() == CHUNK;
        candles.extend(chunk);
        match last {
            // Open times lie within 2^53 of zero, so the step never
            // overflows.
            Some(last) if full => next = last + 1,
            _ => break,
        }
    }
    Ok(Some(JobInput {
        job_id,
        submission,
        bundle,
        candles,
    }))
}

/// Writes a job's end unless it has ended already. A job whose end is never
/// written stays running until the next start fails it.
async fn finish(jobs: &Jobs, job_id: &JobId, outcome: JobOutcome) {
    let (id, end) = (job_id.clone(), outcome.clone());
    let written = patiently(jobs, move |store| {
        store
            .finish_job(&id, &end, now_utc_ms())
            .map_err(|failure| Refusal::storage("ending a job", failure))
    })
    .await;
    match written {
        Ok(true) => info!(job = %job_id, outcome = outcome_name(&outcome), "a job ended"),
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
async fn patiently<T, F>(jobs: &Jobs, work: F) -> Result<T, Refusal>
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
fn failed(code: ErrorCode, message: impl Into<String>) -> JobOutcome {
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
