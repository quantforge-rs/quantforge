//! What both backends' job stores share: a job's columns, read back into
//! its record, how an outcome is written, a provenance's columns, read
//! back into its record, and the refusals, worded once.

use std::str::FromStr;

use crate::model::{
    BundleHash, InputsIdentity, JobId, JobOutcome, JobPhase, JobRecord, ProvenanceRecord,
    SegmentHash, StoredResult,
};
use crate::ports::StorageError;

/// A job's columns, in the order every read selects them.
pub(crate) const COLUMNS: &str = "job_id, state, bundle_hash, request_json, submitted_at_ms, \
     started_at_ms, finished_at_ms, results_json, failure_json";

/// A job's row as the database returned it.
pub(crate) struct JobRow {
    pub(crate) job_id: String,
    pub(crate) state: String,
    pub(crate) bundle_hash: String,
    pub(crate) request_json: String,
    pub(crate) submitted_at_ms: i64,
    pub(crate) started_at_ms: Option<i64>,
    pub(crate) finished_at_ms: Option<i64>,
    pub(crate) results_json: Option<String>,
    pub(crate) failure_json: Option<String>,
}

impl JobRow {
    /// The record, or a refusal of a row no build writes.
    pub(crate) fn record(self) -> Result<JobRecord, StorageError> {
        let unreadable = |what: &str, error: String| {
            StorageError::InvalidArgument(format!(
                "job {} holds {what} this build cannot read: {error}",
                self.job_id
            ))
        };
        let job_id = JobId::new(self.job_id.as_str())
            .map_err(|error| unreadable("an id", error.to_string()))?;
        let phase = JobPhase::from_str(&self.state)
            .map_err(|error| unreadable("a state", error.to_string()))?;
        let bundle = BundleHash::from_str(&self.bundle_hash)
            .map_err(|error| unreadable("a bundle hash", error.to_string()))?;
        Ok(JobRecord {
            job_id,
            phase,
            bundle,
            request_json: self.request_json,
            submitted_at_ms: self.submitted_at_ms,
            started_at_ms: self.started_at_ms,
            finished_at_ms: self.finished_at_ms,
            results_json: self.results_json,
            failure_json: self.failure_json,
        })
    }
}

/// The phase an outcome ends a job in, and its two JSON columns.
pub(crate) fn outcome_columns(outcome: &JobOutcome) -> (JobPhase, Option<&str>, Option<&str>) {
    match outcome {
        JobOutcome::Succeeded { results_json, .. } => {
            (JobPhase::Succeeded, Some(results_json.as_str()), None)
        }
        JobOutcome::Failed { failure_json } => {
            (JobPhase::Failed, None, Some(failure_json.as_str()))
        }
    }
}

/// The files an outcome stores: a succeeded job's results.
pub(crate) fn outcome_results(outcome: &JobOutcome) -> &[StoredResult] {
    match outcome {
        JobOutcome::Succeeded { results, .. } => results,
        JobOutcome::Failed { .. } => &[],
    }
}

/// A job is recorded once.
pub(crate) fn already_recorded(job_id: &JobId) -> StorageError {
    StorageError::InvalidArgument(format!("job {job_id} is already recorded"))
}

/// A log is stored for a job the store holds.
pub(crate) fn no_job(job_id: &JobId) -> StorageError {
    StorageError::InvalidArgument(format!(
        "the store holds no job {job_id}; a log is stored for a job it holds"
    ))
}

/// A job runs a bundle the store holds.
pub(crate) fn no_bundle(hash: &BundleHash) -> StorageError {
    StorageError::InvalidArgument(format!(
        "the store holds no bundle {hash}; store it before a job that runs it"
    ))
}

/// A provenance's columns, in the order every read selects them.
pub(crate) const PROVENANCE_COLUMNS: &str =
    "job_id, identity, provenance_json, backend, schema_version, recorded_at_ms";

/// A provenance's row as the database returned it.
pub(crate) struct ProvenanceRow {
    pub(crate) job_id: String,
    pub(crate) identity: String,
    pub(crate) provenance_json: String,
    pub(crate) backend: String,
    pub(crate) schema_version: i64,
    pub(crate) recorded_at_ms: i64,
}

impl ProvenanceRow {
    /// The record, or a refusal of a row no build writes.
    pub(crate) fn record(self) -> Result<ProvenanceRecord, StorageError> {
        let unreadable = |what: &str, error: String| {
            StorageError::InvalidArgument(format!(
                "the provenance of job {} holds {what} this build cannot read: {error}",
                self.job_id
            ))
        };
        let job_id = JobId::new(self.job_id.as_str())
            .map_err(|error| unreadable("an id", error.to_string()))?;
        let identity = InputsIdentity::from_str(&self.identity)
            .map_err(|error| unreadable("an identity", error.to_string()))?;
        let schema_version = u32::try_from(self.schema_version).map_err(|_| {
            unreadable(
                "a schema version",
                format!("{} is not a version", self.schema_version),
            )
        })?;
        Ok(ProvenanceRecord {
            job_id,
            identity,
            provenance_json: self.provenance_json,
            backend: self.backend,
            schema_version,
            recorded_at_ms: self.recorded_at_ms,
        })
    }
}

/// A provenance is recorded for a job the store holds.
pub(crate) fn no_job_to_record(job_id: &JobId) -> StorageError {
    StorageError::InvalidArgument(format!(
        "the store holds no job {job_id}; a provenance is recorded for a job it holds"
    ))
}

/// A provenance names only segments the store holds, or is handed with it.
pub(crate) fn no_segment(hash: &SegmentHash) -> StorageError {
    StorageError::InvalidArgument(format!(
        "the store holds no segment {hash}; a provenance names only segments the store holds"
    ))
}
