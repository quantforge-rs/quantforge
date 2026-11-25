//! The job store of [`AnyStore`]: every call handed to its backend.

use super::AnyStore;
use crate::model::{
    BundleHash, JobId, JobOutcome, JobRecord, NewJob, NewProvenance, ProvenanceRecord, ResultHash,
    TimestampMs,
};
use crate::ports::{JobStore, StorageError};

impl JobStore for AnyStore {
    fn store_bundle(
        &self,
        hash: &BundleHash,
        tar: &[u8],
        stored_at_ms: TimestampMs,
    ) -> Result<bool, StorageError> {
        delegate!(self, store => store.store_bundle(hash, tar, stored_at_ms))
    }

    fn load_bundle(&self, hash: &BundleHash) -> Result<Option<Vec<u8>>, StorageError> {
        delegate!(self, store => store.load_bundle(hash))
    }

    fn submit_job(&self, job: &NewJob) -> Result<(), StorageError> {
        delegate!(self, store => store.submit_job(job))
    }

    fn job(&self, job_id: &JobId) -> Result<Option<JobRecord>, StorageError> {
        delegate!(self, store => store.job(job_id))
    }

    fn jobs(&self, limit: usize) -> Result<Vec<JobRecord>, StorageError> {
        delegate!(self, store => store.jobs(limit))
    }

    fn claim_job(&self, started_at_ms: TimestampMs) -> Result<Option<JobRecord>, StorageError> {
        delegate!(self, store => store.claim_job(started_at_ms))
    }

    fn finish_job(
        &self,
        job_id: &JobId,
        outcome: &JobOutcome,
        finished_at_ms: TimestampMs,
    ) -> Result<bool, StorageError> {
        delegate!(self, store => store.finish_job(job_id, outcome, finished_at_ms))
    }

    fn load_result(&self, hash: &ResultHash) -> Result<Option<Vec<u8>>, StorageError> {
        delegate!(self, store => store.load_result(hash))
    }

    fn store_job_log(
        &self,
        job_id: &JobId,
        log_json: &str,
        stored_at_ms: TimestampMs,
    ) -> Result<bool, StorageError> {
        delegate!(self, store => store.store_job_log(job_id, log_json, stored_at_ms))
    }

    fn job_log(&self, job_id: &JobId) -> Result<Option<String>, StorageError> {
        delegate!(self, store => store.job_log(job_id))
    }

    fn record_provenance(
        &self,
        job_id: &JobId,
        provenance: &NewProvenance,
        recorded_at_ms: TimestampMs,
    ) -> Result<bool, StorageError> {
        delegate!(self, store => store.record_provenance(job_id, provenance, recorded_at_ms))
    }

    fn provenance(&self, job_id: &JobId) -> Result<Option<ProvenanceRecord>, StorageError> {
        delegate!(self, store => store.provenance(job_id))
    }

    fn cancel_job(
        &self,
        job_id: &JobId,
        finished_at_ms: TimestampMs,
    ) -> Result<Option<JobRecord>, StorageError> {
        delegate!(self, store => store.cancel_job(job_id, finished_at_ms))
    }

    fn fail_running_jobs(
        &self,
        failure_json: &str,
        finished_at_ms: TimestampMs,
    ) -> Result<usize, StorageError> {
        delegate!(self, store => store.fail_running_jobs(failure_json, finished_at_ms))
    }
}
