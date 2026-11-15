//! The execution tables as both backends write them: which table a record
//! lives in, the columns it is found by, and the row its document gives.
//! The row itself, every column its document decides, is `rows`'s.

use crate::model::execution::RecordKind;
use crate::model::{
    EXECUTION_FILLS, EXECUTION_INCIDENTS, EXECUTION_INTENTS, EXECUTION_OPERATIONS,
    EXECUTION_ORDERS, MARGIN_RESERVATIONS, OUTBOX_EVENTS, POSITION_CYCLES, SIGNAL_DELIVERIES,
    SIGNAL_INBOX, TRANSFER_TABLES, TimestampMs, TransferTable, TransferValue,
};
use crate::ports::StorageError;

pub(crate) use super::columns::{
    KIND_KEY_FINGERPRINT, KIND_VENUE, Named, PoolKey, delivery_row, number, position_columns,
    scope_columns, unsettled_states,
};

/// The account's columns, which every natural key starts with.
const ACCOUNT: [&str; 5] = [
    "venue",
    "category",
    "environment",
    "account_kind",
    "account_id",
];

/// One execution table.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Table {
    /// The record it holds, or `None` for the deliveries, which are notes.
    pub(crate) kind: Option<RecordKind>,
    pub(crate) spec: &'static TransferTable,
    /// The columns a record is found by: unique together.
    pub(crate) key: &'static [&'static str],
    /// The column the store numbers when a row is first written.
    pub(crate) numbered: Option<&'static str>,
    pub(crate) document: &'static str,
    /// Whether rows are replaced, each time at the next revision.
    pub(crate) revised: bool,
    /// Columns the unit's time fills: when a row was first written
    /// (`created`), and when it was last (`updated`).
    pub(crate) created: Option<&'static str>,
    pub(crate) updated: Option<&'static str>,
}

const fn table(
    kind: RecordKind,
    spec: &'static TransferTable,
    key: &'static [&'static str],
    document: &'static str,
) -> Table {
    Table {
        kind: Some(kind),
        spec,
        key,
        numbered: Some("seq"),
        document,
        revised: true,
        created: None,
        updated: None,
    }
}

pub(crate) const SIGNALS: Table = table(
    RecordKind::Signal,
    &SIGNAL_INBOX,
    &[
        "venue",
        "category",
        "environment",
        "account_kind",
        "account_id",
        "profile_id",
        "signal_id",
    ],
    "signal_json",
);

pub(crate) const DELIVERIES: Table = Table {
    kind: None,
    spec: &SIGNAL_DELIVERIES,
    key: &["seq"],
    numbered: Some("seq"),
    document: "payload",
    revised: false,
    created: None,
    updated: None,
};

pub(crate) const OPERATIONS: Table = Table {
    numbered: Some("operation_id"),
    created: Some("created_at_ms"),
    updated: Some("updated_at_ms"),
    ..table(
        RecordKind::Operation,
        &EXECUTION_OPERATIONS,
        &["operation_id"],
        "operation_json",
    )
};

pub(crate) const INTENTS: Table = Table {
    created: Some("created_at_ms"),
    updated: Some("updated_at_ms"),
    ..table(
        RecordKind::Intent,
        &EXECUTION_INTENTS,
        &[
            "venue",
            "category",
            "environment",
            "account_kind",
            "account_id",
            "profile_id",
            "signal_id",
        ],
        "intent_json",
    )
};

pub(crate) const ORDERS: Table = Table {
    updated: Some("updated_at_ms"),
    ..table(
        RecordKind::Order,
        &EXECUTION_ORDERS,
        &[
            "venue",
            "category",
            "environment",
            "account_kind",
            "account_id",
            "client_order_id",
        ],
        "order_json",
    )
};

pub(crate) const FILLS: Table = Table {
    revised: false,
    created: Some("recorded_at_ms"),
    ..table(
        RecordKind::Fill,
        &EXECUTION_FILLS,
        &[
            "venue",
            "category",
            "environment",
            "account_kind",
            "account_id",
            "symbol",
            "trade_id",
        ],
        "fill_json",
    )
};

pub(crate) const CYCLES: Table = Table {
    numbered: None,
    updated: Some("updated_at_ms"),
    ..table(
        RecordKind::Cycle,
        &POSITION_CYCLES,
        &["cycle_id"],
        "cycle_json",
    )
};

pub(crate) const RESERVATIONS: Table = table(
    RecordKind::Reservation,
    &MARGIN_RESERVATIONS,
    &["operation_id"],
    "reservation_json",
);

pub(crate) const INCIDENTS: Table = table(
    RecordKind::Incident,
    &EXECUTION_INCIDENTS,
    &[
        "venue",
        "category",
        "environment",
        "account_kind",
        "account_id",
        "incident_key",
    ],
    "incident_json",
);

pub(crate) const EVENTS: Table = table(
    RecordKind::Event,
    &OUTBOX_EVENTS,
    &[
        "venue",
        "category",
        "environment",
        "account_kind",
        "account_id",
        "event_key",
    ],
    "event_json",
);

/// The table a kind of record lives in.
pub(crate) const fn table_of(kind: RecordKind) -> &'static Table {
    match kind {
        RecordKind::Signal => &SIGNALS,
        RecordKind::Operation => &OPERATIONS,
        RecordKind::Intent => &INTENTS,
        RecordKind::Order => &ORDERS,
        RecordKind::Fill => &FILLS,
        RecordKind::Cycle => &CYCLES,
        RecordKind::Reservation => &RESERVATIONS,
        RecordKind::Incident => &INCIDENTS,
        RecordKind::Event => &EVENTS,
    }
}

impl Table {
    pub(crate) fn name(&self) -> &'static str {
        self.spec.name
    }

    /// Where the table comes in the order a copy takes its tables, which
    /// is the order every writer locks them in.
    pub(crate) fn rank(&self) -> usize {
        TRANSFER_TABLES
            .iter()
            .position(|table| table.name == self.spec.name)
            .unwrap_or(TRANSFER_TABLES.len())
    }

    /// Where `column` is in a row of the table.
    pub(crate) fn index(&self, column: &str) -> Result<usize, StorageError> {
        self.spec.column_index(column).ok_or_else(|| {
            StorageError::InvalidArgument(format!("{} has no column {column}", self.name()))
        })
    }

    /// Whether a unit's time, and not the document, fills `column`.
    fn stamped(&self, column: &str) -> bool {
        self.created == Some(column) || self.updated == Some(column)
    }

    /// Whether the store, and not the document, decides `column`.
    pub(crate) fn kept_by_store(&self, column: &str) -> bool {
        self.numbered == Some(column) && self.key != [column]
            || column == "revision"
            || self.stamped(column)
    }

    /// A whole row, in the order of the table's columns: `columns` where
    /// the document decides, the revision, and `at_ms` where the unit's
    /// time is kept. A column the store numbers is left NULL.
    pub(crate) fn row(
        &self,
        columns: &[Named],
        revision: u64,
        at_ms: TimestampMs,
    ) -> Result<Vec<TransferValue>, StorageError> {
        self.spec
            .columns
            .iter()
            .map(|column| {
                let given = columns.iter().find(|(name, _)| *name == column.name);
                match given {
                    Some((_, value)) => Ok(value.clone()),
                    None if column.name == "revision" => number(revision),
                    None if self.stamped(column.name) => Ok(TransferValue::Integer(at_ms)),
                    None if self.numbered == Some(column.name) => Ok(TransferValue::Null),
                    None => Err(StorageError::InvalidArgument(format!(
                        "nothing decides {}.{}",
                        self.name(),
                        column.name
                    ))),
                }
            })
            .collect()
    }

    /// The values `columns` gives the table's natural key.
    pub(crate) fn key_of(&self, columns: &[Named]) -> Result<Vec<Named>, StorageError> {
        self.key
            .iter()
            .map(|name| {
                columns
                    .iter()
                    .find(|(column, _)| column == name)
                    .cloned()
                    .ok_or_else(|| {
                        StorageError::InvalidArgument(format!(
                            "{} is found by {name}, which the record does not give",
                            self.name()
                        ))
                    })
            })
            .collect()
    }
}

/// The account columns, for a key that starts with them.
pub(crate) const fn account_key() -> [&'static str; 5] {
    ACCOUNT
}
