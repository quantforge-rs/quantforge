//! A job's inputs as the server reads them as the job starts, and its
//! provenance, recorded before the job runs. The bundle is checked against
//! its code hash and admitted again; the bars are read as `bars` says, and
//! the job runs on exactly those, whose segments its provenance names. The
//! provenance is recorded with the segments packed for the job, and the
//! worker is handed the inputs and nothing else.

use std::sync::Arc;

use sha2::{Digest, Sha256};
use tracing::error;

use super::Jobs;
use super::bars::{Bars, Fault, read_bars};
use super::provenance::StoredProvenance;
use super::queue::{Ended, failed, patiently};
use super::worker::{Cancel, JobInput};
use crate::model::{
    InputsIdentity, JobPhase, JobRecord, MarketId, NewProvenance, PackedSegment, SegmentHash,
    now_utc_ms,
};
use crate::protocol::bundle::{Bundle, Runtime};
use crate::protocol::jobs::{
    self, JobSubmission, PROVENANCE_FORMAT, ProvenanceData, ProvenanceInputs, ProvenanceRuntime,
    ProvenanceStrategy,
};
use crate::protocol::{ContentHash, ErrorCode, PROTO_VERSION};
use crate::server::refusal::Refusal;
use crate::server::selection::Selection;

/// Why a job that could not be read as it started failed, when its store
/// was out of time through every attempt.
const BUSY: &str =
    "the server's store could not answer in time as the job started; submit it again";

/// The job's input: its submission, its bundle checked against its name
/// and admitted again against the runtime, and its bars, with its
/// provenance recorded. `None` when the job was cancelled meanwhile. The
/// job is looked up once it is registered, so that a cancel either finds
/// it registered or is seen here.
pub(super) async fn read_input(
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
        // The job runs the code its hash names, or none.
        let digest: [u8; 32] = Sha256::digest(&tar).into();
        if digest != *hash.digest() {
            return Err(Refusal::internal(format!(
                "the server's copy of bundle {hash} is altered: it hashes to {}",
                ContentHash::sha256(digest)
            )));
        }
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
    let bars = match read_bars(jobs, &market, &selection, cancel).await {
        Ok(Some(bars)) => bars,
        Ok(None) => return Ok(None),
        Err(Fault::Refused(refusal)) => return Err(refused("candles", refusal)),
        Err(Fault::Ended(ended)) => return Err(ended),
    };
    let stored = StoredProvenance {
        format: PROVENANCE_FORMAT,
        inputs: inputs_of(&submission, &bundle, runtime, &market, &selection, &bars),
        strategy: ProvenanceStrategy {
            entry: bundle.manifest().entry().to_string(),
            params: bundle.manifest().params().as_map().clone(),
        },
    };
    let unrecorded = |why: String| {
        error!(job = %record.job_id, %why, "a job's provenance cannot be recorded");
        Ended::Finished(failed(
            ErrorCode::Internal,
            "the server cannot record the job's provenance",
        ))
    };
    let provenance =
        recorded(&stored, bars.packed).map_err(|failure| unrecorded(failure.to_string()))?;
    if cancel.is_cancelled() {
        return Ok(None);
    }
    let (id, provenance) = (record.job_id.clone(), Arc::new(provenance));
    patiently(jobs, move |store| {
        store
            .record_provenance(&id, &provenance, now_utc_ms())
            .map_err(|failure| Refusal::storage("recording a job's provenance", failure))
    })
    .await
    .map_err(|refusal| match refusal.code {
        ErrorCode::Unavailable => Ended::Finished(failed(ErrorCode::Unavailable, BUSY)),
        _ => unrecorded(refusal.message),
    })?;
    Ok(Some(JobInput {
        job_id,
        submission,
        bundle,
        candles: bars.candles,
    }))
}

/// What the job runs on, as its provenance records it.
fn inputs_of(
    submission: &JobSubmission,
    bundle: &Bundle,
    runtime: &Runtime,
    market: &MarketId,
    selection: &Selection,
    bars: &Bars,
) -> ProvenanceInputs {
    ProvenanceInputs {
        code_hash: submission.bundle,
        data: ProvenanceData {
            exchange: selection.venue.as_str().to_string(),
            category: market.category().as_str().to_string(),
            symbol: market.symbol.as_str().to_string(),
            interval: market.interval.as_str().to_string(),
            start_open_ms: selection.start_open_ms,
            end_open_ms: selection.end_open_ms,
            bar_count: u64::try_from(bars.candles.len()).unwrap_or(u64::MAX),
            segments: bars.segments.clone(),
        },
        backtest: submission.backtest.clone(),
        // Admitted, the bundle runs each package at the version it pins.
        runtime: ProvenanceRuntime {
            engine: runtime.engine,
            python: runtime.python,
            deps: bundle.manifest().deps().clone(),
        },
        proto_version: PROTO_VERSION,
    }
}

/// The provenance as the store records it: its identity, its JSON, every
/// segment it names, and the segments packed for the job.
fn recorded(
    stored: &StoredProvenance,
    packed: Vec<PackedSegment>,
) -> Result<NewProvenance, serde_json::Error> {
    let identity = InputsIdentity::from_digest(*stored.inputs.identity()?.digest());
    Ok(NewProvenance {
        identity,
        provenance_json: serde_json::to_string(stored)?,
        segments: stored
            .inputs
            .data
            .segments
            .iter()
            .map(|segment| SegmentHash::from_digest(*segment.hash.digest()))
            .collect(),
        packed,
    })
}
