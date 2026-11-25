//! The bars a job runs on, read as it starts: those of the segments that
//! hold its range, each opened against its name, then those no segment
//! holds yet, from the candle store, packed window by window into segments
//! of the job's own, which its provenance names beside the others. A
//! rerun reads the segments a provenance names the same way.

use std::sync::Arc;

use tokio::sync::OwnedSemaphorePermit;
use tracing::error;

use super::Jobs;
use super::queue::{Ended, failed, patiently};
use super::worker::Cancel;
use crate::model::{
    Candle, Interval, MarketId, PackedSegment, SegmentHash, TimestampMs, ms_to_rfc3339,
};
use crate::ports::CandleQuery;
use crate::protocol::jobs::ProvenanceSegment;
use crate::protocol::{ContentHash, ErrorCode};
use crate::server::refusal::Refusal;
use crate::server::selection::{Selection, named};
use crate::storage::segments;

/// How many candles one store call reads for a job.
const CHUNK: usize = 50_000;

/// How many segments one store call reads: about as many bars as `CHUNK`.
const SEGMENTS_PER_CALL: usize = 32;

/// A job's bars, oldest first, and the segments that hold them: those of
/// the store, and those packed for the job.
pub(super) struct Bars {
    pub(super) candles: Vec<Candle>,
    pub(super) segments: Vec<ProvenanceSegment>,
    pub(super) packed: Vec<PackedSegment>,
}

/// Why a segment a job names could not be read.
pub(super) enum SegmentFault {
    /// The store holds nothing under its name.
    Missing,
    /// What the store holds under its name is not that segment.
    Damaged(String),
}

/// Why a job's bars could not be read.
pub(super) enum Fault {
    /// The store refused, or stayed out of time.
    Refused(Refusal),
    /// The job ends here, with its outcome.
    Ended(Ended),
}

impl From<Refusal> for Fault {
    fn from(refusal: Refusal) -> Self {
        Self::Refused(refusal)
    }
}

/// The job's bars: those of the segments that hold its range, then those
/// no segment holds yet, from the candle store, packed for the job window
/// by window. `None` when the job was cancelled meanwhile.
pub(super) async fn read_bars(
    jobs: &Jobs,
    market: &MarketId,
    selection: &Selection,
    cancel: &Cancel,
) -> Result<Option<Bars>, Fault> {
    let (start, end) = (selection.start_open_ms, selection.end_open_ms);
    let planned = market.clone();
    let plan = patiently(jobs, move |store| {
        store
            .resolve_segments(&planned, start, end)
            .map_err(|failure| Refusal::storage("planning a job's range", failure))
    })
    .await?;
    let hashes: Vec<SegmentHash> = plan.segments.iter().map(|segment| segment.hash).collect();
    let Some(opened) = read_segments(jobs, &hashes, cancel, None).await? else {
        return Ok(None);
    };
    let mut bars = Bars {
        candles: Vec::new(),
        segments: Vec::new(),
        packed: Vec::new(),
    };
    for (segment, opened) in plan.segments.iter().zip(opened) {
        let candles = opened.map_err(|fault| {
            let why = match fault {
                SegmentFault::Missing => "the store holds nothing under its name".to_string(),
                SegmentFault::Damaged(why) => why,
            };
            error!(segment = %segment.hash, %why, "a segment a job reads cannot be read");
            Fault::Ended(Ended::Finished(failed(
                ErrorCode::Internal,
                format!("the server's copy of segment {} is damaged", segment.hash),
            )))
        })?;
        bars.candles.extend(
            candles
                .into_iter()
                .filter(|candle| (start..=end).contains(&candle.open_time_ms)),
        );
        bars.segments.push(ProvenanceSegment {
            hash: ContentHash::sha256(*segment.hash.digest()),
            first_open_ms: segment.first_open_ms,
            last_open_ms: segment.last_open_ms,
            bar_count: segment.bar_count,
        });
    }
    let Some(tip) = plan.tip else {
        return Ok(Some(bars));
    };
    let Some(candles) =
        read_candles(jobs, market, tip.first_open_ms, tip.last_open_ms, cancel).await?
    else {
        return Ok(None);
    };
    let interval = market.interval;
    let packed = tokio::task::spawn_blocking(move || pack_windows(interval, candles))
        .await
        .map_err(|_| Refusal::internal("the server failed while packing a job's bars"))?;
    let (candles, windows) = packed.map_err(|(window_start_ms, why)| {
        Fault::Ended(Ended::Finished(failed(
            ErrorCode::InvalidRequest,
            format!(
                "the bars of {} from {} cannot be packed into a segment, so the job cannot be \
                 recorded: {why}; `quantforge data validate` shows what is wrong with them",
                named(market),
                ms_to_rfc3339(window_start_ms)
            ),
        )))
    })?;
    bars.candles.extend(candles);
    for (packed, segment) in windows {
        bars.packed.push(packed);
        bars.segments.push(segment);
    }
    Ok(Some(bars))
}

/// Each segment's bars, read from the store a few segments per call and
/// opened against their names, in the order given. `None` when `cancel`
/// was sent meanwhile. Each call holds `slot`, a rerun's, until it ends:
/// a rerun's handler is dropped when its client hangs up, while a job's
/// own task outlives its calls.
pub(super) async fn read_segments(
    jobs: &Jobs,
    hashes: &[SegmentHash],
    cancel: &Cancel,
    slot: Option<&Arc<OwnedSemaphorePermit>>,
) -> Result<Option<Vec<Result<Vec<Candle>, SegmentFault>>>, Refusal> {
    let mut opened = Vec::with_capacity(hashes.len());
    for chunk in hashes.chunks(SEGMENTS_PER_CALL) {
        if cancel.is_cancelled() {
            return Ok(None);
        }
        let (chunk, held) = (Arc::new(chunk.to_vec()), slot.cloned());
        let read = patiently(jobs, move |store| {
            let _held = &held;
            chunk
                .iter()
                .map(|hash| {
                    let stored = store
                        .load_segment(hash)
                        .map_err(|failure| Refusal::storage("loading a segment", failure))?;
                    Ok(match stored {
                        None => Err(SegmentFault::Missing),
                        Some(bytes) => segments::open(&bytes, hash)
                            .map_err(|failure| SegmentFault::Damaged(failure.to_string())),
                    })
                })
                .collect::<Result<Vec<_>, Refusal>>()
        })
        .await?;
        opened.extend(read);
    }
    Ok(Some(opened))
}

/// `market`'s candles from `first` to `last`, a chunk per store call.
/// `None` when `cancel` was sent meanwhile.
async fn read_candles(
    jobs: &Jobs,
    market: &MarketId,
    first: TimestampMs,
    last: TimestampMs,
    cancel: &Cancel,
) -> Result<Option<Vec<Candle>>, Refusal> {
    let mut candles: Vec<Candle> = Vec::new();
    let mut next = first;
    loop {
        if cancel.is_cancelled() {
            return Ok(None);
        }
        let market = market.clone();
        let chunk = patiently(jobs, move |store| {
            store
                .load_candles(
                    &market,
                    CandleQuery {
                        start_time_ms: Some(next),
                        end_time_ms: Some(last),
                        limit: Some(CHUNK),
                    },
                )
                .map_err(|failure| Refusal::storage("reading a job's candles", failure))
        })
        .await?;
        let newest = chunk.last().map(|candle| candle.open_time_ms);
        let full = chunk.len() == CHUNK;
        candles.extend(chunk);
        match newest {
            // Open times lie within 2^53 of zero, so the step never
            // overflows.
            Some(newest) if full => next = newest + 1,
            _ => return Ok(Some(candles)),
        }
    }
}

/// A window's bars packed into a segment of a job's own, with the entry
/// its provenance lists.
type Window = (PackedSegment, ProvenanceSegment);

/// `candles`, oldest first, packed window by window, and the candles
/// themselves; or the start of the first window that cannot be packed, and
/// why.
fn pack_windows(
    interval: Interval,
    candles: Vec<Candle>,
) -> Result<(Vec<Candle>, Vec<Window>), (TimestampMs, String)> {
    let mut windows = Vec::new();
    let mut from = 0;
    while from < candles.len() {
        let open_time_ms = candles[from].open_time_ms;
        let window = segments::window(interval, open_time_ms)
            .ok_or((open_time_ms, "its window cannot be written".to_string()))?;
        let to = candles[from..]
            .iter()
            .position(|candle| !window.contains(&candle.open_time_ms))
            .map_or(candles.len(), |offset| from + offset);
        windows.push(pack(&candles[from..to]).map_err(|why| (window.start, why))?);
        from = to;
    }
    Ok((candles, windows))
}

/// One window's bars packed into a segment: its stored bytes under its
/// name, and the entry a provenance lists for it.
fn pack(candles: &[Candle]) -> Result<Window, String> {
    let payload = segments::encode(candles).map_err(|failure| failure.to_string())?;
    let hash = segments::hash(&payload);
    let data = segments::compress(&payload).map_err(|failure| failure.to_string())?;
    let (Some(first), Some(last)) = (candles.first(), candles.last()) else {
        return Err("a window of no bars".to_string());
    };
    let bar_count = u32::try_from(candles.len()).map_err(|_| "too many bars".to_string())?;
    Ok((
        PackedSegment {
            hash,
            payload_len: u64::try_from(payload.len()).unwrap_or(u64::MAX),
            data,
        },
        ProvenanceSegment {
            hash: ContentHash::sha256(*hash.digest()),
            first_open_ms: first.open_time_ms,
            last_open_ms: last.open_time_ms,
            bar_count,
        },
    ))
}
