//! The job store on PostgreSQL, with SQLite's answers: each change names
//! the phase it moves a job from and carries the copy guard, and a claim
//! skips a job another claim holds locked, so stores claiming at once each
//! get a job of their own. A job's end and the results it names are one
//! transaction.

use tokio_postgres::error::SqlState;
use tokio_postgres::types::{ToSql, Type};
use tokio_postgres::{GenericClient, IsolationLevel, Row};

use super::bridge::Effect;
use super::{PostgresStore, copying, limit};
use crate::model::{
    BundleHash, JobId, JobOutcome, JobPhase, JobRecord, NewJob, ResultHash, TimestampMs,
};
use crate::ports::{JobStore, StorageError};
use crate::storage::jobs::{
    COLUMNS, JobRow, already_recorded, no_bundle, outcome_columns, outcome_results,
};

const STORE_BUNDLE: &str = concat!(
    r#"
    INSERT INTO bundles(code_hash, size_bytes, data, stored_at_ms)
    SELECT $1, $2, $3, $4
    WHERE "#,
    copying::not_copying!(),
    r#"
    ON CONFLICT (code_hash) DO NOTHING
    "#
);

const LOAD_BUNDLE: &str = "SELECT data FROM bundles WHERE code_hash = $1";

const SUBMIT: &str = concat!(
    r#"
    INSERT INTO jobs(job_id, state, bundle_hash, request_json, submitted_at_ms)
    SELECT $1, $2, $3, $4, $5
    WHERE EXISTS (SELECT 1 FROM bundles WHERE code_hash = $3) AND "#,
    copying::not_copying!()
);

/// The oldest queued job a claim does not find locked by another.
const CLAIM: &str = concat!(
    r#"
    UPDATE jobs SET state = $1, started_at_ms = $2
    WHERE seq = (SELECT seq FROM jobs WHERE state = $3 ORDER BY seq LIMIT 1 FOR UPDATE SKIP LOCKED)
      AND "#,
    copying::not_copying!(),
    r#"
    RETURNING job_id, state, bundle_hash, request_json, submitted_at_ms, started_at_ms,
              finished_at_ms, results_json, failure_json
    "#
);

const FINISH: &str = concat!(
    r#"
    UPDATE jobs SET state = $2, finished_at_ms = $3, results_json = $4, failure_json = $5
    WHERE job_id = $1 AND state = $6 AND "#,
    copying::not_copying!()
);

/// Run after `FINISH` in its transaction, which holds the guard by then.
const STORE_RESULT: &str = r#"
    INSERT INTO job_results(hash, size_bytes, data, stored_at_ms)
    VALUES ($1, $2, $3, $4)
    ON CONFLICT (hash) DO NOTHING
    "#;

const LOAD_RESULT: &str = "SELECT data FROM job_results WHERE hash = $1";

const CANCEL: &str = concat!(
    r#"
    UPDATE jobs SET state = $2, finished_at_ms = $3
    WHERE job_id = $1 AND state IN ($4, $5) AND "#,
    copying::not_copying!(),
    r#"
    RETURNING job_id, state, bundle_hash, request_json, submitted_at_ms, started_at_ms,
              finished_at_ms, results_json, failure_json
    "#
);

const FAIL_RUNNING: &str = concat!(
    r#"
    UPDATE jobs SET state = $1, finished_at_ms = $2, failure_json = $3
    WHERE state = $4 AND "#,
    copying::not_copying!()
);

fn job_row(row: &Row) -> Result<JobRow, tokio_postgres::Error> {
    Ok(JobRow {
        job_id: row.try_get(0)?,
        state: row.try_get(1)?,
        bundle_hash: row.try_get(2)?,
        request_json: row.try_get(3)?,
        submitted_at_ms: row.try_get(4)?,
        started_at_ms: row.try_get(5)?,
        finished_at_ms: row.try_get(6)?,
        results_json: row.try_get(7)?,
        failure_json: row.try_get(8)?,
    })
}

/// A job's row by its ID.
async fn read_job<C: GenericClient>(
    client: &C,
    job_id: &str,
) -> Result<Option<JobRow>, tokio_postgres::Error> {
    let sql = format!("SELECT {COLUMNS} FROM jobs WHERE job_id = $1");
    client
        .query_typed_opt(&sql, &[(&job_id, Type::TEXT)])
        .await?
        .map(|row| job_row(&row))
        .transpose()
}

impl JobStore for PostgresStore {
    fn store_bundle(
        &self,
        hash: &BundleHash,
        tar: &[u8],
        stored_at_ms: TimestampMs,
    ) -> Result<bool, StorageError> {
        let (hash, tar) = (hash.to_string(), tar.to_vec());
        let size = i64::try_from(tar.len()).unwrap_or(i64::MAX);
        self.bridge.execute(Effect::Write, move |client| {
            Box::pin(async move {
                let stored = client
                    .execute_typed(
                        STORE_BUNDLE,
                        &[
                            (&hash, Type::TEXT),
                            (&size, Type::INT8),
                            (&tar, Type::BYTEA),
                            (&stored_at_ms, Type::INT8),
                        ],
                    )
                    .await?;
                if stored > 0 {
                    return Ok(Ok(true));
                }
                // Nothing written: held already, or held off by a copy.
                Ok(match copying::refusal(&**client).await? {
                    Some(refused) => Err(refused),
                    None => Ok(false),
                })
            })
        })?
    }

    fn load_bundle(&self, hash: &BundleHash) -> Result<Option<Vec<u8>>, StorageError> {
        self.keyed(LOAD_BUNDLE, hash.to_string())
    }

    fn submit_job(&self, job: &NewJob) -> Result<(), StorageError> {
        let job = job.clone();
        self.bridge.execute(Effect::Write, move |client| {
            Box::pin(async move {
                let bundle = job.bundle.to_string();
                let inserted = client
                    .execute_typed(
                        SUBMIT,
                        &[
                            (&job.job_id.as_str(), Type::TEXT),
                            (&JobPhase::Queued.as_str(), Type::TEXT),
                            (&bundle, Type::TEXT),
                            (&job.request_json, Type::TEXT),
                            (&job.submitted_at_ms, Type::INT8),
                        ],
                    )
                    .await;
                match inserted {
                    Ok(rows) => copying::written(&**client, rows, || no_bundle(&job.bundle)).await,
                    Err(error) if error.code() == Some(&SqlState::UNIQUE_VIOLATION) => {
                        Ok(Err(already_recorded(&job.job_id)))
                    }
                    Err(error) => Err(error),
                }
            })
        })?
    }

    fn job(&self, job_id: &JobId) -> Result<Option<JobRecord>, StorageError> {
        let job_id = job_id.as_str().to_string();
        let row = self.bridge.execute(Effect::Read, move |client| {
            Box::pin(async move { read_job(&**client, &job_id).await })
        })?;
        row.map(JobRow::record).transpose()
    }

    fn jobs(&self, count: usize) -> Result<Vec<JobRecord>, StorageError> {
        let count = limit(count);
        let rows = self.bridge.execute(Effect::Read, move |client| {
            Box::pin(async move {
                let sql = format!("SELECT {COLUMNS} FROM jobs ORDER BY seq DESC LIMIT $1");
                client
                    .query_typed(&sql, &[(&count, Type::INT8)])
                    .await?
                    .iter()
                    .map(job_row)
                    .collect::<Result<Vec<_>, _>>()
            })
        })?;
        rows.into_iter().map(JobRow::record).collect()
    }

    fn claim_job(&self, started_at_ms: TimestampMs) -> Result<Option<JobRecord>, StorageError> {
        let claimed = self.bridge.execute(Effect::Write, move |client| {
            Box::pin(async move {
                let params: [(&(dyn ToSql + Sync), Type); 3] = [
                    (&JobPhase::Running.as_str(), Type::TEXT),
                    (&started_at_ms, Type::INT8),
                    (&JobPhase::Queued.as_str(), Type::TEXT),
                ];
                if let Some(row) = client.query_typed_opt(CLAIM, &params).await? {
                    return Ok(Ok(Some(job_row(&row)?)));
                }
                // Nothing claimed: no job is queued, or a copy holds the
                // store off.
                Ok(match copying::refusal(&**client).await? {
                    Some(refused) => Err(refused),
                    None => Ok(None),
                })
            })
        })??;
        claimed.map(JobRow::record).transpose()
    }

    fn finish_job(
        &self,
        job_id: &JobId,
        outcome: &JobOutcome,
        finished_at_ms: TimestampMs,
    ) -> Result<bool, StorageError> {
        let (phase, results, failure) = outcome_columns(outcome);
        let (job_id, results, failure) = (
            job_id.as_str().to_string(),
            results.map(str::to_string),
            failure.map(str::to_string),
        );
        let stored: Vec<(String, i64, Vec<u8>)> = outcome_results(outcome)
            .iter()
            .map(|result| {
                (
                    result.hash.to_string(),
                    i64::try_from(result.size_bytes).unwrap_or(i64::MAX),
                    result.data.clone(),
                )
            })
            .collect();
        self.bridge.execute(Effect::Write, move |client| {
            Box::pin(async move {
                // The end and the results it names, in one transaction: a
                // job never shows results the store does not hold.
                let transaction = client
                    .build_transaction()
                    .isolation_level(IsolationLevel::ReadCommitted)
                    .start()
                    .await?;
                let finished = transaction
                    .execute_typed(
                        FINISH,
                        &[
                            (&job_id, Type::TEXT),
                            (&phase.as_str(), Type::TEXT),
                            (&finished_at_ms, Type::INT8),
                            (&results, Type::TEXT),
                            (&failure, Type::TEXT),
                            (&JobPhase::Running.as_str(), Type::TEXT),
                        ],
                    )
                    .await?;
                if finished == 0 {
                    return Ok(match copying::refusal(&*transaction).await? {
                        Some(refused) => Err(refused),
                        None => Ok(false),
                    });
                }
                for (hash, size, data) in &stored {
                    transaction
                        .execute_typed(
                            STORE_RESULT,
                            &[
                                (hash, Type::TEXT),
                                (size, Type::INT8),
                                (data, Type::BYTEA),
                                (&finished_at_ms, Type::INT8),
                            ],
                        )
                        .await?;
                }
                transaction.commit().await?;
                Ok(Ok(true))
            })
        })?
    }

    fn load_result(&self, hash: &ResultHash) -> Result<Option<Vec<u8>>, StorageError> {
        self.keyed(LOAD_RESULT, hash.to_string())
    }

    fn store_job_log(
        &self,
        job_id: &JobId,
        log_json: &str,
        stored_at_ms: TimestampMs,
    ) -> Result<bool, StorageError> {
        self.store_log(job_id, log_json, stored_at_ms)
    }

    fn job_log(&self, job_id: &JobId) -> Result<Option<String>, StorageError> {
        self.load_log(job_id)
    }

    fn cancel_job(
        &self,
        job_id: &JobId,
        finished_at_ms: TimestampMs,
    ) -> Result<Option<JobRecord>, StorageError> {
        let job_id = job_id.as_str().to_string();
        let row = self.bridge.execute(Effect::Write, move |client| {
            Box::pin(async move {
                let transaction = client
                    .build_transaction()
                    .isolation_level(IsolationLevel::ReadCommitted)
                    .start()
                    .await?;
                let cancelled = transaction
                    .query_typed_opt(
                        CANCEL,
                        &[
                            (&job_id, Type::TEXT),
                            (&JobPhase::Cancelled.as_str(), Type::TEXT),
                            (&finished_at_ms, Type::INT8),
                            (&JobPhase::Queued.as_str(), Type::TEXT),
                            (&JobPhase::Running.as_str(), Type::TEXT),
                        ],
                    )
                    .await?;
                let row = match cancelled {
                    Some(row) => Some(job_row(&row)?),
                    None => {
                        if let Some(refused) = copying::refusal(&*transaction).await? {
                            return Ok(Err(refused));
                        }
                        // Unknown, or ended already: as it stands.
                        read_job(&*transaction, &job_id).await?
                    }
                };
                transaction.commit().await?;
                Ok(Ok(row))
            })
        })??;
        row.map(JobRow::record).transpose()
    }

    fn fail_running_jobs(
        &self,
        failure_json: &str,
        finished_at_ms: TimestampMs,
    ) -> Result<usize, StorageError> {
        let failure_json = failure_json.to_string();
        self.bridge.execute(Effect::Write, move |client| {
            Box::pin(async move {
                let failed = client
                    .execute_typed(
                        FAIL_RUNNING,
                        &[
                            (&JobPhase::Failed.as_str(), Type::TEXT),
                            (&finished_at_ms, Type::INT8),
                            (&failure_json, Type::TEXT),
                            (&JobPhase::Running.as_str(), Type::TEXT),
                        ],
                    )
                    .await?;
                if failed > 0 {
                    return Ok(Ok(usize::try_from(failed).unwrap_or(usize::MAX)));
                }
                Ok(match copying::refusal(&**client).await? {
                    Some(refused) => Err(refused),
                    None => Ok(0),
                })
            })
        })?
    }
}
