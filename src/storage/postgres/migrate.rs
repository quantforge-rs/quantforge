//! The schema chain on PostgreSQL (`docs/storage-schema.md`): the same
//! versions as SQLite, starting at schema 5, where PostgreSQL begins.
//!
//! Each pass is one transaction that first takes the advisory lock
//! [`schema::LOCK_KEY`], then reads the version, applies one step (or, on a
//! database without a schema, the whole chain), stamps the version and
//! logs the step. PostgreSQL's DDL is transactional, so a step that fails
//! leaves the database at the last completed one, and processes starting
//! together wait on the lock and find the work done. The transaction is
//! `READ COMMITTED` whatever the server's default: each statement then
//! sees what the lock's previous holder committed, where a `REPEATABLE
//! READ` snapshot, taken at the lock statement, would not.

use std::future::Future;
use std::pin::Pin;

use deadpool_postgres::ClientWrapper;
use tokio_postgres::types::Type;
use tokio_postgres::{IsolationLevel, Transaction};
use tracing::{info, warn};

use super::bridge::{Bridge, Effect};
use super::schema;
use crate::storage::sqlite::SCHEMA_VERSION;
use crate::{StorageError, now_utc_ms};

/// The work of one step, inside the pass's transaction.
pub(super) type StepWork<'t> =
    Pin<Box<dyn Future<Output = Result<(), tokio_postgres::Error>> + Send + 't>>;

/// The step that brings a store from version `to - 1` to `to`; the first
/// step of the chain starts from an empty database.
#[derive(Clone, Copy)]
pub(super) struct Step {
    pub(super) to: u32,
    pub(super) apply: for<'t> fn(&'t Transaction<'t>) -> StepWork<'t>,
}

/// The chain, in order. A step's DDL never changes once a build ran it.
pub(super) const STEPS: [Step; 4] = [
    Step {
        to: 5,
        apply: step_5_schema,
    },
    Step {
        to: 6,
        apply: step_6_jobs,
    },
    Step {
        to: 7,
        apply: step_7_job_results,
    },
    Step {
        to: 8,
        apply: step_8_job_logs,
    },
];

/// What one pass found and did.
enum Pass {
    Current,
    Created { to: u32 },
    Migrated { from: u32, to: u32 },
}

/// Brings the database to [`SCHEMA_VERSION`], creating the schema when it
/// holds none. A newer schema, and tables of ours without a recorded
/// version, are refused untouched. `shown` names the database in the log.
pub(super) fn ensure_current(bridge: &Bridge, shown: &str) -> Result<(), StorageError> {
    ensure_current_with(bridge, &STEPS, SCHEMA_VERSION, shown)
}

/// [`ensure_current`] over any chain ending at `target`, so that the tests
/// can run steps no build has.
pub(super) fn ensure_current_with(
    bridge: &Bridge,
    steps: &'static [Step],
    target: u32,
    shown: &str,
) -> Result<(), StorageError> {
    loop {
        let pass = bridge.execute(Effect::Write, move |client| {
            Box::pin(run_pass(client, steps, target))
        })??;
        match pass {
            Pass::Current => return Ok(()),
            Pass::Created { to } => warn!(
                schema_version = to,
                connection = %shown,
                "database held no QuantForge schema and was created empty; if you expected \
                 existing data (run state, candles), check the PostgreSQL URL"
            ),
            Pass::Migrated { from, to } => {
                info!(from, to, connection = %shown, "postgres schema migrated")
            }
        }
    }
}

/// One pass, in one transaction: a refusal rolls it back as it drops.
async fn run_pass(
    client: &mut ClientWrapper,
    steps: &'static [Step],
    target: u32,
) -> Result<Result<Pass, StorageError>, tokio_postgres::Error> {
    let transaction = client
        .build_transaction()
        .isolation_level(IsolationLevel::ReadCommitted)
        .start()
        .await?;
    // Stores starting together wait their turn, however long a step takes:
    // only the statement limit bounds the wait, never the session's lock wait.
    transaction
        .batch_execute(&format!(
            "SET LOCAL lock_timeout = 0; SELECT pg_advisory_xact_lock({})",
            schema::LOCK_KEY
        ))
        .await?;
    let found = match current_version(&transaction).await? {
        Ok(found) => found,
        Err(refused) => return Ok(Err(refused)),
    };
    if found == target {
        transaction.commit().await?;
        return Ok(Ok(Pass::Current));
    }
    if found > target {
        return Ok(Err(StorageError::SchemaUnsupported {
            found,
            supported: target,
        }));
    }
    let to = if found == 0 {
        for step in steps {
            (step.apply)(&transaction).await?;
        }
        target
    } else {
        let Some(step) = steps.iter().find(|step| step.to == found + 1) else {
            return Ok(Err(StorageError::InvalidArgument(format!(
                "no migration step leaves schema version {found}"
            ))));
        };
        (step.apply)(&transaction).await?;
        step.to
    };
    let (Ok(from_column), Ok(to_column)) = (i32::try_from(found), i32::try_from(to)) else {
        return Ok(Err(StorageError::InvalidArgument(format!(
            "schema version {to} does not fit the migration log"
        ))));
    };
    stamp(&transaction, from_column, to_column).await?;
    transaction.commit().await?;
    Ok(Ok(if found == 0 {
        Pass::Created { to }
    } else {
        Pass::Migrated { from: found, to }
    }))
}

/// The version the database records: 0 when its schema holds none of our
/// tables. Tables of ours without a recorded version are refused, never
/// taken for an empty database.
async fn current_version(
    transaction: &Transaction<'_>,
) -> Result<Result<u32, StorageError>, tokio_postgres::Error> {
    let schema: Option<String> = transaction
        .query_typed_one("SELECT CAST(current_schema() AS TEXT)", &[])
        .await?
        .try_get(0)?;
    let Some(schema) = schema else {
        return Ok(Err(StorageError::InvalidArgument(
            "the role's search_path names no schema that exists, so there is nowhere to create \
             the tables"
                .to_string(),
        )));
    };
    let ours: Vec<String> = transaction
        .query_typed(
            r#"
            SELECT CAST(tablename AS TEXT) FROM pg_tables
            WHERE schemaname = current_schema() AND tablename = ANY($1)
            ORDER BY tablename
            "#,
            &[(&schema::TABLES.as_slice(), Type::TEXT_ARRAY)],
        )
        .await?
        .iter()
        .map(|row| row.try_get(0))
        .collect::<Result<_, _>>()?;
    if !ours.iter().any(|table| table == "meta") {
        if ours.is_empty() {
            return Ok(Ok(0));
        }
        return Ok(Err(StorageError::InvalidArgument(format!(
            "schema {schema} already holds {} but records no QuantForge schema version; give \
             QuantForge a database of its own",
            ours.join(", ")
        ))));
    }
    let recorded: Option<String> = transaction
        .query_typed_opt("SELECT value FROM meta WHERE key = 'schema_version'", &[])
        .await?
        .map(|row| row.try_get(0))
        .transpose()?;
    Ok(match recorded {
        None => Err(StorageError::InvalidArgument(format!(
            "schema {schema} has a meta table but records no QuantForge schema version; give \
             QuantForge a database of its own"
        ))),
        Some(value) => value.parse().map_err(|_| {
            StorageError::InvalidArgument(format!(
                "the database records schema version {value:?}, which is not a number"
            ))
        }),
    })
}

/// Stamps the version and logs the step, inside the pass's transaction.
async fn stamp(
    transaction: &Transaction<'_>,
    from: i32,
    to: i32,
) -> Result<(), tokio_postgres::Error> {
    transaction
        .execute_typed(
            r#"
            INSERT INTO meta(key, value)
            VALUES ('schema_version', $1)
            ON CONFLICT (key) DO UPDATE SET value = excluded.value
            "#,
            &[(&to.to_string(), Type::TEXT)],
        )
        .await?;
    transaction
        .execute_typed(
            r#"
            INSERT INTO schema_migrations(version, from_version, applied_at_ms, quantforge_version)
            VALUES ($1, $2, $3, $4)
            "#,
            &[
                (&to, Type::INT4),
                (&from, Type::INT4),
                (&now_utc_ms(), Type::INT8),
                (&env!("CARGO_PKG_VERSION"), Type::TEXT),
            ],
        )
        .await?;
    Ok(())
}

/// Schema 5 (QUA-125): every table of SQLite's schema 5, and the store's
/// control record.
fn step_5_schema<'t>(transaction: &'t Transaction<'t>) -> StepWork<'t> {
    Box::pin(async move {
        transaction.batch_execute(schema::SCHEMA_5).await?;
        schema::create_control_row(transaction).await
    })
}

/// Schema 6 (QUA-358): bundles and jobs, and the byte-ordered keys.
fn step_6_jobs<'t>(transaction: &'t Transaction<'t>) -> StepWork<'t> {
    Box::pin(async move { transaction.batch_execute(schema::SCHEMA_6).await })
}

/// Schema 7 (QUA-356): the results of the jobs that succeeded.
fn step_7_job_results<'t>(transaction: &'t Transaction<'t>) -> StepWork<'t> {
    Box::pin(async move { transaction.batch_execute(schema::SCHEMA_7).await })
}

/// Schema 8 (QUA-355): what is kept of each job's log.
fn step_8_job_logs<'t>(transaction: &'t Transaction<'t>) -> StepWork<'t> {
    Box::pin(async move { transaction.batch_execute(schema::SCHEMA_8).await })
}
