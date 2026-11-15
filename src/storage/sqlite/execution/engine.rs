//! The statements the shared plan's reads and writes become on SQLite.
//! Table and column names come from the tables' own definitions; every
//! value is bound.

use rusqlite::types::{Value, ValueRef};
use rusqlite::{Connection, OptionalExtension, params, params_from_iter};

use super::super::errors::failed;
use crate::model::execution::{NewOperation, OperationRecord, Record, Stored};
use crate::model::{ColumnKind, OperationId, TimestampMs, TransferValue};
use crate::ports::StorageError;
use crate::storage::execution::plan::Write;
use crate::storage::execution::read::{self, Query, RawRow};
use crate::storage::execution::rows;
use crate::storage::execution::tables::{self, Named, OPERATIONS, PoolKey, Table};

fn bound(value: &TransferValue) -> Value {
    match value {
        TransferValue::Null => Value::Null,
        TransferValue::Integer(value) => Value::Integer(*value),
        TransferValue::Text(text) => Value::Text(text.clone()),
        TransferValue::Blob(bytes) => Value::Blob(bytes.clone()),
    }
}

/// `column = ?n` for each of `equals`, or `IS NULL` where the value is
/// none, with the values to bind.
fn conditions(equals: &[Named]) -> (Vec<String>, Vec<Value>) {
    let mut clauses = Vec::new();
    let mut values = Vec::new();
    for (column, value) in equals {
        if *value == TransferValue::Null {
            clauses.push(format!("{column} IS NULL"));
        } else {
            values.push(bound(value));
            clauses.push(format!("{column} = ?{}", values.len()));
        }
    }
    (clauses, values)
}

/// Every row `query` names, each as one value for each column.
pub(super) fn select(connection: &Connection, query: &Query) -> Result<Vec<RawRow>, StorageError> {
    let spec = query.table.spec;
    let columns: Vec<&str> = spec.columns.iter().map(|column| column.name).collect();
    let (mut clauses, values) = conditions(&query.equals);
    if let Some((column, words)) = &query.one_of {
        // Fixed words of the model's own state machines, never input.
        let listed: Vec<String> = words.iter().map(|word| format!("'{word}'")).collect();
        clauses.push(format!("{column} IN ({})", listed.join(", ")));
    }
    let filter = if clauses.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", clauses.join(" AND "))
    };
    let limit = query
        .limit
        .map(|limit| format!(" LIMIT {}", i64::try_from(limit).unwrap_or(i64::MAX)))
        .unwrap_or_default();
    let sql = format!(
        "SELECT {} FROM {}{filter} ORDER BY {}{limit}",
        columns.join(", "),
        spec.name,
        query.order
    );
    let mut statement = connection.prepare(&sql).map_err(failed)?;
    let mut rows = statement.query(params_from_iter(values)).map_err(failed)?;
    let mut found = Vec::new();
    while let Some(row) = rows.next().map_err(failed)? {
        let mut values = Vec::with_capacity(spec.columns.len());
        for (index, column) in spec.columns.iter().enumerate() {
            let stored = row.get_ref(index).map_err(failed)?;
            values.push(match (column.kind, stored) {
                (_, ValueRef::Null) => TransferValue::Null,
                (ColumnKind::Integer, ValueRef::Integer(value)) => TransferValue::Integer(value),
                (ColumnKind::Text, ValueRef::Text(bytes)) => match std::str::from_utf8(bytes) {
                    Ok(text) => TransferValue::Text(text.to_string()),
                    Err(_) => return Err(unreadable(query.table, column.name)),
                },
                _ => return Err(unreadable(query.table, column.name)),
            });
        }
        found.push(values);
    }
    Ok(found)
}

fn unreadable(table: &Table, column: &str) -> StorageError {
    StorageError::InvalidArgument(format!(
        "a stored row of {} cannot be read: its {column} holds a value of another type",
        table.name()
    ))
}

/// The columns of `table` an insert names: all but the one it numbers
/// itself, where the row gives none.
fn insertable<'r>(
    table: &Table,
    row: &'r [TransferValue],
) -> (Vec<&'static str>, Vec<&'r TransferValue>) {
    table
        .spec
        .columns
        .iter()
        .zip(row)
        .filter(|(column, value)| {
            table.numbered != Some(column.name) || **value != TransferValue::Null
        })
        .map(|(column, value)| (column.name, value))
        .unzip()
}

/// Runs one write of a unit. An insert that meets its key, or an update
/// that finds its row moved, is a failure here: the transaction holds the
/// database's one write lock, so what the plan read still stands.
pub(super) fn write(connection: &Connection, write: &Write) -> Result<(), StorageError> {
    match write {
        Write::Insert { table, row } => {
            let (columns, values) = insertable(table, row);
            let marks: Vec<String> = (1..=columns.len()).map(|n| format!("?{n}")).collect();
            let sql = format!(
                "INSERT INTO {} ({}) VALUES ({})",
                table.name(),
                columns.join(", "),
                marks.join(", ")
            );
            connection
                .execute(&sql, params_from_iter(values.into_iter().map(bound)))
                .map_err(failed)?;
        }
        Write::Update {
            table,
            key,
            expected,
            row,
        } => {
            let mut values = Vec::new();
            let mut assignments = Vec::new();
            for (column, value) in table.spec.columns.iter().zip(row) {
                let kept = table.numbered == Some(column.name)
                    || table.created == Some(column.name)
                    || key.iter().any(|(name, _)| *name == column.name);
                if kept {
                    continue;
                }
                values.push(bound(value));
                assignments.push(format!("{} = ?{}", column.name, values.len()));
            }
            let mut clauses = Vec::new();
            for (column, value) in key {
                values.push(bound(value));
                clauses.push(format!("{column} = ?{}", values.len()));
            }
            values.push(Value::Integer(i64::try_from(*expected).unwrap_or(i64::MAX)));
            let sql = format!(
                "UPDATE {} SET {} WHERE {} AND revision = ?{}",
                table.name(),
                assignments.join(", "),
                clauses.join(" AND "),
                values.len()
            );
            let changed = connection
                .execute(&sql, params_from_iter(values))
                .map_err(failed)?;
            if changed != 1 {
                return Err(StorageError::InvalidArgument(format!(
                    "a row of {} moved inside the transaction that read it",
                    table.name()
                )));
            }
        }
        Write::Pool { key, to } => {
            connection
                .execute(
                    r#"
                    INSERT INTO reservation_pools(venue, environment, account_kind, account_id, pool, revision)
                    VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                    ON CONFLICT(venue, environment, account_kind, account_id, pool)
                    DO UPDATE SET revision = excluded.revision
                    "#,
                    params![
                        key.venue,
                        key.environment,
                        key.account_kind,
                        key.account_id,
                        key.pool,
                        i64::try_from(*to).unwrap_or(i64::MAX)
                    ],
                )
                .map_err(failed)?;
        }
    }
    Ok(())
}

/// A pool's revision: 0 until something is reserved from it.
pub(super) fn pool_revision(connection: &Connection, pool: &PoolKey) -> Result<u64, StorageError> {
    let revision: Option<i64> = connection
        .query_row(
            r#"
            SELECT revision FROM reservation_pools
            WHERE venue = ?1 AND environment = ?2 AND account_kind = ?3 AND account_id = ?4
              AND pool = ?5
            "#,
            params![
                pool.venue,
                pool.environment,
                pool.account_kind,
                pool.account_id,
                pool.pool
            ],
            |row| row.get(0),
        )
        .optional()
        .map_err(failed)?;
    u64::try_from(revision.unwrap_or(0)).map_err(|_| {
        StorageError::InvalidArgument("a stored pool revision is below zero".to_string())
    })
}

/// The operation already recorded under `asked`'s key, if it has a key
/// and one is.
pub(super) fn operation_under(
    connection: &Connection,
    asked: &NewOperation,
) -> Result<Option<Stored<OperationRecord>>, StorageError> {
    let Some(key) = &asked.key else {
        return Ok(None);
    };
    let mut equals: Vec<Named> = tables::scope_columns(&asked.scope)
        .into_iter()
        .filter(|(column, _)| tables::account_key().contains(column))
        .collect();
    equals.push(("operation_key", TransferValue::Text(key.to_string())));
    let rows = select(connection, &Query::of(&OPERATIONS, equals))?;
    let Some(row) = rows.first() else {
        return Ok(None);
    };
    let held = read::decode(&OPERATIONS, row)?;
    match held.record {
        Record::Operation(record) => Ok(Some(Stored {
            record,
            revision: held.revision,
        })),
        _ => Ok(None),
    }
}

/// Records `asked` under the next number: its row is written with the
/// number the table gives, then given the document that names it.
pub(super) fn number_operation(
    connection: &Connection,
    asked: &NewOperation,
    at_ms: TimestampMs,
) -> Result<OperationRecord, StorageError> {
    let refused = |error: crate::ModelError| StorageError::InvalidArgument(error.to_string());
    // A placeholder that passes every rule but the number, which is not
    // known until the row exists.
    let placeholder = asked
        .clone()
        .numbered(OperationId::new(1).map_err(refused)?)
        .map_err(refused)?;
    let columns = rows::derived(&Record::Operation(placeholder))?;
    let mut row = OPERATIONS.row(&columns, 1, at_ms)?;
    row[OPERATIONS.index("operation_id")?] = TransferValue::Null;
    write(
        connection,
        &Write::Insert {
            table: &OPERATIONS,
            row,
        },
    )?;
    let number = u64::try_from(connection.last_insert_rowid())
        .ok()
        .and_then(|number| OperationId::new(number).ok())
        .ok_or_else(|| StorageError::InvalidArgument("no operation number given".to_string()))?;
    let record = asked.clone().numbered(number).map_err(refused)?;
    connection
        .execute(
            "UPDATE execution_operations SET operation_json = ?1 WHERE operation_id = ?2",
            params![
                record.to_stored().map_err(refused)?,
                i64::try_from(number.get()).unwrap_or(i64::MAX)
            ],
        )
        .map_err(failed)?;
    Ok(record)
}
