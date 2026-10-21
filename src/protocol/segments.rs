//! Stored candles, served as segments.
//!
//! A segment is one market's closed bars from one window of open times,
//! encoded once and never changed afterwards, so its [`ContentHash`] names
//! it for good. A server lists the segments it holds as a [`SegmentIndex`]
//! and serves each one by its hash. How a segment's bytes are laid out, and
//! so which bytes its hash covers, is fixed by its
//! [`format`](SegmentEntry::format): format 1 is the [segment spec]. Its
//! hash covers the segment's payload, and the payload travels as one zstd
//! frame, so a client decompresses what it fetched before checking the hash.
//! This module defines the bodies; the routes that list and fetch segments
//! come with the server that holds them.
//!
//! [segment spec]: <https://github.com/quantforge-rs/quantforge/blob/main/docs/segment-spec.md>

use serde::{Deserialize, Serialize};

use super::ContentHash;

/// The segment format this build writes and reads: format 1, the
/// [segment spec](https://github.com/quantforge-rs/quantforge/blob/main/docs/segment-spec.md).
pub const SEGMENT_FORMAT: u32 = 1;

/// The segments a server holds.
///
/// ```
/// use quantforge::protocol::segments::{SEGMENT_FORMAT, SegmentIndex};
///
/// let json = r#"{"segments":[{"hash":"sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855","format":1,"exchange":"binance","symbol":"BTCUSDT","interval":"1m","range":{"first_open_ms":1792454400000,"last_open_ms":1792540740000,"bar_count":1440},"size_bytes":48213}]}"#;
/// let index: SegmentIndex = serde_json::from_str(json)?;
/// assert_eq!(index.segments[0].format, SEGMENT_FORMAT);
/// assert_eq!(index.segments[0].range.bar_count, 1440);
/// assert_eq!(serde_json::to_string(&index)?, json);
/// # Ok::<(), serde_json::Error>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentIndex {
    /// One entry per segment.
    pub segments: Vec<SegmentEntry>,
}

/// One segment: which bars it holds, and the name to fetch them by.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentEntry {
    /// The segment's name: the SHA-256 of its payload, which the fetched
    /// bytes must decompress to.
    pub hash: ContentHash,
    /// How the segment's bytes are laid out, which also fixes the bytes
    /// `hash` covers: [`SEGMENT_FORMAT`] today. A reader refuses a format it
    /// does not know.
    pub format: u32,
    /// The exchange, by the name QuantForge registers it under.
    pub exchange: String,
    /// The exchange's symbol, in uppercase.
    pub symbol: String,
    /// The bar interval, such as `1m`.
    pub interval: String,
    /// The bars it holds.
    pub range: SegmentRange,
    /// The size of the segment's bytes as served, compressed, to check a
    /// download against.
    pub size_bytes: u64,
}

/// The bars a segment holds: every bar the server has whose open time lies
/// from `first_open_ms` to `last_open_ms`. Where the exchange published no
/// bar there is none, so `bar_count` can be lower than the span would hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentRange {
    /// The open time of the first bar.
    pub first_open_ms: i64,
    /// The open time of the last bar.
    pub last_open_ms: i64,
    /// How many bars the segment holds.
    pub bar_count: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    const EMPTY: &str = "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    fn entry() -> SegmentEntry {
        SegmentEntry {
            hash: EMPTY.parse().expect("hash"),
            format: SEGMENT_FORMAT,
            exchange: "binance".to_string(),
            symbol: "BTCUSDT".to_string(),
            interval: "1h".to_string(),
            range: SegmentRange {
                first_open_ms: 1_792_454_400_000,
                last_open_ms: 1_792_537_200_000,
                bar_count: 23,
            },
            size_bytes: 1_204,
        }
    }

    #[test]
    fn an_index_has_its_exact_json() {
        let index = SegmentIndex {
            segments: vec![entry()],
        };
        let json = format!(
            r#"{{"segments":[{{"hash":"{EMPTY}","format":1,"exchange":"binance","symbol":"BTCUSDT","interval":"1h","range":{{"first_open_ms":1792454400000,"last_open_ms":1792537200000,"bar_count":23}},"size_bytes":1204}}]}}"#
        );
        assert_eq!(serde_json::to_string(&index).expect("serialize"), json);
        assert_eq!(
            serde_json::from_str::<SegmentIndex>(&json).expect("index"),
            index
        );
    }

    #[test]
    fn an_empty_index_is_an_object_with_an_empty_list() {
        let empty = SegmentIndex {
            segments: Vec::new(),
        };
        assert_eq!(
            serde_json::to_string(&empty).expect("serialize"),
            r#"{"segments":[]}"#
        );
        assert!(serde_json::from_str::<SegmentIndex>("[]").is_err());
    }

    #[test]
    fn fields_this_build_does_not_know_are_ignored() {
        let json = format!(
            r#"{{"hash":"{EMPTY}","format":1,"exchange":"binance","symbol":"BTCUSDT","interval":"1h","range":{{"first_open_ms":1792454400000,"last_open_ms":1792537200000,"bar_count":23,"gaps":1}},"size_bytes":1204,"codec":"zstd"}}"#
        );
        assert_eq!(
            serde_json::from_str::<SegmentEntry>(&json).expect("entry"),
            entry()
        );
    }
}
