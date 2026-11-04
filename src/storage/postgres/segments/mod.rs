//! The segment store on PostgreSQL: `SegmentStore` with SQLite's answers,
//! by the protocol of `docs/storage-schema.md` ("Segments on PostgreSQL"),
//! its loop and its packing shared through `storage::refresh`.
//!
//! A window is read, and a plan made, in one read-only `REPEATABLE READ`
//! transaction, so each sees one moment of the store. Packing runs on the
//! caller's thread between two bridge calls: no transaction is open, and
//! compression never takes up the bridge's workers. A write-back ([`build`])
//! is one `READ COMMITTED` transaction: its lock on the mark holds a
//! writer's next mark back or, if a writer got there first, waits for it
//! and reads the revision it left. So the blob, the index entry and the end
//! of the mark are published together, and only over the candles that were
//! packed. A write-back that loses its answer is `OutcomeUnknown`, and
//! refreshing again is safe: the mark decides every step.

mod build;

use deadpool_postgres::{ClientWrapper, Transaction};
use tokio_postgres::types::Type;
use tokio_postgres::{IsolationLevel, Row};

use super::PostgresStore;
use super::bridge::Effect;
use super::candles::{Key, MAX_OPEN_TIME};
use crate::storage::segments as format;
use crate::storage::{common, refresh};
use crate::{
    MarketId, SegmentHash, SegmentPlan, SegmentRef, SegmentRefresh, SegmentStore, StorageError,
    TimestampMs,
};

/// The lowest dirty window between two bounds, either of them open.
const LOWEST_DIRTY: &str = r#"
    SELECT MIN(window_start_ms) FROM segment_dirty
    WHERE exchange = $1 AND symbol = $2 AND interval = $3
      AND ($4::bigint IS NULL OR window_start_ms > $4)
      AND ($5::bigint IS NULL OR window_start_ms < $5)
"#;

/// The bounds on `window_start_ms` are what the key reads: an entry that
/// overlaps the range starts no earlier than the window of the range's
/// first open time.
const PLAN: &str = r#"
    SELECT i.window_start_ms, i.first_open_ms, i.last_open_ms, i.bar_count, i.hash,
           s.payload_len, length(s.data)
    FROM segment_index i JOIN segments s ON s.hash = i.hash
    WHERE i.exchange = $1 AND i.symbol = $2 AND i.interval = $3
      AND i.window_start_ms >= $4 AND i.window_start_ms < $5
      AND i.last_open_ms >= $6 AND i.first_open_ms <= $7
    ORDER BY i.window_start_ms ASC
"#;

const BLOB: &str = "SELECT data FROM segments WHERE hash = $1";

/// A read-only transaction whose statements all see one moment of the
/// store, whichever writers commit meanwhile. It takes no lock a writer
/// waits for.
async fn snapshot(client: &mut ClientWrapper) -> Result<Transaction<'_>, tokio_postgres::Error> {
    client
        .build_transaction()
        .isolation_level(IsolationLevel::RepeatableRead)
        .read_only(true)
        .start()
        .await
}

/// An index entry as the database returned it, before its name is read.
struct Entry {
    window_start_ms: TimestampMs,
    first_open_ms: TimestampMs,
    last_open_ms: TimestampMs,
    bar_count: i32,
    hash: String,
    payload_len: i32,
    stored_len: i32,
}

impl Entry {
    fn of(row: &Row) -> Result<Self, tokio_postgres::Error> {
        Ok(Self {
            window_start_ms: row.try_get(0)?,
            first_open_ms: row.try_get(1)?,
            last_open_ms: row.try_get(2)?,
            bar_count: row.try_get(3)?,
            hash: row.try_get(4)?,
            payload_len: row.try_get(5)?,
            stored_len: row.try_get(6)?,
        })
    }

    fn segment(self) -> Result<SegmentRef, StorageError> {
        Ok(SegmentRef {
            window_start_ms: self.window_start_ms,
            first_open_ms: self.first_open_ms,
            last_open_ms: self.last_open_ms,
            bar_count: u32::try_from(self.bar_count).map_err(StorageError::other)?,
            hash: self
                .hash
                .parse::<SegmentHash>()
                .map_err(StorageError::other)?,
            payload_len: u64::try_from(self.payload_len).map_err(StorageError::other)?,
            stored_len: u64::try_from(self.stored_len).map_err(StorageError::other)?,
        })
    }
}

impl SegmentStore for PostgresStore {
    fn refresh_segments(&self, market: &MarketId) -> Result<SegmentRefresh, StorageError> {
        refresh::refresh(&mut &*self, market)
    }

    fn resolve_segments(
        &self,
        market: &MarketId,
        first_open_ms: TimestampMs,
        last_open_ms: TimestampMs,
    ) -> Result<SegmentPlan, StorageError> {
        refresh::check_range(first_open_ms, last_open_ms)?;
        let key = Key::of(market);
        let interval = market.interval;
        // A first open time too extreme for any window bounds nothing.
        let lowest_entry =
            format::window(interval, first_open_ms).map_or(TimestampMs::MIN, |window| window.start);
        let found = self.bridge.execute(Effect::Read, move |client| {
            Box::pin(async move {
                let transaction = snapshot(client).await?;
                let newest: Option<TimestampMs> = transaction
                    .query_typed_one(
                        MAX_OPEN_TIME,
                        &[
                            (&key.exchange, Type::TEXT),
                            (&key.symbol, Type::TEXT),
                            (&key.interval, Type::TEXT),
                        ],
                    )
                    .await?
                    .try_get(0)?;
                let Some(newest) = newest else {
                    return Ok(Ok(None));
                };
                let tip_window = match common::window_start(interval, newest) {
                    Ok(tip_window) => tip_window,
                    Err(refused) => return Ok(Err(refused)),
                };
                let lowest_dirty: Option<TimestampMs> = transaction
                    .query_typed_one(
                        LOWEST_DIRTY,
                        &[
                            (&key.exchange, Type::TEXT),
                            (&key.symbol, Type::TEXT),
                            (&key.interval, Type::TEXT),
                            (&None::<TimestampMs>, Type::INT8),
                            (&None::<TimestampMs>, Type::INT8),
                        ],
                    )
                    .await?
                    .try_get(0)?;
                let tip_start = refresh::tip_start(tip_window, lowest_dirty);
                let entries = transaction
                    .query_typed(
                        PLAN,
                        &[
                            (&key.exchange, Type::TEXT),
                            (&key.symbol, Type::TEXT),
                            (&key.interval, Type::TEXT),
                            (&lowest_entry, Type::INT8),
                            (&tip_start, Type::INT8),
                            (&first_open_ms, Type::INT8),
                            (&last_open_ms, Type::INT8),
                        ],
                    )
                    .await?
                    .iter()
                    .map(Entry::of)
                    .collect::<Result<Vec<_>, _>>()?;
                transaction.commit().await?;
                Ok(Ok(Some((tip_start, entries))))
            })
        })??;
        let Some((tip_start, entries)) = found else {
            return Ok(refresh::all_tip(first_open_ms, last_open_ms));
        };
        let segments = entries
            .into_iter()
            .map(Entry::segment)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(refresh::plan(
            segments,
            tip_start,
            first_open_ms,
            last_open_ms,
        ))
    }

    fn load_segment(&self, hash: &SegmentHash) -> Result<Option<Vec<u8>>, StorageError> {
        let hash = hash.to_string();
        self.bridge.execute(Effect::Read, move |client| {
            Box::pin(async move {
                client
                    .query_typed_opt(BLOB, &[(&hash, Type::TEXT)])
                    .await?
                    .map(|row| row.try_get(0))
                    .transpose()
            })
        })
    }
}
