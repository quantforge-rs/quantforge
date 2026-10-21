//! Candle segments as the [`SegmentStore`](crate::SegmentStore) port hands
//! them out: what a segment is named, which candles it packs, and which
//! candles of a range are still read from the candle store. The bytes
//! themselves are [`crate::storage::segments`].

use super::{ModelError, TimestampMs};
use std::{fmt, str::FromStr};

const PREFIX: &str = "sha256:";

/// The name of a segment: the SHA-256 digest of its payload, written
/// `sha256:` and 64 lowercase hex digits, the same text as the protocol's
/// content hashes.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SegmentHash([u8; 32]);

impl SegmentHash {
    /// The name of a payload whose SHA-256 digest is `digest`.
    pub const fn from_digest(digest: [u8; 32]) -> Self {
        Self(digest)
    }

    /// The SHA-256 digest.
    pub const fn digest(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for SegmentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(PREFIX)?;
        self.0.iter().try_for_each(|byte| write!(f, "{byte:02x}"))
    }
}

impl fmt::Debug for SegmentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SegmentHash({self})")
    }
}

/// Reads the written form back. Uppercase hex is refused, so that each hash
/// has one spelling to compare and look up by.
impl FromStr for SegmentHash {
    type Err = ModelError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let invalid = || ModelError::InvalidSegmentHash(text.to_string());
        let hex = text.strip_prefix(PREFIX).ok_or_else(invalid)?;
        let lowercase = |byte: u8| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte);
        if hex.len() != 64 || !hex.bytes().all(lowercase) {
            return Err(invalid());
        }
        let mut digest = [0; 32];
        hex::decode_to_slice(hex, &mut digest).map_err(|_| invalid())?;
        Ok(Self(digest))
    }
}

/// One segment as the index lists it: the window it packs, the candles in
/// it, and its name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SegmentRef {
    /// The first open time of the window it packs.
    pub window_start_ms: TimestampMs,
    /// The open time of its first candle.
    pub first_open_ms: TimestampMs,
    /// The open time of its last candle.
    pub last_open_ms: TimestampMs,
    /// How many candles it holds: fewer than the window has room for where
    /// the exchange published none, or none were synced.
    pub bar_count: u32,
    /// Its name.
    pub hash: SegmentHash,
    /// The size of its payload.
    pub payload_len: u64,
    /// The size of its stored bytes, the payload zstd-compressed: what a
    /// server sends for it.
    pub stored_len: u64,
}

/// Where the candles of a range come from: the segments that hold them, in
/// order, then the tip, whose candles are read from the candle store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SegmentPlan {
    /// The segments holding candles of the range, oldest first. The first
    /// and the last can also hold candles outside it.
    pub segments: Vec<SegmentRef>,
    /// The rest of the range, after every segment: candles not packed yet.
    /// `None` when the segments cover the whole range.
    pub tip: Option<SegmentTip>,
}

/// Open times whose candles are read from the candle store, both bounds
/// included.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SegmentTip {
    pub first_open_ms: TimestampMs,
    pub last_open_ms: TimestampMs,
}

/// What one refresh of a market's segments did, counted in windows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SegmentRefresh {
    /// Packed into a new segment: for the first time, or because their
    /// candles changed.
    pub built: usize,
    /// Repacked into the payload they already had, so their segment stands.
    pub unchanged: usize,
    /// Left unpacked because they cannot be: more candles than a segment
    /// holds, or a stored decimal that is not in its exact form. Every
    /// candle from the first of them on stays in the tip.
    pub blocked: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    const EMPTY: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    #[test]
    fn a_hash_reads_back_its_written_form() {
        let text = format!("sha256:{EMPTY}");
        let hash: SegmentHash = text.parse().expect("hash");
        assert_eq!(hash.digest()[..2], [0xe3, 0xb0]);
        assert_eq!(hash.to_string(), text);
        assert_eq!(format!("{hash:?}"), format!("SegmentHash({text})"));
        assert_eq!(SegmentHash::from_digest(*hash.digest()), hash);
    }

    #[test]
    fn anything_but_sha256_and_64_lowercase_hex_digits_is_refused() {
        for text in [
            String::new(),
            EMPTY.to_string(),
            format!("sha256:{}", EMPTY.to_uppercase()),
            format!("SHA256:{EMPTY}"),
            format!("sha256:{EMPTY}0"),
            format!("sha256:{}", &EMPTY[1..]),
            format!("sha256:{}g", &EMPTY[1..]),
            format!(" sha256:{EMPTY}"),
        ] {
            assert!(
                matches!(text.parse::<SegmentHash>(), Err(ModelError::InvalidSegmentHash(input)) if input == text),
                "{text:?}"
            );
        }
    }
}
