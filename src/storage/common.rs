//! What every backend's stores share: reading a stored decimal back exactly,
//! and the segment window a candle marks when it changes.

use std::fmt;

use rust_decimal::Decimal;

use crate::storage::segments as format;
use crate::{Interval, StorageError, TimestampMs};

/// A stored decimal text no `Decimal` holds exactly: more than 28 decimal
/// places, or beyond its range. QuantForge writes `Decimal::to_string`,
/// which always reads back, so such text was written behind the store's
/// back; it is refused, never rounded.
#[derive(Debug)]
pub(crate) struct InexactDecimal {
    text: String,
    reason: String,
}

impl fmt::Display for InexactDecimal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "stored decimal {:?} cannot be read exactly: {}",
            self.text, self.reason
        )
    }
}

impl std::error::Error for InexactDecimal {}

impl From<InexactDecimal> for StorageError {
    fn from(error: InexactDecimal) -> Self {
        StorageError::InvalidArgument(error.to_string())
    }
}

/// Reads a stored decimal with its scale, or refuses text it cannot hold
/// exactly.
pub(crate) fn read_decimal(text: &str) -> Result<Decimal, InexactDecimal> {
    Decimal::from_str_exact(text).map_err(|error| InexactDecimal {
        text: text.to_string(),
        reason: error.to_string(),
    })
}

/// The start of the segment window holding `open_time_ms`; a candle whose
/// window cannot be written is refused before anything is stored.
pub(crate) fn window_start(
    interval: Interval,
    open_time_ms: TimestampMs,
) -> Result<TimestampMs, StorageError> {
    format::window(interval, open_time_ms)
        .map(|window| window.start)
        .ok_or_else(|| out_of_range(open_time_ms))
}

/// The error for a candle whose segment window cannot be written.
pub(crate) fn out_of_range(open_time_ms: TimestampMs) -> StorageError {
    StorageError::InvalidArgument(format!(
        "candle open time {open_time_ms} lies too close to the end of the timestamp range to \
         belong to a segment window"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stored_decimal_reads_back_with_its_scale_or_is_refused_never_rounded() {
        for text in [
            "1.10",
            "1.1",
            "0.00000000",
            "-0.5",
            "79228162514264337593543950335",
        ] {
            assert_eq!(read_decimal(text).expect("exact").to_string(), text);
        }
        for text in [
            "1.00000000000000000000000000001",
            "79228162514264337593543950336",
            "one",
        ] {
            let refused = StorageError::from(read_decimal(text).expect_err("inexact"));
            assert!(
                matches!(&refused, StorageError::InvalidArgument(message)
                    if message.contains(&format!("stored decimal {text:?} cannot be read exactly"))),
                "{refused:?}"
            );
        }
    }
}
