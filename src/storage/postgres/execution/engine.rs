//! The statements the shared plan's reads and writes become on
//! PostgreSQL. Table and column names come from the tables' own
//! definitions; every value is a typed parameter. A row a unit reads to
//! decide on is locked `FOR UPDATE`, so it stands until the unit ends.

use tokio_postgres::types::{ToSql, Type};
use tokio_postgres::{Error, GenericClient, Row};

use crate::model::execution::{NewOperation, OperationRecord, Record, Stored};
use crate::model::{ColumnKind, OperationId, TimestampMs, TransferValue};
use crate::ports::StorageError;
use crate::storage::execution::plan::Write;
use crate::storage::execution::read::{self, Query, RawRow};
use crate::storage::execution::rows;
use crate::storage::execution::tables::{self, Named, OPERATIONS, PoolKey, Table};
use crate::storage::postgres::limit;

/// A statement's parameter, owned, so that it can cross an await.
#[derive(Debug)]
pub(super) enum Param {
    Text(Option<String>),
    Integer(Option<i64>),
}

impl Param {
    pub(super) fn typed(&self) -> (&(dyn ToSql + Sync), Type) {
        match self {
            Self::Text(value) => (value, Type::TEXT),
            Self::Integer(value) => (value, Type::INT8),
        }
    }

    /// `value` as a parameter of a column of `kind`.
    fn of(kind: ColumnKind, value: &TransferValue) -> Self {
        match (value, kind) {
            (TransferValue::Text(text), _) => Self::Text(Some(text.clone())),
            (TransferValue::Integer(number), _) => Self::Integer(Some(*number)),
            (_, ColumnKind::Integer) => Self::Integer(None),
            _ => Self::Text(None),
        }
    }
}

pub(super) fn typed(params: &[Param]) -> Vec<(&(dyn ToSql + Sync), Type)> {
    params.iter().map(Param::typed).collect()
}

fn kind_of(table: &Table, column: &str) -> ColumnKind {
    table
        .spec
        .columns
        .iter()
        .find(|candidate| candidate.name == column)
        .map_or(ColumnKind::Text, |found| found.kind)
}

/// `column = $n` for each of `equals`, or `IS NULL` where the value is
/// none, with the parameters to bind after the `already` given.
fn conditions(table: &Table, equals: &[Named], params: &mut Vec<Param>) -> Vec<String> {
    let mut clauses = Vec::new();
    for (column, value) in equals {
        if *value == TransferValue::Null {
            clauses.push(format!("{column} IS NULL"));
        } else {
            params.push(Param::of(kind_of(table, column), value));
            clauses.push(format!("{column} = ${}", params.len()));
        }
    }
    clauses
}

fn decode(table: &Table, row: &Row) -> Result<RawRow, Error> {
    let mut values = Vec::with_capacity(table.spec.columns.len());
    for (index, column) in table.spec.columns.iter().enumerate() {
        values.push(match column.kind {
            ColumnKind::Integer => row
                .try_get::<_, Option<i64>>(index)?
                .map_or(TransferValue::Null, TransferValue::Integer),
            ColumnKind::Text | ColumnKind::Blob => row
                .try_get::<_, Option<String>>(index)?
                .map_or(TransferValue::Null, TransferValue::Text),
        });
    }
    Ok(values)
}

/// Every row `query` names, each as one value for each column; locked
/// until the transaction ends where `lock` says so.
pub(super) async fn select<C: GenericClient>(
    client: &C,
    query: &Query,
    lock: bool,
) -> Result<Vec<RawRow>, Error> {
    let spec = query.table.spec;
    let columns: Vec<&str> = spec.columns.iter().map(|column| column.name).collect();
    let mut params = Vec::new();
    let mut clauses = conditions(query.table, &query.equals, &mut params);
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
    let bounded = match query.limit {
        Some(rows) => {
            params.push(Param::Integer(Some(limit(rows))));
            format!(" LIMIT ${}", params.len())
        }
        None => String::new(),
    };
    let locked = if lock { " FOR UPDATE" } else { "" };
    let sql = format!(
        "SELECT {} FROM {}{filter} ORDER BY {}{bounded}{locked}",
        columns.join(", "),
        spec.name,
        query.order
    );
    client
        .query_typed(&sql, &typed(&params))
        .await?
        .iter()
        .map(|row| decode(query.table, row))
        .collect()
}

/// What a write did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Written {
    Done,
    /// An insert met a row another writer put under its key since the
    /// unit read: the unit is decided again.
    Clashed,
}

/// The columns of `table` an insert names, with their values: all but the
/// one it numbers itself, where the row gives none.
fn insertable(table: &Table, row: &[TransferValue]) -> (Vec<&'static str>, Vec<Param>) {
    table
        .spec
        .columns
        .iter()
        .zip(row)
        .filter(|(column, value)| {
            table.numbered != Some(column.name) || **value != TransferValue::Null
        })
        .map(|(column, value)| (column.name, Param::of(column.kind, value)))
        .unzip()
}

/// Runs one write of a unit.
pub(super) async fn write<C: GenericClient>(client: &C, write: &Write) -> Result<Written, Error> {
    match write {
        Write::Insert { table, row } => {
            let (columns, params) = insertable(table, row);
            let marks: Vec<String> = (1..=columns.len()).map(|n| format!("${n}")).collect();
            // Only the key a record is found by gives way: any other
            // unique key met is a failure, raised as one.
            let numbered_only = table
                .numbered
                .is_some_and(|numbered| table.key == [numbered]);
            let on_conflict = if numbered_only {
                String::new()
            } else {
                format!(" ON CONFLICT ({}) DO NOTHING", table.key.join(", "))
            };
            let sql = format!(
                "INSERT INTO {} ({}) VALUES ({}){on_conflict}",
                table.name(),
                columns.join(", "),
                marks.join(", ")
            );
            let written = client.execute_typed(&sql, &typed(&params)).await?;
            Ok(if written == 1 {
                Written::Done
            } else {
                Written::Clashed
            })
        }
        Write::Update {
            table,
            key,
            expected,
            row,
        } => {
            let mut params = Vec::new();
            let mut assignments = Vec::new();
            for (column, value) in table.spec.columns.iter().zip(row) {
                let kept = table.numbered == Some(column.name)
                    || table.created == Some(column.name)
                    || key.iter().any(|(name, _)| *name == column.name);
                if kept {
                    continue;
                }
                params.push(Param::of(column.kind, value));
                assignments.push(format!("{} = ${}", column.name, params.len()));
            }
            let mut clauses = conditions(table, key, &mut params);
            params.push(Param::Integer(Some(limit(
                usize::try_from(*expected).unwrap_or(usize::MAX),
            ))));
            clauses.push(format!("revision = ${}", params.len()));
            let sql = format!(
                "UPDATE {} SET {} WHERE {}",
                table.name(),
                assignments.join(", "),
                clauses.join(" AND ")
            );
            let changed = client.execute_typed(&sql, &typed(&params)).await?;
            // The row was locked when it was read, so it has not moved.
            Ok(if changed == 1 {
                Written::Done
            } else {
                Written::Clashed
            })
        }
        Write::Pool { key, to } => {
            client
                .execute_typed(
                    "UPDATE reservation_pools SET revision = $6 WHERE venue = $1 AND \
                     environment = $2 AND account_kind = $3 AND account_id = $4 AND pool = $5",
                    &[
                        (&key.venue, Type::TEXT),
                        (&key.environment, Type::TEXT),
                        (&key.account_kind, Type::TEXT),
                        (&key.account_id, Type::TEXT),
                        (&key.pool, Type::TEXT),
                        (
                            &limit(usize::try_from(*to).unwrap_or(usize::MAX)),
                            Type::INT8,
                        ),
                    ],
                )
                .await?;
            Ok(Written::Done)
        }
    }
}

/// A pool's revision, 0 until something is reserved from it. Inside a
/// unit the pool's row is made if it is missing and then locked, so that
/// two units reserving from one pool decide one after the other.
pub(super) async fn pool_revision<C: GenericClient>(
    client: &C,
    pool: &PoolKey,
    lock: bool,
) -> Result<u64, Error> {
    let key: [(&(dyn ToSql + Sync), Type); 5] = [
        (&pool.venue, Type::TEXT),
        (&pool.environment, Type::TEXT),
        (&pool.account_kind, Type::TEXT),
        (&pool.account_id, Type::TEXT),
        (&pool.pool, Type::TEXT),
    ];
    if lock {
        client
            .execute_typed(
                "INSERT INTO reservation_pools(venue, environment, account_kind, account_id, \
                 pool, revision) VALUES ($1, $2, $3, $4, $5, 0) ON CONFLICT DO NOTHING",
                &key,
            )
            .await?;
    }
    let locked = if lock { " FOR UPDATE" } else { "" };
    let sql = format!(
        "SELECT revision FROM reservation_pools WHERE venue = $1 AND environment = $2 AND \
         account_kind = $3 AND account_id = $4 AND pool = $5{locked}"
    );
    let revision: Option<i64> = client
        .query_typed_opt(&sql, &key)
        .await?
        .map(|row| row.try_get(0))
        .transpose()?;
    Ok(u64::try_from(revision.unwrap_or(0)).unwrap_or(0))
}

/// The operation already recorded under `asked`'s key, if it has a key
/// and one is.
pub(super) async fn operation_under<C: GenericClient>(
    client: &C,
    asked: &NewOperation,
) -> Result<Result<Option<Stored<OperationRecord>>, StorageError>, Error> {
    let Some(key) = &asked.key else {
        return Ok(Ok(None));
    };
    let mut equals: Vec<Named> = tables::scope_columns(&asked.scope)
        .into_iter()
        .filter(|(column, _)| tables::account_key().contains(column))
        .collect();
    equals.push(("operation_key", TransferValue::Text(key.to_string())));
    let rows = select(client, &Query::of(&OPERATIONS, equals), true).await?;
    let Some(row) = rows.first() else {
        return Ok(Ok(None));
    };
    Ok(
        read::decode(&OPERATIONS, row).map(|held| match held.record {
            Record::Operation(record) => Some(Stored {
                record,
                revision: held.revision,
            }),
            _ => None,
        }),
    )
}

/// Records `asked` under the next number: its row is written with the
/// number the table gives, then given the document that names it. `None`
/// where another writer recorded its key first.
pub(super) async fn number_operation<C: GenericClient>(
    client: &C,
    asked: &NewOperation,
    at_ms: TimestampMs,
) -> Result<Result<Option<OperationRecord>, StorageError>, Error> {
    let refused = |error: crate::ModelError| StorageError::InvalidArgument(error.to_string());
    // A placeholder that passes every rule but the number, which is not
    // known until the row exists.
    let placeholder = OperationId::new(1)
        .and_then(|one| asked.clone().numbered(one))
        .map_err(refused);
    let row = placeholder.and_then(|placeholder| {
        let columns = rows::derived(&Record::Operation(placeholder))?;
        let mut row = OPERATIONS.row(&columns, 1, at_ms)?;
        row[OPERATIONS.index("operation_id")?] = TransferValue::Null;
        Ok(row)
    });
    let row = match row {
        Ok(row) => row,
        Err(refusal) => return Ok(Err(refusal)),
    };
    let (columns, params) = insertable(&OPERATIONS, &row);
    let marks: Vec<String> = (1..=columns.len()).map(|n| format!("${n}")).collect();
    let sql = format!(
        "INSERT INTO execution_operations ({}) VALUES ({}) ON CONFLICT (venue, category, \
         environment, account_kind, account_id, operation_key) DO NOTHING RETURNING operation_id",
        columns.join(", "),
        marks.join(", ")
    );
    let Some(numbered) = client.query_typed_opt(&sql, &typed(&params)).await? else {
        return Ok(Ok(None));
    };
    let number: i64 = numbered.try_get(0)?;
    let record = u64::try_from(number)
        .ok()
        .and_then(|number| OperationId::new(number).ok())
        .ok_or_else(|| StorageError::InvalidArgument("no operation number given".to_string()))
        .and_then(|number| asked.clone().numbered(number).map_err(refused))
        .and_then(|record| Ok((record.to_stored().map_err(refused)?, record)));
    let (document, record) = match record {
        Ok(found) => found,
        Err(refusal) => return Ok(Err(refusal)),
    };
    client
        .execute_typed(
            "UPDATE execution_operations SET operation_json = $1 WHERE operation_id = $2",
            &[(&document, Type::TEXT), (&number, Type::INT8)],
        )
        .await?;
    Ok(Ok(Some(record)))
}
