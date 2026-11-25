//! `POST /v1/backtests/{job_id}/rerun`: a succeeded job run again from its
//! provenance alone, on the bars of the segments it names and the bundle of
//! its code hash, each checked against its name first; and its result
//! compared with the job's (`compare`). The rerun waits for a slot like any
//! job and runs on a worker like any job, registered so that a stopping
//! server stops it; a client that hangs up stops it too. It changes nothing
//! the server holds: the report is its answer, and the log records each
//! verdict.

use std::sync::Arc;

use axum::Json;
use axum::extract::rejection::PathRejection;
use axum::extract::{Path, State};
use axum::response::Response;
use sha2::{Digest, Sha256};
use tracing::{error, info};

use super::Jobs;
use super::bars::{SegmentFault, read_segments};
use super::compare::compare;
use super::provenance::ledger;
use super::queue::patiently;
use super::results::listed;
use super::routes::job_named;
use super::worker::{Cancel, JobInput, NO_RUNTIME, WorkerOutcome};
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

    // What the job ran on, each part checked against its name.
    let bundle = read_bundle(&jobs, inputs.code_hash).await?;
    let hashes: Vec<SegmentHash> = inputs
        .data
        .segments
        .iter()
        .map(|segment| SegmentHash::from_digest(*segment.hash.digest()))
        .collect();
    let opened = read_segments(&jobs, &hashes, &Cancel::new())
        .await?
        .unwrap_or_default();
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
    let bundle = match bundle {
        Ok(bundle) if problems.is_empty() => bundle,
        _ => {
            return Err(Refusal::conflict(format!(
                "job {job_id} cannot be rerun: {}",
                problems.join("; ")
            )));
        }
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
        job_id: run_name()?,
        submission: submission_of(&inputs),
        bundle,
        candles,
    };
    let (results, failure) = match run_again(&jobs, input).await? {
        Rerun::Produced(files) => (compare(&jobs, &job_id, &recorded, &files).await?, None),
        Rerun::Failed(error) => {
            let results = recorded
                .iter()
                .map(|reference| ResultComparison {
                    name: reference.name.clone(),
                    recorded: reference.hash,
                    rerun: None,
                    divergence: None,
                })
                .collect();
            (results, Some(error))
        }
    };
    let same = failure.is_none()
        && results
            .iter()
            .all(|compared| compared.rerun == Some(compared.recorded));
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

/// The bundle of a job's code hash, checked against its name; or why it
/// cannot be had, which refuses the rerun.
async fn read_bundle(
    jobs: &Jobs,
    code_hash: ContentHash,
) -> Result<Result<Bundle, String>, Refusal> {
    let limits = jobs.inner.limits;
    patiently(jobs, move |store| {
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

/// A rerun registered as running here, until it is dropped: then its
/// signal is sent, which stops a run its client no longer waits for, and
/// it is forgotten.
struct Registered {
    jobs: Jobs,
    key: String,
    cancel: Cancel,
}

impl Drop for Registered {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.jobs.unregister_key(&self.key);
    }
}

/// Runs `input` on the worker once a slot is free, as a job runs. A
/// stopping server answers that it stopped, and so does a failure of the
/// server's own; only the bundle's own failure is the rerun's outcome.
async fn run_again(jobs: &Jobs, input: JobInput) -> Result<Rerun, Refusal> {
    let mut stopping = jobs.inner.stopping.subscribe();
    let slot = tokio::select! {
        biased;
        _ = stopping.wait_for(|stopping| *stopping) => None,
        slot = Arc::clone(&jobs.inner.slots).acquire_owned() => slot.ok(),
    };
    let stopped = || {
        Refusal::unavailable(
            "the server stopped before the rerun could finish; ask for it again once the server \
             is back",
        )
    };
    let slot = slot.ok_or_else(stopped)?;
    let key = input.job_id.to_string();
    let (cancel, log) = jobs.register_key(key.clone());
    let _registered = Registered {
        jobs: jobs.clone(),
        key,
        cancel: cancel.clone(),
    };
    let worker = Arc::clone(&jobs.inner.worker);
    let ran = tokio::task::spawn_blocking(move || {
        let _slot = slot;
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
        WorkerOutcome::Cancelled => Err(stopped()),
    }
}
