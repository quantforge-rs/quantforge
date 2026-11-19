//! What every backend's stores share: reading a stored decimal back exactly,
//! reading a stored market's key back, the segment window a candle marks
//! when it changes, and how a refusal names a SQLite file's path.

use std::borrow::Cow;
use std::fmt;
use std::path::Path;
use std::str::FromStr;

use rust_decimal::Decimal;
use tracing::warn;

use crate::storage::segments as format;
use crate::{ExchangeId, Interval, MarketId, StorageError, Symbol, TimestampMs};

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

/// The market a row's key columns name, or `None`, logged, for a key this
/// build cannot read back: a product line it does not know, a symbol not in
/// the form QuantForge writes, or an interval it does not know. Such a key
/// is never read as another market.
pub(crate) fn stored_market(exchange: &str, symbol: &str, interval: &str) -> Option<MarketId> {
    let market = ExchangeId::from_storage_name(exchange).and_then(|line| {
        let symbol = Symbol::new(symbol)
            .ok()
            .filter(|normalized| normalized.as_str() == symbol)?;
        let interval = Interval::from_str(interval).ok()?;
        Some(MarketId::new(line, symbol, interval))
    });
    if market.is_none() {
        warn!(
            exchange,
            symbol, interval, "left out stored candles under a market this build cannot name"
        );
    }
    market
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

/// Whether a path may be a secret given in a path's place: a URL or a
/// connection string with a password holds an `@` or an `=`, and a path to
/// a file rarely does.
pub(crate) fn may_be_a_secret(text: &str) -> bool {
    text.contains(['@', '='])
}

/// A path as a refusal that finds no SQLite database at it names it: whole,
/// or as `the path given` where it may be a secret.
pub(crate) fn path_named(path: &Path) -> Cow<'_, str> {
    let text = path.to_string_lossy();
    if may_be_a_secret(&text) {
        Cow::Borrowed("the path given")
    } else {
        text
    }
}

/// A path as one word of a command a message shows to be run: as it is
/// where a POSIX shell takes it as one word, else in single quotes, as
/// Python's `shlex.quote` writes one. On Windows a backslash is a path's own
/// character.
pub(crate) fn shell_word(path: &Path) -> String {
    let text = path.to_string_lossy();
    let plain = |c: char| {
        c.is_ascii_alphanumeric() || "_@%+=:,./-".contains(c) || (cfg!(windows) && c == '\\')
    };
    if !text.is_empty() && text.chars().all(plain) {
        text.into_owned()
    } else {
        format!("'{}'", text.replace('\'', r#"'"'"'"#))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_that_may_be_a_secret_is_named_without_it() {
        for (path, named) in [
            ("data/market.sqlite", "data/market.sqlite"),
            (
                "/tmp/test stores/market.sqlite",
                "/tmp/test stores/market.sqlite",
            ),
            (
                "postgres://someone:word@db.example.com/market",
                "the path given",
            ),
            ("host=db.example.com password=word", "the path given"),
        ] {
            assert_eq!(path_named(Path::new(path)), named);
        }
    }

    #[test]
    fn a_path_in_a_command_is_one_word_to_a_shell() {
        for (path, word) in [
            ("data/market.sqlite", "data/market.sqlite"),
            (
                "/tmp/a_b-c@d%e+f=g:h,i/market.sqlite",
                "/tmp/a_b-c@d%e+f=g:h,i/market.sqlite",
            ),
            (
                "/tmp/test stores/market.sqlite",
                "'/tmp/test stores/market.sqlite'",
            ),
            (
                "/tmp/it's/market.sqlite",
                r#"'/tmp/it'"'"'s/market.sqlite'"#,
            ),
            ("/tmp/$HOME/*.sqlite", "'/tmp/$HOME/*.sqlite'"),
            ("", "''"),
        ] {
            assert_eq!(shell_word(Path::new(path)), word, "{path}");
        }
    }

    #[test]
    fn a_stored_market_is_read_back_only_in_the_form_quantforge_writes() {
        let market = stored_market("binance_linear", "BTCUSDT", "1m").expect("a market");
        assert_eq!(market.exchange, ExchangeId::BinanceLinear);
        assert_eq!(market.symbol.as_str(), "BTCUSDT");
        assert_eq!(market.interval, Interval::M1);
        for (exchange, symbol, interval) in [
            ("binance", "BTCUSDT", "1m"),
            ("okx_spot", "BTCUSDT", "1m"),
            ("BINANCE_SPOT", "BTCUSDT", "1m"),
            ("binance_spot", "btcusdt", "1m"),
            ("binance_spot", " BTCUSDT", "1m"),
            ("binance_spot", "", "1m"),
            ("binance_spot", "BTCUSDT", "1M"),
            ("binance_spot", "BTCUSDT", "2m"),
        ] {
            assert_eq!(
                stored_market(exchange, symbol, interval),
                None,
                "{exchange} {symbol:?} {interval}"
            );
        }
    }

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
