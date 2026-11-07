//! The engine against stores held in memory: what a copy writes and
//! refuses, how it resumes (`copy`), and what a verification finds
//! (`verify`). [`Memory`] is a store keyed as the real ones are.

mod copy;
mod verify;

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use super::*;
use crate::model::futures::ActivationEpoch;
use crate::model::transfer::{BOT_RUNS, CANDLES, ORDER_EVENTS, SEGMENT_DIRTY};
use crate::model::{ColumnKind, Environment, StoreId, TRANSFER_SCHEMA, TransferMarket};
use crate::ports::StorageError;

/// A table's rows, by market and key, as its primary key orders them.
type Rows = BTreeMap<(Option<TransferMarket>, TransferValue), TransferRow>;

/// A store's tables in memory, keyed as the real ones are, which can fail
/// a write and pretend to have moved.
pub(super) struct Memory {
    identity: Mutex<StoreIdentity>,
    tables: Mutex<BTreeMap<&'static str, Rows>>,
    record: Mutex<Option<TransferRecord>>,
    pub(super) moved: AtomicBool,
    /// Writes left before one fails; `usize::MAX` for never.
    pub(super) writes_left: AtomicUsize,
    pub(super) writes: AtomicUsize,
    pub(super) numbering_moved: AtomicBool,
}

impl Memory {
    pub(super) fn new(store_id: &str) -> Self {
        Self {
            identity: Mutex::new(StoreIdentity {
                store_id: StoreId::new(store_id).expect("store id"),
                schema_version: TRANSFER_SCHEMA,
                environment: None,
                activation: StoreActivation::Inactive,
                activation_epoch: ActivationEpoch(0),
            }),
            tables: Mutex::new(BTreeMap::new()),
            record: Mutex::new(None),
            moved: AtomicBool::new(false),
            writes_left: AtomicUsize::new(usize::MAX),
            writes: AtomicUsize::new(0),
            numbering_moved: AtomicBool::new(false),
        }
    }

    pub(super) fn source() -> Self {
        let store = Self::new(SOURCE_ID);
        for table in &TRANSFER_TABLES {
            for row in rows_of(table) {
                store.put(table, row);
            }
        }
        store
    }

    pub(super) fn put(&self, table: &TransferTable, row: TransferRow) {
        let key = key(table, &row);
        self.tables
            .lock()
            .expect("tables")
            .entry(table.name)
            .or_default()
            .insert(key, row);
    }

    pub(super) fn remove(&self, table: &TransferTable, row: &TransferRow) {
        if let Some(rows) = self.tables.lock().expect("tables").get_mut(table.name) {
            rows.remove(&key(table, row));
        }
    }

    pub(super) fn rows(&self, table: &TransferTable) -> Vec<TransferRow> {
        self.tables
            .lock()
            .expect("tables")
            .get(table.name)
            .map(|rows| rows.values().cloned().collect())
            .unwrap_or_default()
    }

    pub(super) fn all(&self) -> BTreeMap<&'static str, Vec<TransferRow>> {
        TRANSFER_TABLES
            .iter()
            .map(|table| (table.name, self.rows(table)))
            .collect()
    }

    pub(super) fn with_identity(self, change: impl FnOnce(&mut StoreIdentity)) -> Self {
        change(&mut self.identity.lock().expect("identity"));
        self
    }
}

pub(super) const SOURCE_ID: &str = "0123456789abcdef0123456789abcdef";
pub(super) const TARGET_ID: &str = "fedcba9876543210fedcba9876543210";

fn market_of(table: &TransferTable, row: &TransferRow) -> Option<TransferMarket> {
    if !table.by_market {
        return None;
    }
    let text = |index: usize| match row.get(index) {
        Some(TransferValue::Text(text)) => text.clone(),
        _ => String::new(),
    };
    Some(TransferMarket {
        exchange: text(0),
        symbol: text(1),
        interval: text(2),
    })
}

fn key(table: &TransferTable, row: &TransferRow) -> (Option<TransferMarket>, TransferValue) {
    (market_of(table, row), row[table.key_index()].clone())
}

impl TransferSource for Memory {
    fn transfer_identity(&self) -> Result<StoreIdentity, StorageError> {
        Ok(self.identity.lock().expect("identity").clone())
    }

    fn markets(&self, table: &TransferTable) -> Result<Vec<TransferMarket>, StorageError> {
        let mut markets: Vec<TransferMarket> = self
            .rows(table)
            .iter()
            .filter_map(|row| market_of(table, row))
            .collect();
        markets.dedup();
        // Any order will do; the engine sorts.
        markets.reverse();
        Ok(markets)
    }

    fn read_rows(
        &self,
        table: &TransferTable,
        market: Option<&TransferMarket>,
        after: Option<&TransferValue>,
        limit: usize,
    ) -> Result<Vec<TransferRow>, StorageError> {
        let mut rows: Vec<TransferRow> = self
            .rows(table)
            .into_iter()
            .filter(|row| market_of(table, row).as_ref() == market)
            .filter(|row| after.is_none_or(|after| &row[table.key_index()] > after))
            .collect();
        rows.sort_by(|a, b| a[table.key_index()].cmp(&b[table.key_index()]));
        rows.truncate(limit);
        Ok(rows)
    }
}

impl TransferSnapshot for Memory {
    fn moved(&self) -> Result<bool, StorageError> {
        Ok(self.moved.load(Ordering::SeqCst))
    }
}

impl TransferTarget for Memory {
    fn transfer_record(&self) -> Result<Option<TransferRecord>, StorageError> {
        Ok(self.record.lock().expect("record").clone())
    }

    fn begin_transfer(
        &self,
        record: &TransferRecord,
        environment: Option<Environment>,
    ) -> Result<TransferBegun, StorageError> {
        let holding: Vec<&'static str> = TRANSFER_TABLES
            .iter()
            .filter(|table| !self.rows(table).is_empty())
            .map(|table| table.name)
            .collect();
        if !holding.is_empty() {
            return Ok(TransferBegun::HoldingRows(holding));
        }
        *self.record.lock().expect("record") = Some(record.clone());
        if let Some(environment) = environment {
            self.identity.lock().expect("identity").environment = Some(environment);
        }
        Ok(TransferBegun::Recorded)
    }

    fn write_rows(&self, table: &TransferTable, rows: &[TransferRow]) -> Result<u64, StorageError> {
        let left = self.writes_left.load(Ordering::SeqCst);
        if left == 0 {
            return Err(StorageError::OutcomeUnknown(
                "the write was cut off".to_string(),
            ));
        }
        if left != usize::MAX {
            self.writes_left.store(left - 1, Ordering::SeqCst);
        }
        self.writes.fetch_add(1, Ordering::SeqCst);
        let mut tables = self.tables.lock().expect("tables");
        let stored = tables.entry(table.name).or_default();
        let mut written = 0;
        for row in rows {
            if let std::collections::btree_map::Entry::Vacant(entry) = stored.entry(key(table, row))
            {
                entry.insert(row.clone());
                written += 1;
            }
        }
        Ok(written)
    }

    fn finish_transfer(&self, record: &TransferRecord) -> Result<(), StorageError> {
        self.numbering_moved.store(true, Ordering::SeqCst);
        *self.record.lock().expect("record") = Some(record.clone());
        Ok(())
    }
}

pub(super) fn text(value: &str) -> TransferValue {
    TransferValue::Text(value.to_string())
}

pub(super) fn int(value: i64) -> TransferValue {
    TransferValue::Integer(value)
}

/// The markets [`row_of`] fills a market table with, in byte order.
pub(super) const MARKETS: [(&str, &str, &str); 2] = [
    ("binance_spot", "BTCUSDT", "1m"),
    ("bybit_linear", "BTCUSDT", "1m"),
];

/// A row of `table` for the `n`th of its kind: every column filled.
pub(super) fn row_of(table: &TransferTable, n: i64, market: usize) -> TransferRow {
    let (exchange, symbol, interval) = MARKETS[market];
    table
        .columns
        .iter()
        .enumerate()
        .map(|(index, column)| match (table.by_market, index) {
            (true, 0) => text(exchange),
            (true, 1) => text(symbol),
            (true, 2) => text(interval),
            _ if column.name == table.key => match column.kind {
                ColumnKind::Integer => int(n * 3),
                _ => text(&format!("key-{n:03}")),
            },
            _ if column.name == "status" => text("stopped"),
            _ => match column.kind {
                ColumnKind::Integer => int(n),
                ColumnKind::Text => text(&format!("{}.{n}0", column.name)),
                ColumnKind::Blob => TransferValue::Blob(vec![n as u8; 3]),
            },
        })
        .collect()
}

pub(super) fn rows_of(table: &TransferTable) -> Vec<TransferRow> {
    let markets = if table.by_market { 2 } else { 1 };
    (0..markets)
        .flat_map(|market| (1..=7).map(move |n| row_of(table, n, market)))
        .collect()
}

pub(super) fn engine<'a>(
    source: &'a Memory,
    target: &'a Memory,
    batch_rows: usize,
) -> TransferEngine<'a> {
    TransferEngine::new(
        source,
        target,
        TransferConfig {
            batch_rows,
            assume_quiescent: false,
        },
    )
}

pub(super) fn refusal(result: Result<TransferSummary, EngineError>) -> String {
    match result {
        Err(EngineError::TransferRefused(reason)) => reason,
        other => panic!("expected a refusal, got {other:?}"),
    }
}
