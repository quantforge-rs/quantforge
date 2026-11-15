//! The values execution rows are made of: a scope and a position as
//! their columns, a pool as its key, and the small conversions every row
//! is built with.

use super::tables::DELIVERIES;
use crate::model::execution::{ExecutionScope, SignalDelivery};
use crate::model::futures::{IntentState, PoolScope, PositionKey};
use crate::model::{AccountRef, TimestampMs, TransferValue, VenueAccountId};
use crate::ports::StorageError;

/// The account kinds the `account_kind` column spells, as a run's does.
pub(crate) const KIND_VENUE: &str = "venue";
pub(crate) const KIND_KEY_FINGERPRINT: &str = "key_fingerprint";

/// A column's value by its name.
pub(crate) type Named = (&'static str, TransferValue);

pub(crate) fn text(value: impl ToString) -> TransferValue {
    TransferValue::Text(value.to_string())
}

pub(crate) fn optional<T: ToString>(value: Option<T>) -> TransferValue {
    value.map_or(TransferValue::Null, text)
}

/// A store's number as its column holds it.
pub(crate) fn number(value: u64) -> Result<TransferValue, StorageError> {
    i64::try_from(value)
        .map(TransferValue::Integer)
        .map_err(|_| StorageError::InvalidArgument(format!("{value} does not fit a column")))
}

pub(crate) fn optional_number(value: Option<u64>) -> Result<TransferValue, StorageError> {
    value.map_or(Ok(TransferValue::Null), number)
}

pub(crate) fn time(value: Option<TimestampMs>) -> TransferValue {
    value.map_or(TransferValue::Null, TransferValue::Integer)
}

/// The slot marker: 1 while the record holds its position's one slot.
pub(crate) fn slot(held: bool) -> TransferValue {
    if held {
        TransferValue::Integer(1)
    } else {
        TransferValue::Null
    }
}

fn account_columns(venue: &str, environment: &str, account: &VenueAccountId) -> [Named; 4] {
    let kind = match account {
        VenueAccountId::Venue(_) => KIND_VENUE,
        VenueAccountId::KeyFingerprint(_) => KIND_KEY_FINGERPRINT,
    };
    [
        ("venue", text(venue)),
        ("environment", text(environment)),
        ("account_kind", text(kind)),
        ("account_id", text(account.as_str())),
    ]
}

/// A scope as the six columns every execution row starts with.
pub(crate) fn scope_columns(scope: &ExecutionScope) -> Vec<Named> {
    let [venue, environment, kind, id] = account_columns(
        scope.venue().as_str(),
        scope.environment().as_str(),
        scope.account(),
    );
    vec![
        venue,
        ("category", text(scope.category().as_str())),
        environment,
        kind,
        id,
        ("profile_id", optional(scope.profile())),
    ]
}

/// The columns that name one position, whoever's profile acts on it: its
/// account's and its symbol.
pub(crate) fn position_columns(key: &PositionKey) -> Vec<Named> {
    let account = key.account();
    let [venue, environment, kind, id] = account_columns(
        account.venue().as_str(),
        account.environment().as_str(),
        account.venue_account_id(),
    );
    vec![
        venue,
        ("category", text(key.instrument().category().as_str())),
        environment,
        kind,
        id,
        ("symbol", text(&key.instrument().symbol)),
    ]
}

/// One pool of one venue account: what a reservation is held against.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PoolKey {
    pub(crate) venue: String,
    pub(crate) environment: String,
    pub(crate) account_kind: String,
    pub(crate) account_id: String,
    pub(crate) pool: String,
}

impl PoolKey {
    pub(crate) fn of(account: &AccountRef, pool: &PoolScope) -> Self {
        let [venue, environment, kind, id] = account_columns(
            account.venue().as_str(),
            account.environment().as_str(),
            account.venue_account_id(),
        )
        .map(|(_, value)| match value {
            TransferValue::Text(text) => text,
            _ => String::new(),
        });
        Self {
            venue,
            environment,
            account_kind: kind,
            account_id: id,
            pool: pool_text(pool),
        }
    }

    /// The columns of `margin_reservations` that name the pool.
    pub(crate) fn columns(&self) -> Vec<Named> {
        vec![
            ("venue", text(&self.venue)),
            ("environment", text(&self.environment)),
            ("account_kind", text(&self.account_kind)),
            ("account_id", text(&self.account_id)),
            ("pool", text(&self.pool)),
        ]
    }
}

/// A pool as its column spells it: `asset:USDT`, or `account_usd`.
pub(crate) fn pool_text(pool: &PoolScope) -> String {
    match pool {
        PoolScope::Asset(asset) => format!("asset:{asset}"),
        PoolScope::AccountUsd => "account_usd".to_string(),
    }
}

/// The row a delivery's note is written as.
pub(crate) fn delivery_row(note: &SignalDelivery) -> Result<Vec<TransferValue>, StorageError> {
    let mut columns = scope_columns(&note.scope);
    columns.extend([
        ("signal_id", text(&note.signal)),
        ("disposition", text(note.disposition.as_str())),
        ("payload_hash", text(note.payload_hash)),
        ("payload", optional(note.payload.as_deref())),
        ("fire_time_ms", TransferValue::Integer(note.fire_time_ms)),
        (
            "received_at_ms",
            TransferValue::Integer(note.received_at_ms),
        ),
    ]);
    DELIVERIES.row(&columns, 1, note.received_at_ms)
}

/// The states of an intent that is not settled yet, as its column spells
/// them.
pub(crate) fn unsettled_states() -> Vec<&'static str> {
    IntentState::ALL
        .into_iter()
        .filter(|state| !state.is_terminal())
        .map(IntentState::as_str)
        .collect()
}
