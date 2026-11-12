//! The job store on SQLite: the bundles a server was sent, its jobs and
//! their results, each change one statement or transaction that names the
//! phase it moves a job from, so a job is claimed once and never leaves a
//! final phase.

use rusqlite::{ErrorCode, OptionalExtension, Row, TransactionBehavior, params};

use super::SqliteStore;
use super::errors::{commit_failed, failed};
use crate::model::{
    BundleHash, JobId, JobOutcome, JobPhase, JobRecord, NewJob, ResultHash, TimestampMs,
};
use crate::ports::{JobStore, StorageError};
use crate::storage::jobs::{
    COLUMNS, JobRow, already_recorded, no_bundle, no_job, outcome_columns, outcome_results,
};

// Indentation inside this literal is load-bearing: SQLite records the exact
// statement text in `sqlite_master`, so reformatting it would change the
// schema of freshly created databases.
pub(super) const SCHEMA_DDL: &str = r#"
                CREATE TABLE IF NOT EXISTS bundles (
                  code_hash TEXT PRIMARY KEY,
                  size_bytes INTEGER NOT NULL,
                  data BLOB NOT NULL,
                  stored_at_ms INTEGER NOT NULL
                );

                CREATE TABLE IF NOT EXISTS jobs (
                  seq INTEGER PRIMARY KEY AUTOINCREMENT,
                  job_id TEXT NOT NULL UNIQUE,
                  state TEXT NOT NULL,
                  bundle_hash TEXT NOT NULL,
                  request_json TEXT NOT NULL,
                  submitted_at_ms INTEGER NOT NULL,
                  started_at_ms INTEGER,
                  finished_at_ms INTEGER,
                  results_json TEXT,
                  failure_json TEXT
                );

                CREATE INDEX IF NOT EXISTS idx_jobs_state
                  ON jobs(state, seq);
"#;

// Indentation inside this literal is load-bearing, as above.
pub(super) const RESULTS_DDL: &str = r#"
                CREATE TABLE IF NOT EXISTS job_results (
                  hash TEXT PRIMARY KEY,
                  size_bytes INTEGER NOT NULL,
                  data BLOB NOT NULL,
                  stored_at_ms INTEGER NOT NULL
                );
"#;

// Indentation inside this literal is load-bearing, as above.
pub(super) const LOGS_DDL: &str = r#"
                CREATE TABLE IF NOT EXISTS job_logs (
                  job_id TEXT PRIMARY KEY,
                  log_json TEXT NOT NULL,
                  stored_at_ms INTEGER NOT NULL
                );
"#;

fn job_row(row: &Row<'_>) -> rusqlite::Result<JobRow> {
    Ok(JobRow {
        job_id: row.get(0)?,
        state: row.get(1)?,
        bundle_hash: row.get(2)?,
        request_json: row.get(3)?,
        submitted_at_ms: row.get(4)?,
        started_at_ms: row.get(5)?,
        finished_at_ms: row.get(6)?,
        results_json: row.get(7)?,
        failure_json: row.get(8)?,
    })
}

/// A query's rows as records.
fn records(rows: Vec<JobRow>) -> Result<Vec<JobRecord>, StorageError> {
    rows.into_iter().map(JobRow::record).collect()
}

impl JobStore for SqliteStore {
    fn store_bundle(
        &self,
        hash: &BundleHash,
        tar: &[u8],
        stored_at_ms: TimestampMs,
    ) -> Result<bool, StorageError> {
        let connection = self.open()?;
        let stored = connection
            .execute(
                r#"
                INSERT INTO bundles(code_hash, size_bytes, data, stored_at_ms)
                VALUES (?1, ?2, ?3, ?4)
                ON CONFLICT(code_hash) DO NOTHING
                "#,
                params![hash.to_string(), tar.len() as i64, tar, stored_at_ms],
            )
            .map_err(commit_failed)?;
        Ok(stored > 0)
    }

    fn load_bundle(&self, hash: &BundleHash) -> Result<Option<Vec<u8>>, StorageError> {
        let connection = self.open()?;
        connection
            .query_row(
                "SELECT data FROM bundles WHERE code_hash = ?1",
                [hash.to_string()],
                |row| row.get(0),
            )
            .optional()
            .map_err(failed)
    }

    fn submit_job(&self, job: &NewJob) -> Result<(), StorageError> {
        let connection = self.open()?;
        let inserted = connection.execute(
            r#"
            INSERT INTO jobs(job_id, state, bundle_hash, request_json, submitted_at_ms)
            SELECT ?1, ?2, ?3, ?4, ?5
            WHERE EXISTS (SELECT 1 FROM bundles WHERE code_hash = ?3)
            "#,
            params![
                job.job_id.as_str(),
                JobPhase::Queued.as_str(),
                job.bundle.to_string(),
                job.request_json,
                job.submitted_at_ms
            ],
        );
        match inserted {
            Ok(0) => Err(no_bundle(&job.bundle)),
            Ok(_) => Ok(()),
            Err(rusqlite::Error::SqliteFailure(failure, _))
                if failure.code == ErrorCode::ConstraintViolation =>
            {
                Err(already_recorded(&job.job_id))
            }
            Err(error) => Err(commit_failed(error)),
        }
    }

    fn job(&self, job_id: &JobId) -> Result<Option<JobRecord>, StorageError> {
        let connection = self.open()?;
        let row = connection
            .query_row(
                &format!("SELECT {COLUMNS} FROM jobs WHERE job_id = ?1"),
                [job_id.as_str()],
                job_row,
            )
            .optional()
            .map_err(failed)?;
        row.map(JobRow::record).transpose()
    }

    fn jobs(&self, limit: usize) -> Result<Vec<JobRecord>, StorageError> {
        let connection = self.open()?;
        let mut statement = connection
            .prepare(&format!(
                "SELECT {COLUMNS} FROM jobs ORDER BY seq DESC LIMIT ?1"
            ))
            .map_err(failed)?;
        let rows = statement
            .query_map([i64::try_from(limit).unwrap_or(i64::MAX)], job_row)
            .map_err(failed)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(failed)?;
        records(rows)
    }

    fn claim_job(&self, started_at_ms: TimestampMs) -> Result<Option<JobRecord>, StorageError> {
        let connection = self.open()?;
        // One statement: SQLite's write lock makes it the only claim at its
        // moment, so two stores never claim one job.
        let row = connection
            .query_row(
                &format!(
                    r#"
                    UPDATE jobs SET state = ?1, started_at_ms = ?2
                    WHERE seq = (SELECT seq FROM jobs WHERE state = ?3 ORDER BY seq LIMIT 1)
                    RETURNING {COLUMNS}
                    "#
                ),
                params![
                    JobPhase::Running.as_str(),
                    started_at_ms,
                    JobPhase::Queued.as_str()
                ],
                job_row,
            )
            .optional()
            .map_err(commit_failed)?;
        row.map(JobRow::record).transpose()
    }

    fn finish_job(
        &self,
        job_id: &JobId,
        outcome: &JobOutcome,
        finished_at_ms: TimestampMs,
    ) -> Result<bool, StorageError> {
        let (phase, results, failure) = outcome_columns(outcome);
        let mut connection = self.open()?;
        // The end and the results it names, in one transaction: a job never
        // shows results the store does not hold.
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failed)?;
        let finished = transaction
            .execute(
                r#"
                UPDATE jobs SET state = ?2, finished_at_ms = ?3, results_json = ?4, failure_json = ?5
                WHERE job_id = ?1 AND state = ?6
                "#,
                params![
                    job_id.as_str(),
                    phase.as_str(),
                    finished_at_ms,
                    results,
                    failure,
                    JobPhase::Running.as_str()
                ],
            )
            .map_err(failed)?;
        if finished == 0 {
            return Ok(false);
        }
        for result in outcome_results(outcome) {
            transaction
                .execute(
                    r#"
                    INSERT INTO job_results(hash, size_bytes, data, stored_at_ms)
                    VALUES (?1, ?2, ?3, ?4)
                    ON CONFLICT(hash) DO NOTHING
                    "#,
                    params![
                        result.hash.to_string(),
                        i64::try_from(result.size_bytes).unwrap_or(i64::MAX),
                        result.data,
                        finished_at_ms
                    ],
                )
                .map_err(failed)?;
        }
        transaction.commit().map_err(commit_failed)?;
        Ok(true)
    }

    fn load_result(&self, hash: &ResultHash) -> Result<Option<Vec<u8>>, StorageError> {
        let connection = self.open()?;
        connection
            .query_row(
                "SELECT data FROM job_results WHERE hash = ?1",
                [hash.to_string()],
                |row| row.get(0),
            )
            .optional()
            .map_err(failed)
    }

    fn store_job_log(
        &self,
        job_id: &JobId,
        log_json: &str,
        stored_at_ms: TimestampMs,
    ) -> Result<bool, StorageError> {
        let connection = self.open()?;
        let stored = connection
            .execute(
                r#"
                INSERT INTO job_logs(job_id, log_json, stored_at_ms)
                SELECT ?1, ?2, ?3
                WHERE EXISTS (SELECT 1 FROM jobs WHERE job_id = ?1)
                ON CONFLICT(job_id) DO NOTHING
                "#,
                params![job_id.as_str(), log_json, stored_at_ms],
            )
            .map_err(commit_failed)?;
        if stored > 0 {
            return Ok(true);
        }
        // Nothing written: the job has its log already, or there is no job.
        let known: bool = connection
            .query_row(
                "SELECT EXISTS (SELECT 1 FROM jobs WHERE job_id = ?1)",
                [job_id.as_str()],
                |row| row.get(0),
            )
            .map_err(failed)?;
        if known {
            Ok(false)
        } else {
            Err(no_job(job_id))
        }
    }

    fn job_log(&self, job_id: &JobId) -> Result<Option<String>, StorageError> {
        let connection = self.open()?;
        connection
            .query_row(
                "SELECT log_json FROM job_logs WHERE job_id = ?1",
                [job_id.as_str()],
                |row| row.get(0),
            )
            .optional()
            .map_err(failed)
    }

    fn cancel_job(
        &self,
        job_id: &JobId,
        finished_at_ms: TimestampMs,
    ) -> Result<Option<JobRecord>, StorageError> {
        let mut connection = self.open()?;
        // The change and the record it answers, in one transaction.
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failed)?;
        transaction
            .execute(
                r#"
                UPDATE jobs SET state = ?2, finished_at_ms = ?3
                WHERE job_id = ?1 AND state IN (?4, ?5)
                "#,
                params![
                    job_id.as_str(),
                    JobPhase::Cancelled.as_str(),
                    finished_at_ms,
                    JobPhase::Queued.as_str(),
                    JobPhase::Running.as_str()
                ],
            )
            .map_err(failed)?;
        let row = transaction
            .query_row(
                &format!("SELECT {COLUMNS} FROM jobs WHERE job_id = ?1"),
                [job_id.as_str()],
                job_row,
            )
            .optional()
            .map_err(failed)?;
        transaction.commit().map_err(commit_failed)?;
        row.map(JobRow::record).transpose()
    }

    fn fail_running_jobs(
        &self,
        failure_json: &str,
        finished_at_ms: TimestampMs,
    ) -> Result<usize, StorageError> {
        let connection = self.open()?;
        connection
            .execute(
                r#"
                UPDATE jobs SET state = ?1, finished_at_ms = ?2, failure_json = ?3
                WHERE state = ?4
                "#,
                params![
                    JobPhase::Failed.as_str(),
                    finished_at_ms,
                    failure_json,
                    JobPhase::Running.as_str()
                ],
            )
            .map_err(commit_failed)
    }
}
