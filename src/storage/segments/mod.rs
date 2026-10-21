//! Segment format 1: how a market's closed candles are packed, window by
//! window, into payloads that are named by their hash and never change.
//! This module is the reference implementation of the
//! [segment spec](https://github.com/quantforge-rs/quantforge/blob/main/docs/segment-spec.md),
//! which other languages implement from; every storage backend packs its
//! segments with it.
//!
//! - A window is [`WINDOW_BARS`] bars of the interval, counted from the Unix
//!   epoch ([`window`]): one UTC day of `1m` candles.
//! - A window's payload is its candles as columns, every decimal in its
//!   exact text ([`encode`], [`decode`]), so the same candles always make
//!   the same bytes.
//! - A segment is named by the SHA-256 of its payload ([`hash`]), and is
//!   stored and sent as one zstd frame around it ([`compress`],
//!   [`decompress`]). [`open`] reads a fetched segment back, checking it
//!   against its name.
//!
//! Decimals are exact text because a value keeps its scale: `1.10` and
//! `1.1` are equal numbers but different candles, and different segments.

mod payload;

use std::io;
use std::ops::Range;

use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{Candle, Interval, SegmentHash, TimestampMs};

pub(crate) use payload::{TextCandle, encode_text};
pub use payload::{decode, encode};

/// The segment format this build writes and reads.
pub const FORMAT: u32 = 1;

/// How many bars of the interval a window spans, and so the most candles a
/// segment holds.
pub const WINDOW_BARS: u32 = 1440;

/// The longest decimal text: a sign, 29 digits, and the point.
pub const MAX_DECIMAL_LEN: usize = 31;

/// The largest payload: the header, then per candle two times, a trade
/// count and its flag, and five decimals with their lengths.
pub const MAX_PAYLOAD_BYTES: usize =
    16 + WINDOW_BARS as usize * (8 + 8 + 8 + 1 + 5 * (1 + MAX_DECIMAL_LEN));

/// The zstd level this build compresses at. It is not part of the format:
/// the hash covers the payload, so a later build may change it.
const LEVEL: i32 = 3;

/// Why candles cannot be packed, or bytes are not a segment.
#[derive(Debug, Error)]
pub enum SegmentError {
    #[error("a segment holds 1 to {WINDOW_BARS} candles, not {count}")]
    BarCount { count: usize },

    #[error("candle open times must rise, but {open_time_ms} follows {previous_ms}")]
    OutOfOrder {
        previous_ms: TimestampMs,
        open_time_ms: TimestampMs,
    },

    #[error(
        "the {column} of the candle opened at {open_time_ms} is {text:?}, which is not an exact \
         decimal in its written form"
    )]
    InvalidDecimal {
        column: &'static str,
        open_time_ms: TimestampMs,
        text: String,
    },

    #[error("not a segment payload: {reason}")]
    Malformed { reason: &'static str },

    #[error("segment format {0} is not one this build reads; it reads format {FORMAT}")]
    UnsupportedFormat(u32),

    #[error("a segment is exactly one zstd frame")]
    NotOneFrame,

    #[error("the segment's payload is larger than the {MAX_PAYLOAD_BYTES} bytes a payload can be")]
    TooLarge,

    #[error("zstd failed")]
    Zstd(#[source] io::Error),

    #[error("the payload hashes to {found}, not to {expected}")]
    HashMismatch {
        expected: SegmentHash,
        found: SegmentHash,
    },
}

/// How long a window of `interval` candles lasts.
pub fn span_ms(interval: Interval) -> TimestampMs {
    interval.step_ms() * i64::from(WINDOW_BARS)
}

/// The window holding a candle that opens at `open_time_ms`: the open times
/// from a multiple of the span, counted from the Unix epoch, up to the next
/// one. `None` within a span of either end of the timestamp range, where a
/// window's bounds cannot be written.
pub fn window(interval: Interval, open_time_ms: TimestampMs) -> Option<Range<TimestampMs>> {
    let span = span_ms(interval);
    let start = open_time_ms.div_euclid(span).checked_mul(span)?;
    let end = start.checked_add(span)?;
    Some(start..end)
}

/// The name of a payload.
pub fn hash(payload: &[u8]) -> SegmentHash {
    SegmentHash::from_digest(Sha256::digest(payload).into())
}

/// The bytes a segment is stored and sent as: one zstd frame holding the
/// payload, with its size in the frame header.
pub fn compress(payload: &[u8]) -> Result<Vec<u8>, SegmentError> {
    zstd::bulk::compress(payload, LEVEL).map_err(SegmentError::Zstd)
}

/// The payload inside a segment's stored bytes. Anything but exactly one
/// zstd frame is refused, and so is a payload larger than
/// [`MAX_PAYLOAD_BYTES`], before more than that is allocated.
pub fn decompress(bytes: &[u8]) -> Result<Vec<u8>, SegmentError> {
    let frame_len = zstd::zstd_safe::find_frame_compressed_size(bytes)
        .map_err(|_| SegmentError::NotOneFrame)?;
    if frame_len != bytes.len() {
        return Err(SegmentError::NotOneFrame);
    }
    if let Ok(Some(size)) = zstd::zstd_safe::get_frame_content_size(bytes) {
        if size > MAX_PAYLOAD_BYTES as u64 {
            return Err(SegmentError::TooLarge);
        }
    }
    zstd::bulk::decompress(bytes, MAX_PAYLOAD_BYTES).map_err(SegmentError::Zstd)
}

/// The candles of a fetched segment named `expected`: its bytes
/// decompressed, checked against the name, and decoded.
pub fn open(bytes: &[u8], expected: &SegmentHash) -> Result<Vec<Candle>, SegmentError> {
    let payload = decompress(bytes)?;
    let found = hash(&payload);
    if found != *expected {
        return Err(SegmentError::HashMismatch {
            expected: *expected,
            found,
        });
    }
    decode(&payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    // 2026-10-20T00:00:00Z, a UTC midnight.
    const MIDNIGHT: TimestampMs = 1_792_454_400_000;
    const DAY: TimestampMs = 86_400_000;

    #[test]
    fn a_window_of_one_minute_candles_is_a_utc_day() {
        for open_time_ms in [
            MIDNIGHT,
            MIDNIGHT + 13 * 3_600_000 + 37 * 60_000,
            MIDNIGHT + DAY - 60_000,
        ] {
            assert_eq!(
                window(Interval::M1, open_time_ms),
                Some(MIDNIGHT..MIDNIGHT + DAY)
            );
        }
        assert_eq!(
            window(Interval::M1, MIDNIGHT + DAY),
            Some(MIDNIGHT + DAY..MIDNIGHT + 2 * DAY)
        );
        assert_eq!(span_ms(Interval::S1), 1_440_000);
        assert_eq!(span_ms(Interval::H1), 60 * DAY);
    }

    #[test]
    fn a_full_window_holds_exactly_its_bars_wherever_the_grid_starts() {
        // Weekly bars open on Mondays, four days off the epoch's Thursday.
        let monday = 1_791_936_000_000; // 2026-10-19T00:00:00Z
        let week = Interval::W1.step_ms();
        let first = window(Interval::W1, monday).expect("window");
        let in_window = (-3000..3000)
            .map(|index| monday + index * week)
            .filter(|open_time_ms| first.contains(open_time_ms))
            .count();
        assert_eq!(in_window, WINDOW_BARS as usize);
    }

    #[test]
    fn times_before_the_epoch_fall_in_the_window_below_them() {
        assert_eq!(window(Interval::M1, -1), Some(-DAY..0));
        assert_eq!(window(Interval::M1, -DAY), Some(-DAY..0));
        assert_eq!(window(Interval::M1, 0), Some(0..DAY));
    }

    #[test]
    fn a_window_whose_bounds_cannot_be_written_is_none() {
        assert_eq!(window(Interval::M1, i64::MIN), None);
        assert_eq!(window(Interval::M1, i64::MAX), None);
        let last_whole = i64::MAX.div_euclid(DAY) * DAY - DAY;
        assert_eq!(
            window(Interval::M1, last_whole),
            Some(last_whole..last_whole + DAY)
        );
    }

    #[test]
    fn compression_round_trips_the_same_bytes_every_time() {
        let payload: Vec<u8> = (0..20_000u32)
            .flat_map(|i| (i % 251).to_le_bytes())
            .collect();
        let first = compress(&payload).expect("compress");
        assert_eq!(compress(&payload).expect("compress again"), first);
        assert!(first.len() < payload.len());
        assert_eq!(decompress(&first).expect("decompress"), payload);
    }

    #[test]
    fn anything_but_one_frame_of_a_small_enough_payload_is_refused() {
        let frame = compress(b"payload").expect("compress");
        let mut two = frame.clone();
        two.extend_from_slice(&frame);
        let mut trailing = frame.clone();
        trailing.push(0);
        for bytes in [two, trailing, frame[..frame.len() - 1].to_vec(), Vec::new()] {
            assert!(
                matches!(decompress(&bytes), Err(SegmentError::NotOneFrame)),
                "{bytes:?}"
            );
        }

        let bomb = compress(&vec![0; MAX_PAYLOAD_BYTES + 1]).expect("compress");
        assert!(bomb.len() < 1_000, "zeros compress well");
        assert!(matches!(decompress(&bomb), Err(SegmentError::TooLarge)));
        let just_fits = compress(&vec![0; MAX_PAYLOAD_BYTES]).expect("compress");
        assert_eq!(
            decompress(&just_fits).expect("decompress").len(),
            MAX_PAYLOAD_BYTES
        );
    }
}
