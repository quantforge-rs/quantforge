//! The compiled half of the `quantforge_py` Python package: the extension
//! module `quantforge_py._native`, which the package's `__init__.py` reads
//! its names from.
//!
//! It holds what the package says about itself, and the engine's types as
//! Python classes: market identity, candles, and a backtest's settings and
//! result, each built by the engine's own rule and refused in the engine's
//! own words; the strategy a class implements, which an engine drives
//! through the adapter (`adapter.rs`); and `run_backtest`, which runs such
//! a strategy over candles in the engine (`run.rs`).
//! `docs/python-package.md` says how the package is named, built and
//! tested, and `docs/python-types.md` what each type takes and gives and
//! how a backtest is run.

// As in the `quantforge` crate. The unsafe code an extension module needs is
// what PyO3's macros expand to, which the lint leaves alone.
#![forbid(unsafe_code)]
#![deny(missing_docs)]

use pyo3::prelude::*;

mod adapter;
mod backtest;
mod bench;
mod candles;
mod convert;
mod floats;
mod market;
mod run;
mod strategy;
mod trade;

/// The compiled extension module of QuantForge's Python package. Import
/// `quantforge_py` instead: it publishes these names, and this module's
/// own layout may change.
#[pymodule(name = "_native")]
mod native {
    use pyo3::prelude::*;
    use pyo3::types::PyTuple;
    use quantforge::StorageBackend;

    #[pymodule_export]
    use crate::backtest::{BacktestConfig, BacktestResult, EquityPoint};
    // Private: the package's own tests run the other kind of host through
    // it.
    #[pymodule_export]
    use crate::bench::embedded_backtest;
    #[pymodule_export]
    use crate::candles::{Candle, CandleBatch};
    #[pymodule_export]
    use crate::floats::{decimal_from_f64, decimal_to_f64};
    #[pymodule_export]
    use crate::market::{Category, ExchangeId, Interval, MarketId, Symbol};
    #[pymodule_export]
    use crate::run::run_backtest;
    #[pymodule_export]
    use crate::strategy::{Strategy, StrategyContext, TargetPosition};
    #[pymodule_export]
    use crate::trade::ClosedTrade;

    /// The version of the engine the package was built with.
    #[pymodule_export]
    const ENGINE_VERSION: &str = quantforge::ENGINE_VERSION;

    #[pymodule_init]
    fn init(module: &Bound<'_, PyModule>) -> PyResult<()> {
        // The package's own version. An installed wheel says the same in its
        // metadata; a host that registers this module itself has only this.
        module.add("__version__", env!("CARGO_PKG_VERSION"))?;
        // The stores this build can open, by the names `--storage` takes.
        let built: Vec<&str> = StorageBackend::ALL
            .into_iter()
            .filter(|backend| backend.is_built())
            .map(StorageBackend::as_str)
            .collect();
        module.add("STORAGE_BACKENDS", PyTuple::new(module.py(), built)?)?;
        // Every interval, product family, product line and target, as a
        // constant of its class.
        crate::market::name_constants(module.py())?;
        crate::strategy::name_targets(module.py())
    }
}
