//! The store's own record on PostgreSQL (`docs/storage-schema.md`):
//! `StoreControl` with SQLite's rules and messages, shared through
//! `storage::runs`. The binding and an adoption each run in one `READ
//! COMMITTED` transaction that first locks the row it changes, so two
//! operators acting at once decide one after the other.

use std::str::FromStr;

use tokio_postgres::types::Type;
use tokio_postgres::{IsolationLevel, Row};

use super::bridge::Effect;
use super::{PostgresStore, durability};
use crate::model::futures::ActivationEpoch;
use crate::model::{
    DurabilityReport, Environment, ExecutionMode, IdentitySource, LegacyRun, RunIdentity,
    SchemaMigration, StoreActivation, StoreId, StoreIdentity,
};
use crate::storage::runs::{
    adopted_state, already_bound, identity_columns, legacy_run, parse_environment, refuse_unadopted,
};
use crate::{StorageError, StoreControl, now_utc_ms};

const RECORD: &str = r#"
    SELECT c.store_id, c.environment, c.activation, c.activation_epoch, m.value
    FROM store_control c, meta m
    WHERE c.id = 1 AND m.key = 'schema_version'
"#;

const MIGRATIONS: &str = r#"
    SELECT version, from_version, applied_at_ms, quantforge_version
    FROM schema_migrations
    ORDER BY version ASC
"#;

const LOCK_BINDING: &str =
    "SELECT store_id, environment FROM store_control WHERE id = 1 FOR UPDATE";

const BIND: &str = "UPDATE store_control SET environment = $1, changed_at_ms = $2 WHERE id = 1";

const LEGACY_RUNS: &str = r#"
    SELECT run_id, exchange, symbol, interval, state_json, started_at_ms
    FROM bot_runs
    WHERE identity_source = $1
    ORDER BY started_at_ms ASC, run_id COLLATE "C" ASC
"#;

const LOCK_RUN: &str =
    "SELECT identity_source, state_json FROM bot_runs WHERE run_id = $1 FOR UPDATE";

const ADOPT: &str = r#"
    UPDATE bot_runs
    SET state_json = $2, environment = $3, account_kind = $4, account_id = $5,
        identity_source = $6, identity_at_ms = $7
    WHERE run_id = $1
"#;

/// The store's record from its row.
fn record(row: &Row) -> Result<StoreIdentity, StorageError> {
    let invalid = |what: String| StorageError::InvalidArgument(what);
    let get = |index: usize| -> Result<Option<String>, StorageError> {
        row.try_get(index).map_err(StorageError::other)
    };
    let store_id = StoreId::new(get(0)?.unwrap_or_default()).map_err(StorageError::other)?;
    let environment = get(1)?
        .map(|text| parse_environment(&text))
        .transpose()
        .map_err(StorageError::other)?;
    let activation =
        StoreActivation::from_str(&get(2)?.unwrap_or_default()).map_err(StorageError::other)?;
    let epoch: i32 = row.try_get(3).map_err(StorageError::other)?;
    let activation_epoch = ActivationEpoch(
        u32::try_from(epoch)
            .map_err(|_| invalid(format!("activation epoch {epoch} is out of range")))?,
    );
    let version = get(4)?.unwrap_or_default();
    let schema_version = version.parse().map_err(|_| {
        invalid(format!(
            "the database records schema version {version:?}, which is not a number"
        ))
    })?;
    Ok(StoreIdentity {
        store_id,
        schema_version,
        environment,
        activation,
        activation_epoch,
    })
}

impl StoreControl for PostgresStore {
    fn store_identity(&self) -> Result<StoreIdentity, StorageError> {
        self.bridge.execute(Effect::Read, |client| {
            Box::pin(async move { Ok(record(&client.query_typed_one(RECORD, &[]).await?)) })
        })?
    }

    fn durability(&self) -> Result<DurabilityReport, StorageError> {
        self.bridge.execute(Effect::Read, |client| {
            Box::pin(async move { durability::effective(client).await })
        })
    }

    fn migrations(&self) -> Result<Vec<SchemaMigration>, StorageError> {
        let rows: Vec<(i32, i32, i64, String)> = self.bridge.execute(Effect::Read, |client| {
            Box::pin(async move {
                client
                    .query_typed(MIGRATIONS, &[])
                    .await?
                    .iter()
                    .map(|row| {
                        Ok((
                            row.try_get(0)?,
                            row.try_get(1)?,
                            row.try_get(2)?,
                            row.try_get(3)?,
                        ))
                    })
                    .collect()
            })
        })?;
        rows.into_iter()
            .map(|(to, from, applied_at_ms, quantforge_version)| {
                let version = |value: i32| {
                    u32::try_from(value).map_err(|_| {
                        StorageError::InvalidArgument(format!(
                            "schema version {value} is out of range"
                        ))
                    })
                };
                Ok(SchemaMigration {
                    to_version: version(to)?,
                    from_version: version(from)?,
                    applied_at_ms,
                    quantforge_version,
                })
            })
            .collect()
    }

    fn bind_environment(&self, environment: Environment) -> Result<(), StorageError> {
        self.bridge.execute(Effect::Write, move |client| {
            Box::pin(async move {
                let transaction = client
                    .build_transaction()
                    .isolation_level(IsolationLevel::ReadCommitted)
                    .start()
                    .await?;
                let row = transaction.query_typed_one(LOCK_BINDING, &[]).await?;
                let store_id: String = row.try_get(0)?;
                let bound: Option<String> = row.try_get(1)?;
                let bound = match bound.map(|text| parse_environment(&text)).transpose() {
                    Ok(bound) => bound,
                    Err(error) => return Ok(Err(StorageError::other(error))),
                };
                match bound {
                    Some(bound) if bound == environment => return Ok(Ok(())),
                    Some(bound) => {
                        let refused = match StoreId::new(store_id) {
                            Ok(store_id) => already_bound(&store_id, bound),
                            Err(error) => StorageError::other(error),
                        };
                        return Ok(Err(refused));
                    }
                    None => {}
                }
                transaction
                    .execute_typed(
                        BIND,
                        &[
                            (&environment.as_str(), Type::TEXT),
                            (&now_utc_ms(), Type::INT8),
                        ],
                    )
                    .await?;
                transaction.commit().await?;
                Ok(Ok(()))
            })
        })?
    }

    fn legacy_runs(&self) -> Result<Vec<LegacyRun>, StorageError> {
        type Columns = (String, String, String, String, String, i64);
        let rows: Vec<Columns> = self.bridge.execute(Effect::Read, |client| {
            Box::pin(async move {
                client
                    .query_typed(
                        LEGACY_RUNS,
                        &[(&IdentitySource::Legacy.as_str(), Type::TEXT)],
                    )
                    .await?
                    .iter()
                    .map(|row| {
                        Ok((
                            row.try_get(0)?,
                            row.try_get(1)?,
                            row.try_get(2)?,
                            row.try_get(3)?,
                            row.try_get(4)?,
                            row.try_get(5)?,
                        ))
                    })
                    .collect()
            })
        })?;
        rows.into_iter()
            .map(
                |(run_id, exchange, symbol, interval, state_json, started_at_ms)| {
                    // A row no build writes, as SQLite reports it.
                    legacy_run(
                        run_id,
                        &exchange,
                        &symbol,
                        &interval,
                        &state_json,
                        started_at_ms,
                    )
                    .map_err(StorageError::other)
                },
            )
            .collect()
    }

    fn adopt_run(
        &self,
        run_id: &str,
        identity: &RunIdentity,
        execution_mode: Option<ExecutionMode>,
    ) -> Result<(), StorageError> {
        refuse_unadopted(identity)?;
        let run_id = run_id.to_string();
        let (environment, account_kind, account_id) = identity_columns(identity);
        let account_id = account_id.map(str::to_string);
        let source = identity.source.as_str();
        self.bridge.execute(Effect::Write, move |client| {
            Box::pin(async move {
                let transaction = client
                    .build_transaction()
                    .isolation_level(IsolationLevel::ReadCommitted)
                    .start()
                    .await?;
                let found = transaction
                    .query_typed_opt(LOCK_RUN, &[(&run_id, Type::TEXT)])
                    .await?
                    .map(|row| Ok::<_, tokio_postgres::Error>((row.try_get(0)?, row.try_get(1)?)))
                    .transpose()?;
                // A refusal rolls the transaction back as it drops.
                let state_json = match adopted_state(&run_id, found, execution_mode) {
                    Ok(state_json) => state_json,
                    Err(refused) => return Ok(Err(refused)),
                };
                transaction
                    .execute_typed(
                        ADOPT,
                        &[
                            (&run_id, Type::TEXT),
                            (&state_json, Type::TEXT),
                            (&environment, Type::TEXT),
                            (&account_kind, Type::TEXT),
                            (&account_id, Type::TEXT),
                            (&source, Type::TEXT),
                            (&now_utc_ms(), Type::INT8),
                        ],
                    )
                    .await?;
                transaction.commit().await?;
                Ok(Ok(()))
            })
        })?
    }
}
