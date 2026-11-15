//! The store's activation on PostgreSQL (`docs/execution-store.md`): the
//! change itself, one transaction with the row of its log, and the log
//! read back. The rules are `storage::execution::lease`'s, as on SQLite.

use std::str::FromStr;

use tokio_postgres::IsolationLevel;
use tokio_postgres::types::Type;

use super::bridge::Effect;
use super::control::{LOCK_BINDING, RECORD, record};
use super::execution::{hold_off_holders, standing};
use super::{PostgresStore, copying};
use crate::StorageError;
use crate::model::futures::ActivationEpoch;
use crate::model::{ControlEvent, StoreActivation, StoreIdentity, TimestampMs};
use crate::storage::execution::lease::{VOID_LEASES, activation_change};

const SET_ACTIVATION: &str = r#"
    UPDATE store_control SET activation = $1, activation_epoch = $2, changed_at_ms = $3
    WHERE id = 1
"#;

const LOG_ACTIVATION: &str = r#"
    INSERT INTO store_control_events(from_activation, to_activation, activation_epoch, changed_at_ms)
    VALUES ($1, $2, $3, $4)
"#;

const CONTROL_EVENTS: &str = r#"
    SELECT from_activation, to_activation, activation_epoch, changed_at_ms
    FROM store_control_events
    ORDER BY seq ASC
"#;

impl PostgresStore {
    /// Moves the store's activation to `to`, with the row of its log, in
    /// one transaction that first locks the table of leases and then the
    /// control row: of two operators at once one finds it done, and every
    /// unit written under a lease, and every lease call, either ended
    /// before or sees the change. An activation voids every lease in the
    /// same transaction.
    pub(super) fn change_activation(
        &self,
        to: StoreActivation,
        at_ms: TimestampMs,
    ) -> Result<StoreIdentity, StorageError> {
        self.bridge.execute(Effect::Write, move |client| {
            Box::pin(async move {
                let transaction = client
                    .build_transaction()
                    .isolation_level(IsolationLevel::ReadCommitted)
                    .start()
                    .await?;
                hold_off_holders(&*transaction).await?;
                transaction.query_typed(LOCK_BINDING, &[]).await?;
                let before = match standing(&*transaction).await? {
                    Ok(before) => before,
                    Err(refused) => return Ok(Err(refused)),
                };
                // A refusal rolls the transaction back as it drops.
                if let Some(refused) = copying::refusal(&*transaction).await? {
                    return Ok(Err(refused));
                }
                let (after, event) = match activation_change(&before, to, at_ms) {
                    Ok(change) => change,
                    Err(refused) => return Ok(Err(refused)),
                };
                let epoch = |epoch: ActivationEpoch| i64::from(epoch.0);
                let Ok(narrow) = i32::try_from(after.epoch.0) else {
                    return Ok(Err(StorageError::InvalidArgument(format!(
                        "activation epoch {} does not fit the store's record",
                        after.epoch.0
                    ))));
                };
                transaction
                    .execute_typed(
                        SET_ACTIVATION,
                        &[
                            (&after.activation.as_str(), Type::TEXT),
                            (&narrow, Type::INT4),
                            (&at_ms, Type::INT8),
                        ],
                    )
                    .await?;
                transaction
                    .execute_typed(
                        LOG_ACTIVATION,
                        &[
                            (&event.from.as_str(), Type::TEXT),
                            (&event.to.as_str(), Type::TEXT),
                            (&epoch(event.epoch), Type::INT8),
                            (&event.at_ms, Type::INT8),
                        ],
                    )
                    .await?;
                if after.activation == StoreActivation::Active {
                    transaction.batch_execute(VOID_LEASES).await?;
                }
                let identity = record(&transaction.query_typed_one(RECORD, &[]).await?);
                transaction.commit().await?;
                Ok(identity)
            })
        })?
    }

    /// Every change of the store's activation, oldest first.
    pub(super) fn activation_log(&self) -> Result<Vec<ControlEvent>, StorageError> {
        let rows: Vec<(String, String, i64, i64)> =
            self.bridge.execute(Effect::Read, |client| {
                Box::pin(async move {
                    client
                        .query_typed(CONTROL_EVENTS, &[])
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
            .map(|(from, to, epoch, at_ms)| {
                let state =
                    |text: &str| StoreActivation::from_str(text).map_err(StorageError::other);
                Ok(ControlEvent {
                    from: state(&from)?,
                    to: state(&to)?,
                    epoch: ActivationEpoch(u32::try_from(epoch).map_err(|_| {
                        StorageError::InvalidArgument(format!(
                            "activation epoch {epoch} is out of range"
                        ))
                    })?),
                    at_ms,
                })
            })
            .collect()
    }
}
