//! A position's lease row and the store's standing, on SQLite.

use rusqlite::{Connection, OptionalExtension, params_from_iter, types::Value};

use super::super::control::read_identity;
use super::super::errors::failed;
use crate::model::TransferValue;
use crate::model::futures::PositionKey;
use crate::ports::StorageError;
use crate::storage::execution::lease::{LeaseColumns, LeaseRow, StoreStanding};
use crate::storage::execution::tables;

/// What the store's control row says of its authority.
pub(super) fn standing(connection: &Connection) -> Result<StoreStanding, StorageError> {
    Ok(StoreStanding::of(&read_identity(connection)?))
}

/// The six values that name a position's lease row.
fn key_values(key: &PositionKey) -> Vec<Value> {
    tables::position_columns(key)
        .into_iter()
        .map(|(_, value)| match value {
            TransferValue::Text(text) => Value::Text(text),
            _ => Value::Null,
        })
        .collect()
}

const KEYED: &str = "venue = ?1 AND category = ?2 AND environment = ?3 AND account_kind = ?4 \
                     AND account_id = ?5 AND symbol = ?6";

/// The lease row of `key`, if the position ever had one.
pub(super) fn row(
    connection: &Connection,
    key: &PositionKey,
) -> Result<Option<LeaseRow>, StorageError> {
    let columns: Option<LeaseColumns> = connection
        .query_row(
            &format!(
                "SELECT fence, holder, profile_id, activation_epoch, acquired_at_ms, \
                 expires_at_ms FROM position_leases WHERE {KEYED}"
            ),
            params_from_iter(key_values(key)),
            |row| {
                Ok(LeaseColumns {
                    fence: row.get(0)?,
                    holder: row.get(1)?,
                    profile_id: row.get(2)?,
                    activation_epoch: row.get(3)?,
                    acquired_at_ms: row.get(4)?,
                    expires_at_ms: row.get(5)?,
                })
            },
        )
        .optional()
        .map_err(failed)?;
    columns.map(|columns| columns.row(key)).transpose()
}

/// Writes `columns` as the lease row of `key`, the first or in place of
/// the one it has.
pub(super) fn write(
    connection: &Connection,
    columns: &LeaseColumns,
    key: &PositionKey,
) -> Result<(), StorageError> {
    let optional_text = |value: &Option<String>| value.clone().map_or(Value::Null, Value::Text);
    let optional_number = |value: Option<i64>| value.map_or(Value::Null, Value::Integer);
    let mut values = key_values(key);
    values.extend([
        Value::Integer(columns.fence),
        optional_text(&columns.holder),
        optional_text(&columns.profile_id),
        optional_number(columns.activation_epoch),
        optional_number(columns.acquired_at_ms),
        optional_number(columns.expires_at_ms),
    ]);
    connection
        .execute(
            r#"
            INSERT INTO position_leases(
              venue, category, environment, account_kind, account_id, symbol, fence, holder,
              profile_id, activation_epoch, acquired_at_ms, expires_at_ms
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
            ON CONFLICT(venue, category, environment, account_kind, account_id, symbol)
            DO UPDATE SET fence = excluded.fence, holder = excluded.holder,
              profile_id = excluded.profile_id, activation_epoch = excluded.activation_epoch,
              acquired_at_ms = excluded.acquired_at_ms, expires_at_ms = excluded.expires_at_ms
            "#,
            params_from_iter(values),
        )
        .map_err(failed)?;
    Ok(())
}
