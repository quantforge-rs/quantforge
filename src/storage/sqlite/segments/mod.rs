//! Segment tables and the `SegmentStore` implementation: the packed
//! segments by name, the index of which segment packs which window, and
//! the marks writers leave on windows whose candles changed.
//!
//! Every window below the tip, the window holding a market's newest candle,
//! is either marked dirty or indexed under the name of exactly its current
//! candles. Writers keep that true by marking a window in the transaction
//! that changes its candles, and a refresh ([`build`]) by unmarking a
//! window only in the transaction that indexes it, and only if no writer
//! marked it again in between. A plan therefore trusts the index only below
//! the lowest dirty window.

mod build;
#[cfg(test)]
mod tests;

use std::collections::BTreeSet;

use rusqlite::{Connection, OptionalExtension, Transaction, params};

use super::{SqliteStore, to_from_sql_error};
use crate::storage::segments as format;
use crate::{
    MarketId, SegmentHash, SegmentPlan, SegmentRef, SegmentRefresh, SegmentStore, SegmentTip,
    StorageError, TimestampMs,
};

// Indentation inside this literal is load-bearing: SQLite records the exact
// statement text in `sqlite_master`, so reformatting it would change the
// schema of freshly created databases.
pub(super) const SCHEMA_DDL: &str = r#"
                CREATE TABLE IF NOT EXISTS segments (
                  hash TEXT PRIMARY KEY,
                  format INTEGER NOT NULL,
                  payload_len INTEGER NOT NULL,
                  data BLOB NOT NULL
                );

                CREATE TABLE IF NOT EXISTS segment_index (
                  exchange TEXT NOT NULL,
                  symbol TEXT NOT NULL,
                  interval TEXT NOT NULL,
                  window_start_ms INTEGER NOT NULL,
                  first_open_ms INTEGER NOT NULL,
                  last_open_ms INTEGER NOT NULL,
                  bar_count INTEGER NOT NULL,
                  hash TEXT NOT NULL,
                  PRIMARY KEY (exchange, symbol, interval, window_start_ms)
                );

                CREATE TABLE IF NOT EXISTS segment_dirty (
                  id INTEGER PRIMARY KEY AUTOINCREMENT,
                  exchange TEXT NOT NULL,
                  symbol TEXT NOT NULL,
                  interval TEXT NOT NULL,
                  window_start_ms INTEGER NOT NULL,
                  UNIQUE (exchange, symbol, interval, window_start_ms)
                );
"#;

impl SegmentStore for SqliteStore {
    fn refresh_segments(&self, market: &MarketId) -> Result<SegmentRefresh, StorageError> {
        build::refresh(self, market)
    }

    fn resolve_segments(
        &self,
        market: &MarketId,
        first_open_ms: TimestampMs,
        last_open_ms: TimestampMs,
    ) -> Result<SegmentPlan, StorageError> {
        if first_open_ms > last_open_ms {
            return Err(StorageError::InvalidArgument(format!(
                "the first open time {first_open_ms} is after the last, {last_open_ms}"
            )));
        }
        let mut connection = self.open()?;
        // One snapshot, so that the marks and the index agree.
        let transaction = connection.transaction().map_err(StorageError::other)?;
        let Some(tip_window) = tip_window(&transaction, market)? else {
            return Ok(SegmentPlan {
                segments: Vec::new(),
                tip: Some(SegmentTip {
                    first_open_ms,
                    last_open_ms,
                }),
            });
        };
        let lowest_dirty: Option<TimestampMs> = transaction
            .query_row(
                r#"
                SELECT MIN(window_start_ms) FROM segment_dirty
                WHERE exchange = ?1 AND symbol = ?2 AND interval = ?3
                "#,
                params![
                    market.exchange.as_str(),
                    market.symbol.as_str(),
                    market.interval.as_str()
                ],
                |row| row.get(0),
            )
            .map_err(StorageError::other)?;
        let tip_start = lowest_dirty.map_or(tip_window, |dirty| dirty.min(tip_window));

        let mut stmt = transaction
            .prepare(
                r#"
                SELECT i.window_start_ms, i.first_open_ms, i.last_open_ms, i.bar_count, i.hash,
                       s.payload_len, length(s.data)
                FROM segment_index i JOIN segments s ON s.hash = i.hash
                WHERE i.exchange = ?1 AND i.symbol = ?2 AND i.interval = ?3
                  AND i.window_start_ms < ?4 AND i.last_open_ms >= ?5 AND i.first_open_ms <= ?6
                ORDER BY i.window_start_ms ASC
                "#,
            )
            .map_err(StorageError::other)?;
        let rows = stmt
            .query_map(
                params![
                    market.exchange.as_str(),
                    market.symbol.as_str(),
                    market.interval.as_str(),
                    tip_start,
                    first_open_ms,
                    last_open_ms
                ],
                |row| {
                    Ok(SegmentRef {
                        window_start_ms: row.get(0)?,
                        first_open_ms: row.get(1)?,
                        last_open_ms: row.get(2)?,
                        bar_count: row.get(3)?,
                        hash: row
                            .get::<_, String>(4)?
                            .parse::<SegmentHash>()
                            .map_err(to_from_sql_error)?,
                        payload_len: row.get(5)?,
                        stored_len: row.get(6)?,
                    })
                },
            )
            .map_err(StorageError::other)?;
        let mut segments = Vec::new();
        for row in rows {
            segments.push(row.map_err(StorageError::other)?);
        }

        Ok(SegmentPlan {
            segments,
            tip: (last_open_ms >= tip_start).then(|| SegmentTip {
                first_open_ms: first_open_ms.max(tip_start),
                last_open_ms,
            }),
        })
    }

    fn load_segment(&self, hash: &SegmentHash) -> Result<Option<Vec<u8>>, StorageError> {
        self.open()?
            .query_row(
                "SELECT data FROM segments WHERE hash = ?1",
                [hash.to_string()],
                |row| row.get(0),
            )
            .optional()
            .map_err(StorageError::other)
    }
}

/// Marks the windows starting at `window_starts` dirty, inside the
/// transaction that changed their candles. A mark already there is
/// replaced, so it gets a new id, and a refresh that read the old one
/// knows that the candles it packed are no longer the window's.
pub(super) fn mark_dirty(
    transaction: &Transaction<'_>,
    market: &MarketId,
    window_starts: &BTreeSet<TimestampMs>,
) -> Result<(), StorageError> {
    let mut stmt = transaction
        .prepare_cached(
            r#"
            INSERT OR REPLACE INTO segment_dirty(exchange, symbol, interval, window_start_ms)
            VALUES (?1, ?2, ?3, ?4)
            "#,
        )
        .map_err(StorageError::other)?;
    for window_start in window_starts {
        stmt.execute(params![
            market.exchange.as_str(),
            market.symbol.as_str(),
            market.interval.as_str(),
            window_start
        ])
        .map_err(StorageError::other)?;
    }
    Ok(())
}

/// The start of the window holding the market's newest candle, which is
/// never packed: candles may still join it. `None` without candles.
fn tip_window(
    connection: &Connection,
    market: &MarketId,
) -> Result<Option<TimestampMs>, StorageError> {
    let newest: Option<TimestampMs> = connection
        .query_row(
            r#"
            SELECT MAX(open_time_ms) FROM candles
            WHERE exchange = ?1 AND symbol = ?2 AND interval = ?3
            "#,
            params![
                market.exchange.as_str(),
                market.symbol.as_str(),
                market.interval.as_str()
            ],
            |row| row.get(0),
        )
        .map_err(StorageError::other)?;
    newest
        .map(|open_time_ms| {
            format::window(market.interval, open_time_ms)
                .map(|window| window.start)
                .ok_or_else(|| out_of_range(open_time_ms))
        })
        .transpose()
}

/// The error for a candle whose segment window cannot be written, which
/// `upsert_candles` refuses before storing it.
pub(super) fn out_of_range(open_time_ms: TimestampMs) -> StorageError {
    StorageError::InvalidArgument(format!(
        "candle open time {open_time_ms} lies too close to the end of the timestamp range to \
         belong to a segment window"
    ))
}
