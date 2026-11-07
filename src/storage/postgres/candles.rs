//! The candle table on PostgreSQL: `CandleStore` with SQLite's semantics.
//!
//! A batch is one transaction of two statements: the candles, sent as
//! arrays unnested into one `INSERT … ON CONFLICT … DO UPDATE` that changes
//! a row only where a column's text differs, then a fresh revision on the
//! dirty mark of every window whose candles changed, so a refresh can never
//! publish the window as it was (`docs/storage-schema.md`, "Segments on
//! PostgreSQL"). PostgreSQL refuses a key twice in one statement, so
//! [`Batch`] keeps the last candle of each open time, as SQLite's row by
//! row writes leave it, and marks a window whose duplicates differ, as
//! those writes would. Decimals are written as `to_string` and read back
//! exactly.

use std::collections::{BTreeMap, BTreeSet};

use tokio_postgres::types::Type;
use tokio_postgres::{IsolationLevel, Row};

use super::bridge::Effect;
use super::{PostgresStore, copying, limit};
use crate::storage::common::{self, read_decimal};
use crate::{Candle, CandleQuery, CandleStore, MarketId, StorageError, TimestampMs};

const UPSERT: &str = r#"
    INSERT INTO candles(exchange, symbol, interval, open_time_ms, close_time_ms,
                        open, high, low, close, volume, trades)
    SELECT $1, $2, $3, t.open_time_ms, t.close_time_ms, t.open, t.high, t.low, t.close,
           t.volume, t.trades
    FROM unnest($4::bigint[], $5::bigint[], $6::text[], $7::text[], $8::text[], $9::text[],
                $10::text[], $11::bigint[])
      AS t(open_time_ms, close_time_ms, open, high, low, close, volume, trades)
    ON CONFLICT (exchange, symbol, interval, open_time_ms) DO UPDATE SET
      close_time_ms = excluded.close_time_ms,
      open = excluded.open,
      high = excluded.high,
      low = excluded.low,
      close = excluded.close,
      volume = excluded.volume,
      trades = excluded.trades
    WHERE candles.close_time_ms IS DISTINCT FROM excluded.close_time_ms
      OR candles.open IS DISTINCT FROM excluded.open
      OR candles.high IS DISTINCT FROM excluded.high
      OR candles.low IS DISTINCT FROM excluded.low
      OR candles.close IS DISTINCT FROM excluded.close
      OR candles.volume IS DISTINCT FROM excluded.volume
      OR candles.trades IS DISTINCT FROM excluded.trades
    RETURNING open_time_ms
"#;

/// Every mark a fresh revision, which a refresh's write-back compares.
const MARK: &str = r#"
    INSERT INTO segment_dirty(exchange, symbol, interval, window_start_ms)
    SELECT $1, $2, $3, unnest($4::bigint[])
    ON CONFLICT (exchange, symbol, interval, window_start_ms) DO UPDATE SET id = DEFAULT
"#;

const LOAD: &str = r#"
    SELECT open_time_ms, close_time_ms, open, high, low, close, volume, trades
    FROM candles
    WHERE exchange = $1 AND symbol = $2 AND interval = $3
      AND ($4::bigint IS NULL OR open_time_ms >= $4)
      AND ($5::bigint IS NULL OR open_time_ms <= $5)
    ORDER BY open_time_ms ASC
    LIMIT $6
"#;

const RECENT: &str = r#"
    SELECT open_time_ms, close_time_ms, open, high, low, close, volume, trades
    FROM candles
    WHERE exchange = $1 AND symbol = $2 AND interval = $3
    ORDER BY open_time_ms DESC
    LIMIT $4
"#;

pub(super) const MAX_OPEN_TIME: &str = r#"
    SELECT MAX(open_time_ms) FROM candles
    WHERE exchange = $1 AND symbol = $2 AND interval = $3
"#;

/// A candle's columns as they are stored: what decides whether a write
/// changes a row.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Columns {
    close_time_ms: TimestampMs,
    open: String,
    high: String,
    low: String,
    close: String,
    volume: String,
    trades: Option<i64>,
}

impl Columns {
    fn of(candle: &Candle) -> Self {
        Self {
            close_time_ms: candle.close_time_ms,
            open: candle.open.to_string(),
            high: candle.high.to_string(),
            low: candle.low.to_string(),
            close: candle.close.to_string(),
            volume: candle.volume.to_string(),
            // Stored as SQLite stores it: a `u64` in a signed column.
            trades: candle.trades.map(|value| value as i64),
        }
    }
}

/// A batch as one statement takes it: one row per open time, the last of
/// its duplicates, in open-time order, so rows lock in key order; each
/// row's window; and the windows a duplicate that differs from another
/// marks, whatever the stored row holds.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Batch {
    pub(super) open_time_ms: Vec<TimestampMs>,
    pub(super) window_start_ms: Vec<TimestampMs>,
    pub(super) close_time_ms: Vec<TimestampMs>,
    pub(super) open: Vec<String>,
    pub(super) high: Vec<String>,
    pub(super) low: Vec<String>,
    pub(super) close: Vec<String>,
    pub(super) volume: Vec<String>,
    pub(super) trades: Vec<Option<i64>>,
    pub(super) forced: BTreeSet<TimestampMs>,
}

impl Batch {
    /// The batch for `candles`, or the refusal of a candle whose window
    /// cannot be written, before anything is.
    pub(super) fn of(market: &MarketId, candles: &[Candle]) -> Result<Self, StorageError> {
        let mut rows: BTreeMap<TimestampMs, (TimestampMs, Columns)> = BTreeMap::new();
        let mut forced = BTreeSet::new();
        for candle in candles {
            let window = common::window_start(market.interval, candle.open_time_ms)?;
            let columns = Columns::of(candle);
            // Written row by row, a duplicate that differs from the one
            // before it changes the row, whatever was stored.
            if let Some((_, earlier)) = rows.insert(candle.open_time_ms, (window, columns.clone()))
            {
                if earlier != columns {
                    forced.insert(window);
                }
            }
        }
        let mut batch = Self {
            forced,
            ..Self::default()
        };
        for (open_time_ms, (window, columns)) in rows {
            batch.open_time_ms.push(open_time_ms);
            batch.window_start_ms.push(window);
            batch.close_time_ms.push(columns.close_time_ms);
            batch.open.push(columns.open);
            batch.high.push(columns.high);
            batch.low.push(columns.low);
            batch.close.push(columns.close);
            batch.volume.push(columns.volume);
            batch.trades.push(columns.trades);
        }
        Ok(batch)
    }

    /// The window of a row of the batch.
    fn window_of(&self, open_time_ms: TimestampMs) -> Option<TimestampMs> {
        self.open_time_ms
            .binary_search(&open_time_ms)
            .ok()
            .map(|index| self.window_start_ms[index])
    }
}

/// A market's three key columns, owned, for work that runs on the bridge.
pub(super) struct Key {
    pub(super) exchange: String,
    pub(super) symbol: String,
    pub(super) interval: String,
}

impl Key {
    pub(super) fn of(market: &MarketId) -> Self {
        Self {
            exchange: market.exchange.as_str().to_string(),
            symbol: market.symbol.as_str().to_string(),
            interval: market.interval.as_str().to_string(),
        }
    }
}

/// A row as the database returned it, before its decimals are read.
struct Stored {
    open_time_ms: TimestampMs,
    close_time_ms: TimestampMs,
    texts: [String; 5],
    trades: Option<i64>,
}

impl Stored {
    fn of(row: &Row) -> Result<Self, tokio_postgres::Error> {
        Ok(Self {
            open_time_ms: row.try_get(0)?,
            close_time_ms: row.try_get(1)?,
            texts: [
                row.try_get(2)?,
                row.try_get(3)?,
                row.try_get(4)?,
                row.try_get(5)?,
                row.try_get(6)?,
            ],
            trades: row.try_get(7)?,
        })
    }

    /// The candle, or the refusal of text no `Decimal` holds exactly.
    fn candle(self) -> Result<Candle, StorageError> {
        let [open, high, low, close, volume] = self.texts;
        Ok(Candle {
            open_time_ms: self.open_time_ms,
            close_time_ms: self.close_time_ms,
            open: read_decimal(&open)?,
            high: read_decimal(&high)?,
            low: read_decimal(&low)?,
            close: read_decimal(&close)?,
            volume: read_decimal(&volume)?,
            trades: self.trades.map(|value| value as u64),
        })
    }
}

impl PostgresStore {
    /// Runs a read returning candle rows, then reads their decimals.
    fn read_candles<F>(&self, work: F) -> Result<Vec<Candle>, StorageError>
    where
        F: for<'c> FnOnce(
                &'c mut deadpool_postgres::ClientWrapper,
            ) -> super::bridge::Work<'c, Vec<Row>>
            + Send
            + 'static,
    {
        let rows = self.bridge.execute(Effect::Read, move |client| {
            Box::pin(async move {
                let rows = work(client).await?;
                rows.iter().map(Stored::of).collect::<Result<Vec<_>, _>>()
            })
        })?;
        rows.into_iter().map(Stored::candle).collect()
    }
}

impl CandleStore for PostgresStore {
    fn init(&self) -> Result<(), StorageError> {
        PostgresStore::init(self)
    }

    fn upsert_candles(&self, market: &MarketId, candles: &[Candle]) -> Result<usize, StorageError> {
        if candles.is_empty() {
            return Ok(0);
        }
        let batch = Batch::of(market, candles)?;
        let key = Key::of(market);
        let marked = self.bridge.execute(Effect::Write, move |client| {
            Box::pin(async move {
                let transaction = client
                    .build_transaction()
                    .isolation_level(IsolationLevel::ReadCommitted)
                    .start()
                    .await?;
                let written = transaction
                    .query_typed(
                        UPSERT,
                        &[
                            (&key.exchange, Type::TEXT),
                            (&key.symbol, Type::TEXT),
                            (&key.interval, Type::TEXT),
                            (&batch.open_time_ms, Type::INT8_ARRAY),
                            (&batch.close_time_ms, Type::INT8_ARRAY),
                            (&batch.open, Type::TEXT_ARRAY),
                            (&batch.high, Type::TEXT_ARRAY),
                            (&batch.low, Type::TEXT_ARRAY),
                            (&batch.close, Type::TEXT_ARRAY),
                            (&batch.volume, Type::TEXT_ARRAY),
                            (&batch.trades, Type::INT8_ARRAY),
                        ],
                    )
                    .await?;
                // Each refusal rolls the transaction back as it drops.
                if let Some(refused) = copying::refusal(&*transaction).await? {
                    return Ok(Err(refused));
                }
                let mut windows = batch.forced.clone();
                for row in &written {
                    let open_time_ms: TimestampMs = row.try_get(0)?;
                    match batch.window_of(open_time_ms) {
                        Some(window) => windows.insert(window),
                        None => {
                            return Ok(Err(StorageError::InvalidArgument(format!(
                                "the database wrote a candle at {open_time_ms}, which the batch \
                                 does not hold"
                            ))));
                        }
                    };
                }
                if !windows.is_empty() {
                    let windows: Vec<TimestampMs> = windows.into_iter().collect();
                    transaction
                        .execute_typed(
                            MARK,
                            &[
                                (&key.exchange, Type::TEXT),
                                (&key.symbol, Type::TEXT),
                                (&key.interval, Type::TEXT),
                                (&windows, Type::INT8_ARRAY),
                            ],
                        )
                        .await?;
                }
                transaction.commit().await?;
                Ok(Ok(()))
            })
        })?;
        marked?;
        Ok(candles.len())
    }

    fn load_candles(
        &self,
        market: &MarketId,
        query: CandleQuery,
    ) -> Result<Vec<Candle>, StorageError> {
        let key = Key::of(market);
        let limit = query.limit.map(limit);
        self.read_candles(move |client| {
            Box::pin(async move {
                client
                    .query_typed(
                        LOAD,
                        &[
                            (&key.exchange, Type::TEXT),
                            (&key.symbol, Type::TEXT),
                            (&key.interval, Type::TEXT),
                            (&query.start_time_ms, Type::INT8),
                            (&query.end_time_ms, Type::INT8),
                            (&limit, Type::INT8),
                        ],
                    )
                    .await
            })
        })
    }

    fn load_recent_candles(
        &self,
        market: &MarketId,
        count: usize,
    ) -> Result<Vec<Candle>, StorageError> {
        let key = Key::of(market);
        let count = limit(count);
        let mut candles = self.read_candles(move |client| {
            Box::pin(async move {
                client
                    .query_typed(
                        RECENT,
                        &[
                            (&key.exchange, Type::TEXT),
                            (&key.symbol, Type::TEXT),
                            (&key.interval, Type::TEXT),
                            (&count, Type::INT8),
                        ],
                    )
                    .await
            })
        })?;
        candles.reverse();
        Ok(candles)
    }

    fn max_open_time_ms(&self, market: &MarketId) -> Result<Option<TimestampMs>, StorageError> {
        let key = Key::of(market);
        self.bridge.execute(Effect::Read, move |client| {
            Box::pin(async move {
                client
                    .query_typed_one(
                        MAX_OPEN_TIME,
                        &[
                            (&key.exchange, Type::TEXT),
                            (&key.symbol, Type::TEXT),
                            (&key.interval, Type::TEXT),
                        ],
                    )
                    .await?
                    .try_get(0)
            })
        })
    }
}
