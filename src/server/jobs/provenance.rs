//! `GET /v1/backtests/{job_id}/provenance`: what a job ran on, as the
//! server recorded it before the job ran, read back and checked against the
//! identity it was recorded under; and the ledger a rerun runs a job again
//! from. A record that no longer matches its identity is refused, logged,
//! rather than shown as what the job ran on.

use axum::Json;
use axum::extract::rejection::PathRejection;
use axum::extract::{Path, State};
use axum::response::Response;
use serde::{Deserialize, Serialize};
use tracing::error;

use super::Jobs;
use super::results::listed;
use super::routes::{job_named, no_job};
use crate::model::{JobId, JobPhase, JobRecord};
use crate::protocol::ContentHash;
use crate::protocol::jobs::{
    self, PROVENANCE_FORMAT, Provenance, ProvenanceInputs, ProvenanceRecorded, ProvenanceStrategy,
};
use crate::server::history::fresh;
use crate::server::refusal::Refusal;
use crate::server::store::ServedStore;

/// What a store keeps of a job's provenance, as the server's JSON: its
/// format, its inputs and its strategy. The identity, the store and the
/// time are the store's own columns, and the results the job's record.
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct StoredProvenance {
    pub(super) format: u32,
    pub(super) inputs: ProvenanceInputs,
    pub(super) strategy: ProvenanceStrategy,
}

/// A job's record and its provenance, checked against the identity it was
/// recorded under.
pub(super) struct Ledger {
    pub(super) record: JobRecord,
    pub(super) stored: StoredProvenance,
    pub(super) identity: ContentHash,
    pub(super) recorded: ProvenanceRecorded,
}

/// The job's ledger, or the refusal of a job the store lacks, or that has
/// recorded no provenance.
pub(super) fn ledger(store: &dyn ServedStore, job_id: &JobId) -> Result<Ledger, Refusal> {
    let record = store
        .job(job_id)
        .map_err(|failure| Refusal::storage("reading a job", failure))?
        .ok_or_else(|| no_job(job_id.as_str()))?;
    let row = store
        .provenance(job_id)
        .map_err(|failure| Refusal::storage("reading a job's provenance", failure))?;
    let Some(row) = row else {
        let why = match record.phase {
            JobPhase::Queued => "is still queued; its provenance is recorded as it starts",
            JobPhase::Running => {
                "is still reading its inputs; its provenance is recorded before it runs"
            }
            JobPhase::Succeeded | JobPhase::Failed | JobPhase::Cancelled => {
                "has no provenance: it ended before the server read its inputs, or a build that \
                 recorded none ran it"
            }
        };
        return Err(Refusal::conflict(format!("job {job_id} {why}")));
    };
    let damaged = |why: String| {
        error!(job = %job_id, %why, "a stored provenance does not match its identity");
        Refusal::internal(format!(
            "the server's copy of job {job_id}'s provenance is damaged"
        ))
    };
    let stored: StoredProvenance = serde_json::from_str(&row.provenance_json)
        .map_err(|failure| damaged(failure.to_string()))?;
    if stored.format != PROVENANCE_FORMAT {
        return Err(damaged(format!(
            "it is of provenance format {}, which this build does not read",
            stored.format
        )));
    }
    let identity = stored
        .inputs
        .identity()
        .map_err(|failure| damaged(failure.to_string()))?;
    if identity.digest() != row.identity.digest() {
        return Err(damaged(format!(
            "its inputs hash to {identity}, not to {}, the identity it was recorded under",
            row.identity
        )));
    }
    Ok(Ledger {
        record,
        stored,
        identity,
        recorded: ProvenanceRecorded {
            at_ms: row.recorded_at_ms,
            backend: row.backend,
            schema_version: row.schema_version,
        },
    })
}

/// `GET /v1/backtests/{job_id}/provenance`.
pub(in crate::server) async fn provenance(
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
    let results = match ledger.record.phase {
        JobPhase::Succeeded => Some(listed(&ledger.record)?),
        _ => None,
    };
    let job_id = jobs::JobId::new(job_id.as_str())
        .map_err(|_| Refusal::internal(format!("the server cannot read job {job_id}")))?;
    Ok(fresh(Json(Provenance {
        job_id,
        format: PROVENANCE_FORMAT,
        identity: ledger.identity,
        inputs: ledger.stored.inputs,
        strategy: ledger.stored.strategy,
        recorded: ledger.recorded,
        results,
    })))
}
