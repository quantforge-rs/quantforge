//! What every backend's segment store shares, so both refresh and plan
//! alike: the refresh loop over a backend's [`DirtyWindows`], a window's
//! snapshot and what packing it decides, and what a plan makes of the tip
//! and the marks.
//!
//! Each dirty window below the tip is read in a snapshot, packed with no
//! transaction open, and written back in a short transaction that first
//! checks the window was not marked again meanwhile. Packing never holds a
//! write lock, so a writer such as `trade run`'s poll loop is never kept
//! waiting for it, and every window commits on its own, so an interrupted
//! refresh loses nothing it finished.

use std::ops::Range;

use tracing::warn;

use crate::storage::common;
use crate::storage::segments::{self as format, SegmentError, TextCandle};
use crate::{
    MarketId, SegmentHash, SegmentPlan, SegmentRef, SegmentRefresh, SegmentTip, StorageError,
    TimestampMs, ms_to_rfc3339,
};

/// How often a window is read again when a writer keeps marking it between
/// the read and the write. After that the next refresh takes it up.
const ATTEMPTS: usize = 3;

/// A backend's side of a refresh: where the tip is, which windows are
/// dirty, and reading and writing back one of them.
pub(crate) trait DirtyWindows {
    /// The start of the window holding the market's newest candle, which is
    /// never packed: candles may still join it. `None` without candles.
    fn tip_window(&mut self, market: &MarketId) -> Result<Option<TimestampMs>, StorageError>;

    /// The lowest dirty window after `after`, below the tip window.
    fn next_dirty(
        &mut self,
        market: &MarketId,
        after: Option<TimestampMs>,
        tip_window: TimestampMs,
    ) -> Result<Option<TimestampMs>, StorageError>;

    /// Reads a dirty window in one snapshot, taking no write lock. `None`
    /// when it is no longer marked.
    fn read_window(
        &mut self,
        market: &MarketId,
        window: &Range<TimestampMs>,
    ) -> Result<Option<Snapshot>, StorageError>;

    /// Writes a packed window back and unmarks it, if it still carries the
    /// mark the snapshot read: then no candle of it changed since.
    fn write_window(
        &mut self,
        market: &MarketId,
        window_start: TimestampMs,
        id: i64,
        packed: &Packed,
    ) -> Result<Written, StorageError>;
}

/// Packs every dirty window below the tip.
pub(crate) fn refresh(
    windows: &mut impl DirtyWindows,
    market: &MarketId,
) -> Result<SegmentRefresh, StorageError> {
    let mut refresh = SegmentRefresh::default();
    let Some(tip_window) = windows.tip_window(market)? else {
        return Ok(refresh);
    };
    let mut after = None;
    while let Some(window_start) = windows.next_dirty(market, after, tip_window)? {
        match build_window(windows, market, window_start)? {
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

fn build_window(
    windows: &mut impl DirtyWindows,
    market: &MarketId,
    window_start: TimestampMs,
) -> Result<Built, StorageError> {
    let window = format::window(market.interval, window_start)
        .ok_or_else(|| common::out_of_range(window_start))?;
    for _ in 0..ATTEMPTS {
        let Some(snapshot) = windows.read_window(market, &window)? else {
            return Ok(Built::Elsewhere);
        };
        let packed = match pack(&snapshot) {
            Ok(packed) => packed,
            Err(error) => return Ok(Built::Blocked(error)),
        };
        match windows.write_window(market, window.start, snapshot.id, &packed)? {
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
pub(crate) struct Snapshot {
    /// Its mark, which a writer replaces whenever it changes the window.
    pub(crate) id: i64,
    pub(crate) candles: Vec<TextCandle>,
    /// The name it is indexed under, if any.
    pub(crate) indexed: Option<String>,
}

/// What writing a window back does.
#[derive(Clone)]
pub(crate) enum Packed {
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
pub(crate) fn pack(snapshot: &Snapshot) -> Result<Packed, SegmentError> {
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
pub(crate) enum Written {
    Done,
    /// Another refresh unmarked the window first.
    Elsewhere,
    /// A writer marked the window again after the snapshot.
    Marked,
}

/// Refuses a range that ends before it starts.
pub(crate) fn check_range(
    first_open_ms: TimestampMs,
    last_open_ms: TimestampMs,
) -> Result<(), StorageError> {
    if first_open_ms > last_open_ms {
        return Err(StorageError::InvalidArgument(format!(
            "the first open time {first_open_ms} is after the last, {last_open_ms}"
        )));
    }
    Ok(())
}

/// The plan of a market without candles: the whole range is tip.
pub(crate) fn all_tip(first_open_ms: TimestampMs, last_open_ms: TimestampMs) -> SegmentPlan {
    SegmentPlan {
        segments: Vec::new(),
        tip: Some(SegmentTip {
            first_open_ms,
            last_open_ms,
        }),
    }
}

/// Where a plan stops trusting the index: at the tip window, or at the
/// lowest dirty window below it.
pub(crate) fn tip_start(tip_window: TimestampMs, lowest_dirty: Option<TimestampMs>) -> TimestampMs {
    lowest_dirty.map_or(tip_window, |dirty| dirty.min(tip_window))
}

/// The plan of a range: the entries below `tip_start` that overlap it, then
/// whatever of it lies from `tip_start` on.
pub(crate) fn plan(
    segments: Vec<SegmentRef>,
    tip_start: TimestampMs,
    first_open_ms: TimestampMs,
    last_open_ms: TimestampMs,
) -> SegmentPlan {
    SegmentPlan {
        segments,
        tip: (last_open_ms >= tip_start).then(|| SegmentTip {
            first_open_ms: first_open_ms.max(tip_start),
            last_open_ms,
        }),
    }
}
