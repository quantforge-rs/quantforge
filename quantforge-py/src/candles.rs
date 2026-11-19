//! Candles: one bar, and a batch of bars held column by column, with the
//! market the bars are of where the batch names one. A batch's columns
//! cross to NumPy arrays and back in `arrays.rs`.

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList, PyTuple};

use crate::arrays;
use crate::convert::{
    column, decimal, decimal_object, decimal_tuple, each, each_in, integer, optional_integer,
    refused, repr_of,
};
use crate::market::MarketId;

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
/// `to_numpy()` gives the columns as NumPy arrays, and
/// `CandleBatch.from_numpy(...)` takes eight arrays back. That is the one
/// place a batch's decimals become floats, by the engine's own conversion:
/// a float there is the float nearest to its decimal, and a float that is
/// read is the decimal of its shortest text. The two need NumPy, an extra
/// of the package, `pip install "quantforge-py[numpy]"`, and say so where
/// there is none. Nothing else of a batch does.
///
/// The package's `candles_to_dataframe(batch)` gives the same columns as a
/// pandas DataFrame, indexed by the UTC time each bar opened, and
/// `dataframe_to_candles(frame, market=...)` builds a batch from one.
///
/// Each of the three takes `market=`, the `MarketId` the candles are of,
/// which `market` gives back: `None` where the batch names none. `len()` is
/// the number of candles, and each column reads as a tuple, built anew each
/// time it is read. Two batches are equal when their candles are and they
/// name the same market, or both none.
#[pyclass(frozen, eq, module = "quantforge_py")]
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CandleBatch {
    /// The candles, column by column.
    pub(crate) candles: quantforge::CandleBatch,
    /// The market the candles are of, where the batch names one.
    pub(crate) market: Option<quantforge::MarketId>,
}

#[pymethods]
impl CandleBatch {
    /// The batch of `candles`, which is anything that iterates `Candle`s,
    /// in the order given. `market` is the `MarketId` they are of, or
    /// `None`.
    #[staticmethod]
    #[pyo3(signature = (candles, /, *, market=None))]
    fn from_candles(
        candles: &Bound<'_, PyAny>,
        market: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Self> {
        let candles = each(candles, "candles", "a Candle", |candle: &Candle| {
            candle.0.clone()
        })?;
        Ok(Self {
            candles: quantforge::CandleBatch::from_candles(&candles),
            market: MarketId::of_a_batch(market)?,
        })
    }

    /// The batch of eight columns, each anything that iterates its values.
    /// Row `i` of every column belongs to the same candle. `market` is the
    /// `MarketId` the candles are of, or `None`.
    #[staticmethod]
    #[pyo3(signature = (*, open_time_ms, close_time_ms, open, high, low, close, volume, trades, market=None))]
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
        market: Option<&Bound<'_, PyAny>>,
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
        let market = MarketId::of_a_batch(market)?;
        let candles = quantforge::CandleBatch::from_columns(columns).map_err(refused)?;
        Ok(Self { candles, market })
    }

    /// The batch of eight NumPy arrays, one for each column and each of one
    /// dimension: `open_time_ms` and `close_time_ms` of int64; `open`,
    /// `high`, `low`, `close` and `volume` of float64; and `trades` of
    /// uint64, a `numpy.ma.MaskedArray` where a count is unknown. Row `i`
    /// of every array belongs to the same candle. `market` is the
    /// `MarketId` the candles are of, or `None`.
    ///
    /// A float is read as the decimal of its shortest text, as
    /// `decimal_from_f64` reads one, and one that has none is refused by
    /// its row: NaN, an infinity, or a float beyond the engine's decimal.
    /// The arrays are read and not kept: changing one afterwards changes
    /// nothing in the batch.
    ///
    /// It needs NumPy 2, from 2.0 on, an extra of the package, and raises
    /// an `ImportError` that says what to install where there is none.
    #[staticmethod]
    #[pyo3(signature = (*, open_time_ms, close_time_ms, open, high, low, close, volume, trades, market=None))]
    #[allow(clippy::too_many_arguments)]
    fn from_numpy(
        open_time_ms: &Bound<'_, PyAny>,
        close_time_ms: &Bound<'_, PyAny>,
        open: &Bound<'_, PyAny>,
        high: &Bound<'_, PyAny>,
        low: &Bound<'_, PyAny>,
        close: &Bound<'_, PyAny>,
        volume: &Bound<'_, PyAny>,
        trades: &Bound<'_, PyAny>,
        market: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Self> {
        // Every argument is held to its type, in the order written, before
        // a length is compared or a value read.
        let columns = arrays::Columns::of(arrays::Given {
            open_time_ms,
            close_time_ms,
            open,
            high,
            low,
            close,
            volume,
            trades,
        })?;
        let market = MarketId::of_a_batch(market)?;
        Ok(Self {
            candles: columns.batch()?,
            market,
        })
    }

    /// The candles, row by row, in order.
    fn to_candles<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        PyList::new(py, self.candles.to_candles().into_iter().map(Candle))
    }

    /// The columns as NumPy arrays: a dict of eight, by column name in the
    /// columns' order, each of one dimension and as long as the batch.
    ///
    /// `open_time_ms` and `close_time_ms` are int64 and `trades` is uint64,
    /// all three exact; `trades` is a `numpy.ma.MaskedArray`, masked where
    /// the exchange reported no count. `open`, `high`, `low`, `close` and
    /// `volume` are float64: each decimal as the float nearest to it, as
    /// `decimal_to_f64` gives it. The arrays are new each time, and the
    /// caller's own: changing one changes nothing in the batch. They hold
    /// no market, which stays with the batch.
    ///
    /// It needs NumPy 2, from 2.0 on, an extra of the package, and raises
    /// an `ImportError` that says what to install where there is none.
    fn to_numpy<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        arrays::to_numpy(py, &self.candles)
    }

    /// The market the candles are of, or `None` where the batch names none.
    #[getter]
    fn market(&self) -> Option<MarketId> {
        self.market.clone().map(MarketId)
    }

    /// Every candle's open time, in milliseconds since the Unix epoch, UTC.
    #[getter]
    fn open_time_ms<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        PyTuple::new(py, &self.candles.columns().open_time_ms)
    }

    /// Every candle's close time, in milliseconds since the Unix epoch, UTC.
    #[getter]
    fn close_time_ms<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        PyTuple::new(py, &self.candles.columns().close_time_ms)
    }

    /// Every candle's first price.
    #[getter]
    fn open<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        decimal_tuple(py, &self.candles.columns().open)
    }

    /// Every candle's highest price.
    #[getter]
    fn high<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        decimal_tuple(py, &self.candles.columns().high)
    }

    /// Every candle's lowest price.
    #[getter]
    fn low<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        decimal_tuple(py, &self.candles.columns().low)
    }

    /// Every candle's last price.
    #[getter]
    fn close<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        decimal_tuple(py, &self.candles.columns().close)
    }

    /// Every candle's volume, in the base asset.
    #[getter]
    fn volume<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        decimal_tuple(py, &self.candles.columns().volume)
    }

    /// Every candle's trade count, `None` where the exchange reported none.
    #[getter]
    fn trades<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        PyTuple::new(py, &self.candles.columns().trades)
    }

    fn __len__(&self) -> usize {
        self.candles.len()
    }

    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        let counted = match self.candles.len() {
            1 => "<1 candle>".to_string(),
            count => format!("<{count} candles>"),
        };
        // The candles are counted, so the whole is not evaluated; the
        // market is written as it is given.
        match &self.market {
            Some(market) => {
                let market = Bound::new(py, MarketId(market.clone()))?.repr()?;
                Ok(format!(
                    "CandleBatch({counted}, market={})",
                    market.to_cow()?
                ))
            }
            None => Ok(format!("CandleBatch({counted})")),
        }
    }
}

/// The candles a run is given, in order, and the market they say they are
/// of: a `CandleBatch`'s own, with the market it names if it names one, or
/// those of anything that iterates `Candle`s, read to its end, which name
/// no market.
pub(crate) fn taken(
    candles: &Bound<'_, PyAny>,
) -> PyResult<(Vec<quantforge::Candle>, Option<quantforge::MarketId>)> {
    if let Ok(batch) = candles.cast::<CandleBatch>() {
        let batch = batch.get();
        return Ok((batch.candles.to_candles(), batch.market.clone()));
    }
    let candles = each_in(
        candles,
        "candles",
        "a CandleBatch or an iterable of Candle",
        "a Candle",
        |candle: &Candle| candle.0.clone(),
    )?;
    Ok((candles, None))
}
