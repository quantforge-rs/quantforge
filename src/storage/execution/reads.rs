//! The reads of [`crate::ports::ExecutionStore`], written once over a
//! backend's rows: each names the rows it wants, and every row it is
//! handed is checked as [`decode`] checks it.

use super::read::{Query, RawRow, Rows, decode, delivery, unreadable};
use super::tables::{
    self, CYCLES, DELIVERIES, EVENTS, FILLS, INCIDENTS, INTENTS, Named, OPERATIONS, ORDERS,
    PoolKey, RESERVATIONS, SIGNALS, Table,
};
use crate::model::execution::{
    CycleRecord, DeliveryState, EventKey, ExecutionScope, FillRecord, Incident, IncidentKey,
    IncidentState, IntentRecord, OperationRecord, OrderRecord, OutboxEvent, PoolReservations,
    Record, Reservation, ReservationState, SignalDelivery, SignalRecord, SignalState,
    SpotReplaceState, Stored,
};
use crate::model::futures::{PoolScope, PositionCycleId, PositionKey, SignalId};
use crate::model::{AccountRef, InstrumentId, OperationId, TransferValue, VenueTradeId};
use crate::ports::StorageError;

fn many<T>(
    rows: Vec<RawRow>,
    table: &'static Table,
    pick: impl Fn(Record) -> Option<T>,
) -> Result<Vec<Stored<T>>, StorageError> {
    rows.iter()
        .map(|row| {
            let found = decode(table, row)?;
            let revision = found.revision;
            pick(found.record)
                .map(|record| Stored { record, revision })
                .ok_or_else(|| unreadable(table, "it holds another kind of record"))
        })
        .collect()
}

fn one<T>(
    rows: Vec<RawRow>,
    table: &'static Table,
    pick: impl Fn(Record) -> Option<T>,
) -> Result<Option<Stored<T>>, StorageError> {
    Ok(many(rows, table, pick)?.into_iter().next())
}

/// Picks one kind of record out of [`Record`].
macro_rules! pick {
    ($variant:ident) => {
        |record| match record {
            Record::$variant(record) => Some(record),
            _ => None,
        }
    };
}

fn text(value: impl ToString) -> TransferValue {
    TransferValue::Text(value.to_string())
}

/// The row of `table` that `scope` and one more column name.
fn keyed(
    scope: &ExecutionScope,
    table: &'static Table,
    last: Named,
) -> Result<Query, StorageError> {
    let mut columns = tables::scope_columns(scope);
    columns.push(last);
    Ok(Query::of(table, table.key_of(&columns)?))
}

fn numbered(column: &'static str, number: u64) -> Result<Vec<Named>, StorageError> {
    Ok(vec![(column, tables::number(number)?)])
}

pub(crate) fn signal(
    store: &impl Rows,
    scope: &ExecutionScope,
    signal: &SignalId,
) -> Result<Option<Stored<SignalRecord>>, StorageError> {
    let query = keyed(scope, &SIGNALS, ("signal_id", text(signal)))?;
    one(store.rows(&query)?, &SIGNALS, pick!(Signal))
}

pub(crate) fn received_signals(
    store: &impl Rows,
    limit: usize,
) -> Result<Vec<Stored<SignalRecord>>, StorageError> {
    let received = vec![("state", text(SignalState::Received))];
    let query = Query::of(&SIGNALS, received).first(limit);
    many(store.rows(&query)?, &SIGNALS, pick!(Signal))
}

pub(crate) fn deliveries(
    store: &impl Rows,
    scope: &ExecutionScope,
    signal: &SignalId,
) -> Result<Vec<SignalDelivery>, StorageError> {
    let mut equals = tables::scope_columns(scope);
    equals.push(("signal_id", text(signal)));
    let rows = store.rows(&Query::of(&DELIVERIES, equals))?;
    rows.iter().map(delivery).collect()
}

pub(crate) fn intent(
    store: &impl Rows,
    scope: &ExecutionScope,
    signal: &SignalId,
) -> Result<Option<Stored<IntentRecord>>, StorageError> {
    let query = keyed(scope, &INTENTS, ("signal_id", text(signal)))?;
    one(store.rows(&query)?, &INTENTS, pick!(Intent))
}

pub(crate) fn position_intents(
    store: &impl Rows,
    key: &PositionKey,
) -> Result<Vec<Stored<IntentRecord>>, StorageError> {
    let query = Query::of(&INTENTS, tables::position_columns(key))
        .in_states("state", tables::unsettled_states());
    many(store.rows(&query)?, &INTENTS, pick!(Intent))
}

pub(crate) fn unsettled_intents(
    store: &impl Rows,
) -> Result<Vec<Stored<IntentRecord>>, StorageError> {
    let query = Query::of(&INTENTS, Vec::new()).in_states("state", tables::unsettled_states());
    many(store.rows(&query)?, &INTENTS, pick!(Intent))
}

pub(crate) fn operation(
    store: &impl Rows,
    id: OperationId,
) -> Result<Option<Stored<OperationRecord>>, StorageError> {
    let query = Query::of(&OPERATIONS, numbered("operation_id", id.get())?);
    one(store.rows(&query)?, &OPERATIONS, pick!(Operation))
}

pub(crate) fn unsettled_replacements(
    store: &impl Rows,
) -> Result<Vec<Stored<OperationRecord>>, StorageError> {
    let open = SpotReplaceState::ALL
        .into_iter()
        .filter(|state| !state.is_terminal())
        .map(SpotReplaceState::as_str)
        .collect();
    let query = Query::of(&OPERATIONS, Vec::new()).in_states("replace_state", open);
    many(store.rows(&query)?, &OPERATIONS, pick!(Operation))
}

pub(crate) fn order(
    store: &impl Rows,
    scope: &ExecutionScope,
    client_order_id: &str,
) -> Result<Option<Stored<OrderRecord>>, StorageError> {
    let query = keyed(scope, &ORDERS, ("client_order_id", text(client_order_id)))?;
    one(store.rows(&query)?, &ORDERS, pick!(Order))
}

pub(crate) fn orders(
    store: &impl Rows,
    operation: OperationId,
) -> Result<Vec<Stored<OrderRecord>>, StorageError> {
    let query = Query::of(&ORDERS, numbered("operation_id", operation.get())?);
    many(store.rows(&query)?, &ORDERS, pick!(Order))
}

pub(crate) fn fill(
    store: &impl Rows,
    scope: &ExecutionScope,
    instrument: &InstrumentId,
    trade: &VenueTradeId,
) -> Result<Option<FillRecord>, StorageError> {
    let mut columns = tables::scope_columns(scope);
    columns.push(("symbol", text(&instrument.symbol)));
    columns.push(("trade_id", text(trade)));
    let query = Query::of(&FILLS, FILLS.key_of(&columns)?);
    Ok(one(store.rows(&query)?, &FILLS, pick!(Fill))?.map(|stored| stored.record))
}

pub(crate) fn fills(
    store: &impl Rows,
    cycle: PositionCycleId,
) -> Result<Vec<FillRecord>, StorageError> {
    let query = Query::of(&FILLS, numbered("cycle_id", cycle.get())?);
    let found = many(store.rows(&query)?, &FILLS, pick!(Fill))?;
    Ok(found.into_iter().map(|stored| stored.record).collect())
}

pub(crate) fn cycle(
    store: &impl Rows,
    id: PositionCycleId,
) -> Result<Option<Stored<CycleRecord>>, StorageError> {
    let query = Query::of(&CYCLES, numbered("cycle_id", id.get())?);
    one(store.rows(&query)?, &CYCLES, pick!(Cycle))
}

pub(crate) fn open_cycle(
    store: &impl Rows,
    key: &PositionKey,
) -> Result<Option<Stored<CycleRecord>>, StorageError> {
    let mut equals = tables::position_columns(key);
    equals.push(("open_slot", TransferValue::Integer(1)));
    one(
        store.rows(&Query::of(&CYCLES, equals))?,
        &CYCLES,
        pick!(Cycle),
    )
}

pub(crate) fn open_cycles(store: &impl Rows) -> Result<Vec<Stored<CycleRecord>>, StorageError> {
    let open = vec![("open_slot", TransferValue::Integer(1))];
    many(
        store.rows(&Query::of(&CYCLES, open))?,
        &CYCLES,
        pick!(Cycle),
    )
}

pub(crate) fn pool(
    store: &impl Rows,
    account: &AccountRef,
    pool: &PoolScope,
) -> Result<PoolReservations, StorageError> {
    let key = PoolKey::of(account, pool);
    let mut equals = key.columns();
    equals.push(("state", text(ReservationState::Held)));
    let (revision, rows) = store.pool_rows(&key, &Query::of(&RESERVATIONS, equals))?;
    let held = many(rows, &RESERVATIONS, pick!(Reservation))?;
    Ok(PoolReservations {
        revision,
        held: held.into_iter().map(|stored| stored.record).collect(),
    })
}

pub(crate) fn reservation(
    store: &impl Rows,
    operation: OperationId,
) -> Result<Option<Stored<Reservation>>, StorageError> {
    let query = Query::of(&RESERVATIONS, numbered("operation_id", operation.get())?);
    one(store.rows(&query)?, &RESERVATIONS, pick!(Reservation))
}

pub(crate) fn incident(
    store: &impl Rows,
    scope: &ExecutionScope,
    key: &IncidentKey,
) -> Result<Option<Stored<Incident>>, StorageError> {
    let query = keyed(scope, &INCIDENTS, ("incident_key", text(key)))?;
    one(store.rows(&query)?, &INCIDENTS, pick!(Incident))
}

pub(crate) fn open_incidents(store: &impl Rows) -> Result<Vec<Stored<Incident>>, StorageError> {
    let open = vec![("state", text(IncidentState::Open))];
    many(
        store.rows(&Query::of(&INCIDENTS, open))?,
        &INCIDENTS,
        pick!(Incident),
    )
}

pub(crate) fn event(
    store: &impl Rows,
    scope: &ExecutionScope,
    key: &EventKey,
) -> Result<Option<Stored<OutboxEvent>>, StorageError> {
    let query = keyed(scope, &EVENTS, ("event_key", text(key)))?;
    one(store.rows(&query)?, &EVENTS, pick!(Event))
}

pub(crate) fn pending_events(
    store: &impl Rows,
    limit: usize,
) -> Result<Vec<Stored<OutboxEvent>>, StorageError> {
    let pending = vec![("delivery", text(DeliveryState::Pending))];
    let query = Query::of(&EVENTS, pending).first(limit);
    many(store.rows(&query)?, &EVENTS, pick!(Event))
}
