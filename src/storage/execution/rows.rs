//! The row a record is written as: every column its document decides,
//! worked out here and nowhere else, so a row's columns always say what
//! its document says, and a read can hold a stored row to that.

use super::columns::{
    Named, number, optional, optional_number, pool_text, scope_columns, slot, text, time,
};
use super::tables::table_of;
use crate::model::execution::{Halt, Record};
use crate::model::futures::IntentKind;
use crate::ports::StorageError;

/// Every column of `record`'s row that its document decides, the document
/// itself among them.
pub(crate) fn derived(record: &Record) -> Result<Vec<Named>, StorageError> {
    let unreadable = |error| StorageError::InvalidArgument(format!("{error}"));
    let document = text(record.to_stored().map_err(unreadable)?);
    let table = table_of(record.kind());
    let mut columns = match record {
        Record::Signal(signal) => {
            let mut columns = scope_columns(signal.scope());
            columns.extend([
                ("signal_id", text(signal.id())),
                ("payload_hash", text(signal.payload_hash())),
                ("state", text(signal.state())),
                (
                    "refusal",
                    optional(signal.refusal().map(|refused| refused.refusal)),
                ),
                ("received_at_ms", time(Some(signal.received_at_ms()))),
                ("judged_at_ms", time(signal.judged_at_ms())),
            ]);
            columns
        }
        Record::Operation(operation) => {
            let mut columns = scope_columns(operation.scope());
            columns.extend([
                ("operation_id", number(operation.id().get())?),
                ("kind", text(operation.kind())),
                ("operation_key", optional(operation.key())),
                ("symbol", text(&operation.instrument().symbol)),
                ("signal_id", optional(operation.signal())),
                (
                    "cycle_id",
                    optional_number(operation.cycle().map(|cycle| cycle.get()))?,
                ),
                (
                    "replace_state",
                    optional(operation.replacement().map(|replaced| replaced.state())),
                ),
            ]);
            columns
        }
        Record::Intent(intent) => {
            let (kind, side) = match intent.intent().kind() {
                IntentKind::Entry(entry) => ("entry", entry.side),
                IntentKind::Exit(exit) => ("exit", exit.side),
            };
            let mut columns = scope_columns(intent.scope());
            columns.extend([
                ("signal_id", text(intent.signal())),
                ("kind", text(kind)),
                ("symbol", text(&intent.key().instrument().symbol)),
                ("side", text(side)),
                ("state", text(intent.intent().state())),
                ("entry_slot", slot(intent.blocks_entries())),
                (
                    "operation_id",
                    optional_number(intent.operation().map(|operation| operation.get()))?,
                ),
                (
                    "cycle_id",
                    optional_number(intent.cycle().map(|cycle| cycle.get()))?,
                ),
            ]);
            columns
        }
        Record::Order(order) => {
            let mut columns = scope_columns(order.scope());
            columns.extend([
                ("operation_id", number(order.operation().get())?),
                ("client_order_id", text(order.client_order_id())),
                ("symbol", text(&order.instrument().symbol)),
                ("venue_order_id", optional(order.venue_order_id())),
                ("phase", text(order.phase())),
                ("recorded_at_ms", time(Some(order.recorded_at_ms()))),
            ]);
            columns
        }
        Record::Fill(fill) => {
            let venue = fill.fill();
            let mut columns = scope_columns(fill.scope());
            columns.extend([
                ("symbol", text(&venue.instrument.symbol)),
                ("trade_id", text(&venue.trade_id)),
                ("venue_order_id", text(&venue.venue_order_id)),
                (
                    "operation_id",
                    optional_number(fill.operation().map(|operation| operation.get()))?,
                ),
                (
                    "cycle_id",
                    optional_number(fill.cycle().map(|cycle| cycle.get()))?,
                ),
                ("time_ms", time(Some(venue.time_ms))),
            ]);
            columns
        }
        Record::Cycle(cycle) => {
            let held = cycle.cycle();
            let mut columns = scope_columns(cycle.scope());
            columns.extend([
                ("cycle_id", number(cycle.id().get())?),
                ("symbol", text(&cycle.key().instrument().symbol)),
                ("side", text(held.side())),
                ("protection", text(held.protection())),
                ("open_slot", slot(cycle.is_open())),
                ("opened_at_ms", time(Some(held.opened_at_ms()))),
                ("closed_at_ms", time(held.closed_at_ms())),
            ]);
            columns
        }
        Record::Reservation(held) => {
            let mut columns = scope_columns(held.scope());
            columns.extend([
                ("operation_id", number(held.operation().get())?),
                ("symbol", text(&held.instrument().symbol)),
                ("pool", text(pool_text(held.pool()))),
                ("state", text(held.state())),
                ("reserved_at_ms", time(Some(held.reserved_at_ms()))),
                (
                    "released_at_ms",
                    time(held.released().map(|released| released.at_ms)),
                ),
            ]);
            columns
        }
        Record::Incident(incident) => {
            let symbol = match incident.halts() {
                Halt::Position(instrument) => Some(&instrument.symbol),
                Halt::Profile => None,
            };
            let mut columns = scope_columns(incident.scope());
            columns.extend([
                ("incident_key", text(incident.key())),
                ("kind", text(incident.kind())),
                ("symbol", optional(symbol)),
                ("state", text(incident.state())),
                ("opened_at_ms", time(Some(incident.opened_at_ms()))),
                (
                    "cleared_at_ms",
                    time(incident.cleared().map(|cleared| cleared.at_ms)),
                ),
            ]);
            columns
        }
        Record::Event(event) => {
            let mut columns = scope_columns(event.scope());
            columns.extend([
                ("event_key", text(event.key())),
                ("kind", text(event.kind())),
                ("delivery", text(event.delivery())),
                ("created_at_ms", time(Some(event.created_at_ms()))),
                ("delivery_at_ms", time(event.delivery_at_ms())),
            ]);
            columns
        }
    };
    columns.push((table.document, document));
    Ok(columns)
}
