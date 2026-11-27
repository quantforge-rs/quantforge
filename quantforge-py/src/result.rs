//! A remote job's result as the package's client reads it: its metrics
//! written as the file the result format gives them, and its two tables
//! read into the package's own types (`docs/python-jobs.md`).
//!
//! A server names each result file by the SHA-256 of its bytes, and
//! `quantforge::protocol::results` is the reference implementation of the
//! format those bytes are in (`docs/result-spec.md`). The client reads a
//! result with the same code, compiled into this module:
//!
//! - **The metrics are written back as `metrics.json`,** by the writer a
//!   server writes that file with, so that the client can hold the metrics
//!   it was answered to the hash its server lists for the file, without
//!   downloading it.
//! - **A table is read strictly,** by `decode_trades` and `decode_equity`:
//!   a file that is not byte for byte what the format writes for its rows
//!   is refused, never read approximately.
//! - **A row becomes the engine's own type,** a `ClosedTrade` or an
//!   `EquityPoint`, each amount the exact decimal the file holds, with its
//!   digits. A backtest sends no order to a venue, so a trade read here
//!   has no order ID, as a trade of a local run has none.
//!
//! What is the client's own stays in Python (`_results.py`): what it asks
//! a server, what it holds a file to before it is read here, and what it
//! raises. The three entries are private.

use std::str::FromStr;

use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyTuple};
use quantforge::protocol::ExactDecimal;
use quantforge::protocol::results::{Metrics, decode_equity, decode_trades};

use crate::backtest::EquityPoint;
use crate::convert::refused;
use crate::trade::ClosedTrade;

/// An amount of a result's metrics, read as the protocol reads one.
fn amount(text: &str, name: &str) -> PyResult<ExactDecimal> {
    ExactDecimal::from_str(text).map_err(|error| refused(format_args!("{name}: {error}")))
}

/// The `metrics.json` of a result's metrics, as the result format writes
/// it: `_result_metrics(initial_cash, final_equity, total_return_pct,
/// max_drawdown_pct, trade_count)`.
///
/// Each of the first four is an amount as the protocol writes one, the
/// text of an exact decimal, and `trade_count` the number of closed
/// trades. Gives the file's bytes: one JSON object, its fields in the
/// format's order, with no whitespace. An amount that is no exact decimal,
/// or one the engine's decimal cannot hold without rounding, is a
/// `ValueError` that names it.
#[pyfunction]
#[pyo3(
    name = "_result_metrics",
    signature = (initial_cash, final_equity, total_return_pct, max_drawdown_pct, trade_count)
)]
pub(crate) fn metrics<'py>(
    py: Python<'py>,
    initial_cash: &str,
    final_equity: &str,
    total_return_pct: &str,
    max_drawdown_pct: &str,
    trade_count: u64,
) -> PyResult<Bound<'py, PyBytes>> {
    let metrics = Metrics {
        initial_cash: amount(initial_cash, "initial_cash")?,
        final_equity: amount(final_equity, "final_equity")?,
        total_return_pct: amount(total_return_pct, "total_return_pct")?,
        max_drawdown_pct: amount(max_drawdown_pct, "max_drawdown_pct")?,
        trade_count,
    };
    let file = metrics.encode().map_err(refused)?;
    Ok(PyBytes::new(py, &file))
}

/// The closed trades of a `trades.arrow`, in the file's order:
/// `_result_trades(file)`.
///
/// `file` is the file's bytes. Gives a tuple of `ClosedTrade`, one for
/// each row: its symbol, its two times, and its four amounts, each the
/// exact decimal the file holds; its two order IDs are `None`. A file that
/// is not exactly a `trades` table of the result format, and a row whose
/// symbol is not one the engine writes, are a `ValueError` that says what
/// is wrong.
///
/// The interpreter is given up while the file is read.
#[pyfunction]
#[pyo3(name = "_result_trades", signature = (file, /))]
pub(crate) fn trades<'py>(file: &Bound<'py, PyBytes>) -> PyResult<Bound<'py, PyTuple>> {
    let py = file.py();
    let bytes = file.as_bytes();
    let rows = py.detach(|| decode_trades(bytes)).map_err(refused)?;
    let mut trades = Vec::with_capacity(rows.len());
    for (index, row) in rows.into_iter().enumerate() {
        // The engine's own rule for a symbol, which a symbol a server
        // wrote already meets: one that it would write otherwise is not
        // the engine's.
        let symbol = quantforge::Symbol::new(row.symbol.as_str())
            .map_err(|error| refused(format_args!("the symbol of its row {index}: {error}")))?;
        if symbol.as_str() != row.symbol {
            return Err(refused(format_args!(
                "the symbol of its row {index}, {:?}, is not in its canonical form",
                row.symbol
            )));
        }
        trades.push(ClosedTrade(quantforge::ClosedTrade {
            symbol,
            entry_time_ms: row.entry_time_ms,
            exit_time_ms: row.exit_time_ms,
            entry_price: row.entry_price.get(),
            exit_price: row.exit_price.get(),
            qty: row.qty.get(),
            gross_quote_pnl: row.gross_quote_pnl.get(),
            entry_order_id: None,
            exit_order_id: None,
        }));
    }
    PyTuple::new(py, trades)
}

/// The equity curve of an `equity.arrow`, in the file's order:
/// `_result_equity(file)`.
///
/// `file` is the file's bytes. Gives a tuple of `EquityPoint`, one for
/// each row: the open time of its bar, and the equity at that bar's close,
/// the exact decimal the file holds. A file that is not exactly an
/// `equity` table of the result format is a `ValueError` that says what is
/// wrong.
///
/// The interpreter is given up while the file is read.
#[pyfunction]
#[pyo3(name = "_result_equity", signature = (file, /))]
pub(crate) fn equity<'py>(file: &Bound<'py, PyBytes>) -> PyResult<Bound<'py, PyTuple>> {
    let py = file.py();
    let bytes = file.as_bytes();
    let rows = py.detach(|| decode_equity(bytes)).map_err(refused)?;
    let curve = rows.into_iter().map(|row| {
        EquityPoint(quantforge::EquityPoint {
            open_time_ms: row.open_time_ms,
            equity: row.equity.get(),
        })
    });
    PyTuple::new(py, curve)
}
