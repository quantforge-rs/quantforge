//! The schema chain (`docs/storage-schema.md`): every schema a build has
//! written moves forward to the current one when the store is opened,
//! step by step. Each step runs in one `BEGIN IMMEDIATE` transaction with
//! its version stamp and its log row, so a step that fails leaves the
//! database at the last completed one, and two processes opening the same
//! file serialize on the lock and the second finds the work done. A fresh
//! file runs the whole chain in one transaction and logs one step from 0.
//! Nothing is ever deleted: a row an earlier build wrote stays, and where a
//! step rewrites one, the original is kept beside it.

use super::{META_DDL, SCHEMA_VERSION, SqliteStore, candles, control, journal, segments};
use crate::storage::segments as format;
use crate::{Interval, StorageError, now_utc_ms};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use std::str::FromStr;
use tracing::info;

// Indentation inside this literal is load-bearing: SQLite records the exact
// statement text in `sqlite_master`, so reformatting it would change the
// schema of freshly created databases.
const LOG_DDL: &str = r#"
                CREATE TABLE IF NOT EXISTS schema_migrations (
                  version INTEGER PRIMARY KEY,
                  from_version INTEGER NOT NULL,
                  applied_at_ms INTEGER NOT NULL,
                  quantforge_version TEXT NOT NULL
                );
"#;

/// The step that brings a store from version `to - 1` to `to`.
struct Step {
    to: u32,
    apply: fn(&Transaction<'_>) -> Result<(), StorageError>,
}

/// The chain, in order. Each step's DDL is the text the build that
/// introduced it wrote, which never changes again.
const STEPS: [Step; 5] = [
    Step {
        to: 1,
        apply: step_1_candles,
    },
    Step {
        to: 2,
        apply: step_2_journal,
    },
    Step {
        to: 3,
        apply: step_3_execution_mode,
    },
    Step {
        to: 4,
        apply: step_4_segments,
    },
    Step {
        to: 5,
        apply: step_5_identity,
    },
];

/// Brings the store to [`SCHEMA_VERSION`], creating it when the file holds
/// no schema yet. A store from a newer build is refused untouched.
pub(super) fn ensure_current(store: &SqliteStore) -> Result<(), StorageError> {
    let mut connection = store.open()?;
    loop {
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::other)?;
        let found = current_version(&transaction)?;
        if found == SCHEMA_VERSION {
            return Ok(());
        }
        if found > SCHEMA_VERSION {
            return Err(StorageError::SchemaUnsupported {
                found,
                supported: SCHEMA_VERSION,
            });
        }
        let to = if found == 0 {
            for step in &STEPS {
                (step.apply)(&transaction)?;
            }
            SCHEMA_VERSION
        } else {
            let step = STEPS
                .iter()
                .find(|step| step.to == found + 1)
                .ok_or_else(|| {
                    StorageError::InvalidArgument(format!(
                        "no migration step leaves schema version {found}"
                    ))
                })?;
            (step.apply)(&transaction)?;
            step.to
        };
        stamp(&transaction, found, to)?;
        transaction.commit().map_err(StorageError::other)?;
        if found == 0 {
            info!(schema_version = to, db_path = %store.path().display(), "sqlite store created");
        } else {
            info!(
                from = found,
                to,
                db_path = %store.path().display(),
                "sqlite schema migrated"
            );
        }
    }
}

/// The version the store records: 0 for a file without a schema.
pub(super) fn current_version(connection: &Connection) -> Result<u32, StorageError> {
    let has_meta: Option<String> = connection
        .query_row(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'meta'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(StorageError::other)?;
    if has_meta.is_none() {
        return Ok(0);
    }
    let recorded: Option<String> = connection
        .query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(StorageError::other)?;
    match recorded {
        None => Ok(0),
        Some(value) => value.parse().map_err(|_| {
            StorageError::InvalidArgument(format!(
                "the database records schema version {value:?}, which is not a number"
            ))
        }),
    }
}

/// Stamps the version and logs the step, inside the step's transaction.
fn stamp(transaction: &Transaction<'_>, from: u32, to: u32) -> Result<(), StorageError> {
    transaction
        .execute(
            r#"
            INSERT INTO meta(key, value)
            VALUES ('schema_version', ?1)
            ON CONFLICT(key) DO UPDATE SET value = excluded.value
            "#,
            [to.to_string()],
        )
        .map_err(StorageError::other)?;
    batch(transaction, LOG_DDL)?;
    transaction
        .execute(
            r#"
            INSERT INTO schema_migrations(version, from_version, applied_at_ms, quantforge_version)
            VALUES (?1, ?2, ?3, ?4)
            "#,
            params![to, from, now_utc_ms(), env!("CARGO_PKG_VERSION")],
        )
        .map_err(StorageError::other)?;
    Ok(())
}

fn batch(transaction: &Transaction<'_>, ddl: &str) -> Result<(), StorageError> {
    transaction.execute_batch(ddl).map_err(StorageError::other)
}

/// QuantForge 0.1.0: the candle table.
fn step_1_candles(transaction: &Transaction<'_>) -> Result<(), StorageError> {
    batch(transaction, META_DDL)?;
    batch(transaction, candles::SCHEMA_DDL)
}

/// QuantForge 0.2.0: the run journal.
fn step_2_journal(transaction: &Transaction<'_>) -> Result<(), StorageError> {
    batch(transaction, journal::SCHEMA_DDL)
}

/// A development build: `state_json` gained a required `execution_mode`.
/// A run without one stays as it is and loads only once an operator
/// states its mode (`StoreControl::adopt_run`); nothing is guessed.
fn step_3_execution_mode(_transaction: &Transaction<'_>) -> Result<(), StorageError> {
    Ok(())
}

/// A development build: the segment tables. Every window with candles is
/// marked dirty, so the next refresh packs what the earlier build stored.
fn step_4_segments(transaction: &Transaction<'_>) -> Result<(), StorageError> {
    batch(transaction, segments::SCHEMA_DDL)?;
    mark_every_window(transaction)
}

/// Schema 5 (QUA-37): text venue IDs, the identity every run carries, and
/// the store's own control record.
fn step_5_identity(transaction: &Transaction<'_>) -> Result<(), StorageError> {
    batch(transaction, journal::IDENTITY_DDL)?;
    journal::events::migrate_ids(transaction)?;
    batch(transaction, control::SCHEMA_DDL)?;
    control::create_row(transaction)
}

fn mark_every_window(transaction: &Transaction<'_>) -> Result<(), StorageError> {
    let markets: Vec<(String, String, String)> = {
        let mut stmt = transaction
            .prepare("SELECT DISTINCT exchange, symbol, interval FROM candles")
            .map_err(StorageError::other)?;
        let rows = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .map_err(StorageError::other)?;
        rows.collect::<Result<_, _>>()
            .map_err(StorageError::other)?
    };
    for (exchange, symbol, interval) in markets {
        let span = format::span_ms(Interval::from_str(&interval).map_err(StorageError::other)?);
        let starts: Vec<i64> = {
            let mut stmt = transaction
                .prepare(
                    r#"
                    SELECT DISTINCT (open_time_ms / ?1) * ?1 FROM candles
                    WHERE exchange = ?2 AND symbol = ?3 AND interval = ?4
                    "#,
                )
                .map_err(StorageError::other)?;
            let rows = stmt
                .query_map(params![span, exchange, symbol, interval], |row| row.get(0))
                .map_err(StorageError::other)?;
            rows.collect::<Result<_, _>>()
                .map_err(StorageError::other)?
        };
        for start in starts {
            transaction
                .execute(
                    r#"
                    INSERT OR REPLACE INTO segment_dirty(exchange, symbol, interval, window_start_ms)
                    VALUES (?1, ?2, ?3, ?4)
                    "#,
                    params![exchange, symbol, interval, start],
                )
                .map_err(StorageError::other)?;
        }
    }
    Ok(())
}
