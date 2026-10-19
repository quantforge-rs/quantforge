//! Candles as parallel columns: the interchange shape for the Python
//! bindings, where each column becomes one NumPy or Arrow array.

use super::{Candle, ModelError, TimestampMs};
use rust_decimal::Decimal;

/// The eight candle columns as plain vectors: the unchecked form a
/// [`CandleBatch`] is built from and taken apart into. Row `i` of every
/// column belongs to the same candle.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CandleColumns {
    pub open_time_ms: Vec<TimestampMs>,
    pub close_time_ms: Vec<TimestampMs>,
    pub open: Vec<Decimal>,
    pub high: Vec<Decimal>,
    pub low: Vec<Decimal>,
    pub close: Vec<Decimal>,
    pub volume: Vec<Decimal>,
    /// The exchange's trade count, `None` when it did not report one.
    pub trades: Vec<Option<u64>>,
}

/// Candles stored column by column, every column the same length.
///
/// Converting to and from `Vec<Candle>` moves values without converting
/// them, so the round trip is exact: every decimal keeps its scale and sign,
/// trailing zeros included. Crossing into float64 for Python is a separate
/// step that follows the
/// [decimal and float64 policy](https://github.com/quantforge-rs/quantforge/blob/main/docs/decimal-float-policy.md).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CandleBatch {
    columns: CandleColumns,
}

impl CandleBatch {
    /// Splits `candles` into columns, in row order.
    pub fn from_candles(candles: &[Candle]) -> Self {
        Self {
            columns: CandleColumns {
                open_time_ms: candles.iter().map(|c| c.open_time_ms).collect(),
                close_time_ms: candles.iter().map(|c| c.close_time_ms).collect(),
                open: candles.iter().map(|c| c.open).collect(),
                high: candles.iter().map(|c| c.high).collect(),
                low: candles.iter().map(|c| c.low).collect(),
                close: candles.iter().map(|c| c.close).collect(),
                volume: candles.iter().map(|c| c.volume).collect(),
                trades: candles.iter().map(|c| c.trades).collect(),
            },
        }
    }

    /// Checks that every column has as many rows as `open_time_ms`, and
    /// names the first one that does not.
    pub fn from_columns(columns: CandleColumns) -> Result<Self, ModelError> {
        let expected = columns.open_time_ms.len();
        let lengths = [
            ("close_time_ms", columns.close_time_ms.len()),
            ("open", columns.open.len()),
            ("high", columns.high.len()),
            ("low", columns.low.len()),
            ("close", columns.close.len()),
            ("volume", columns.volume.len()),
            ("trades", columns.trades.len()),
        ];
        if let Some((column, found)) = lengths.into_iter().find(|(_, len)| *len != expected) {
            return Err(ModelError::CandleColumnLength {
                column,
                found,
                expected,
            });
        }
        Ok(Self { columns })
    }

    /// Reassembles the rows, in order.
    pub fn to_candles(&self) -> Vec<Candle> {
        let c = &self.columns;
        // Zipped rather than indexed: the columns are the same length by
        // construction, so no row is dropped, and nothing can panic.
        c.open_time_ms
            .iter()
            .zip(&c.close_time_ms)
            .zip(&c.open)
            .zip(&c.high)
            .zip(&c.low)
            .zip(&c.close)
            .zip(&c.volume)
            .zip(&c.trades)
            .map(
                |(
                    ((((((open_time_ms, close_time_ms), open), high), low), close), volume),
                    trades,
                )| {
                    Candle {
                        open_time_ms: *open_time_ms,
                        close_time_ms: *close_time_ms,
                        open: *open,
                        high: *high,
                        low: *low,
                        close: *close,
                        volume: *volume,
                        trades: *trades,
                    }
                },
            )
            .collect()
    }

    /// Read access to every column; they cannot be changed in place, which
    /// keeps their lengths equal.
    pub fn columns(&self) -> &CandleColumns {
        &self.columns
    }

    /// Gives up the columns, for example to hand them to Python.
    pub fn into_columns(self) -> CandleColumns {
        self.columns
    }

    /// The number of candles.
    pub fn len(&self) -> usize {
        self.columns.open_time_ms.len()
    }

    pub fn is_empty(&self) -> bool {
        self.columns.open_time_ms.is_empty()
    }
}
