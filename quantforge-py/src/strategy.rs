//! The strategy a Python class implements, with what its callbacks are
//! handed and what they hand back: the class to derive from, the engine's
//! state at a callback, and the position a strategy asks for.
//!
//! These are the candle strategy's own boundary, as the engine has it
//! (`quantforge::{Strategy, StrategyContext, TargetPosition}`): three
//! callbacks, one context of four fields, two targets. Nothing of an
//! exchange, an account, a store or an external signal is part of it.
//! `adapter.rs` is how the engine drives a class derived from [`Strategy`].

use std::hash::{Hash, Hasher};

use pyo3::exceptions::PyTypeError;
use pyo3::intern;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyTuple, PyType};

use crate::convert::{decimal, decimal_object, integer, repr_of, wrong_type};
use crate::market::{MarketId, name_each};

/// The position a strategy asks for.
///
/// `TargetPosition.FLAT` holds nothing, and `TargetPosition.LONG_ALL_IN`
/// holds the base asset bought with all the cash. The two are constants of
/// the class, and `TargetPosition.ALL` holds both. There is no other
/// target, and none is built: `on_bar` returns one of the two, or `None` to
/// leave the position as it is.
#[pyclass(frozen, eq, hash, module = "quantforge_py")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TargetPosition(pub(crate) quantforge::TargetPosition);

impl TargetPosition {
    /// Every target, in the order `ALL` gives them. `constant` below stops
    /// compiling when the engine gains one, which is when this list grows.
    const EVERY: [Self; 2] = [
        Self(quantforge::TargetPosition::Flat),
        Self(quantforge::TargetPosition::LongAllIn),
    ];

    /// The name of the target's constant: the engine's name for it, in
    /// capitals with its words apart.
    fn constant(self) -> &'static str {
        match self.0 {
            quantforge::TargetPosition::Flat => "FLAT",
            quantforge::TargetPosition::LongAllIn => "LONG_ALL_IN",
        }
    }
}

// The engine's type compares and is no key of its own; a constant of two is
// one in Python, as a member of an enum is.
impl Hash for TargetPosition {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.constant().hash(state);
    }
}

#[pymethods]
impl TargetPosition {
    fn __repr__(&self) -> String {
        format!("TargetPosition.{}", self.constant())
    }
}

/// Names each target as a constant of its class, with `ALL`.
pub(crate) fn name_targets(py: Python<'_>) -> PyResult<()> {
    name_each(
        py,
        TargetPosition::EVERY.map(|target| (target.constant().to_string(), target)),
    )
}

/// The engine's state as a strategy's callback is handed it.
///
/// `StrategyContext(*, market, now_ms, cash, position_qty)`: `market` is
/// the `MarketId` the run is on; `now_ms` the time of the callback, in
/// milliseconds since the Unix epoch, UTC, which in a backtest is the open
/// time of the bar; `cash` the quote currency held; and `position_qty` the
/// base asset held. The two amounts are exact decimals.
///
/// Each callback is handed a context of its own, which holds what the
/// engine said then and is never changed afterwards. An engine builds them;
/// one built by hand is for calling a strategy's callbacks in a test.
#[pyclass(frozen, eq, module = "quantforge_py")]
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StrategyContext(pub(crate) quantforge::StrategyContext);

#[pymethods]
impl StrategyContext {
    #[new]
    #[pyo3(signature = (*, market, now_ms, cash, position_qty))]
    fn new(
        market: &Bound<'_, PyAny>,
        now_ms: &Bound<'_, PyAny>,
        cash: &Bound<'_, PyAny>,
        position_qty: &Bound<'_, PyAny>,
    ) -> PyResult<Self> {
        let market = match market.cast::<MarketId>() {
            Ok(market) => market.get().0.clone(),
            Err(_) => return Err(wrong_type("market", "a MarketId", market)),
        };
        Ok(Self(quantforge::StrategyContext {
            market,
            now_ms: integer(now_ms, "now_ms")?,
            cash: decimal(cash, "cash")?,
            position_qty: decimal(position_qty, "position_qty")?,
        }))
    }

    /// The market the run is on.
    #[getter]
    fn market(&self) -> MarketId {
        MarketId(self.0.market.clone())
    }

    /// The time of the callback, in milliseconds since the Unix epoch, UTC:
    /// in a backtest, the open time of the bar.
    #[getter]
    fn now_ms(&self) -> i64 {
        self.0.now_ms
    }

    /// The quote currency held.
    #[getter]
    fn cash<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        decimal_object(py, self.0.cash)
    }

    /// The base asset held.
    #[getter]
    fn position_qty<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        decimal_object(py, self.0.position_qty)
    }

    fn __repr__(slf: &Bound<'_, Self>) -> PyResult<String> {
        let py = slf.py();
        let this = slf.get();
        repr_of(
            "StrategyContext",
            &[
                ("market", Bound::new(py, this.market())?.into_any()),
                ("now_ms", this.0.now_ms.into_pyobject(py)?.into_any()),
                ("cash", decimal_object(py, this.0.cash)?),
                ("position_qty", decimal_object(py, this.0.position_qty)?),
            ],
        )
    }
}

/// A trading strategy: derive a class from `Strategy` and define `on_bar`,
/// and an engine drives it bar by bar.
///
/// The engine calls `on_start(ctx)` once before the first bar, `on_bar(ctx,
/// bar)` once for every closed candle in order, and `on_finish(ctx)` once
/// after the last. `ctx` is a `StrategyContext`, the engine's state at that
/// moment, and `bar` a `Candle`. A callback that raises ends the run there,
/// with its exception, and no callback follows it.
///
/// `on_bar` returns the position to hold after the bar, a `TargetPosition`,
/// or `None` to leave the position as it is. `on_start` and `on_finish`
/// return `None`, and do nothing unless the class defines them.
///
/// The class takes whatever arguments its own `__init__` does, and keeps
/// its state in itself. It is named by its `name`, a `str`, where it has
/// one, and by its class otherwise. A strategy decides from the bars it has
/// been handed and from its own state alone: no clock, no randomness and no
/// I/O, so that the same bars give the same decisions on every run.
// A unit: the state of a strategy is its Python object's own.
#[pyclass(frozen, subclass, module = "quantforge_py")]
#[derive(Debug)]
pub(crate) struct Strategy;

#[pymethods]
impl Strategy {
    // The arguments are those of the class derived from this one, whose
    // `__init__` reads them: Python hands a class's `__new__` what it hands
    // its `__init__`.
    #[new]
    #[classmethod]
    #[pyo3(signature = (*args, **kwargs), text_signature = "(*args, **kwargs)")]
    fn new(
        cls: &Bound<'_, PyType>,
        args: &Bound<'_, PyTuple>,
        kwargs: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<Self> {
        let py = cls.py();
        let base = py.get_type::<Self>();
        if cls.is(&base) {
            return Err(PyTypeError::new_err(
                "Strategy cannot be built: derive a class from it that defines on_bar",
            ));
        }
        let on_bar = intern!(py, "on_bar");
        if cls.getattr(on_bar)?.is(&base.getattr(on_bar)?) {
            return Err(PyTypeError::new_err(format!(
                "{} cannot be built: it does not define on_bar",
                cls.name()?
            )));
        }
        // As `object` refuses arguments no `__init__` is there to take.
        let init = intern!(py, "__init__");
        let given = !args.is_empty() || kwargs.is_some_and(|kwargs| !kwargs.is_empty());
        if given
            && cls
                .getattr(init)?
                .is(&py.get_type::<PyAny>().getattr(init)?)
        {
            return Err(PyTypeError::new_err(format!(
                "{}() takes no arguments",
                cls.name()?
            )));
        }
        Ok(Self)
    }

    /// Called once before the first bar. Does nothing unless a class
    /// defines it.
    #[allow(unused_variables)]
    fn on_start(&self, ctx: &Bound<'_, PyAny>) {}

    /// Called once for every closed candle, in order: returns the
    /// `TargetPosition` to hold after the bar, or `None` to leave the
    /// position as it is. A class defines its own; this one decides nothing.
    #[allow(unused_variables)]
    fn on_bar(&self, ctx: &Bound<'_, PyAny>, bar: &Bound<'_, PyAny>) {}

    /// Called once after the last bar, unless a callback raised. Does
    /// nothing unless a class defines it.
    #[allow(unused_variables)]
    fn on_finish(&self, ctx: &Bound<'_, PyAny>) {}
}
