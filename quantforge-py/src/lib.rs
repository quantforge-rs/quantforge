//! The compiled half of the `quantforge_py` Python package: the extension
//! module `quantforge_py._native`, which the package's `__init__.py` reads
//! its names from.
//!
//! So far it holds only what the package says about itself. The engine's
//! types, the strategy adapter and the backtest entry point are bound here
//! by the stories that follow; `docs/python-package.md` says how the package
//! is named, built and tested.

// As in the `quantforge` crate. The unsafe code an extension module needs is
// what PyO3's macros expand to, which the lint leaves alone.
#![forbid(unsafe_code)]
#![deny(missing_docs)]

use pyo3::prelude::*;

/// The compiled extension module of QuantForge's Python package. Import
/// `quantforge_py` instead: it publishes these names, and this module's
/// own layout may change.
#[pymodule(name = "_native")]
mod native {
    use pyo3::prelude::*;
    use pyo3::types::PyTuple;
    use quantforge::StorageBackend;

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
        module.add("STORAGE_BACKENDS", PyTuple::new(module.py(), built)?)
    }
}
