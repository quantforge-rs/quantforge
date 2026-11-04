//! A refresh's reads and writes on SQLite: a window is read in a deferred
//! snapshot and written back in a short `IMMEDIATE` transaction, for the
//! loop in [`crate::storage::refresh`].

use std::ops::Range;

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use super::{format, tip_window};
use crate::storage::refresh::{DirtyWindows, Packed, Snapshot, Written};
use crate::storage::segments::TextCandle;
use crate::{MarketId, StorageError, TimestampMs};

impl DirtyWindows for Connection {
    fn tip_window(&mut self, market: &MarketId) -> Result<Option<TimestampMs>, StorageError> {
        tip_window(self, market)
    }

    fn next_dirty(
        &mut self,
        market: &MarketId,
        after: Option<TimestampMs>,
        tip_window: TimestampMs,
    ) -> Result<Option<TimestampMs>, StorageError> {
        self.query_row(
            r#"
                SELECT MIN(window_start_ms) FROM segment_dirty
                WHERE exchange = ?1 AND symbol = ?2 AND interval = ?3
                  AND window_start_ms < ?4 AND (?5 IS NULL OR window_start_ms > ?5)
                "#,
            params![
                market.exchange.as_str(),
                market.symbol.as_str(),
                market.interval.as_str(),
                tip_window,
                after
            ],
            |row| row.get(0),
        )
        .map_err(StorageError::other)
    }

    fn read_window(
        &mut self,
        market: &MarketId,
        window: &Range<TimestampMs>,
    ) -> Result<Option<Snapshot>, StorageError> {
        let transaction = self.transaction().map_err(StorageError::other)?;
        let key = params![
            market.exchange.as_str(),
            market.symbol.as_str(),
            market.interval.as_str(),
            window.start
        ];
        let id: Option<i64> = transaction
            .query_row(
                r#"
                SELECT id FROM segment_dirty
                WHERE exchange = ?1 AND symbol = ?2 AND interval = ?3 AND window_start_ms = ?4
                "#,
                key,
                |row| row.get(0),
            )
            .optional()
            .map_err(StorageError::other)?;
        let Some(id) = id else {
            return Ok(None);
        };
        let indexed: Option<String> = transaction
            .query_row(
                r#"
                SELECT hash FROM segment_index
                WHERE exchange = ?1 AND symbol = ?2 AND interval = ?3 AND window_start_ms = ?4
                "#,
                key,
                |row| row.get(0),
            )
            .optional()
            .map_err(StorageError::other)?;
        let mut stmt = transaction
            .prepare(
                r#"
                SELECT open_time_ms, close_time_ms, open, high, low, close, volume, trades
                FROM candles
                WHERE exchange = ?1 AND symbol = ?2 AND interval = ?3
                  AND open_time_ms >= ?4 AND open_time_ms < ?5
                ORDER BY open_time_ms ASC
                "#,
            )
            .map_err(StorageError::other)?;
        let rows = stmt
            .query_map(
                params![
                    market.exchange.as_str(),
                    market.symbol.as_str(),
                    market.interval.as_str(),
                    window.start,
                    window.end
                ],
                |row| {
                    Ok(TextCandle {
                        open_time_ms: row.get(0)?,
                        close_time_ms: row.get(1)?,
                        decimals: [
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                            row.get(5)?,
                            row.get(6)?,
                        ],
                        // Stored as `upsert_candles` writes it, in a signed column.
                        trades: row.get::<_, Option<i64>>(7)?.map(|value| value as u64),
                    })
                },
            )
            .map_err(StorageError::other)?;
        let mut candles = Vec::new();
        for row in rows {
            candles.push(row.map_err(StorageError::other)?);
        }
        Ok(Some(Snapshot {
            id,
            candles,
            indexed,
        }))
    }

    fn write_window(
        &mut self,
        market: &MarketId,
        window_start: TimestampMs,
        id: i64,
        packed: &Packed,
    ) -> Result<Written, StorageError> {
        let transaction = self
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::other)?;
        let key = params![
            market.exchange.as_str(),
            market.symbol.as_str(),
            market.interval.as_str(),
            window_start
        ];
        let current: Option<i64> = transaction
            .query_row(
                r#"
                SELECT id FROM segment_dirty
                WHERE exchange = ?1 AND symbol = ?2 AND interval = ?3 AND window_start_ms = ?4
                "#,
                key,
                |row| row.get(0),
            )
            .optional()
            .map_err(StorageError::other)?;
        match current {
            None => return Ok(Written::Elsewhere),
            Some(current) if current != id => return Ok(Written::Marked),
            Some(_) => {}
        }
        match packed {
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
                    .execute(
                        r#"
                        INSERT INTO segments(hash, format, payload_len, data)
                        VALUES (?1, ?2, ?3, ?4)
                        ON CONFLICT(hash) DO NOTHING
                        "#,
                        params![hash, format::FORMAT, *payload_len as i64, stored],
                    )
                    .map_err(StorageError::other)?;
                transaction
                    .execute(
                        r#"
                        INSERT INTO segment_index(
                          exchange, symbol, interval, window_start_ms,
                          first_open_ms, last_open_ms, bar_count, hash
                        )
                        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                        ON CONFLICT(exchange, symbol, interval, window_start_ms) DO UPDATE SET
                          first_open_ms = excluded.first_open_ms,
                          last_open_ms = excluded.last_open_ms,
                          bar_count = excluded.bar_count,
                          hash = excluded.hash
                        "#,
                        params![
                            market.exchange.as_str(),
                            market.symbol.as_str(),
                            market.interval.as_str(),
                            window_start,
                            first_open_ms,
                            last_open_ms,
                            *bar_count as i64,
                            hash
                        ],
                    )
                    .map_err(StorageError::other)?;
            }
            Packed::Keep => {}
            Packed::Clear => {
                transaction
                    .execute(
                        r#"
                        DELETE FROM segment_index
                        WHERE exchange = ?1 AND symbol = ?2 AND interval = ?3 AND window_start_ms = ?4
                        "#,
                        key,
                    )
                    .map_err(StorageError::other)?;
            }
        }
        transaction
            .execute("DELETE FROM segment_dirty WHERE id = ?1", [id])
            .map_err(StorageError::other)?;
        transaction.commit().map_err(StorageError::other)?;
        Ok(Written::Done)
    }
}
