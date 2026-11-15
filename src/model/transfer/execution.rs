//! The execution store's tables, as both backends' DDL declare them: the
//! signal inbox, the operations, the ledger and the outbox. Every row
//! carries its scope first: the venue, the product family, the
//! environment, the account and, where one acts, the profile.

use super::tables::{TransferColumn, TransferTable, integer, text};

/// The scope columns every execution row starts with, after its number.
macro_rules! scoped {
    ($key:expr, $($rest:expr),* $(,)?) => {
        &[
            $key,
            text("venue"),
            text("category"),
            text("environment"),
            text("account_kind"),
            text("account_id"),
            text("profile_id"),
            $($rest),*
        ]
    };
}

/// A table of documents, numbered by the store: a row holds a whole
/// record, so a batch holds few.
const fn documents(
    name: &'static str,
    key: &'static str,
    columns: &'static [TransferColumn],
) -> TransferTable {
    TransferTable {
        name,
        columns,
        key,
        by_market: false,
        numbered: true,
        max_batch: Some(256),
    }
}

/// The signals each profile was sent, in the order they first arrived.
pub const SIGNAL_INBOX: TransferTable = documents(
    "signal_inbox",
    "seq",
    scoped![
        integer("seq"),
        text("signal_id"),
        text("payload_hash"),
        text("state"),
        text("refusal"),
        integer("revision"),
        text("signal_json"),
        integer("received_at_ms"),
        integer("judged_at_ms"),
    ],
);

/// Every delivery of every signal, in the order they arrived.
pub const SIGNAL_DELIVERIES: TransferTable = documents(
    "signal_deliveries",
    "seq",
    scoped![
        integer("seq"),
        text("signal_id"),
        text("disposition"),
        text("payload_hash"),
        text("payload"),
        integer("fire_time_ms"),
        integer("received_at_ms"),
    ],
);

/// The operations, by the numbers the store gave them.
pub const EXECUTION_OPERATIONS: TransferTable = documents(
    "execution_operations",
    "operation_id",
    scoped![
        integer("operation_id"),
        text("kind"),
        text("operation_key"),
        text("symbol"),
        text("signal_id"),
        integer("cycle_id"),
        text("replace_state"),
        integer("revision"),
        text("operation_json"),
        integer("created_at_ms"),
        integer("updated_at_ms"),
    ],
);

/// The intents of admitted signals, in the order they were admitted.
pub const EXECUTION_INTENTS: TransferTable = documents(
    "execution_intents",
    "seq",
    scoped![
        integer("seq"),
        text("signal_id"),
        text("kind"),
        text("symbol"),
        text("side"),
        text("state"),
        integer("entry_slot"),
        integer("operation_id"),
        integer("cycle_id"),
        integer("revision"),
        text("intent_json"),
        integer("created_at_ms"),
        integer("updated_at_ms"),
    ],
);

/// The orders recorded before they were sent, in that order.
pub const EXECUTION_ORDERS: TransferTable = documents(
    "execution_orders",
    "seq",
    scoped![
        integer("seq"),
        integer("operation_id"),
        text("client_order_id"),
        text("symbol"),
        text("venue_order_id"),
        text("phase"),
        integer("revision"),
        text("order_json"),
        integer("recorded_at_ms"),
        integer("updated_at_ms"),
    ],
);

/// The fills, in the order they were recorded.
pub const EXECUTION_FILLS: TransferTable = documents(
    "execution_fills",
    "seq",
    scoped![
        integer("seq"),
        text("symbol"),
        text("trade_id"),
        text("venue_order_id"),
        integer("operation_id"),
        integer("cycle_id"),
        text("fill_json"),
        integer("time_ms"),
        integer("recorded_at_ms"),
    ],
);

/// The position cycles, by their numbers: each the number of the
/// operation that opened it, so the store numbers none of its own.
pub const POSITION_CYCLES: TransferTable = TransferTable {
    numbered: false,
    ..documents(
        "position_cycles",
        "cycle_id",
        scoped![
            integer("cycle_id"),
            text("symbol"),
            text("side"),
            text("protection"),
            integer("open_slot"),
            integer("revision"),
            text("cycle_json"),
            integer("opened_at_ms"),
            integer("closed_at_ms"),
            integer("updated_at_ms"),
        ],
    )
};

/// The reservations, in the order they were made.
pub const MARGIN_RESERVATIONS: TransferTable = documents(
    "margin_reservations",
    "seq",
    scoped![
        integer("seq"),
        integer("operation_id"),
        text("symbol"),
        text("pool"),
        text("state"),
        integer("revision"),
        text("reservation_json"),
        integer("reserved_at_ms"),
        integer("released_at_ms"),
    ],
);

/// The incidents, in the order they were raised.
pub const EXECUTION_INCIDENTS: TransferTable = documents(
    "execution_incidents",
    "seq",
    scoped![
        integer("seq"),
        text("incident_key"),
        text("kind"),
        text("symbol"),
        text("state"),
        integer("revision"),
        text("incident_json"),
        integer("opened_at_ms"),
        integer("cleared_at_ms"),
    ],
);

/// The outbox, in the order its events were created: the order a notifier
/// delivers them in.
pub const OUTBOX_EVENTS: TransferTable = documents(
    "outbox_events",
    "seq",
    scoped![
        integer("seq"),
        text("event_key"),
        text("kind"),
        text("delivery"),
        integer("revision"),
        text("event_json"),
        integer("created_at_ms"),
        integer("delivery_at_ms"),
    ],
);
