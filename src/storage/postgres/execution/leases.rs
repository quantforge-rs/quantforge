//! A position's lease row and the store's standing, on PostgreSQL. A unit
//! or a lease call locks its position's lease row and then reads the
//! standing; a change of activation locks the whole table of leases
//! first ([`hold_off_holders`]), so it waits for each of them or is seen
//! by it.

use std::str::FromStr;

use tokio_postgres::types::{ToSql, Type};
use tokio_postgres::{Error, GenericClient};

use crate::model::futures::{ActivationEpoch, PositionKey};
use crate::model::{StoreActivation, StoreId, TransferValue};
use crate::ports::StorageError;
use crate::storage::execution::lease::{LeaseColumns, LeaseRow, StoreStanding};
use crate::storage::execution::tables;
use crate::storage::runs::parse_environment;

/// How a row is locked while it is read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::storage::postgres) enum Lock {
    None,
    /// Held alone.
    Own,
}

impl Lock {
    const fn clause(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Own => " FOR UPDATE",
        }
    }
}

/// What a change of activation runs first: no lease row is locked, and
/// no lease made, while this is held, and it waits for every transaction
/// that holds one. Reads of a lease pass it.
pub(in crate::storage::postgres) async fn hold_off_holders<C: GenericClient>(
    transaction: &C,
) -> Result<(), Error> {
    transaction
        .batch_execute("LOCK TABLE position_leases IN EXCLUSIVE MODE")
        .await
}

const STANDING: &str =
    "SELECT store_id, environment, activation, activation_epoch FROM store_control WHERE id = 1";

/// What the store's control row says of its authority, as committed when
/// the statement runs.
pub(in crate::storage::postgres) async fn standing<C: GenericClient>(
    client: &C,
) -> Result<Result<StoreStanding, StorageError>, Error> {
    let row = client.query_typed_one(STANDING, &[]).await?;
    let store_id: String = row.try_get(0)?;
    let environment: Option<String> = row.try_get(1)?;
    let activation: String = row.try_get(2)?;
    let epoch: i32 = row.try_get(3)?;
    let read = || -> Result<StoreStanding, StorageError> {
        Ok(StoreStanding {
            store_id: StoreId::new(store_id).map_err(StorageError::other)?,
            environment: environment
                .map(|text| parse_environment(&text))
                .transpose()
                .map_err(StorageError::other)?,
            activation: StoreActivation::from_str(&activation).map_err(StorageError::other)?,
            epoch: ActivationEpoch(u32::try_from(epoch).map_err(|_| {
                StorageError::InvalidArgument(format!("activation epoch {epoch} is out of range"))
            })?),
        })
    };
    Ok(read())
}

/// The six texts that name a position's lease row.
fn key_values(key: &PositionKey) -> Vec<String> {
    tables::position_columns(key)
        .into_iter()
        .map(|(_, value)| match value {
            TransferValue::Text(text) => text,
            _ => String::new(),
        })
        .collect()
}

fn key_params(values: &[String]) -> Vec<(&(dyn ToSql + Sync), Type)> {
    values
        .iter()
        .map(|value| (value as &(dyn ToSql + Sync), Type::TEXT))
        .collect()
}

const KEYED: &str = "venue = $1 AND category = $2 AND environment = $3 AND account_kind = $4 \
                     AND account_id = $5 AND symbol = $6";

/// Makes the position's lease row if it has none, held by no one, so that
/// there is a row to lock.
pub(super) async fn ensure<C: GenericClient>(client: &C, key: &PositionKey) -> Result<(), Error> {
    let values = key_values(key);
    client
        .execute_typed(
            "INSERT INTO position_leases(venue, category, environment, account_kind, \
             account_id, symbol, fence) VALUES ($1, $2, $3, $4, $5, $6, 0) ON CONFLICT DO NOTHING",
            &key_params(&values),
        )
        .await?;
    Ok(())
}

/// The lease row of `key`, if the position has one.
pub(super) async fn row<C: GenericClient>(
    client: &C,
    key: &PositionKey,
    lock: Lock,
) -> Result<Result<Option<LeaseRow>, StorageError>, Error> {
    let values = key_values(key);
    let sql = format!(
        "SELECT fence, holder, profile_id, activation_epoch, acquired_at_ms, expires_at_ms FROM \
         position_leases WHERE {KEYED}{}",
        lock.clause()
    );
    let Some(found) = client.query_typed_opt(&sql, &key_params(&values)).await? else {
        return Ok(Ok(None));
    };
    let columns = LeaseColumns {
        fence: found.try_get(0)?,
        holder: found.try_get(1)?,
        profile_id: found.try_get(2)?,
        activation_epoch: found.try_get(3)?,
        acquired_at_ms: found.try_get(4)?,
        expires_at_ms: found.try_get(5)?,
    };
    Ok(columns.row(key).map(Some))
}

/// Writes `columns` over the lease row of `key`, which exists.
pub(super) async fn write<C: GenericClient>(
    client: &C,
    columns: &LeaseColumns,
    key: &PositionKey,
) -> Result<(), Error> {
    let values = key_values(key);
    let mut params = key_params(&values);
    params.extend([
        (&columns.fence as &(dyn ToSql + Sync), Type::INT8),
        (&columns.holder, Type::TEXT),
        (&columns.profile_id, Type::TEXT),
        (&columns.activation_epoch, Type::INT8),
        (&columns.acquired_at_ms, Type::INT8),
        (&columns.expires_at_ms, Type::INT8),
    ]);
    let sql = format!(
        "UPDATE position_leases SET fence = $7, holder = $8, profile_id = $9, activation_epoch \
         = $10, acquired_at_ms = $11, expires_at_ms = $12 WHERE {KEYED}"
    );
    client.execute_typed(&sql, &params).await?;
    Ok(())
}
