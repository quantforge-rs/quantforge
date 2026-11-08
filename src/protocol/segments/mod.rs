//! Stored candles over HTTP: the markets a server stores, one market's bars
//! in a range, and the immutable segments that hold most of them.
//!
//! A segment is one market's closed bars from one window of open times,
//! encoded once and never changed afterwards, so its [`ContentHash`] names
//! it for good. How a segment's bytes are laid out, and so which bytes its
//! hash covers, is fixed by its [`format`](SegmentEntry::format): format 1
//! is the [segment spec]. Its hash covers the segment's payload, and the
//! payload travels as one zstd frame, so a client decompresses what it
//! fetched before checking the hash.
//!
//! [segment spec]: <https://github.com/quantforge-rs/quantforge/blob/main/docs/segment-spec.md>
//!
//! # Routes
//!
//! | Route | Answer |
//! | --- | --- |
//! | `GET /v1/markets` | A [`MarketIndex`]: every market the server stores bars of, and the open times they span |
//! | `GET /v1/candles` | A [`CandleRange`]: one market's bars in a range, as the segments that hold them and the tip after them; its query string is a [`CandleRangeQuery`] |
//! | `GET /v1/segments/{hash}` | One segment's bytes as the server stores them: one zstd frame, as [`SEGMENT_MEDIA_TYPE`] |
//!
//! `HEAD` works wherever `GET` does, and no route needs credentials. A
//! failed request is answered with an [`ErrorEnvelope`](super::ErrorEnvelope)
//! whose code says what to do:
//!
//! | Status and code | When |
//! | --- | --- |
//! | 400 `invalid_request` | A parameter is missing, repeated, or not in its canonical form; a range starts after it ends, or spans more bars than the server serves in one answer, which the message names; a range's tip holds more bars than the server sends in one answer, which a shorter range avoids; an integer is 2^53 or more in magnitude; a market without `category` is ambiguous; a hash is not in its written form |
//! | 404 `not_found` | The server holds no segment of that hash |
//! | 405 `method_not_allowed` | A method other than `GET` or `HEAD`; `Allow` lists those two |
//! | 503 `unavailable` | The server's store could not answer in time; `Retry-After` says how many seconds to wait |
//! | 500 `internal` | The server failed: its store refused, or its copy of a segment does not match the hash |
//!
//! # Reading a range
//!
//! 1. Ask `GET /v1/candles` for the market and the range.
//! 2. Fetch each segment the answer lists, by its hash, from the server or
//!    from any cache that holds it. Refuse bytes whose length is not the
//!    segment's [`size_bytes`](SegmentEntry::size_bytes), decompress them,
//!    refuse a payload whose hash is not the segment's, and decode it by its
//!    format.
//! 3. Keep each segment's bars whose open times lie in the range: the first
//!    and the last segment can hold bars outside it.
//! 4. Append the [`tip`](CandleRange::tip), whose bars all come after the
//!    last segment's and lie in the range.
//!
//! The bars are then exactly the ones the server stores, in open-time
//! order, each price and quantity with the scale the exchange reported, and
//! no bar where the exchange published none.
//!
//! # Caching
//!
//! A segment never changes, so `GET /v1/segments/{hash}` answers with
//! [`SEGMENT_CACHE_CONTROL`], which lets any cache keep it for a year
//! without asking again, and with the entity tag `"<hash>"`, quotes
//! included. A request whose `If-None-Match` names that tag, weakly or not,
//! or is `*`, gets `304 Not Modified` and no body, provided the server holds
//! the segment. The server checks a segment's bytes against its hash before
//! it sends them, so that no cache keeps a damaged copy for a year. The
//! other two routes answer what the server stores at the time, which grows,
//! so they carry `Cache-Control: no-cache`, and a refusal `no-store`.
//!
//! # A market without `category`
//!
//! A `GET /v1/candles` query may leave out `category`: the form a market
//! took before it carried one, when every market was `spot`. The server
//! reads it as `spot` unless the exchange holds the symbol and interval in
//! another family as well, and then refuses it as `invalid_request`, naming
//! the families it holds, rather than guess. Its answer always names the
//! category it served. Every other request and answer names a market with
//! all four strings.
//!
//! # Nothing stored
//!
//! A market the server stores no bars of, or a range before, after or
//! between the segments of the bars it holds, is not an error: the answer
//! lists no segments and no tip, and its [`message`](CandleRange::message)
//! says which of the two it is. A range that falls in a gap inside one
//! segment lists that segment, and none of its bars lies in the range.

mod candles;
mod markets;

use serde::{Deserialize, Serialize};

use super::ContentHash;

pub use candles::{CandleRange, CandleRangeQuery};
pub use markets::{MarketEntry, MarketIndex};

/// The segment format this build writes and reads: format 1, the
/// [segment spec](https://github.com/quantforge-rs/quantforge/blob/main/docs/segment-spec.md).
pub const SEGMENT_FORMAT: u32 = 1;

/// The media type of a segment's bytes as `GET /v1/segments/{hash}` serves
/// them: one zstd frame, sent as it is and never as a content encoding,
/// since the hash covers what the frame decompresses to.
pub const SEGMENT_MEDIA_TYPE: &str = "application/zstd";

/// The `Cache-Control` of a segment: any cache may keep it for a year,
/// 31,536,000 seconds, and need never ask whether it changed.
pub const SEGMENT_CACHE_CONTROL: &str = "public, max-age=31536000, immutable";

/// One segment: which bars it holds, and the name to fetch them by.
///
/// ```
/// use quantforge::protocol::segments::{SEGMENT_FORMAT, SegmentEntry};
///
/// let json = r#"{"hash":"sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855","format":1,"exchange":"binance","category":"spot","symbol":"BTCUSDT","interval":"1m","range":{"first_open_ms":1792454400000,"last_open_ms":1792540740000,"bar_count":1440},"size_bytes":48213}"#;
/// let entry: SegmentEntry = serde_json::from_str(json)?;
/// assert_eq!(entry.format, SEGMENT_FORMAT);
/// assert_eq!(entry.range.bar_count, 1440);
/// assert_eq!(serde_json::to_string(&entry)?, json);
/// # Ok::<(), serde_json::Error>(())
/// ```
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
    /// The product family: `spot`, `linear` or `inverse`. With `exchange` it
    /// names the product line.
    pub category: String,
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

    pub(super) fn entry() -> SegmentEntry {
        SegmentEntry {
            hash: EMPTY.parse().expect("hash"),
            format: SEGMENT_FORMAT,
            exchange: "binance".to_string(),
            category: "spot".to_string(),
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
    fn an_entry_has_its_exact_json() {
        let json = format!(
            r#"{{"hash":"{EMPTY}","format":1,"exchange":"binance","category":"spot","symbol":"BTCUSDT","interval":"1h","range":{{"first_open_ms":1792454400000,"last_open_ms":1792537200000,"bar_count":23}},"size_bytes":1204}}"#
        );
        assert_eq!(serde_json::to_string(&entry()).expect("serialize"), json);
        assert_eq!(
            serde_json::from_str::<SegmentEntry>(&json).expect("entry"),
            entry()
        );
    }

    #[test]
    fn a_segment_is_served_as_one_zstd_frame_cached_for_a_year() {
        assert_eq!(SEGMENT_MEDIA_TYPE, "application/zstd");
        assert_eq!(SEGMENT_CACHE_CONTROL, "public, max-age=31536000, immutable");
        assert_eq!(365 * 24 * 60 * 60, 31_536_000);
    }

    #[test]
    fn fields_this_build_does_not_know_are_ignored() {
        let json = format!(
            r#"{{"hash":"{EMPTY}","format":1,"exchange":"binance","category":"spot","symbol":"BTCUSDT","interval":"1h","range":{{"first_open_ms":1792454400000,"last_open_ms":1792537200000,"bar_count":23,"gaps":1}},"size_bytes":1204,"codec":"zstd"}}"#
        );
        assert_eq!(
            serde_json::from_str::<SegmentEntry>(&json).expect("entry"),
            entry()
        );
    }
}
