//! The store's own record (`docs/storage-schema.md`): one row with its ID,
//! the environment it is bound to and its activation, the migration log,
//! and the operator's actions on legacy runs — the `StoreControl`
//! implementation.

use super::{SqliteStore, journal, migrate, to_from_sql_error};
use crate::model::futures::ActivationEpoch;
use crate::model::{
    Environment, ExchangeId, ExecutionMode, IdentitySource, Interval, LegacyRun, MarketId,
    RunIdentity, RunStatus, SchemaMigration, StoreActivation, StoreId, StoreIdentity, Symbol,
};
use crate::{BotRunState, StorageError, StoreControl, now_utc_ms};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use std::str::FromStr;

// Indentation inside this literal is load-bearing: SQLite records the exact
// statement text in `sqlite_master`, so reformatting it would change the
// schema of freshly created databases.
pub(super) const SCHEMA_DDL: &str = r#"
                CREATE TABLE IF NOT EXISTS store_control (
                  id INTEGER PRIMARY KEY CHECK (id = 1),
                  store_id TEXT NOT NULL,
                  environment TEXT,
                  activation TEXT NOT NULL,
                  activation_epoch INTEGER NOT NULL,
                  changed_at_ms INTEGER NOT NULL
                );
"#;

/// The migration default: a fresh ID, no environment, never activated.
pub(super) fn create_row(transaction: &Transaction<'_>) -> Result<(), StorageError> {
    transaction
        .execute(
            r#"
            INSERT INTO store_control(id, store_id, environment, activation, activation_epoch, changed_at_ms)
            VALUES (1, ?1, NULL, ?2, 0, ?3)
            "#,
            params![
                StoreId::generate().as_str(),
                StoreActivation::Inactive.as_str(),
                now_utc_ms()
            ],
        )
        .map_err(StorageError::other)?;
    Ok(())
}

fn read_identity(connection: &Connection) -> Result<StoreIdentity, StorageError> {
    let schema_version = migrate::current_version(connection)?;
    connection
        .query_row(
            "SELECT store_id, environment, activation, activation_epoch FROM store_control WHERE id = 1",
            [],
            |row| {
                let store_id = StoreId::new(row.get::<_, String>(0)?).map_err(to_from_sql_error)?;
                let environment = row
                    .get::<_, Option<String>>(1)?
                    .map(|text| journal::parse_environment(&text))
                    .transpose()
                    .map_err(to_from_sql_error)?;
                let activation = StoreActivation::from_str(&row.get::<_, String>(2)?)
                    .map_err(to_from_sql_error)?;
                let epoch: i64 = row.get(3)?;
                let activation_epoch = ActivationEpoch(
                    u32::try_from(epoch).map_err(|_| {
                        to_from_sql_error(StorageError::InvalidArgument(format!(
                            "activation epoch {epoch} is out of range"
                        )))
                    })?,
                );
                Ok(StoreIdentity {
                    store_id,
                    schema_version,
                    environment,
                    activation,
                    activation_epoch,
                })
            },
        )
        .map_err(StorageError::other)
}

impl StoreControl for SqliteStore {
    fn store_identity(&self) -> Result<StoreIdentity, StorageError> {
        read_identity(&self.open()?)
    }

    fn migrations(&self) -> Result<Vec<SchemaMigration>, StorageError> {
        let connection = self.open()?;
        let mut stmt = connection
            .prepare(
                r#"
                SELECT version, from_version, applied_at_ms, quantforge_version
                FROM schema_migrations
                ORDER BY version ASC
                "#,
            )
            .map_err(StorageError::other)?;
        let rows = stmt
            .query_map([], |row| {
                Ok(SchemaMigration {
                    to_version: row.get(0)?,
                    from_version: row.get(1)?,
                    applied_at_ms: row.get(2)?,
                    quantforge_version: row.get(3)?,
                })
            })
            .map_err(StorageError::other)?;
        rows.collect::<Result<_, _>>().map_err(StorageError::other)
    }

    fn bind_environment(&self, environment: Environment) -> Result<(), StorageError> {
        let mut connection = self.open()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::other)?;
        let identity = read_identity(&transaction)?;
        match identity.environment {
            Some(bound) if bound == environment => return Ok(()),
            Some(bound) => {
                return Err(StorageError::InvalidArgument(format!(
                    "store {} is bound to {bound}; a store keeps its environment for life",
                    identity.store_id
                )));
            }
            None => {}
        }
        transaction
            .execute(
                "UPDATE store_control SET environment = ?1, changed_at_ms = ?2 WHERE id = 1",
                params![environment.as_str(), now_utc_ms()],
            )
            .map_err(StorageError::other)?;
        transaction.commit().map_err(StorageError::other)
    }

    fn legacy_runs(&self) -> Result<Vec<LegacyRun>, StorageError> {
        let connection = self.open()?;
        let mut stmt = connection
            .prepare(
                r#"
                SELECT run_id, exchange, symbol, interval, state_json, started_at_ms
                FROM bot_runs
                WHERE identity_source = ?1
                ORDER BY started_at_ms ASC, run_id ASC
                "#,
            )
            .map_err(StorageError::other)?;
        let rows = stmt
            .query_map([IdentitySource::Legacy.as_str()], |row| {
                let run_id: String = row.get(0)?;
                let exchange: String = row.get(1)?;
                let exchange = ExchangeId::from_storage_name(&exchange).ok_or_else(|| {
                    to_from_sql_error(StorageError::InvalidArgument(format!(
                        "run {run_id} is stored under unknown exchange {exchange:?}"
                    )))
                })?;
                let symbol = Symbol::new(row.get::<_, String>(2)?).map_err(to_from_sql_error)?;
                let interval =
                    Interval::from_str(&row.get::<_, String>(3)?).map_err(to_from_sql_error)?;
                let state: serde_json::Value =
                    serde_json::from_str(&row.get::<_, String>(4)?).map_err(to_from_sql_error)?;
                let status: RunStatus =
                    serde_json::from_value(state["status"].clone()).map_err(to_from_sql_error)?;
                let execution_mode = match state.get("execution_mode") {
                    None => None,
                    Some(value) => Some(
                        serde_json::from_value::<ExecutionMode>(value.clone())
                            .map_err(to_from_sql_error)?,
                    ),
                };
                Ok(LegacyRun {
                    run_id,
                    market: MarketId::new(exchange, symbol, interval),
                    status,
                    execution_mode,
                    started_at_ms: row.get(5)?,
                })
            })
            .map_err(StorageError::other)?;
        rows.collect::<Result<_, _>>().map_err(StorageError::other)
    }

    fn adopt_run(
        &self,
        run_id: &str,
        identity: &RunIdentity,
        execution_mode: Option<ExecutionMode>,
    ) -> Result<(), StorageError> {
        if identity.source != IdentitySource::Adopted {
            return Err(StorageError::InvalidArgument(format!(
                "an adopted run's identity source is adopted, not {}",
                identity.source
            )));
        }
        let mut connection = self.open()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::other)?;
        let row: Option<(String, String)> = transaction
            .query_row(
                "SELECT identity_source, state_json FROM bot_runs WHERE run_id = ?1",
                [run_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(StorageError::other)?;
        let Some((source, state_json)) = row else {
            return Err(StorageError::InvalidArgument(format!(
                "no run found for run_id={run_id}"
            )));
        };
        if source != IdentitySource::Legacy.as_str() {
            return Err(StorageError::InvalidArgument(format!(
                "run {run_id} already has a {source} identity; only a legacy run is adopted"
            )));
        }
        let mut state: serde_json::Value =
            serde_json::from_str(&state_json).map_err(StorageError::other)?;
        match (state.get("execution_mode").is_some(), execution_mode) {
            (true, Some(_)) => {
                return Err(StorageError::InvalidArgument(format!(
                    "run {run_id} already records its execution mode"
                )));
            }
            (false, None) => {
                return Err(StorageError::InvalidArgument(format!(
                    "run {run_id} recorded no execution mode; the operator states it"
                )));
            }
            (false, Some(mode)) => {
                let token = serde_json::to_value(mode).map_err(StorageError::other)?;
                state["execution_mode"] = token;
            }
            (true, None) => {}
        }
        // The adopted record must load as a run, or the adoption is refused
        // before anything is written.
        serde_json::from_value::<BotRunState>(state.clone()).map_err(|err| {
            StorageError::InvalidArgument(format!(
                "run {run_id} cannot be adopted: its state does not load ({err})"
            ))
        })?;
        let (environment, account_kind, account_id) = journal::identity_columns(identity);
        transaction
            .execute(
                r#"
                UPDATE bot_runs
                SET state_json = ?2, environment = ?3, account_kind = ?4, account_id = ?5,
                    identity_source = ?6, identity_at_ms = ?7
                WHERE run_id = ?1
                "#,
                params![
                    run_id,
                    serde_json::to_string(&state).map_err(StorageError::other)?,
                    environment,
                    account_kind,
                    account_id,
                    identity.source.as_str(),
                    now_utc_ms()
                ],
            )
            .map_err(StorageError::other)?;
        transaction.commit().map_err(StorageError::other)
    }
}
