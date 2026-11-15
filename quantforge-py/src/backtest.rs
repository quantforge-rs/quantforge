//! A backtest's settings and what one produces: the config, a point of the
//! equity curve, and the result, which holds the curve and the trades
//! (`trade.rs`).
//!
//! These are the candle strategy's Spot backtest, as the engine has it.
//! Nothing here is a futures execution result: the engine keeps those to
//! itself (`docs/python-types.md`).

use pyo3::prelude::*;
use pyo3::types::PyTuple;

use crate::convert::{decimal, decimal_object, each, flag, integer, refused, repr_of, repr_with};
use crate::trade::ClosedTrade;

/// How a backtest starts and what it charges.
///
/// `BacktestConfig(*, initial_cash, fee_bps, close_out_at_end)`:
/// `initial_cash` is the quote currency to start with, above zero;
/// `fee_bps` is the fee on every fill in basis points of its notional, zero
/// or more; `close_out_at_end` says whether a position still open after the
/// last bar is sold at that bar's close. A config the engine would not run
/// is refused here, with the engine's `ValueError`, such as `invalid
/// configuration: initial_cash must be greater than 0, got 0`.
#[pyclass(frozen, eq, module = "quantforge_py")]
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct BacktestConfig(pub(crate) quantforge::BacktestConfig);

#[pymethods]
impl BacktestConfig {
    #[new]
    #[pyo3(signature = (*, initial_cash, fee_bps, close_out_at_end))]
    fn new(
        initial_cash: &Bound<'_, PyAny>,
        fee_bps: &Bound<'_, PyAny>,
        close_out_at_end: &Bound<'_, PyAny>,
    ) -> PyResult<Self> {
        let config = quantforge::BacktestConfig {
            initial_cash: decimal(initial_cash, "initial_cash")?,
            fee_bps: decimal(fee_bps, "fee_bps")?,
            close_out_at_end: flag(close_out_at_end, "close_out_at_end")?,
        };
        config.validate().map_err(refused)?;
        Ok(Self(config))
    }

    /// Quote currency to start with.
    #[getter]
    fn initial_cash<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        decimal_object(py, self.0.initial_cash)
    }

    /// The fee on every fill, in basis points of its notional: 10 is 0.1%.
    #[getter]
    fn fee_bps<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        decimal_object(py, self.0.fee_bps)
    }

    /// Whether a position still open after the last bar is sold at that
    /// bar's close; otherwise it is only valued there.
    #[getter]
    fn close_out_at_end(&self) -> bool {
        self.0.close_out_at_end
    }

    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        let config = &self.0;
        repr_of(
            "BacktestConfig",
            &[
                ("initial_cash", decimal_object(py, config.initial_cash)?),
                ("fee_bps", decimal_object(py, config.fee_bps)?),
                (
                    "close_out_at_end",
                    config
                        .close_out_at_end
                        .into_pyobject(py)?
                        .to_owned()
                        .into_any(),
                ),
            ],
        )
    }
}

/// Equity at one bar's close: cash plus the position valued at that close,
/// after the bar's fill.
///
/// `EquityPoint(*, open_time_ms, equity)`: the bar's open time, which names
/// the bar, in milliseconds since the Unix epoch, UTC; and the equity, an
/// exact decimal in the quote currency.
#[pyclass(frozen, eq, module = "quantforge_py")]
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct EquityPoint(pub(crate) quantforge::EquityPoint);

#[pymethods]
impl EquityPoint {
    #[new]
    #[pyo3(signature = (*, open_time_ms, equity))]
    fn new(open_time_ms: &Bound<'_, PyAny>, equity: &Bound<'_, PyAny>) -> PyResult<Self> {
        Ok(Self(quantforge::EquityPoint {
            open_time_ms: integer(open_time_ms, "open_time_ms")?,
            equity: decimal(equity, "equity")?,
        }))
    }

    /// The bar's open time, in milliseconds since the Unix epoch, UTC.
    #[getter]
    fn open_time_ms(&self) -> i64 {
        self.0.open_time_ms
    }

    /// The equity marked at the bar's close, in the quote currency.
    #[getter]
    fn equity<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        decimal_object(py, self.0.equity)
    }

    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        repr_of(
            "EquityPoint",
            &[
                (
                    "open_time_ms",
                    self.0.open_time_ms.into_pyobject(py)?.into_any(),
                ),
                ("equity", decimal_object(py, self.0.equity)?),
            ],
        )
    }
}

/// What a backtest produced. Amounts are in the quote currency.
///
/// `BacktestResult(*, initial_cash, final_equity, total_return_pct,
/// trade_count, max_drawdown_pct, trades, equity_curve)` holds the fields
/// as given: `trades` is anything that iterates `ClosedTrade`s, and
/// `equity_curve` anything that iterates `EquityPoint`s. Both read back as
/// tuples, built anew each time they are read.
#[pyclass(frozen, eq, module = "quantforge_py")]
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct BacktestResult(pub(crate) quantforge::BacktestResult);

#[pymethods]
impl BacktestResult {
    #[new]
    #[pyo3(signature = (
        *,
        initial_cash,
        final_equity,
        total_return_pct,
        trade_count,
        max_drawdown_pct,
        trades,
        equity_curve,
    ))]
    #[allow(clippy::too_many_arguments)]
    fn new(
        initial_cash: &Bound<'_, PyAny>,
        final_equity: &Bound<'_, PyAny>,
        total_return_pct: &Bound<'_, PyAny>,
        trade_count: &Bound<'_, PyAny>,
        max_drawdown_pct: &Bound<'_, PyAny>,
        trades: &Bound<'_, PyAny>,
        equity_curve: &Bound<'_, PyAny>,
    ) -> PyResult<Self> {
        Ok(Self(quantforge::BacktestResult {
            initial_cash: decimal(initial_cash, "initial_cash")?,
            final_equity: decimal(final_equity, "final_equity")?,
            total_return_pct: decimal(total_return_pct, "total_return_pct")?,
            trade_count: integer(trade_count, "trade_count")?,
            max_drawdown_pct: decimal(max_drawdown_pct, "max_drawdown_pct")?,
            trades: each(trades, "trades", "a ClosedTrade", |trade: &ClosedTrade| {
                trade.0.clone()
            })?,
            equity_curve: each(
                equity_curve,
                "equity_curve",
                "an EquityPoint",
                |point: &EquityPoint| point.0,
            )?,
        }))
    }

    /// The starting cash.
    #[getter]
    fn initial_cash<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        decimal_object(py, self.0.initial_cash)
    }

    /// Cash plus any open position valued at the last close, after
    /// close-out.
    #[getter]
    fn final_equity<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        decimal_object(py, self.0.final_equity)
    }

    /// The gain over the starting cash, in percent.
    #[getter]
    fn total_return_pct<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        decimal_object(py, self.0.total_return_pct)
    }

    /// The number of closed trades.
    #[getter]
    fn trade_count(&self) -> usize {
        self.0.trade_count
    }

    /// The largest fall of the close-marked equity from an earlier peak,
    /// in percent.
    #[getter]
    fn max_drawdown_pct<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        decimal_object(py, self.0.max_drawdown_pct)
    }

    /// Every round trip, in order.
    #[getter]
    fn trades<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        PyTuple::new(py, self.0.trades.iter().cloned().map(ClosedTrade))
    }

    /// The close-marked equity, one point per bar in the order given. It
    /// leaves out the close-out fee, which `final_equity` includes.
    #[getter]
    fn equity_curve<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        PyTuple::new(py, self.0.equity_curve.iter().copied().map(EquityPoint))
    }

    // The two sequences are counted, not written out: a curve holds a point
    // for every bar of the run.
    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        let result = &self.0;
        let counted = |count: usize, one: &str, many: &str| match count {
            1 => format!("<1 {one}>"),
            count => format!("<{count} {many}>"),
        };
        repr_with(
            "BacktestResult",
            &[
                ("initial_cash", decimal_object(py, result.initial_cash)?),
                ("final_equity", decimal_object(py, result.final_equity)?),
                (
                    "total_return_pct",
                    decimal_object(py, result.total_return_pct)?,
                ),
                (
                    "trade_count",
                    result.trade_count.into_pyobject(py)?.into_any(),
                ),
                (
                    "max_drawdown_pct",
                    decimal_object(py, result.max_drawdown_pct)?,
                ),
            ],
            &[
                ("trades", counted(result.trades.len(), "trade", "trades")),
                (
                    "equity_curve",
                    counted(result.equity_curve.len(), "point", "points"),
                ),
            ],
        )
    }
}
