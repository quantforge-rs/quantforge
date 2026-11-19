//! What the package's pandas functions are built on: pandas and NumPy as
//! they are found, and a backtest result's trades and equity curve as
//! columns (`docs/python-pandas.md`).
//!
//! The functions themselves are written in Python, in
//! `python/quantforge_py/_frames.py`, where a frame is put together and
//! taken apart with pandas' own calls. What is the engine's stays here, so
//! that a frame holds nothing Python worked out:
//!
//! - **Which pandas and which NumPy are taken,** held to the bounds of the
//!   package's extras in the one place both are (`extra.rs`).
//! - **A decimal as a float64,** by the engine's own conversion and no
//!   other (`docs/decimal-float-policy.md`), and every time as the engine's
//!   own integer: the columns of a result are NumPy arrays made as a
//!   batch's are (`arrays.rs`).
//!
//! All four entries are private. The package publishes the functions built
//! on them, and no name of this module.

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};
use quantforge::{ClosedTrade, VenueOrderId};
use rust_decimal::Decimal;

use crate::arrays::{self, filled};
use crate::backtest::BacktestResult;
use crate::convert::wrong_type;
use crate::extra::Extra;

/// pandas, the extra the package's DataFrame functions need.
///
/// - **`oldest`** is the lower bound of the package's `pandas` extra, and
///   the oldest pandas the tests run on: the first that runs beside
///   NumPy 2, which the arrays a frame is made of need.
/// - **`next`** is its upper bound. What a frame's columns are is stated
///   type by type, and pandas 3 changed what a column of text is and how a
///   frame shares its memory. A pandas 4 may change as much again, and
///   there is none to test on: the bound is raised once there is one and
///   the tests run on it.
pub(crate) const PANDAS: Extra = Extra {
    module: "pandas",
    library: "pandas",
    needs: "a DataFrame needs",
    oldest: &[2, 2, 2],
    next: &[4],
    install: r#"pip install "quantforge-py[pandas]""#,
};

/// pandas itself, imported when a frame is first asked for and held to the
/// versions the package's `pandas` extra takes: `_pandas()`.
///
/// Where Python finds none to import this is a `ModuleNotFoundError` that
/// says what to install, and where it finds one the extra does not take an
/// `ImportError`. A pandas that fails as it is imported raises its own
/// error.
#[pyfunction]
#[pyo3(name = "_pandas")]
pub(crate) fn found_pandas(py: Python<'_>) -> PyResult<Bound<'_, PyModule>> {
    PANDAS.imported(py)
}

/// NumPy itself, imported and held to the versions the arrays take, as
/// `CandleBatch.to_numpy` holds it: `_numpy()`.
///
/// A frame is made of a batch's arrays and read back through them, so it
/// needs the NumPy they need, and is refused in their words where there is
/// none or one the `numpy` extra does not take.
#[pyfunction]
#[pyo3(name = "_numpy")]
pub(crate) fn found_numpy(py: Python<'_>) -> PyResult<Bound<'_, PyModule>> {
    arrays::imported(py)
}

/// The engine's own result that `value` holds: a `BacktestResult`.
fn result_of<'a, 'py>(value: &'a Bound<'py, PyAny>) -> PyResult<&'a Bound<'py, BacktestResult>> {
    match value.cast::<BacktestResult>() {
        Ok(result) => Ok(result),
        Err(_) => Err(wrong_type("result", "a BacktestResult", value)),
    }
}

/// The trades of a `BacktestResult` column by column, in the order of a
/// trade's fields: `_trade_columns(result)`.
///
/// A dict of nine columns, each with one row for every trade, in the
/// result's order: `symbol` a list of text; `entry_time_ms` and
/// `exit_time_ms` int64 arrays, the engine's own milliseconds;
/// `entry_price`, `exit_price`, `qty` and `gross_quote_pnl` float64 arrays,
/// each decimal as the float nearest to it; and `entry_order_id` and
/// `exit_order_id` lists of text, `None` where no order reached a venue.
/// Every array is new, and made as a batch's are.
///
/// NumPy is asked for before `result` is looked at, as a batch's own
/// arrays ask for it.
#[pyfunction]
#[pyo3(name = "_trade_columns", signature = (result, /))]
pub(crate) fn trade_columns<'py>(result: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyDict>> {
    let py = result.py();
    let numpy = arrays::imported(py)?;
    let result = result_of(result)?;
    let trades = &result.get().0.trades;
    let columns = PyDict::new(py);
    let symbols = trades.iter().map(|trade| trade.symbol.as_str());
    columns.set_item("symbol", PyList::new(py, symbols)?)?;
    let entered = filled(&numpy, trades, |trade| trade.entry_time_ms)?;
    columns.set_item("entry_time_ms", entered)?;
    let exited = filled(&numpy, trades, |trade| trade.exit_time_ms)?;
    columns.set_item("exit_time_ms", exited)?;
    // A decimal as the float nearest to it, by the engine's own conversion.
    let floats = |amount_of: fn(&ClosedTrade) -> Decimal| {
        filled(&numpy, trades, |trade| {
            quantforge::decimal_to_f64(amount_of(trade))
        })
    };
    columns.set_item("entry_price", floats(|trade| trade.entry_price)?)?;
    columns.set_item("exit_price", floats(|trade| trade.exit_price)?)?;
    columns.set_item("qty", floats(|trade| trade.qty)?)?;
    columns.set_item("gross_quote_pnl", floats(|trade| trade.gross_quote_pnl)?)?;
    // An order that never reached a venue has no ID, and stays `None`.
    let ids = |order_of: fn(&ClosedTrade) -> Option<&VenueOrderId>| {
        let ids = trades
            .iter()
            .map(|trade| order_of(trade).map(VenueOrderId::as_str));
        PyList::new(py, ids)
    };
    let entry_orders = ids(|trade| trade.entry_order_id.as_ref())?;
    columns.set_item("entry_order_id", entry_orders)?;
    let exit_orders = ids(|trade| trade.exit_order_id.as_ref())?;
    columns.set_item("exit_order_id", exit_orders)?;
    Ok(columns)
}

/// The equity curve of a `BacktestResult` as two columns:
/// `_equity_columns(result)`.
///
/// A dict of `open_time_ms`, an int64 array of the engine's own
/// milliseconds, and `equity`, a float64 array of each decimal as the float
/// nearest to it, with one row for every point of the curve, in order.
/// Both arrays are new, and made as a batch's are.
///
/// NumPy is asked for before `result` is looked at, as a batch's own
/// arrays ask for it.
#[pyfunction]
#[pyo3(name = "_equity_columns", signature = (result, /))]
pub(crate) fn equity_columns<'py>(result: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyDict>> {
    let py = result.py();
    let numpy = arrays::imported(py)?;
    let result = result_of(result)?;
    let curve = &result.get().0.equity_curve;
    let columns = PyDict::new(py);
    let times = filled(&numpy, curve, |point| point.open_time_ms)?;
    columns.set_item("open_time_ms", times)?;
    let equity = filled(&numpy, curve, |point| {
        quantforge::decimal_to_f64(point.equity)
    })?;
    columns.set_item("equity", equity)?;
    Ok(columns)
}
