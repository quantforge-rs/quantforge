//! A job's provenance on PostgreSQL, with SQLite's answers: recorded once
//! under its job, with the segments the server packed for it, in one `READ
//! COMMITTED` transaction that holds its job's row, carries the copy guard
//! after its first write, and finds every segment the record names; and
//! read back as it was recorded.

use tokio_postgres::IsolationLevel;
use tokio_postgres::types::Type;

use super::bridge::Effect;
use super::{PostgresStore, copying};
use crate::model::{JobId, NewProvenance, ProvenanceRecord, TimestampMs};
use crate::ports::StorageError;
use crate::storage::jobs::{PROVENANCE_COLUMNS, ProvenanceRow, no_job_to_record, no_segment};
use crate::storage::segments::FORMAT;
use crate::storage::settings::StorageBackend;
use crate::storage::sqlite::SCHEMA_VERSION;

/// Holds the job's row, so that two records of one job take turns and the
/// second finds the first.
const LOCK_JOB: &str = "SELECT job_id FROM jobs WHERE job_id = $1 FOR UPDATE";

const RECORDED: &str = "SELECT EXISTS (SELECT 1 FROM job_provenance WHERE job_id = $1)";

const RECORD: &str = r#"
    INSERT INTO job_provenance(job_id, identity, provenance_json, backend, schema_version,
                               recorded_at_ms)
    VALUES ($1, $2, $3, $4, $5, $6)
    "#;

/// A segment is never changed: its name is its content.
const STORE_PACKED: &str = r#"
    INSERT INTO segments(hash, format, payload_len, data)
    VALUES ($1, $2, $3, $4)
    ON CONFLICT (hash) DO NOTHING
    "#;

/// Which of the names the store holds.
const HELD: &str = "SELECT hash FROM segments WHERE hash = ANY($1)";

impl PostgresStore {
    /// `JobStore::record_provenance`.
    pub(super) fn record_job_provenance(
        &self,
        job_id: &JobId,
        provenance: &NewProvenance,
        recorded_at_ms: TimestampMs,
    ) -> Result<bool, StorageError> {
        let (job, provenance) = (job_id.clone(), provenance.clone());
        let schema_version = i64::from(SCHEMA_VERSION);
        self.bridge.execute(Effect::Write, move |client| {
            Box::pin(async move {
                let id = job.as_str();
                let transaction = client
                    .build_transaction()
                    .isolation_level(IsolationLevel::ReadCommitted)
                    .start()
                    .await?;
                let held_job = transaction
                    .query_typed_opt(LOCK_JOB, &[(&id, Type::TEXT)])
                    .await?;
                let recorded: bool = transaction
                    .query_typed_one(RECORDED, &[(&id, Type::TEXT)])
                    .await?
                    .try_get(0)?;
                if held_job.is_none() || recorded {
                    // Nothing to write: held off by a copy, recorded
                    // already, or there is no such job.
                    if let Some(refused) = copying::refusal(&*transaction).await? {
                        return Ok(Err(refused));
                    }
                    return Ok(if recorded {
                        Ok(false)
                    } else {
                        Err(no_job_to_record(&job))
                    });
                }
                let identity = provenance.identity.to_string();
                transaction
                    .execute_typed(
                        RECORD,
                        &[
                            (&id, Type::TEXT),
                            (&identity, Type::TEXT),
                            (&provenance.provenance_json, Type::TEXT),
                            (&StorageBackend::Postgres.as_str(), Type::TEXT),
                            (&schema_version, Type::INT8),
                            (&recorded_at_ms, Type::INT8),
                        ],
                    )
                    .await?;
                // The record holds its table now: a copy that began first
                // has recorded itself, and refuses the write.
                if let Some(refused) = copying::refusal(&*transaction).await? {
                    return Ok(Err(refused));
                }
                for packed in &provenance.packed {
                    let (hash, length) = (
                        packed.hash.to_string(),
                        i32::try_from(packed.payload_len).unwrap_or(i32::MAX),
                    );
                    transaction
                        .execute_typed(
                            STORE_PACKED,
                            &[
                                (&hash, Type::TEXT),
                                (&(FORMAT as i32), Type::INT4),
                                (&length, Type::INT4),
                                (&packed.data, Type::BYTEA),
                            ],
                        )
                        .await?;
                }
                let named: Vec<String> = provenance
                    .segments
                    .iter()
                    .map(|hash| hash.to_string())
                    .collect();
                let held: Vec<String> = transaction
                    .query_typed(HELD, &[(&named, Type::TEXT_ARRAY)])
                    .await?
                    .iter()
                    .map(|row| row.try_get(0))
                    .collect::<Result<_, _>>()?;
                if let Some(missing) = provenance
                    .segments
                    .iter()
                    .find(|hash| !held.contains(&hash.to_string()))
                {
                    return Ok(Err(no_segment(missing)));
                }
                transaction.commit().await?;
                Ok(Ok(true))
            })
        })?
    }

    /// `JobStore::provenance`.
    pub(super) fn job_provenance(
        &self,
        job_id: &JobId,
    ) -> Result<Option<ProvenanceRecord>, StorageError> {
        let id = job_id.as_str().to_string();
        let row = self.bridge.execute(Effect::Read, move |client| {
            Box::pin(async move {
                let sql =
                    format!("SELECT {PROVENANCE_COLUMNS} FROM job_provenance WHERE job_id = $1");
                client
                    .query_typed_opt(&sql, &[(&id, Type::TEXT)])
                    .await?
                    .map(|row| {
                        Ok::<_, tokio_postgres::Error>(ProvenanceRow {
                            job_id: row.try_get(0)?,
                            identity: row.try_get(1)?,
                            provenance_json: row.try_get(2)?,
                            backend: row.try_get(3)?,
                            schema_version: row.try_get(4)?,
                            recorded_at_ms: row.try_get(5)?,
                        })
                    })
                    .transpose()
            })
        })?;
        row.map(ProvenanceRow::record).transpose()
    }
}
