//! The bench the adapter is tested on: one backtest of a Python strategy in
//! the engine, run as each kind of host runs one. It is private, and reached
//! by the package's own tests alone; the function a user calls is bound by
//! the story that follows, on the adapter this exercises.

use std::thread;

use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;

use crate::adapter::PyStrategy;
use crate::backtest::{BacktestConfig, BacktestResult};
use crate::candles::Candle;
use crate::convert::{each, refused};
use crate::market::MarketId;

/// Runs `strategy` over `candles` in the engine, and gives the result.
///
/// - **As Python hosts a run** (`embedded=False`): on the calling thread,
///   which stays attached throughout. A callback that fails raises its own
///   exception here, and what the engine itself refuses is a `ValueError`
///   in the engine's words.
/// - **As a host that embeds Python runs one** (`embedded=True`): on a
///   thread of the engine's own, attached once around the run and detached
///   while the engine computes, with the calling thread detached until it
///   is over. Such a host has the engine's error and nothing else, so every
///   failure is a `RuntimeError` of that error's text.
#[pyfunction]
#[pyo3(name = "_backtest", signature = (market, candles, strategy, config, *, embedded=false))]
pub(crate) fn backtest(
    py: Python<'_>,
    market: &Bound<'_, MarketId>,
    candles: &Bound<'_, PyAny>,
    strategy: &Bound<'_, PyAny>,
    config: &Bound<'_, BacktestConfig>,
    embedded: bool,
) -> PyResult<BacktestResult> {
    let market = market.get().0.clone();
    let candles = each(candles, "candles", "a Candle", |candle: &Candle| {
        candle.0.clone()
    })?;
    let engine = quantforge::BacktestEngine::new(config.get().0.clone());
    let mut strategy = PyStrategy::new(strategy)?;

    if !embedded {
        let outcome = engine.run(&market, &candles, &mut strategy);
        if let Some(raised) = strategy.failure() {
            return Err(raised);
        }
        return outcome.map(BacktestResult).map_err(refused);
    }

    let joined = py.detach(|| {
        thread::scope(|scope| {
            let run =
                || Python::attach(|py| py.detach(|| engine.run(&market, &candles, &mut strategy)));
            thread::Builder::new()
                .name("quantforge-engine".to_string())
                .spawn_scoped(scope, run)
                .map(|engine| engine.join())
        })
    })?;
    match joined {
        Ok(outcome) => outcome
            .map(BacktestResult)
            .map_err(|error| PyRuntimeError::new_err(error.to_string())),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}
