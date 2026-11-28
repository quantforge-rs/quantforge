//! The compiled half of the `quantforge_py` Python package: the extension
//! module `quantforge_py._native`, which the package's `__init__.py` reads
//! its names from.
//!
//! It holds what the package says about itself, and the engine's types as
//! Python classes: market identity, candles, and a backtest's settings and
//! result, each built by the engine's own rule and refused in the engine's
//! own words; the strategy a class implements, which an engine drives
//! through the adapter (`adapter.rs`); `run_backtest`, which runs such a
//! strategy over candles in the engine (`run.rs`); a batch of candles as
//! NumPy arrays, and back (`arrays.rs`); what the package's pandas
//! functions, which are written in Python, are built on (`frames.rs`); the
//! reader of the candles a store holds (`reader.rs`); the runtime a
//! strategy bundle runs in (`runtime.rs`), with the worker a QuantForge
//! server runs bundles in (`worker.rs`); and what the package's client,
//! which is written in Python, builds a bundle with before it submits one
//! (`bundle.rs`) and reads a remote job's result with (`result.rs`).
//! `docs/python-package.md` says how the package is named, built and
//! tested, `docs/python-types.md` what each type takes and gives and how a
//! backtest is run, `docs/python-numpy.md` what crosses to NumPy and how,
//! `docs/python-pandas.md` what a frame of candles and of a result is,
//! `docs/python-reader.md` which store a reader opens and what it reads,
//! `docs/python-bundles.md` how a bundle is built and submitted, and
//! `docs/python-jobs.md` how a job is waited for and followed, and what
//! its result and its provenance are.

// As in the `quantforge` crate. The unsafe code an extension module needs is
// what PyO3's macros expand to, which the lint leaves alone.
#![forbid(unsafe_code)]
#![deny(missing_docs)]

use pyo3::prelude::*;

mod adapter;
mod arrays;
mod backtest;
mod bench;
mod bundle;
mod candles;
mod convert;
mod extra;
mod floats;
mod frames;
mod market;
mod reader;
mod result;
mod run;
mod runtime;
mod strategy;
mod trade;
mod worker;

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
    // Private: what the client builds a bundle with, writes a manifest by
    // and reads a server's runtime by, each the engine's own code.
    #[pymodule_export]
    use crate::bundle::{bundle, manifest, runtime};
    #[pymodule_export]
    use crate::candles::{Candle, CandleBatch};
    #[pymodule_export]
    use crate::floats::{decimal_from_f64, decimal_to_f64};
    // Private: what the package's pandas functions, written in Python, are
    // built on.
    #[pymodule_export]
    use crate::frames::{equity_columns, found_numpy, found_pandas, trade_columns};
    #[pymodule_export]
    use crate::market::{Category, ExchangeId, Interval, MarketId, Symbol};
    #[pymodule_export]
    use crate::reader::CandleReader;
    // Private: the window of a job a client submits, read as a reader
    // reads a range, and a time as the engine writes one.
    #[pymodule_export]
    use crate::reader::{bounds, rfc3339};
    // Private: what the client reads a remote job's result with, each the
    // engine's own code: its metrics written as the file the result format
    // gives them, and its two tables read into the package's own types.
    #[pymodule_export]
    use crate::result::{equity, metrics, trades};
    #[pymodule_export]
    use crate::run::run_backtest;
    #[pymodule_export]
    use crate::strategy::{Strategy, StrategyContext, TargetPosition};
    #[pymodule_export]
    use crate::trade::ClosedTrade;
    // Private: the worker a server runs Python bundles in, which
    // `quantforge_py._worker` starts.
    #[pymodule_export]
    use crate::worker::run_worker;

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
