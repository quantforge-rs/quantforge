//! What a backtest job produces: its metrics, its trades and its equity
//! curve, as three files a client downloads once the job has succeeded.
//! This page is the contract a client decodes them against: each file's
//! name and media type, each table's columns and types, and how a decimal
//! is written.
//!
//! | Result | File | Media type | Holds |
//! | --- | --- | --- | --- |
//! | `metrics` | `metrics.json` | [`METRICS_MEDIA_TYPE`] | A [`Metrics`]: one JSON object |
//! | `trades` | `trades.arrow` | [`ARROW_FILE_MEDIA_TYPE`] | One [`TradeRow`] per closed trade, in the order they closed |
//! | `equity` | `equity.arrow` | [`ARROW_FILE_MEDIA_TYPE`] | One [`EquityRow`] per bar the job ran on, in open-time order |
//!
//! A job's [`ResultRef`](super::jobs::ResultRef)s name the three by
//! `name`, with each file's hash and size, and
//! [`protocol::jobs`](super::jobs) says where they are fetched. All three
//! are of result format [`RESULT_FORMAT`], and a file never changes: the
//! same result always has the same bytes, on every server and every store,
//! so a file's [`ContentHash`](super::ContentHash) names what the job
//! produced.
//!
//! # The tables
//!
//! `trades.arrow` and `equity.arrow` are Arrow IPC files (the file format,
//! also known as Feather V2, not the stream format), which pyarrow, pandas,
//! Polars, arrow-rs and every other Arrow library read as they are. Each
//! holds one record batch, even when it has no rows, uncompressed, with no
//! dictionaries.
//!
//! `trades`:
//!
//! | Column | Arrow type | Holds |
//! | --- | --- | --- |
//! | `symbol` | `Utf8` | The exchange's symbol, in uppercase |
//! | `entry_time_ms` | `Int64` | When the entry filled: the open time of the bar it filled at |
//! | `exit_time_ms` | `Int64` | When the exit filled: the open time of the bar it filled at, or the last bar's close time for a position closed out at the end |
//! | `entry_price` | `Utf8`, a decimal | The entry's fill price |
//! | `exit_price` | `Utf8`, a decimal | The exit's fill price |
//! | `qty` | `Utf8`, a decimal | The quantity bought and sold, in the base asset |
//! | `gross_quote_pnl` | `Utf8`, a decimal | The change in cash from before the entry to after the exit, in the quote asset: both fees are already deducted |
//!
//! `equity`:
//!
//! | Column | Arrow type | Holds |
//! | --- | --- | --- |
//! | `open_time_ms` | `Int64` | The bar's open time, which names the bar |
//! | `equity` | `Utf8`, a decimal | Cash plus the position valued at the bar's close, after that bar's fill, in the quote asset |
//!
//! [`TRADES_COLUMNS`] and [`EQUITY_COLUMNS`] list the same columns for
//! code. No column is nullable and no value is null. The equity curve is the
//! series [`Metrics::max_drawdown_pct`] is measured on. It does not include
//! the fee of a close-out at the end, which
//! [`Metrics::final_equity`] does, so its last row can lie above the final
//! equity by that fee.
//!
//! # Times and decimals
//!
//! - A time is a count of milliseconds since the Unix epoch, UTC, as a
//!   signed 64-bit integer in a column whose name ends in `_ms`. It is not
//!   an Arrow timestamp: a reader that wants one casts it.
//! - A price, quantity or amount is an exact decimal written as text, in
//!   the form of an [`ExactDecimal`](super::ExactDecimal): an optional minus
//!   sign, digits, and an optional fraction, with no exponent and no sign on
//!   zero, such as `81940.01000000`. Trailing zeros are kept, so the text
//!   is the value the engine computed, digit for digit. It is never an
//!   Arrow decimal, which would fix one scale per column, and never a
//!   float. In Python, `decimal.Decimal(text)` reads it exactly, and
//!   `float(text)` gives the nearest float64, as the
//!   [decimal and float64 policy] has it.
//! - A file's schema carries two metadata entries: `quantforge-result`,
//!   the table's name, and `quantforge-result-format`, the format's number
//!   in decimal digits.
//!
//! # One file per table
//!
//! The [result spec] fixes every byte of a file: the order of its fields,
//! its padding, and its metadata. [`encode_trades`] and [`encode_equity`]
//! write exactly those bytes, and [`decode_trades`] and [`decode_equity`]
//! refuse a file that is not exactly what they would have written, so
//! decoding and encoding again gives the same bytes. A client that only
//! reads may use any Arrow library instead. A table holds at most
//! [`MAX_ROWS`] rows.
//!
//! [decimal and float64 policy]: <https://github.com/quantforge-rs/quantforge/blob/main/docs/decimal-float-policy.md>
//! [result spec]: <https://github.com/quantforge-rs/quantforge/blob/main/docs/result-spec.md>

mod arrow;
mod equity;
mod flatbuf;
mod metrics;
mod reader;
mod trades;

use thiserror::Error;

pub use arrow::ColumnType;
pub use equity::{EQUITY_COLUMNS, EquityRow, decode_equity, encode_equity};
pub use metrics::Metrics;
pub use trades::{TRADES_COLUMNS, TradeRow, decode_trades, encode_trades};

/// The result format these pages specify. A file's schema names it, and a
/// change to any file's bytes raises it.
pub const RESULT_FORMAT: u32 = 1;

/// The media type of `trades.arrow` and `equity.arrow`: an Arrow IPC file.
pub const ARROW_FILE_MEDIA_TYPE: &str = "application/vnd.apache.arrow.file";

/// The media type of `metrics.json`.
pub const METRICS_MEDIA_TYPE: &str = "application/json";

/// What a result file is served with: it never changes, so the client's
/// own cache may keep it for a year without asking again. Only the
/// client's: a result is its job's owner's.
pub const RESULT_CACHE_CONTROL: &str = "private, max-age=31536000, immutable";

/// The most rows a table holds: 16,777,216.
pub const MAX_ROWS: usize = 1 << 24;

/// One of a job's three results.
///
/// ```
/// use quantforge::protocol::results::ResultKind;
///
/// assert_eq!(ResultKind::Trades.name(), "trades");
/// assert_eq!(ResultKind::Trades.file_name(), "trades.arrow");
/// assert_eq!(ResultKind::Trades.media_type(), "application/vnd.apache.arrow.file");
/// assert_eq!(ResultKind::from_file_name("metrics.json"), Some(ResultKind::Metrics));
/// assert_eq!(ResultKind::from_file_name("trades.json"), None);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ResultKind {
    /// The summary: a [`Metrics`].
    Metrics,
    /// The closed trades: [`TradeRow`]s.
    Trades,
    /// The equity curve: [`EquityRow`]s.
    Equity,
}

impl ResultKind {
    /// Every result, in the order a job lists them.
    pub const ALL: [Self; 3] = [Self::Metrics, Self::Trades, Self::Equity];

    /// The result's name, which a job's `ResultRef` carries.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Metrics => "metrics",
            Self::Trades => "trades",
            Self::Equity => "equity",
        }
    }

    /// The file a client downloads it as.
    pub const fn file_name(self) -> &'static str {
        match self {
            Self::Metrics => "metrics.json",
            Self::Trades => "trades.arrow",
            Self::Equity => "equity.arrow",
        }
    }

    /// The file's media type.
    pub const fn media_type(self) -> &'static str {
        match self {
            Self::Metrics => METRICS_MEDIA_TYPE,
            Self::Trades | Self::Equity => ARROW_FILE_MEDIA_TYPE,
        }
    }

    /// The result a file name names, if it names one.
    pub fn from_file_name(file_name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.file_name() == file_name)
    }
}

/// Why a result could not be written, or why bytes are not a result.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum ResultError {
    /// More rows than a table holds.
    #[error("the {result} table would hold {rows} rows, and a table holds at most {most}")]
    TooManyRows {
        /// The result: `trades` or `equity`.
        result: &'static str,
        /// The rows there were.
        rows: usize,
        /// The most a table holds.
        most: usize,
    },

    /// More text in one column than an Arrow `Utf8` column counts.
    #[error("the {column} column of the {result} table holds at most 2147483647 bytes of text")]
    ColumnTooLarge {
        /// The result: `trades` or `equity`.
        result: &'static str,
        /// The column.
        column: &'static str,
    },

    /// Bytes that are not exactly a result of this format.
    #[error("not a format {} {result} file: {reason}", RESULT_FORMAT)]
    NotCanonical {
        /// The result they were read as.
        result: &'static str,
        /// What is wrong with them.
        reason: String,
    },
}

/// A file read as `result`, refused for `reason`.
fn refused(result: &'static str, reason: impl Into<String>) -> ResultError {
    ResultError::NotCanonical {
        result,
        reason: reason.into(),
    }
}

/// The table read from `file`, when writing it again with `encode` gives
/// `file` back byte for byte.
fn exactly<T>(
    result: &'static str,
    file: &[u8],
    rows: T,
    encode: impl FnOnce(&T) -> Result<Vec<u8>, ResultError>,
) -> Result<T, ResultError> {
    match encode(&rows) {
        Ok(written) if written == file => Ok(rows),
        Ok(_) => Err(refused(
            result,
            "it is not the file its contents are written as",
        )),
        Err(failure) => Err(failure),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_result_has_one_name_one_file_and_a_media_type() {
        let named: Vec<(&str, &str, &str)> = ResultKind::ALL
            .iter()
            .map(|kind| (kind.name(), kind.file_name(), kind.media_type()))
            .collect();
        assert_eq!(
            named,
            [
                ("metrics", "metrics.json", "application/json"),
                (
                    "trades",
                    "trades.arrow",
                    "application/vnd.apache.arrow.file"
                ),
                (
                    "equity",
                    "equity.arrow",
                    "application/vnd.apache.arrow.file"
                ),
            ]
        );
        for kind in ResultKind::ALL {
            assert_eq!(ResultKind::from_file_name(kind.file_name()), Some(kind));
            assert_eq!(ResultKind::from_file_name(kind.name()), None);
        }
        assert_eq!(ResultKind::from_file_name("TRADES.ARROW"), None);
        assert_eq!(ResultKind::from_file_name(""), None);
    }

    #[test]
    fn a_refusal_names_the_result_and_the_format() {
        assert_eq!(
            refused("trades", "it ends early").to_string(),
            "not a format 1 trades file: it ends early"
        );
        let too_many = ResultError::TooManyRows {
            result: "equity",
            rows: MAX_ROWS + 1,
            most: MAX_ROWS,
        };
        assert_eq!(
            too_many.to_string(),
            "the equity table would hold 16777217 rows, and a table holds at most 16777216"
        );
    }
}
