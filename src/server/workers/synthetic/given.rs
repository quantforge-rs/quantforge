//! The workloads that say what candles their job was given: a digest of
//! them, every decimal with its scale, which the tests hold to the candles
//! the server read from its store.

use serde_json::json;
use sha2::{Digest, Sha256};

use super::super::frames::Finished;
use super::report;
use crate::model::Candle;
use crate::protocol::{ErrorBody, ErrorCode};

/// The SHA-256 of the candles as text, one line each, every decimal with
/// its scale: what reaches the worker exactly hashes the same.
pub(super) fn digest(candles: &[Candle]) -> String {
    let mut hash = Sha256::new();
    for candle in candles {
        hash.update(format!(
            "{} {} {} {} {} {} {} {:?}\n",
            candle.open_time_ms,
            candle.close_time_ms,
            candle.open,
            candle.high,
            candle.low,
            candle.close,
            candle.volume,
            candle.trades
        ));
    }
    hex::encode(hash.finalize())
}

/// Succeeds when the candles hash to `expected`, and fails saying what they
/// hash to otherwise.
pub(super) fn expect(candles: &[Candle], expected: &str) -> Finished {
    let actual = digest(candles);
    if actual == expected {
        return report(&json!({ "bars": candles.len(), "digest": actual }));
    }
    Finished::Failed {
        error: ErrorBody {
            code: ErrorCode::InvalidRequest,
            message: format!(
                "the {} candles hash to {actual}, not {expected}",
                candles.len()
            ),
            supported: None,
        },
    }
}
