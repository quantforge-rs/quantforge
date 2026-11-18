//! Running a backtest: [`run_backtest`], the function a strategy written in
//! Python is run with, and the one reading of a run's arguments, which the
//! bench shares (`bench.rs`).
//!
//! The run is the engine's own (`quantforge::BacktestEngine::run`), driven
//! through the adapter (`adapter.rs`), and adds nothing to it: the engine
//! refuses what it does not run, calls the strategy, and computes the
//! result, which crosses as it is.
//!
//! - **Python hosts the run.** It is on the calling thread, which stays
//!   attached throughout: the callbacks run where the caller is, and the
//!   interpreter is kept between them (`docs/python-package.md`).
//! - **A callback that fails raises its own exception,** the object it
//!   raised, in the caller.
//! - **What the engine refuses or cannot compute is raised in the engine's
//!   words** ([`raised`]).

use pyo3::exceptions::{PyOverflowError, PyRuntimeError, PyTypeError, PyValueError};
use pyo3::prelude::*;
use quantforge::EngineError;

use crate::adapter::PyStrategy;
use crate::backtest::{BacktestConfig, BacktestResult};
use crate::candles;
use crate::convert::wrong_type;
use crate::market::MarketId;

/// One backtest with every argument read: the engine's own values, and the
/// adapter over the strategy.
pub(crate) struct Run {
    pub(crate) market: quantforge::MarketId,
    pub(crate) candles: Vec<quantforge::Candle>,
    pub(crate) engine: quantforge::BacktestEngine,
    pub(crate) strategy: PyStrategy,
}

impl Run {
    /// Reads a run's arguments in the order they are written, each held to
    /// its type: the candles to their end, the strategy with its name, the
    /// config, the market. Then the market the run is on is settled: the
    /// one given, the one a batch names, and never two that differ. Nothing
    /// is asked of the engine yet, and no callback is called.
    pub(crate) fn read(
        candles: &Bound<'_, PyAny>,
        strategy: &Bound<'_, PyAny>,
        config: &Bound<'_, PyAny>,
        market: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Self> {
        let (candles, named) = candles::taken(candles)?;
        let strategy = PyStrategy::new(strategy)?;
        let config = match config.cast::<BacktestConfig>() {
            Ok(config) => config.get().0.clone(),
            Err(_) => return Err(wrong_type("config", "a BacktestConfig", config)),
        };
        let given = market
            .map(|market| MarketId::taken(market, "a MarketId"))
            .transpose()?;
        Ok(Self {
            market: settled(given, named)?,
            candles,
            engine: quantforge::BacktestEngine::new(config),
            strategy,
        })
    }

    /// Runs it as Python hosts a run: on the calling thread, which stays
    /// attached.
    fn hosted(mut self) -> PyResult<BacktestResult> {
        let outcome = self
            .engine
            .run(&self.market, &self.candles, &mut self.strategy);
        // The exception a callback raised, in place of the engine's account
        // of it.
        if let Some(raised) = self.strategy.failure() {
            return Err(raised);
        }
        outcome.map(BacktestResult).map_err(raised)
    }
}

/// The market a run is on: the one it was `given`, or the one its candles
/// are `named` to be of by the batch that holds them.
///
/// - **Two that differ are refused.** Candles of one market are never run
///   as another's, a product line's as its Spot namesake's least of all.
/// - **No market at all is refused as well.** Candles that name none are of
///   whatever the caller says, and nothing is assumed for them.
fn settled(
    given: Option<quantforge::MarketId>,
    named: Option<quantforge::MarketId>,
) -> PyResult<quantforge::MarketId> {
    match (given, named) {
        (Some(given), Some(named)) if given != named => Err(PyValueError::new_err(format!(
            "the candles are of {}, and market names {}",
            MarketId::named(&named),
            MarketId::named(&given)
        ))),
        (Some(market), _) | (None, Some(market)) => Ok(market),
        (None, None) => Err(PyTypeError::new_err(
            "market is required: the candles name no market",
        )),
    }
}

/// An engine's error as the exception Python is raised for it, with the
/// engine's message unchanged.
///
/// Every error the engine has is named, so that one it gains is decided
/// here before the package builds again.
fn raised(error: EngineError) -> PyErr {
    let said = error.to_string();
    match error {
        // What the run was given is not something the engine runs.
        EngineError::NotSpot { .. } | EngineError::NoCandles | EngineError::InvalidConfig(_) => {
            PyValueError::new_err(said)
        }
        // An amount left the range of the engine's decimal as it was
        // computed: no argument is at fault alone.
        EngineError::Overflow(_) => PyOverflowError::new_err(said),
        // A callback's failure is raised as the callback raised it, before
        // this is reached, and the rest is no part of a backtest.
        EngineError::Strategy(_)
        | EngineError::InvalidState(_)
        | EngineError::Exchange(_)
        | EngineError::Storage(_)
        | EngineError::TransferRefused(_)
        | EngineError::CopyStopped { .. } => PyRuntimeError::new_err(said),
    }
}

/// Runs a strategy over candles in the engine, and gives the result:
/// `run_backtest(candles, strategy, config, *, market=None)`.
///
/// `candles` is a `CandleBatch`, or anything that iterates `Candle`s, and
/// is replayed in the order given; `strategy` is an object of a class
/// derived from `Strategy`; `config` is a `BacktestConfig`; and `market`
/// is the `MarketId` the candles are of, a Spot market, which every
/// callback is handed and every trade takes its symbol from. A batch that
/// names its market is run on it, and `market` may be left out; candles
/// that name none need it, and a batch is never run on a market other than
/// the one it names.
///
/// The run is the engine's deterministic backtest. `on_bar` is called once
/// for every candle with the complete bar, and the position it returns is
/// taken at the next bar's open, a price the strategy had not seen when it
/// decided. Every fill pays the config's fee, a position still open after
/// the last bar is sold at that bar's close where the config says so, and
/// equity is marked at every bar's close. The same candles and config give
/// the same `BacktestResult` on every run and every machine, for a strategy
/// that starts each run in the same state and decides from its bars and
/// that state alone, with no clock and no randomness. A strategy keeps its
/// state from one run to the next, so each run is given one built anew, the
/// same way.
///
/// The run is on the calling thread. An argument of the wrong type is a
/// `TypeError`, and so is a run that names no market; a market other than
/// the one a batch names, a market that is not Spot and candles that are
/// none are a `ValueError`, the last two in the engine's words, before any
/// callback is called; an amount beyond the engine's decimal is an
/// `OverflowError`; and a callback that fails ends the run with its own
/// exception.
#[pyfunction]
#[pyo3(signature = (candles, strategy, config, *, market=None))]
pub(crate) fn run_backtest(
    candles: &Bound<'_, PyAny>,
    strategy: &Bound<'_, PyAny>,
    config: &Bound<'_, PyAny>,
    market: Option<&Bound<'_, PyAny>>,
) -> PyResult<BacktestResult> {
    Run::read(candles, strategy, config, market)?.hosted()
}
