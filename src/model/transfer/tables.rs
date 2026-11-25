//! The tables a transfer copies, as both backends' DDL declare them, in
//! the order it copies them, and the tables each store keeps for itself.

use super::TRANSFER_FORMAT;
use super::execution::{
    EXECUTION_FILLS, EXECUTION_INCIDENTS, EXECUTION_INTENTS, EXECUTION_OPERATIONS,
    EXECUTION_ORDERS, MARGIN_RESERVATIONS, OUTBOX_EVENTS, POSITION_CYCLES, SIGNAL_DELIVERIES,
    SIGNAL_INBOX,
};
use super::jobs::{BUNDLES, JOB_LOGS, JOB_PROVENANCE, JOB_RESULTS, JOBS};

/// What a column holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColumnKind {
    Integer,
    Text,
    Blob,
}

/// A column of a transferred table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransferColumn {
    pub name: &'static str,
    pub kind: ColumnKind,
}

pub(super) const fn integer(name: &'static str) -> TransferColumn {
    TransferColumn {
        name,
        kind: ColumnKind::Integer,
    }
}

pub(super) const fn text(name: &'static str) -> TransferColumn {
    TransferColumn {
        name,
        kind: ColumnKind::Text,
    }
}

pub(super) const fn blob(name: &'static str) -> TransferColumn {
    TransferColumn {
        name,
        kind: ColumnKind::Blob,
    }
}

/// A table a transfer copies, as both backends' DDL declare it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransferTable {
    pub name: &'static str,
    /// Every column, in the order of the DDL.
    pub columns: &'static [TransferColumn],
    /// The column rows are read in order of: an integer, or text compared
    /// byte by byte. With the market, when there is one, it is the key.
    pub key: &'static str,
    /// Whether rows are read market by market, a market being the first
    /// three columns: `exchange`, `symbol` and `interval`.
    pub by_market: bool,
    /// Whether the store numbers the key itself, as `AUTOINCREMENT` and an
    /// identity column do: a copy keeps the numbers, and moves the target's
    /// numbering past them.
    pub numbered: bool,
    /// The most rows of it a batch holds, when its rows are large.
    pub max_batch: Option<usize>,
}

impl TransferTable {
    /// Where the key is in a row. Every table's key is one of its columns,
    /// as a unit test holds them.
    pub fn key_index(&self) -> usize {
        self.column_index(self.key).unwrap_or_default()
    }

    /// Where `name` is in a row, if it is a column of the table.
    pub fn column_index(&self, name: &str) -> Option<usize> {
        self.columns.iter().position(|column| column.name == name)
    }

    /// The kind of its key.
    pub fn key_kind(&self) -> ColumnKind {
        self.columns[self.key_index()].kind
    }

    /// How many rows a batch of it holds when `asked` were asked for.
    pub fn batch_rows(&self, asked: usize) -> usize {
        let asked = asked.max(1);
        self.max_batch.map_or(asked, |most| asked.min(most))
    }

    /// The header its canonical text starts with: `quantforge transfer`, the
    /// format, the table's name and its columns joined by commas, one line
    /// each.
    pub fn canonical_header(&self) -> Vec<u8> {
        let columns: Vec<&str> = self.columns.iter().map(|column| column.name).collect();
        format!(
            "quantforge transfer {TRANSFER_FORMAT}\n{}\n{}\n",
            self.name,
            columns.join(",")
        )
        .into_bytes()
    }
}

/// The candles, read market by market.
pub const CANDLES: TransferTable = TransferTable {
    name: "candles",
    columns: &[
        text("exchange"),
        text("symbol"),
        text("interval"),
        integer("open_time_ms"),
        integer("close_time_ms"),
        text("open"),
        text("high"),
        text("low"),
        text("close"),
        text("volume"),
        integer("trades"),
    ],
    key: "open_time_ms",
    by_market: true,
    numbered: false,
    max_batch: None,
};

/// The runs, by their IDs in byte order.
pub const BOT_RUNS: TransferTable = TransferTable {
    name: "bot_runs",
    columns: &[
        text("run_id"),
        text("exchange"),
        text("symbol"),
        text("interval"),
        text("strategy_name"),
        text("status"),
        text("state_json"),
        integer("started_at_ms"),
        integer("updated_at_ms"),
        integer("stopped_at_ms"),
        text("last_error"),
        text("environment"),
        text("account_kind"),
        text("account_id"),
        text("identity_source"),
        integer("identity_at_ms"),
        text("legacy_state_json"),
    ],
    key: "run_id",
    by_market: false,
    numbered: false,
    max_batch: None,
};

/// The order events, by the sequence number the store gave each.
pub const ORDER_EVENTS: TransferTable = TransferTable {
    name: "order_events",
    columns: &[
        integer("seq"),
        text("run_id"),
        text("symbol"),
        text("side"),
        text("order_type"),
        text("status"),
        text("order_id"),
        text("client_order_id"),
        integer("transact_time_ms"),
        text("raw_json"),
        text("legacy_raw_json"),
    ],
    key: "seq",
    by_market: false,
    numbered: true,
    max_batch: None,
};

/// The closed trades, by the sequence number the store gave each.
pub const CLOSED_TRADES: TransferTable = TransferTable {
    name: "closed_trades",
    columns: &[
        integer("seq"),
        text("run_id"),
        text("symbol"),
        integer("entry_time_ms"),
        integer("exit_time_ms"),
        text("entry_price"),
        text("exit_price"),
        text("qty"),
        text("gross_quote_pnl"),
        text("entry_order_id"),
        text("exit_order_id"),
    ],
    key: "seq",
    by_market: false,
    numbered: true,
    max_batch: None,
};

/// The segments' bytes, by their names: every one stored, a replaced one
/// too. A batch holds at most 64 of them.
pub const SEGMENTS: TransferTable = TransferTable {
    name: "segments",
    columns: &[
        text("hash"),
        integer("format"),
        integer("payload_len"),
        blob("data"),
    ],
    key: "hash",
    by_market: false,
    numbered: false,
    max_batch: Some(64),
};

/// The segment index, read market by market.
pub const SEGMENT_INDEX: TransferTable = TransferTable {
    name: "segment_index",
    columns: &[
        text("exchange"),
        text("symbol"),
        text("interval"),
        integer("window_start_ms"),
        integer("first_open_ms"),
        integer("last_open_ms"),
        integer("bar_count"),
        text("hash"),
    ],
    key: "window_start_ms",
    by_market: true,
    numbered: false,
    max_batch: None,
};

/// The dirty marks, by their revisions, kept as they are.
pub const SEGMENT_DIRTY: TransferTable = TransferTable {
    name: "segment_dirty",
    columns: &[
        integer("id"),
        text("exchange"),
        text("symbol"),
        text("interval"),
        integer("window_start_ms"),
    ],
    key: "id",
    by_market: false,
    numbered: true,
    max_batch: None,
};

/// The automation profiles, by their IDs in byte order.
pub const AUTOMATION_PROFILES: TransferTable = TransferTable {
    name: "automation_profiles",
    columns: &[
        text("profile_id"),
        text("venue"),
        text("environment"),
        text("account_kind"),
        text("account_id"),
        text("category"),
        text("owner_tag"),
        integer("created_at_ms"),
    ],
    key: "profile_id",
    by_market: false,
    numbered: false,
    max_batch: None,
};

/// Every revision of the profiles' policies, in the order they were made.
/// A row holds a whole policy, so a batch holds few.
pub const POLICY_REVISIONS: TransferTable = TransferTable {
    name: "policy_revisions",
    columns: &[
        integer("seq"),
        text("profile_id"),
        integer("revision"),
        integer("changed_at_ms"),
        text("policy_json"),
    ],
    key: "seq",
    by_market: false,
    numbered: true,
    max_batch: Some(64),
};

/// The tables a transfer copies, in the order it copies them: a run before
/// its journals, a segment before the index entry naming it, candles before
/// the marks on their windows, a bundle before the jobs that run it, a
/// result before the job that lists it, a job before its log and its
/// provenance, every segment before the provenance naming it, a profile
/// before the revisions of its policy, a signal before its deliveries and
/// its intent, an operation before its orders, its fills, the cycle it
/// opened and its reservation, and every record before the events that
/// tell of it. Each part of a target is whole when read.
pub const TRANSFER_TABLES: [TransferTable; 24] = [
    CANDLES,
    BOT_RUNS,
    ORDER_EVENTS,
    CLOSED_TRADES,
    SEGMENTS,
    SEGMENT_INDEX,
    SEGMENT_DIRTY,
    BUNDLES,
    JOB_RESULTS,
    JOBS,
    JOB_LOGS,
    JOB_PROVENANCE,
    AUTOMATION_PROFILES,
    POLICY_REVISIONS,
    SIGNAL_INBOX,
    SIGNAL_DELIVERIES,
    EXECUTION_OPERATIONS,
    EXECUTION_INTENTS,
    EXECUTION_ORDERS,
    EXECUTION_FILLS,
    POSITION_CYCLES,
    MARGIN_RESERVATIONS,
    EXECUTION_INCIDENTS,
    OUTBOX_EVENTS,
];

/// The tables every store keeps for itself, which a transfer never copies:
/// a target has its own schema version, its own migration log, and its own
/// record, with an ID of its own, inactive. The source's environment is
/// carried over on its own, never its record. A store's leases, the
/// revisions of its margin pools and the log of its activations are its
/// own as well: a lease is authority to act, which no copy confers.
pub const KEPT_BY_EACH_STORE: [&str; 6] = [
    "meta",
    "store_control",
    "schema_migrations",
    "position_leases",
    "reservation_pools",
    "store_control_events",
];

#[cfg(test)]
mod tests;
