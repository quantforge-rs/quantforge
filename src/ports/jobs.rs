//! The job store: the bundles a server was sent and the jobs that run them,
//! kept in the store the host selected, beside the candles they run on.
//! `docs/storage-schema.md` is the schema, `docs/storage-conformance.md`
//! the behavior every backend passes.

use super::storage::StorageError;
use crate::model::{BundleHash, JobId, JobOutcome, JobRecord, NewJob, ResultHash, TimestampMs};

/// Bundles, jobs and their results. A bundle is stored once under its name
/// and never changed; a job moves from queued to running to one final
/// phase, or from queued or running straight to cancelled, and each move is
/// one guarded change, so a job never leaves a final phase and is never
/// claimed twice. A succeeded job's results are stored in the change that
/// ends it, each once under its name, and never changed; what a server
/// kept of a job's log is stored once, under the job.
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

    /// Ends a running job with its outcome, and in the same change stores
    /// each result of a succeeded one under its name; a result the store
    /// holds already stays as it is. Answers `false`, changing and storing
    /// nothing, when the job is not running, such as one cancelled while it
    /// ran.
    fn finish_job(
        &self,
        job_id: &JobId,
        outcome: &JobOutcome,
        finished_at_ms: TimestampMs,
    ) -> Result<bool, StorageError>;

    /// What the store keeps of a result, as [`StoredResult::data`] gave it,
    /// or `None` when no result has that name.
    ///
    /// [`StoredResult::data`]: crate::model::StoredResult::data
    fn load_result(&self, hash: &ResultHash) -> Result<Option<Vec<u8>>, StorageError>;

    /// Stores what a server kept of a job's log, as the server's JSON,
    /// under the job. Answers whether it was new: a job's log is stored
    /// once, and one stored again changes nothing. Refuses a job the store
    /// does not hold.
    fn store_job_log(
        &self,
        job_id: &JobId,
        log_json: &str,
        stored_at_ms: TimestampMs,
    ) -> Result<bool, StorageError>;

    /// A job's log as it was stored, or `None` when none was: the job
    /// never ran, wrote nothing, or ran in a server that was killed.
    fn job_log(&self, job_id: &JobId) -> Result<Option<String>, StorageError>;

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
