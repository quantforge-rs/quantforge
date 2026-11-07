//! A refresh's reads and writes on PostgreSQL, for the loop in
//! [`crate::storage::refresh`]: a window read in one snapshot, and a
//! write-back that first locks the window's mark.

use std::ops::Range;

use tokio_postgres::types::{ToSql, Type};
use tokio_postgres::{IsolationLevel, Row};

use super::{LOWEST_DIRTY, snapshot};
use crate::storage::common;
use crate::storage::postgres::PostgresStore;
use crate::storage::postgres::bridge::Effect;
use crate::storage::postgres::candles::Key;
use crate::storage::postgres::copying;
use crate::storage::refresh::{DirtyWindows, Packed, Snapshot, Written};
use crate::storage::segments::{self as format, TextCandle};
use crate::{CandleStore, MarketId, StorageError, TimestampMs};

/// One bound parameter of a statement.
type Param<'a> = (&'a (dyn ToSql + Sync), Type);

const MARK: &str = r#"
    SELECT id FROM segment_dirty
    WHERE exchange = $1 AND symbol = $2 AND interval = $3 AND window_start_ms = $4
"#;

/// The mark as a write-back reads it: locked until the transaction ends.
const LOCK_MARK: &str = r#"
    SELECT id FROM segment_dirty
    WHERE exchange = $1 AND symbol = $2 AND interval = $3 AND window_start_ms = $4
    FOR UPDATE
"#;

const ENTRY: &str = r#"
    SELECT hash FROM segment_index
    WHERE exchange = $1 AND symbol = $2 AND interval = $3 AND window_start_ms = $4
"#;

const WINDOW: &str = r#"
    SELECT open_time_ms, close_time_ms, open, high, low, close, volume, trades
    FROM candles
    WHERE exchange = $1 AND symbol = $2 AND interval = $3
      AND open_time_ms >= $4 AND open_time_ms < $5
    ORDER BY open_time_ms ASC
"#;

/// A segment is never changed: its name is its content.
const STORE: &str = r#"
    INSERT INTO segments(hash, format, payload_len, data)
    VALUES ($1, $2, $3, $4)
    ON CONFLICT (hash) DO NOTHING
"#;

const INDEX: &str = r#"
    INSERT INTO segment_index(exchange, symbol, interval, window_start_ms,
                              first_open_ms, last_open_ms, bar_count, hash)
    VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
    ON CONFLICT (exchange, symbol, interval, window_start_ms) DO UPDATE SET
      first_open_ms = excluded.first_open_ms,
      last_open_ms = excluded.last_open_ms,
      bar_count = excluded.bar_count,
      hash = excluded.hash
"#;

const UNINDEX: &str = r#"
    DELETE FROM segment_index
    WHERE exchange = $1 AND symbol = $2 AND interval = $3 AND window_start_ms = $4
"#;

const UNMARK: &str = "DELETE FROM segment_dirty WHERE id = $1";

/// A candle as it is stored, its decimals as text.
fn text_candle(row: &Row) -> Result<TextCandle, tokio_postgres::Error> {
    Ok(TextCandle {
        open_time_ms: row.try_get(0)?,
        close_time_ms: row.try_get(1)?,
        decimals: [
            row.try_get(2)?,
            row.try_get(3)?,
            row.try_get(4)?,
            row.try_get(5)?,
            row.try_get(6)?,
        ],
        // Stored as `upsert_candles` writes it, in a signed column.
        trades: row.try_get::<_, Option<i64>>(7)?.map(|value| value as u64),
    })
}

impl DirtyWindows for &PostgresStore {
    fn tip_window(&mut self, market: &MarketId) -> Result<Option<TimestampMs>, StorageError> {
        self.max_open_time_ms(market)?
            .map(|open_time_ms| common::window_start(market.interval, open_time_ms))
            .transpose()
    }

    fn next_dirty(
        &mut self,
        market: &MarketId,
        after: Option<TimestampMs>,
        tip_window: TimestampMs,
    ) -> Result<Option<TimestampMs>, StorageError> {
        let key = Key::of(market);
        self.bridge.execute(Effect::Read, move |client| {
            Box::pin(async move {
                client
                    .query_typed_one(
                        LOWEST_DIRTY,
                        &[
                            (&key.exchange, Type::TEXT),
                            (&key.symbol, Type::TEXT),
                            (&key.interval, Type::TEXT),
                            (&after, Type::INT8),
                            (&Some(tip_window), Type::INT8),
                        ],
                    )
                    .await?
                    .try_get(0)
            })
        })
    }

    fn read_window(
        &mut self,
        market: &MarketId,
        window: &Range<TimestampMs>,
    ) -> Result<Option<Snapshot>, StorageError> {
        let key = Key::of(market);
        let Range { start, end } = *window;
        self.bridge.execute(Effect::Read, move |client| {
            Box::pin(async move {
                let transaction = snapshot(client).await?;
                let of_window: &[Param<'_>] = &[
                    (&key.exchange, Type::TEXT),
                    (&key.symbol, Type::TEXT),
                    (&key.interval, Type::TEXT),
                    (&start, Type::INT8),
                ];
                let Some(mark) = transaction.query_typed_opt(MARK, of_window).await? else {
                    return Ok(None);
                };
                let id = mark.try_get(0)?;
                let indexed = transaction
                    .query_typed_opt(ENTRY, of_window)
                    .await?
                    .map(|row| row.try_get(0))
                    .transpose()?;
                let candles = transaction
                    .query_typed(
                        WINDOW,
                        &[
                            (&key.exchange, Type::TEXT),
                            (&key.symbol, Type::TEXT),
                            (&key.interval, Type::TEXT),
                            (&start, Type::INT8),
                            (&end, Type::INT8),
                        ],
                    )
                    .await?
                    .iter()
                    .map(text_candle)
                    .collect::<Result<Vec<_>, _>>()?;
                transaction.commit().await?;
                Ok(Some(Snapshot {
                    id,
                    candles,
                    indexed,
                }))
            })
        })
    }

    fn write_window(
        &mut self,
        market: &MarketId,
        window_start: TimestampMs,
        id: i64,
        packed: &Packed,
    ) -> Result<Written, StorageError> {
        let key = Key::of(market);
        let packed = packed.clone();
        self.bridge.execute(Effect::Write, move |client| {
            Box::pin(async move {
                let transaction = client
                    .build_transaction()
                    .isolation_level(IsolationLevel::ReadCommitted)
                    .start()
                    .await?;
                let of_window: &[Param<'_>] = &[
                    (&key.exchange, Type::TEXT),
                    (&key.symbol, Type::TEXT),
                    (&key.interval, Type::TEXT),
                    (&window_start, Type::INT8),
                ];
                let current: Option<i64> = transaction
                    .query_typed_opt(LOCK_MARK, of_window)
                    .await?
                    .map(|row| row.try_get(0))
                    .transpose()?;
                // Anything but the revision read ends here, and the
                // transaction rolls back as it drops.
                if let Some(refused) = copying::refusal(&*transaction).await? {
                    return Ok(Err(refused));
                }
                match current {
                    None => return Ok(Ok(Written::Elsewhere)),
                    Some(current) if current != id => return Ok(Ok(Written::Marked)),
                    Some(_) => {}
                }
                match &packed {
                    Packed::Write {
                        hash,
                        payload_len,
                        stored,
                        first_open_ms,
                        last_open_ms,
                        bar_count,
                    } => {
                        let hash = hash.to_string();
                        transaction
                            .execute_typed(
                                STORE,
                                &[
                                    (&hash, Type::TEXT),
                                    (&(format::FORMAT as i32), Type::INT4),
                                    (&(*payload_len as i32), Type::INT4),
                                    (stored, Type::BYTEA),
                                ],
                            )
                            .await?;
                        transaction
                            .execute_typed(
                                INDEX,
                                &[
                                    (&key.exchange, Type::TEXT),
                                    (&key.symbol, Type::TEXT),
                                    (&key.interval, Type::TEXT),
                                    (&window_start, Type::INT8),
                                    (first_open_ms, Type::INT8),
                                    (last_open_ms, Type::INT8),
                                    (&(*bar_count as i32), Type::INT4),
                                    (&hash, Type::TEXT),
                                ],
                            )
                            .await?;
                    }
                    Packed::Keep => {}
                    Packed::Clear => {
                        transaction.execute_typed(UNINDEX, of_window).await?;
                    }
                }
                transaction
                    .execute_typed(UNMARK, &[(&id, Type::INT8)])
                    .await?;
                transaction.commit().await?;
                Ok(Ok(Written::Done))
            })
        })?
    }
}
