//! A batch's columns as NumPy arrays, and NumPy arrays as a batch:
//! `CandleBatch.to_numpy` and `CandleBatch.from_numpy`
//! (`docs/python-numpy.md`).
//!
//! - **A time and a trade count cross exactly:** int64 and uint64, the
//!   engine's own integers. A count the exchange did not report is a masked
//!   entry of a `numpy.ma.MaskedArray`, and never a number.
//! - **A price and a volume cross as float64,** by the engine's own two
//!   conversions and no other (`docs/decimal-float-policy.md`): a decimal
//!   as the float nearest to it, and a float as the decimal of its shortest
//!   text, refused where it has none.
//! - **An array is read where it lies,** when its cells lie one after
//!   another on their alignment, and through a copy NumPy makes of it
//!   otherwise. An array that is given is new, and NumPy's own.
//! - **An array is held to its type twice:** when it is taken, and again as
//!   its cells are read. NumPy lets an array's type be changed where it
//!   stands, and Python's own code can run between the two.
//! - **Nothing of Python's runs while an array is borrowed,** so that
//!   nothing changes what it is under the borrow, its type, its shape or
//!   where its memory lies: it is borrowed last, and a refusal is worded
//!   once the borrow is given back.
//!
//! NumPy is the package's one extra, and no requirement of it. It is
//! imported when a conversion is first asked for, and never when the
//! package is.
//!
//! `installed` is NumPy as it is found, and what is said where there is
//! none or one too old; `column` is one array as a column, held to its type
//! and read; and `counts` is the trade counts with their mask.

mod column;
mod counts;
mod installed;

use numpy::{Element, PyArray1};
use pyo3::exceptions::PyValueError;
use pyo3::intern;
use pyo3::prelude::*;
use pyo3::sync::PyOnceLock;
use pyo3::types::{PyDict, PyType};
use quantforge::{CandleBatch, CandleColumns, ModelError};
use rust_decimal::Decimal;

use crate::convert::{Row, refused};
use column::{Column, allocated, plain, room_for, written};
use counts::{Counts, TRADES, counts, known};
use installed::imported;

static MASKED_ARRAY: PyOnceLock<Py<PyType>> = PyOnceLock::new();

/// NumPy's `numpy.ma.MaskedArray`.
fn masked_array(py: Python<'_>) -> PyResult<&Bound<'_, PyType>> {
    MASKED_ARRAY.import(py, "numpy.ma", "MaskedArray")
}

/// The type of a column's cells, by the name NumPy and a refusal give it.
trait Cell: Element + Copy {
    const NAME: &'static str;
}

impl Cell for i64 {
    const NAME: &'static str = "int64";
}

impl Cell for u64 {
    const NAME: &'static str = "uint64";
}

impl Cell for f64 {
    const NAME: &'static str = "float64";
}

impl Cell for bool {
    const NAME: &'static str = "bool";
}

impl Cell for u8 {
    const NAME: &'static str = "uint8";
}

/// A type of cell that is read as it lies: every run of its bytes is a
/// value of it.
///
/// A flag is not one. NumPy takes any byte that is not zero for `True`, and
/// a `bool` of this side may only be 0 or 1, so flags are read as bytes.
trait AsItLies: Cell {}

impl AsItLies for i64 {}

impl AsItLies for u64 {}

impl AsItLies for f64 {}

impl AsItLies for u8 {}

/// A new array of one dimension, as long as `values`, with each cell what
/// `cell_of` makes of its value, written straight into it.
///
/// NumPy allocates it, through `numpy.zeros`, and it owns its memory. Where
/// there is no memory for it, that is NumPy's own `MemoryError`: the array
/// bindings' constructors panic there. What `numpy.zeros` gave is held to
/// what was asked of it, so that whatever stands in its place cannot give
/// an array of another length or type.
fn filled<'py, T: Cell, V>(
    numpy: &Bound<'py, PyModule>,
    values: &[V],
    cell_of: impl Fn(&V) -> T,
) -> PyResult<Bound<'py, PyArray1<T>>> {
    let py = numpy.py();
    let made = numpy
        .getattr(intern!(py, "zeros"))?
        .call1((values.len(), numpy::dtype::<T>(py)))?;
    let array = allocated::<T>(&made, values.len())?;
    {
        let mut written = written(&array, values.len())?;
        for (cell, value) in written.as_slice_mut()?.iter_mut().zip(values) {
            *cell = cell_of(value);
        }
    }
    Ok(array)
}

/// The eight columns of `batch` as NumPy arrays: a dict by column name, in
/// the columns' order.
///
/// Every array is new, of one dimension and as long as the batch. The
/// decimals become the floats nearest to them, and the trade counts a
/// masked array, masked where a count is unknown.
pub(crate) fn to_numpy<'py>(py: Python<'py>, batch: &CandleBatch) -> PyResult<Bound<'py, PyDict>> {
    let numpy = imported(py)?;
    let columns = batch.columns();
    let arrays = PyDict::new(py);
    for (name, times) in [
        ("open_time_ms", &columns.open_time_ms),
        ("close_time_ms", &columns.close_time_ms),
    ] {
        arrays.set_item(name, filled(&numpy, times, |time| *time)?)?;
    }
    for (name, decimals) in [
        ("open", &columns.open),
        ("high", &columns.high),
        ("low", &columns.low),
        ("close", &columns.close),
        ("volume", &columns.volume),
    ] {
        let floats = filled(&numpy, decimals, |decimal| {
            quantforge::decimal_to_f64(*decimal)
        })?;
        arrays.set_item(name, floats)?;
    }
    // A count that is unknown is masked. The number under its mask is no
    // count: it is zero only because a cell has to hold something.
    let counts = filled(&numpy, &columns.trades, |count| count.unwrap_or(0))?;
    let unknown = filled(&numpy, &columns.trades, |count| count.is_none())?;
    let mask = PyDict::new(py);
    mask.set_item("mask", unknown)?;
    arrays.set_item("trades", masked_array(py)?.call((counts,), Some(&mask))?)?;
    Ok(arrays)
}

/// The eight arrays `CandleBatch.from_numpy` is given, each as it came.
pub(crate) struct Given<'a, 'py> {
    pub(crate) open_time_ms: &'a Bound<'py, PyAny>,
    pub(crate) close_time_ms: &'a Bound<'py, PyAny>,
    pub(crate) open: &'a Bound<'py, PyAny>,
    pub(crate) high: &'a Bound<'py, PyAny>,
    pub(crate) low: &'a Bound<'py, PyAny>,
    pub(crate) close: &'a Bound<'py, PyAny>,
    pub(crate) volume: &'a Bound<'py, PyAny>,
    pub(crate) trades: &'a Bound<'py, PyAny>,
}

/// A column of times, copied into a vector of the batch's own.
fn times(column: &Column<'_, i64>) -> PyResult<Vec<i64>> {
    let ready = column.ready()?;
    // Nothing of Python's runs once the array is borrowed.
    let borrowed = ready.borrowed()?;
    let cells = borrowed.as_slice()?;
    let mut times = room_for(cells.len(), column.name)?;
    times.extend_from_slice(cells);
    Ok(times)
}

/// A column of floats as the decimals of their shortest text. A float that
/// has no such decimal is refused in the engine's words, by its row.
fn decimals(column: &Column<'_, f64>) -> PyResult<Vec<Decimal>> {
    let ready = column.ready()?;
    // Nothing of Python's runs once the array is borrowed: a float that is
    // refused is only worded once the refusal is raised.
    let borrowed = ready.borrowed()?;
    let cells = borrowed.as_slice()?;
    let mut exact = room_for(cells.len(), column.name)?;
    for (row, cell) in cells.iter().enumerate() {
        match quantforge::decimal_from_f64(*cell) {
            Ok(decimal) => exact.push(decimal),
            Err(refusal) => {
                return Err(PyValueError::new_err(format!(
                    "{}: {refusal}",
                    Row(column.name, row)
                )));
            }
        }
    }
    Ok(exact)
}

/// The eight arrays, each held to its type and none read yet.
pub(crate) struct Columns<'py> {
    open_time_ms: Column<'py, i64>,
    close_time_ms: Column<'py, i64>,
    open: Column<'py, f64>,
    high: Column<'py, f64>,
    low: Column<'py, f64>,
    close: Column<'py, f64>,
    volume: Column<'py, f64>,
    trades: Counts<'py>,
}

impl<'py> Columns<'py> {
    /// Holds each array to its type, in the order written: what it is, what
    /// its cells are, and that it has one dimension. No cell is read.
    pub(crate) fn of(given: Given<'_, 'py>) -> PyResult<Self> {
        imported(given.open_time_ms.py())?;
        Ok(Self {
            open_time_ms: plain(given.open_time_ms, "open_time_ms")?,
            close_time_ms: plain(given.close_time_ms, "close_time_ms")?,
            open: plain(given.open, "open")?,
            high: plain(given.high, "high")?,
            low: plain(given.low, "low")?,
            close: plain(given.close, "close")?,
            volume: plain(given.volume, "volume")?,
            trades: counts(given.trades)?,
        })
    }

    /// The batch of the eight arrays.
    ///
    /// They are held to one length first, the first that is not as long as
    /// `open_time_ms` refused in the engine's words, and only then is a
    /// value read, column by column and row by row. Nothing of an array is
    /// kept.
    pub(crate) fn batch(&self) -> PyResult<CandleBatch> {
        let expected = self.open_time_ms.len();
        for (column, found) in [
            ("close_time_ms", self.close_time_ms.len()),
            ("open", self.open.len()),
            ("high", self.high.len()),
            ("low", self.low.len()),
            ("close", self.close.len()),
            ("volume", self.volume.len()),
            (TRADES, self.trades.counts.len()),
        ] {
            if found != expected {
                return Err(refused(ModelError::CandleColumnLength {
                    column,
                    found,
                    expected,
                }));
            }
        }
        let columns = CandleColumns {
            open_time_ms: times(&self.open_time_ms)?,
            close_time_ms: times(&self.close_time_ms)?,
            open: decimals(&self.open)?,
            high: decimals(&self.high)?,
            low: decimals(&self.low)?,
            close: decimals(&self.close)?,
            volume: decimals(&self.volume)?,
            trades: known(&self.trades)?,
        };
        // An array that changed its length since the lengths were compared
        // is refused here, by the engine itself.
        CandleBatch::from_columns(columns).map_err(refused)
    }
}
