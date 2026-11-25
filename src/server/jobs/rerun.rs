//! `POST /v1/backtests/{job_id}/rerun`: a succeeded job run again from its
//! provenance alone, on the bars of the segments it names and the bundle of
//! its code hash, each checked against its name first; and its result
//! compared with the job's (`compare`). A job whose range spans more bars
//! than this server runs one job on is refused at once. The rerun takes a
//! slot like any job before it reads more than the provenance, so that one
//! waiting for its turn holds no bars, and keeps it until it answers; each
//! store call that reads for it, the worker it runs on and the thread that
//! compares its result keep it until they end, even when its client hangs
//! up. It is registered as running here as it starts to wait for the slot,
//! so that a client that hangs up stops it, and so does a stopping server
//! until the rerun's worker has ended: a result the worker produced is
//! compared and answered all the same. It changes nothing the server holds:
//! the report is its answer, and the log records each verdict.

use std::sync::Arc;

use axum::Json;
use axum::extract::rejection::PathRejection;
use axum::extract::{Path, State};
use axum::response::Response;
use sha2::{Digest, Sha256};
use tokio::sync::OwnedSemaphorePermit;
use tracing::{error, info};

use super::bars::{SegmentFault, read_segments};
use super::compare::compare;
use super::provenance::ledger;
use super::queue::patiently;
use super::results::listed;
use super::routes::job_named;
use super::submission::beyond_span;
use super::worker::{Cancel, JobInput, NO_RUNTIME, WorkerOutcome};
use super::{JobLog, Jobs};
use crate::model::{BundleHash, Candle, JobId, JobPhase, SegmentHash, ms_to_rfc3339};
use crate::protocol::bundle::Bundle;
use crate::protocol::jobs::{
    self, DataSelection, JobSubmission, ProvenanceInputs, ProvenanceRuntime, RerunReport,
    RerunVerdict, ResultComparison,
};
use crate::protocol::{ContentHash, ErrorBody, ErrorCode};
use crate::server::history::fresh;
use crate::server::refusal::Refusal;
use crate::server::results::ResultFile;
use crate::server::selection::Selection;

/// `POST /v1/backtests/{job_id}/rerun`.
pub(in crate::server) async fn rerun(
    State(jobs): State<Jobs>,
    id: Result<Path<String>, PathRejection>,
) -> Result<Response, Refusal> {
    let Path(text) = id.map_err(|rejection| Refusal::invalid(rejection.body_text()))?;
    let job_id = job_named(text)?;
    let lookup = job_id.clone();
    let ledger = jobs
        .inner
        .store
        .call(move |store| ledger(store, &lookup))
        .await?;
    let stands = match ledger.record.phase {
        JobPhase::Succeeded => None,
        JobPhase::Queued => Some("is still queued"),
        JobPhase::Running => Some("is still running"),
        JobPhase::Failed => Some("failed"),
        JobPhase::Cancelled => Some("was cancelled"),
    };
    if let Some(stands) = stands {
        return Err(Refusal::conflict(format!(
            "job {job_id} {stands}; only a job that has succeeded is rerun, against its result"
        )));
    }
    let recorded = listed(&ledger.record)?;
    let inputs = ledger.stored.inputs;
    let submission = submission_of(&inputs);
    let selection = Selection::read_data(&submission.data).map_err(|refused| {
        Refusal::internal(format!(
            "the server cannot read job {job_id}'s range: {}",
            refused.message
        ))
    })?;
    if let Some(beyond) = beyond_span(&jobs, &selection) {
        return Err(Refusal::conflict(format!(
            "job {job_id} cannot be rerun on this server: {beyond}"
        )));
    }

    // Nothing more is read before a slot is free, as a job reads nothing
    // before it has one; the slot is shared with the work handed to other
    // threads, and free once all of it has ended.
    let (run, slot) = take_slot(&jobs).await?;
    let Some((bundle, candles)) = read_inputs(&jobs, &job_id, &inputs, &run.cancel, &slot).await?
    else {
        return Err(stopped());
    };
    let runtime = jobs
        .inner
        .worker
        .runtime()
        .ok_or_else(|| Refusal::unavailable(NO_RUNTIME))?;
    if let Err(refused) = bundle.admit(&runtime) {
        return Err(Refusal::conflict(format!(
            "job {job_id} cannot be rerun on this server: {refused}; the job ran on {}",
            described(&inputs.runtime)
        )));
    }
    if u64::try_from(candles.len()).ok() != Some(inputs.data.bar_count) {
        error!(
            job = %job_id,
            found = candles.len(),
            recorded = inputs.data.bar_count,
            "the segments a job's provenance names do not hold the bars it ran on"
        );
        return Err(Refusal::internal(format!(
            "the segments job {job_id}'s provenance names hold {} of its range's bars, not the \
             {} it ran on",
            candles.len(),
            inputs.data.bar_count
        )));
    }
    let ran_on = ProvenanceRuntime {
        engine: runtime.engine,
        python: runtime.python,
        deps: bundle.manifest().deps().clone(),
    };
    let input = JobInput {
        job_id: run.name.clone(),
        submission,
        bundle,
        candles,
    };
    let (results, failure) = match run_again(&jobs, &run, input, &slot).await? {
        Rerun::Produced(files) => (
            compare(&jobs, &job_id, &recorded, files, Arc::clone(&slot)).await?,
            None,
        ),
        Rerun::Failed(error) => {
            let results = recorded
                .iter()
                .map(|reference| ResultComparison {
                    name: reference.name.clone(),
                    recorded: Some(reference.hash),
                    rerun: None,
                    divergence: None,
                })
                .collect();
            (results, Some(error))
        }
    };
    // A file only the rerun produced has no recorded hash, so it differs.
    let same = failure.is_none()
        && results
            .iter()
            .all(|compared| compared.rerun == compared.recorded);
    let verdict = if same {
        RerunVerdict::Verified
    } else {
        RerunVerdict::Diverged
    };
    info!(job = %job_id, identity = %ledger.identity, ?verdict, "a job was rerun");
    let job_id = jobs::JobId::new(job_id.as_str())
        .map_err(|_| Refusal::internal(format!("the server cannot read job {job_id}")))?;
    Ok(fresh(Json(RerunReport {
        job_id,
        identity: ledger.identity,
        verdict,
        runtime: ran_on,
        results,
        failure,
    })))
}

/// The rerun registered as running here, then a job slot once one is free,
/// taken as the dispatcher takes one; refused once the server stops. It is
/// registered before it waits, so that a stop, or a client that hangs up,
/// ends a rerun that waits as it ends one that runs.
async fn take_slot(jobs: &Jobs) -> Result<(Registered, Arc<OwnedSemaphorePermit>), Refusal> {
    let run = Registered::new(jobs, run_name()?);
    let mut stopping = jobs.inner.stopping.subscribe();
    let slot = tokio::select! {
        biased;
        _ = stopping.wait_for(|stopping| *stopping) => None,
        slot = Arc::clone(&jobs.inner.slots).acquire_owned() => slot.ok(),
    };
    Ok((run, Arc::new(slot.ok_or_else(stopped)?)))
}

/// The refusal of a rerun the server stopped.
fn stopped() -> Refusal {
    Refusal::unavailable(
        "the server stopped before the rerun could finish; ask for it again once the server is \
         back",
    )
}

/// The bundle and the bars the job ran on, read as its provenance names
/// them, each part checked against its name, each store call holding
/// `slot`; `None` once `cancel` is sent. Anything the store no longer holds
/// as recorded refuses the rerun, every difference named.
async fn read_inputs(
    jobs: &Jobs,
    job_id: &JobId,
    inputs: &ProvenanceInputs,
    cancel: &Cancel,
    slot: &Arc<OwnedSemaphorePermit>,
) -> Result<Option<(Bundle, Vec<Candle>)>, Refusal> {
    let bundle = read_bundle(jobs, inputs.code_hash, slot).await?;
    let hashes: Vec<SegmentHash> = inputs
        .data
        .segments
        .iter()
        .map(|segment| SegmentHash::from_digest(*segment.hash.digest()))
        .collect();
    let Some(opened) = read_segments(jobs, &hashes, cancel, Some(slot)).await? else {
        return Ok(None);
    };
    let mut problems: Vec<String> = bundle.as_ref().err().cloned().into_iter().collect();
    let mut candles: Vec<Candle> = Vec::new();
    let range = inputs.data.start_open_ms..=inputs.data.end_open_ms;
    for (segment, opened) in inputs.data.segments.iter().zip(opened) {
        match opened {
            Ok(bars) => candles.extend(
                bars.into_iter()
                    .filter(|candle| range.contains(&candle.open_time_ms)),
            ),
            Err(SegmentFault::Missing) => problems.push(format!(
                "the server no longer holds segment {}, which held the bars from {} to {}",
                segment.hash,
                ms_to_rfc3339(segment.first_open_ms),
                ms_to_rfc3339(segment.last_open_ms)
            )),
            Err(SegmentFault::Damaged(why)) => problems.push(format!(
                "the server's copy of segment {} is altered: {why}",
                segment.hash
            )),
        }
    }
    match bundle {
        Ok(bundle) if problems.is_empty() => Ok(Some((bundle, candles))),
        _ => Err(Refusal::conflict(format!(
            "job {job_id} cannot be rerun: {}",
            problems.join("; ")
        ))),
    }
}

/// The bundle of a job's code hash, checked against its name by a store
/// call that holds `slot`; or why it cannot be had, which refuses the
/// rerun.
async fn read_bundle(
    jobs: &Jobs,
    code_hash: ContentHash,
    slot: &Arc<OwnedSemaphorePermit>,
) -> Result<Result<Bundle, String>, Refusal> {
    let (limits, held) = (jobs.inner.limits, Arc::clone(slot));
    patiently(jobs, move |store| {
        let _held = &held;
        let hash = BundleHash::from_digest(*code_hash.digest());
        let tar = store
            .load_bundle(&hash)
            .map_err(|failure| Refusal::storage("loading a bundle", failure))?;
        let Some(tar) = tar else {
            return Ok(Err(format!("the server no longer holds bundle {hash}")));
        };
        let digest: [u8; 32] = Sha256::digest(&tar).into();
        if digest != *hash.digest() {
            return Ok(Err(format!(
                "the server's copy of bundle {hash} is altered: it hashes to {}",
                ContentHash::sha256(digest)
            )));
        }
        Ok(
            Bundle::from_canonical_tar(&tar, &limits).map_err(|refused| {
                format!("the server's copy of bundle {hash} cannot be read: {refused}")
            }),
        )
    })
    .await
}

/// The submission a rerun runs, from the job's provenance alone.
fn submission_of(inputs: &ProvenanceInputs) -> JobSubmission {
    let data = &inputs.data;
    JobSubmission {
        bundle: inputs.code_hash,
        data: DataSelection {
            exchange: data.exchange.clone(),
            category: Some(data.category.clone()),
            symbol: data.symbol.clone(),
            interval: data.interval.clone(),
            start_open_ms: data.start_open_ms,
            end_open_ms: data.end_open_ms,
        },
        backtest: inputs.backtest.clone(),
    }
}

/// What the name a rerun runs under starts with, which no job's ID does.
pub(super) const RUN_PREFIX: &str = "rerun-";

/// The name a rerun runs under on its worker: its own, so that two reruns
/// of one job never share one.
fn run_name() -> Result<jobs::JobId, Refusal> {
    jobs::JobId::new(format!("{RUN_PREFIX}{}", JobId::generate()))
        .map_err(|failure| Refusal::internal(failure.to_string()))
}

/// `engine 0.2.0, Python 3.11, numpy 2.4.6`.
fn described(runtime: &ProvenanceRuntime) -> String {
    let mut parts = vec![
        format!("engine {}", runtime.engine),
        format!("Python {}", runtime.python),
    ];
    parts.extend(
        runtime
            .deps
            .iter()
            .map(|(name, version)| format!("{} {}", name.as_str(), version.as_str())),
    );
    parts.join(", ")
}

/// How a rerun's run ended.
enum Rerun {
    /// With the files it produced.
    Produced(Vec<ResultFile>),
    /// Without, for its bundle's own reasons.
    Failed(ErrorBody),
}

/// A rerun registered as running here under its name, with its signal and
/// its log, until it is dropped: then its signal is sent, which stops a run
/// its client no longer waits for, and it is forgotten.
struct Registered {
    jobs: Jobs,
    name: jobs::JobId,
    cancel: Cancel,
    log: JobLog,
}

impl Registered {
    fn new(jobs: &Jobs, name: jobs::JobId) -> Self {
        let (cancel, log) = jobs.register_key(name.to_string());
        Self {
            jobs: jobs.clone(),
            name,
            cancel,
            log,
        }
    }
}

impl Drop for Registered {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.jobs.unregister_key(self.name.as_str());
    }
}

/// Runs `input` on the worker under `run`'s signal, the worker holding
/// `slot` until it exits. A stopping server answers that it stopped, and a
/// failure of the server's own is refused as one; only the bundle's own
/// failure is the rerun's outcome.
async fn run_again(
    jobs: &Jobs,
    run: &Registered,
    input: JobInput,
    slot: &Arc<OwnedSemaphorePermit>,
) -> Result<Rerun, Refusal> {
    let (worker, held) = (Arc::clone(&jobs.inner.worker), Arc::clone(slot));
    let (cancel, log) = (run.cancel.clone(), run.log.clone());
    let ran = tokio::task::spawn_blocking(move || {
        let _slot = held;
        worker.run(input, &cancel, &log)
    })
    .await
    .map_err(|_| Refusal::internal("the worker failed while it ran the job again"))?;
    match ran {
        WorkerOutcome::Succeeded(files) => Ok(Rerun::Produced(files)),
        WorkerOutcome::Failed(error) if error.code == ErrorCode::InvalidRequest => {
            Ok(Rerun::Failed(error))
        }
        WorkerOutcome::Failed(error) if error.code == ErrorCode::Unavailable => Err(
            Refusal::unavailable(format!("the rerun could not run: {}", error.message)),
        ),
        WorkerOutcome::Failed(error) => {
            error!(code = ?error.code, message = %error.message, "a rerun failed");
            Err(Refusal::internal(format!(
                "the rerun could not finish: {}",
                error.message
            )))
        }
        WorkerOutcome::Cancelled if run.cancel.is_cancelled() => Err(stopped()),
        WorkerOutcome::Cancelled => {
            error!(rerun = %run.name, "a rerun's worker stopped it without being asked to");
            Err(Refusal::internal(
                "the rerun could not finish: the worker stopped it without being asked to",
            ))
        }
    }
}
