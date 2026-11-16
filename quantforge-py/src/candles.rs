//! Candles: one bar, and a batch of bars held column by column.

use pyo3::prelude::*;
use pyo3::types::{PyList, PyTuple};

use crate::convert::{
    column, decimal, decimal_object, decimal_tuple, each, each_in, integer, optional_integer,
    refused, repr_of,
};

/// One candle: a bar's times, its prices, its volume and its trade count.
///
/// `Candle(*, open_time_ms, close_time_ms, open, high, low, close, volume,
/// trades=None)`: the times are milliseconds since the Unix epoch, UTC; the
/// prices and the volume are exact decimals, a `decimal.Decimal` or an
/// `int`; `trades` is the exchange's trade count, or `None` where it
/// reported none, which is never read as zero.
///
/// The values are kept as given: nothing checks that the prices make a bar.
/// Two candles are equal when every field is, decimals by value.
// `eq` without `hash`: like the engine's candle, it compares and is not a
// key. Python makes a class with an equality and no hash unhashable itself,
// and so are the other records.
#[pyclass(frozen, eq, module = "quantforge_py")]
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Candle(pub(crate) quantforge::Candle);

#[pymethods]
impl Candle {
    #[new]
    #[pyo3(signature = (*, open_time_ms, close_time_ms, open, high, low, close, volume, trades=None))]
    #[allow(clippy::too_many_arguments)]
    fn new(
        open_time_ms: &Bound<'_, PyAny>,
        close_time_ms: &Bound<'_, PyAny>,
        open: &Bound<'_, PyAny>,
        high: &Bound<'_, PyAny>,
        low: &Bound<'_, PyAny>,
        close: &Bound<'_, PyAny>,
        volume: &Bound<'_, PyAny>,
        trades: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Self> {
        Ok(Self(quantforge::Candle {
            open_time_ms: integer(open_time_ms, "open_time_ms")?,
            close_time_ms: integer(close_time_ms, "close_time_ms")?,
            open: decimal(open, "open")?,
            high: decimal(high, "high")?,
            low: decimal(low, "low")?,
            close: decimal(close, "close")?,
            volume: decimal(volume, "volume")?,
            trades: match trades {
                Some(trades) => optional_integer(trades, "trades")?,
                None => None,
            },
        }))
    }

    /// When the bar opened, in milliseconds since the Unix epoch, UTC.
    #[getter]
    fn open_time_ms(&self) -> i64 {
        self.0.open_time_ms
    }

    /// When the bar closed, in milliseconds since the Unix epoch, UTC.
    #[getter]
    fn close_time_ms(&self) -> i64 {
        self.0.close_time_ms
    }

    /// The first price of the bar.
    #[getter]
    fn open<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        decimal_object(py, self.0.open)
    }

    /// The highest price of the bar.
    #[getter]
    fn high<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        decimal_object(py, self.0.high)
    }

    /// The lowest price of the bar.
    #[getter]
    fn low<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        decimal_object(py, self.0.low)
    }

    /// The last price of the bar.
    #[getter]
    fn close<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        decimal_object(py, self.0.close)
    }

    /// The volume traded in the bar, in the base asset.
    #[getter]
    fn volume<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        decimal_object(py, self.0.volume)
    }

    /// The exchange's trade count, or `None` where it reported none.
    #[getter]
    fn trades(&self) -> Option<u64> {
        self.0.trades
    }

    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        let candle = &self.0;
        repr_of(
            "Candle",
            &[
                (
                    "open_time_ms",
                    candle.open_time_ms.into_pyobject(py)?.into_any(),
                ),
                (
                    "close_time_ms",
                    candle.close_time_ms.into_pyobject(py)?.into_any(),
                ),
                ("open", decimal_object(py, candle.open)?),
                ("high", decimal_object(py, candle.high)?),
                ("low", decimal_object(py, candle.low)?),
                ("close", decimal_object(py, candle.close)?),
                ("volume", decimal_object(py, candle.volume)?),
                ("trades", candle.trades.into_pyobject(py)?.into_any()),
            ],
        )
    }
}

/// Candles held column by column, every column the same length: the shape
/// candles are exchanged in.
///
/// `CandleBatch.from_candles(candles)` splits candles into columns, and
/// `to_candles()` gives them back, in order; `CandleBatch.from_columns(*,
/// open_time_ms, close_time_ms, open, high, low, close, volume, trades)`
/// takes the eight columns, and refuses one that is not as long as
/// `open_time_ms` with `ValueError: candle column `open` has 2 rows,
/// expected 3`. Either way no value is converted: every decimal keeps its
/// digits and its decimal places.
///
/// `len()` is the number of candles, and each column reads as a tuple,
/// built anew each time it is read. Two batches are equal when their
/// candles are.
#[pyclass(frozen, eq, module = "quantforge_py")]
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CandleBatch(pub(crate) quantforge::CandleBatch);

#[pymethods]
impl CandleBatch {
    /// The batch of `candles`, which is anything that iterates `Candle`s,
    /// in the order given.
    #[staticmethod]
    #[pyo3(signature = (candles, /))]
    fn from_candles(candles: &Bound<'_, PyAny>) -> PyResult<Self> {
        let candles = each(candles, "candles", "a Candle", |candle: &Candle| {
            candle.0.clone()
        })?;
        Ok(Self(quantforge::CandleBatch::from_candles(&candles)))
    }

    /// The batch of eight columns, each anything that iterates its values.
    /// Row `i` of every column belongs to the same candle.
    #[staticmethod]
    #[pyo3(signature = (*, open_time_ms, close_time_ms, open, high, low, close, volume, trades))]
    #[allow(clippy::too_many_arguments)]
    fn from_columns(
        open_time_ms: &Bound<'_, PyAny>,
        close_time_ms: &Bound<'_, PyAny>,
        open: &Bound<'_, PyAny>,
        high: &Bound<'_, PyAny>,
        low: &Bound<'_, PyAny>,
        close: &Bound<'_, PyAny>,
        volume: &Bound<'_, PyAny>,
        trades: &Bound<'_, PyAny>,
    ) -> PyResult<Self> {
        let columns = quantforge::CandleColumns {
            open_time_ms: column(open_time_ms, "open_time_ms", integer)?,
            close_time_ms: column(close_time_ms, "close_time_ms", integer)?,
            open: column(open, "open", decimal)?,
            high: column(high, "high", decimal)?,
            low: column(low, "low", decimal)?,
            close: column(close, "close", decimal)?,
            volume: column(volume, "volume", decimal)?,
            trades: column(trades, "trades", optional_integer)?,
        };
        quantforge::CandleBatch::from_columns(columns)
            .map(Self)
            .map_err(refused)
    }

    /// The candles, row by row, in order.
    fn to_candles<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        PyList::new(py, self.0.to_candles().into_iter().map(Candle))
    }

    /// Every candle's open time, in milliseconds since the Unix epoch, UTC.
    #[getter]
    fn open_time_ms<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        PyTuple::new(py, &self.0.columns().open_time_ms)
    }

    /// Every candle's close time, in milliseconds since the Unix epoch, UTC.
    #[getter]
    fn close_time_ms<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        PyTuple::new(py, &self.0.columns().close_time_ms)
    }

    /// Every candle's first price.
    #[getter]
    fn open<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        decimal_tuple(py, &self.0.columns().open)
    }

    /// Every candle's highest price.
    #[getter]
    fn high<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        decimal_tuple(py, &self.0.columns().high)
    }

    /// Every candle's lowest price.
    #[getter]
    fn low<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        decimal_tuple(py, &self.0.columns().low)
    }

    /// Every candle's last price.
    #[getter]
    fn close<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        decimal_tuple(py, &self.0.columns().close)
    }

    /// Every candle's volume, in the base asset.
    #[getter]
    fn volume<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        decimal_tuple(py, &self.0.columns().volume)
    }

    /// Every candle's trade count, `None` where the exchange reported none.
    #[getter]
    fn trades<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        PyTuple::new(py, &self.0.columns().trades)
    }

    fn __len__(&self) -> usize {
        self.0.len()
    }

    fn __repr__(&self) -> String {
        match self.0.len() {
            1 => "CandleBatch(<1 candle>)".to_string(),
            count => format!("CandleBatch(<{count} candles>)"),
        }
    }
}

/// The candles a run is given, in order: a `CandleBatch`'s own, or those of
/// anything that iterates `Candle`s, read to its end.
pub(crate) fn taken(candles: &Bound<'_, PyAny>) -> PyResult<Vec<quantforge::Candle>> {
    if let Ok(batch) = candles.cast::<CandleBatch>() {
        return Ok(batch.get().0.to_candles());
    }
    each_in(
        candles,
        "candles",
        "a CandleBatch or an iterable of Candle",
        "a Candle",
        |candle: &Candle| candle.0.clone(),
    )
}
