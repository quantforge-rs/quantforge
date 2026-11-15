//! A closed trade: one round trip of a candle strategy on Spot, as a
//! backtest's result lists it.

use pyo3::prelude::*;
use pyo3::types::PyString;

use crate::convert::{decimal, decimal_object, integer, refused, repr_of, wrong_type};
use crate::market::Symbol;

/// One closed round trip of a candle strategy on Spot.
///
/// `ClosedTrade(*, symbol, entry_time_ms, exit_time_ms, entry_price,
/// exit_price, qty, gross_quote_pnl, entry_order_id=None,
/// exit_order_id=None)`: `symbol` is a `Symbol` or the text of one; the
/// times are milliseconds since the Unix epoch, UTC; the prices, the
/// quantity and the PnL are exact decimals. An order ID is the venue's own,
/// as text, or `None` where no order reached a venue, as in a backtest.
///
/// `gross_quote_pnl` is in the quote asset. In a backtest it is the change
/// in cash over the trade, so both fees are already taken off; it is not a
/// futures PnL, which the engine keeps apart.
#[pyclass(frozen, eq, module = "quantforge_py")]
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ClosedTrade(pub(crate) quantforge::ClosedTrade);

/// A venue's order ID as an argument: text the engine takes for one, or no
/// value, which is how an argument left out or given as `None` arrives.
fn order_id(
    value: Option<&Bound<'_, PyAny>>,
    what: &str,
) -> PyResult<Option<quantforge::VenueOrderId>> {
    let Some(value) = value else {
        return Ok(None);
    };
    match value.cast::<PyString>() {
        Ok(id) => quantforge::VenueOrderId::new(id.to_cow()?)
            .map(Some)
            .map_err(refused),
        Err(_) => Err(wrong_type(what, "a str or None", value)),
    }
}

#[pymethods]
impl ClosedTrade {
    #[new]
    #[pyo3(signature = (
        *,
        symbol,
        entry_time_ms,
        exit_time_ms,
        entry_price,
        exit_price,
        qty,
        gross_quote_pnl,
        entry_order_id=None,
        exit_order_id=None,
    ))]
    #[allow(clippy::too_many_arguments)]
    fn new(
        symbol: &Bound<'_, PyAny>,
        entry_time_ms: &Bound<'_, PyAny>,
        exit_time_ms: &Bound<'_, PyAny>,
        entry_price: &Bound<'_, PyAny>,
        exit_price: &Bound<'_, PyAny>,
        qty: &Bound<'_, PyAny>,
        gross_quote_pnl: &Bound<'_, PyAny>,
        entry_order_id: Option<&Bound<'_, PyAny>>,
        exit_order_id: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Self> {
        Ok(Self(quantforge::ClosedTrade {
            symbol: Symbol::taken(symbol, "symbol")?,
            entry_time_ms: integer(entry_time_ms, "entry_time_ms")?,
            exit_time_ms: integer(exit_time_ms, "exit_time_ms")?,
            entry_price: decimal(entry_price, "entry_price")?,
            exit_price: decimal(exit_price, "exit_price")?,
            qty: decimal(qty, "qty")?,
            gross_quote_pnl: decimal(gross_quote_pnl, "gross_quote_pnl")?,
            entry_order_id: order_id(entry_order_id, "entry_order_id")?,
            exit_order_id: order_id(exit_order_id, "exit_order_id")?,
        }))
    }

    /// The symbol traded.
    #[getter]
    fn symbol(&self) -> Symbol {
        Symbol(self.0.symbol.clone())
    }

    /// When the entry filled, in milliseconds since the Unix epoch, UTC.
    #[getter]
    fn entry_time_ms(&self) -> i64 {
        self.0.entry_time_ms
    }

    /// When the exit filled, in milliseconds since the Unix epoch, UTC.
    #[getter]
    fn exit_time_ms(&self) -> i64 {
        self.0.exit_time_ms
    }

    /// The price the entry filled at.
    #[getter]
    fn entry_price<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        decimal_object(py, self.0.entry_price)
    }

    /// The price the exit filled at.
    #[getter]
    fn exit_price<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        decimal_object(py, self.0.exit_price)
    }

    /// The quantity traded, in the base asset.
    #[getter]
    fn qty<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        decimal_object(py, self.0.qty)
    }

    /// The trade's PnL in the quote asset: in a backtest, the change in
    /// cash over the trade, with both fees already taken off.
    #[getter]
    fn gross_quote_pnl<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        decimal_object(py, self.0.gross_quote_pnl)
    }

    /// The venue's ID of the entry order, or `None` where none reached a
    /// venue.
    #[getter]
    fn entry_order_id(&self) -> Option<&str> {
        self.0.entry_order_id.as_ref().map(|id| id.as_str())
    }

    /// The venue's ID of the exit order, or `None` where none reached a
    /// venue.
    #[getter]
    fn exit_order_id(&self) -> Option<&str> {
        self.0.exit_order_id.as_ref().map(|id| id.as_str())
    }

    fn __repr__(slf: &Bound<'_, Self>) -> PyResult<String> {
        let py = slf.py();
        let this = slf.get();
        let trade = &this.0;
        repr_of(
            "ClosedTrade",
            &[
                ("symbol", Bound::new(py, this.symbol())?.into_any()),
                (
                    "entry_time_ms",
                    trade.entry_time_ms.into_pyobject(py)?.into_any(),
                ),
                (
                    "exit_time_ms",
                    trade.exit_time_ms.into_pyobject(py)?.into_any(),
                ),
                ("entry_price", decimal_object(py, trade.entry_price)?),
                ("exit_price", decimal_object(py, trade.exit_price)?),
                ("qty", decimal_object(py, trade.qty)?),
                (
                    "gross_quote_pnl",
                    decimal_object(py, trade.gross_quote_pnl)?,
                ),
                (
                    "entry_order_id",
                    this.entry_order_id().into_pyobject(py)?.into_any(),
                ),
                (
                    "exit_order_id",
                    this.exit_order_id().into_pyobject(py)?.into_any(),
                ),
            ],
        )
    }
}
