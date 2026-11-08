//! The job store: the bundles a server was sent and the jobs that run them,
//! kept in the store the host selected, beside the candles they run on.
//! `docs/storage-schema.md` is the schema, `docs/storage-conformance.md`
//! the behavior every backend passes.

use super::storage::StorageError;
use crate::model::{BundleHash, JobId, JobOutcome, JobRecord, NewJob, TimestampMs};

/// Bundles and jobs. A bundle is stored once under its name and never
/// changed; a job moves from queued to running to one final phase, or from
/// queued or running straight to cancelled, and each move is one guarded
/// change, so a job never leaves a final phase and is never claimed twice.
pub trait JobStore: Send + Sync {
    /// Stores a bundle's canonical tar under its name. Answers whether it
    /// was new: a bundle stored again changes nothing.
    fn store_bundle(
        &self,
        hash: &BundleHash,
        tar: &[u8],
        stored_at_ms: TimestampMs,
    ) -> Result<bool, StorageError>;

    /// A bundle's canonical tar, or `None` when no bundle has that name.
    fn load_bundle(&self, hash: &BundleHash) -> Result<Option<Vec<u8>>, StorageError>;

    /// Records a job, queued. Refuses a job ID already recorded, and a
    /// bundle the store does not hold.
    fn submit_job(&self, job: &NewJob) -> Result<(), StorageError>;

    /// A job by its ID, or `None` when there is none.
    fn job(&self, job_id: &JobId) -> Result<Option<JobRecord>, StorageError>;

    /// Up to `limit` jobs, the last submitted first.
    fn jobs(&self, limit: usize) -> Result<Vec<JobRecord>, StorageError>;

    /// The job submitted first of those still queued, now running since
    /// `started_at_ms`, or `None` when none is queued. Stores claiming at
    /// once each get a job of their own.
    fn claim_job(&self, started_at_ms: TimestampMs) -> Result<Option<JobRecord>, StorageError>;

    /// Ends a running job with its outcome. Answers `false`, changing
    /// nothing, when the job is not running, such as one cancelled while it
    /// ran.
    fn finish_job(
        &self,
        job_id: &JobId,
        outcome: &JobOutcome,
        finished_at_ms: TimestampMs,
    ) -> Result<bool, StorageError>;

    /// Cancels a queued or running job, and answers its record: cancelled,
    /// or unchanged when it had already ended. `None` when there is no such
    /// job.
    fn cancel_job(
        &self,
        job_id: &JobId,
        finished_at_ms: TimestampMs,
    ) -> Result<Option<JobRecord>, StorageError>;

    /// Fails every running job with `failure_json`: what a server does as it
    /// starts, for the jobs that ran when it stopped. Answers how many.
    fn fail_running_jobs(
        &self,
        failure_json: &str,
        finished_at_ms: TimestampMs,
    ) -> Result<usize, StorageError>;
}
