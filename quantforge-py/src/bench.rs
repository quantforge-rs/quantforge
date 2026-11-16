//! The bench the other kind of host is tested on: one backtest of a Python
//! strategy as a host that embeds Python runs one. It is private, and
//! reached by the package's own tests alone. The run Python itself hosts is
//! the public function (`run.rs`), which the tests reach as a user does.

use std::thread;

use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;

use crate::backtest::BacktestResult;
use crate::run::Run;

/// Runs `strategy` over `candles` in the engine as a host that embeds
/// Python does, and gives the result. It takes what `run_backtest` takes,
/// and reads it the same way.
///
/// The run is on a thread of the engine's own, attached once around the run
/// and detached while the engine computes, with the calling thread detached
/// until it is over. Such a host has the engine's error and nothing else,
/// so every failure is a `RuntimeError` of that error's text: a callback's
/// with its traceback, and a refusal in the engine's words.
#[pyfunction]
#[pyo3(name = "_embedded_backtest", signature = (candles, strategy, config, *, market))]
pub(crate) fn embedded_backtest(
    py: Python<'_>,
    candles: &Bound<'_, PyAny>,
    strategy: &Bound<'_, PyAny>,
    config: &Bound<'_, PyAny>,
    market: &Bound<'_, PyAny>,
) -> PyResult<BacktestResult> {
    let Run {
        market,
        candles,
        engine,
        mut strategy,
    } = Run::read(candles, strategy, config, market)?;

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
