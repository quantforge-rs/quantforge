//! A job's provenance on SQLite: recorded once under its job, with the
//! segments the server packed for it, in one transaction that finds every
//! segment the record names; and read back as it was recorded.

use rusqlite::{OptionalExtension, Row, TransactionBehavior, params};

use super::errors::{commit_failed, failed};
use super::{SCHEMA_VERSION, SqliteStore};
use crate::model::{JobId, NewProvenance, ProvenanceRecord, TimestampMs};
use crate::ports::StorageError;
use crate::storage::jobs::{PROVENANCE_COLUMNS, ProvenanceRow, no_job_to_record, no_segment};
use crate::storage::segments::FORMAT;
use crate::storage::settings::StorageBackend;

// Indentation inside this literal is load-bearing: SQLite records the exact
// statement text in `sqlite_master`, so reformatting it would change the
// schema of freshly created databases.
pub(super) const SCHEMA_DDL: &str = r#"
                CREATE TABLE IF NOT EXISTS job_provenance (
                  job_id TEXT PRIMARY KEY,
                  identity TEXT NOT NULL,
                  provenance_json TEXT NOT NULL,
                  backend TEXT NOT NULL,
                  schema_version INTEGER NOT NULL,
                  recorded_at_ms INTEGER NOT NULL
                );
"#;

fn provenance_row(row: &Row<'_>) -> rusqlite::Result<ProvenanceRow> {
    Ok(ProvenanceRow {
        job_id: row.get(0)?,
        identity: row.get(1)?,
        provenance_json: row.get(2)?,
        backend: row.get(3)?,
        schema_version: row.get(4)?,
        recorded_at_ms: row.get(5)?,
    })
}

impl SqliteStore {
    /// `JobStore::record_provenance`: one transaction, which writes
    /// nothing unless it writes the record.
    pub(super) fn record_job_provenance(
        &self,
        job_id: &JobId,
        provenance: &NewProvenance,
        recorded_at_ms: TimestampMs,
    ) -> Result<bool, StorageError> {
        let mut connection = self.open()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failed)?;
        let exists = |sql: &str, key: &str| -> Result<bool, StorageError> {
            transaction
                .query_row(sql, [key], |row| row.get(0))
                .map_err(failed)
        };
        let id = job_id.as_str();
        if !exists("SELECT EXISTS (SELECT 1 FROM jobs WHERE job_id = ?1)", id)? {
            return Err(no_job_to_record(job_id));
        }
        // Recorded once: a second record rolls back as it drops.
        if exists(
            "SELECT EXISTS (SELECT 1 FROM job_provenance WHERE job_id = ?1)",
            id,
        )? {
            return Ok(false);
        }
        for packed in &provenance.packed {
            transaction
                .execute(
                    r#"
                    INSERT INTO segments(hash, format, payload_len, data)
                    VALUES (?1, ?2, ?3, ?4)
                    ON CONFLICT(hash) DO NOTHING
                    "#,
                    params![
                        packed.hash.to_string(),
                        FORMAT,
                        i64::try_from(packed.payload_len).unwrap_or(i64::MAX),
                        packed.data
                    ],
                )
                .map_err(failed)?;
        }
        for hash in &provenance.segments {
            let name = hash.to_string();
            if !exists(
                "SELECT EXISTS (SELECT 1 FROM segments WHERE hash = ?1)",
                &name,
            )? {
                return Err(no_segment(hash));
            }
        }
        transaction
            .execute(
                r#"
                INSERT INTO job_provenance(job_id, identity, provenance_json, backend,
                                           schema_version, recorded_at_ms)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                "#,
                params![
                    id,
                    provenance.identity.to_string(),
                    provenance.provenance_json,
                    StorageBackend::Sqlite.as_str(),
                    SCHEMA_VERSION,
                    recorded_at_ms
                ],
            )
            .map_err(failed)?;
        transaction.commit().map_err(commit_failed)?;
        Ok(true)
    }

    /// `JobStore::provenance`.
    pub(super) fn job_provenance(
        &self,
        job_id: &JobId,
    ) -> Result<Option<ProvenanceRecord>, StorageError> {
        let connection = self.open()?;
        let row = connection
            .query_row(
                &format!("SELECT {PROVENANCE_COLUMNS} FROM job_provenance WHERE job_id = ?1"),
                [job_id.as_str()],
                provenance_row,
            )
            .optional()
            .map_err(failed)?;
        row.map(ProvenanceRow::record).transpose()
    }
}
