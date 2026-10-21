//! Refreshing a market's segments. Each dirty window below the tip is read
//! in a snapshot, packed with no transaction open, and written back in a
//! short one that first checks the window was not marked again meanwhile.
//! Packing never holds the write lock, so a writer such as `trade run`'s
//! poll loop is never kept waiting for it, and every window commits on its
//! own, so an interrupted refresh loses nothing it finished.

use std::ops::Range;

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use tracing::warn;

use super::{SqliteStore, format, tip_window};
use crate::storage::segments::{SegmentError, TextCandle};
use crate::{MarketId, SegmentHash, SegmentRefresh, StorageError, TimestampMs, ms_to_rfc3339};

/// How often a window is read again when a writer keeps marking it between
/// the read and the write. After that the next refresh takes it up.
const ATTEMPTS: usize = 3;

pub(super) fn refresh(
    store: &SqliteStore,
    market: &MarketId,
) -> Result<SegmentRefresh, StorageError> {
    let mut connection = store.open()?;
    let mut refresh = SegmentRefresh::default();
    let Some(tip_window) = tip_window(&connection, market)? else {
        return Ok(refresh);
    };
    let mut after = None;
    while let Some(window_start) = next_dirty(&connection, market, after, tip_window)? {
        match build_window(&mut connection, market, window_start)? {
            Built::New => refresh.built += 1,
            Built::Same => refresh.unchanged += 1,
            Built::Blocked(error) => {
                warn!(
                    exchange = market.exchange.as_str(),
                    symbol = market.symbol.as_str(),
                    interval = market.interval.as_str(),
                    window_start = %ms_to_rfc3339(window_start),
                    %error,
                    "a window of candles cannot be packed into a segment, so plans read it and every \
                     later window from the candle store; check its candles with `data validate`"
                );
                refresh.blocked += 1;
            }
            Built::Elsewhere | Built::Busy => {}
        }
        after = Some(window_start);
    }
    Ok(refresh)
}

/// What became of one dirty window.
enum Built {
    /// Indexed under a new segment.
    New,
    /// Its candles pack to the segment it is already indexed under.
    Same,
    /// It cannot be packed, and stays dirty.
    Blocked(SegmentError),
    /// Another refresh packed it first.
    Elsewhere,
    /// A writer kept marking it; it stays dirty for the next refresh.
    Busy,
}

/// The lowest dirty window after `after`, below the tip window.
fn next_dirty(
    connection: &Connection,
    market: &MarketId,
    after: Option<TimestampMs>,
    tip_window: TimestampMs,
) -> Result<Option<TimestampMs>, StorageError> {
    connection
        .query_row(
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

fn build_window(
    connection: &mut Connection,
    market: &MarketId,
    window_start: TimestampMs,
) -> Result<Built, StorageError> {
    let window = format::window(market.interval, window_start)
        .ok_or_else(|| super::out_of_range(window_start))?;
    for _ in 0..ATTEMPTS {
        let Some(snapshot) = read_window(connection, market, &window)? else {
            return Ok(Built::Elsewhere);
        };
        let packed = match pack(&snapshot) {
            Ok(packed) => packed,
            Err(error) => return Ok(Built::Blocked(error)),
        };
        match write_window(connection, market, window.start, snapshot.id, &packed)? {
            Written::Done => {
                return Ok(match packed {
                    Packed::Keep => Built::Same,
                    Packed::Write { .. } | Packed::Clear => Built::New,
                });
            }
            Written::Elsewhere => return Ok(Built::Elsewhere),
            Written::Marked => {}
        }
    }
    Ok(Built::Busy)
}

/// A dirty window as one snapshot saw it.
pub(super) struct Snapshot {
    /// Its mark, which a writer replaces whenever it changes the window.
    pub(super) id: i64,
    candles: Vec<TextCandle>,
    /// The name it is indexed under, if any.
    indexed: Option<String>,
}

/// Reads a dirty window in one snapshot, taking no write lock. `None` when
/// it is no longer marked.
pub(super) fn read_window(
    connection: &mut Connection,
    market: &MarketId,
    window: &Range<TimestampMs>,
) -> Result<Option<Snapshot>, StorageError> {
    let transaction = connection.transaction().map_err(StorageError::other)?;
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
                    trades: row.get(7)?,
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

/// What writing a window back does.
pub(super) enum Packed {
    /// Index it under a new segment.
    Write {
        hash: SegmentHash,
        payload_len: usize,
        stored: Vec<u8>,
        first_open_ms: TimestampMs,
        last_open_ms: TimestampMs,
        bar_count: usize,
    },
    /// Its index entry is already right.
    Keep,
    /// It has no candles, so it is not indexed at all.
    Clear,
}

/// Packs a snapshot's candles, outside any transaction.
pub(super) fn pack(snapshot: &Snapshot) -> Result<Packed, SegmentError> {
    let (Some(first), Some(last)) = (snapshot.candles.first(), snapshot.candles.last()) else {
        return Ok(Packed::Clear);
    };
    let payload = format::encode_text(&snapshot.candles)?;
    let hash = format::hash(&payload);
    if snapshot.indexed.as_deref() == Some(hash.to_string().as_str()) {
        return Ok(Packed::Keep);
    }
    Ok(Packed::Write {
        hash,
        payload_len: payload.len(),
        stored: format::compress(&payload)?,
        first_open_ms: first.open_time_ms,
        last_open_ms: last.open_time_ms,
        bar_count: snapshot.candles.len(),
    })
}

/// How a write went.
pub(super) enum Written {
    Done,
    /// Another refresh unmarked the window first.
    Elsewhere,
    /// A writer marked the window again after the snapshot.
    Marked,
}

/// Writes a packed window back and unmarks it, if it still carries the mark
/// the snapshot read: then no candle of it changed since.
pub(super) fn write_window(
    connection: &mut Connection,
    market: &MarketId,
    window_start: TimestampMs,
    id: i64,
    packed: &Packed,
) -> Result<Written, StorageError> {
    let transaction = connection
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
