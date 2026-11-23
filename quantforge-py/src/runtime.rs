//! The runtime a strategy bundle runs in, the one path every host that
//! runs a bundle takes: the bundle's strategy imported from its files,
//! held to what a bundle may import and built from its parameters
//! (`python/quantforge_py/_runtime.py`), then driven by the engine through
//! the adapter (`adapter.rs`), and a strategy that failed worded as its job
//! says it, with its traceback. A server's worker is its first host
//! (`worker.rs`); `docs/python-worker.md` says what each step does.
//!
//! What a host hands over is what the bundle's manifest and the server
//! say, and nothing else: the bundle's files, its entry, its parameters,
//! the packages it declares and the ones on offer. The runtime holds no
//! store and no credential, and needs none.

use std::collections::BTreeMap;
use std::path::Path;

use pyo3::prelude::*;
use pyo3::sync::PyOnceLock;
use pyo3::types::PyType;

use crate::adapter::PyStrategy;

/// The private module the runtime's Python half is.
const MODULE: &str = "quantforge_py._runtime";

static BUNDLE: PyOnceLock<Py<PyType>> = PyOnceLock::new();
static REFUSED: PyOnceLock<Py<PyType>> = PyOnceLock::new();
static FAILED: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

/// Why a bundle's strategy did not run to its end: what its job's failure
/// says, and the traceback, where there is one, for the job's log.
#[derive(Debug)]
pub(crate) struct Failure {
    pub(crate) message: String,
    pub(crate) traceback: String,
}

/// One bundle as a host hands it over: where its files are, and what its
/// manifest says of its strategy and its packages.
#[derive(Debug)]
pub(crate) struct BundleSpec<'a> {
    /// The directory the bundle's files are in.
    pub(crate) root: &'a Path,
    /// Its entry, `path/to/module.py:Class`.
    pub(crate) entry: &'a str,
    /// The module the entry names, as Python imports it.
    pub(crate) module: &'a str,
    /// The class the entry names.
    pub(crate) class: &'a str,
    /// Its parameters, as JSON.
    pub(crate) params: String,
    /// The packages it declares, each with the version it pins.
    pub(crate) deps: &'a BTreeMap<String, String>,
    /// The packages the host offers, each at the version it holds.
    pub(crate) offered: &'a BTreeMap<String, String>,
}

/// The bundle's strategy, built, as the engine drives it: from now on the
/// bundle's root is the working directory and on the import path, and what
/// the bundle's own code imports is held to the rules. Where the bundle
/// cannot give one, the failure says why.
pub(crate) fn strategy(py: Python<'_>, bundle: &BundleSpec<'_>) -> Result<PyStrategy, Failure> {
    let built = (|| -> PyResult<Bound<'_, PyAny>> {
        let class = BUNDLE.import(py, MODULE, "Bundle")?;
        let made = class.call1((
            bundle.root,
            bundle.entry,
            bundle.module,
            bundle.class,
            bundle.params.as_str(),
            bundle.deps,
            bundle.offered,
        ))?;
        made.call_method0("strategy")
    })();
    let strategy = match built {
        Ok(strategy) => strategy,
        Err(error) => return Err(refusal(py, error)),
    };
    PyStrategy::new(&strategy).map_err(|error| Failure {
        message: format!("the bundle's {} cannot be run: {error}", bundle.entry),
        traceback: String::new(),
    })
}

/// The failure a refusal of the runtime's says, with its traceback, or,
/// for anything else the runtime itself raised, its own words.
fn refusal(py: Python<'_>, error: PyErr) -> Failure {
    let said = REFUSED.import(py, MODULE, "Refused").and_then(|refused| {
        if !error.is_instance(py, refused.as_any()) {
            return Ok(None);
        }
        let value = error.value(py);
        Ok(Some(Failure {
            message: value.getattr("message")?.extract()?,
            traceback: value.getattr("traceback")?.extract()?,
        }))
    });
    match said {
        Ok(Some(failure)) => failure,
        _ => Failure {
            message: format!("the bundle could not be loaded: {error}"),
            traceback: String::new(),
        },
    }
}

/// A callback's failure, which the adapter kept as it was raised, as the
/// job says it: where the strategy failed and the exception's own line,
/// and its traceback, the bundle's files named from `root`.
pub(crate) fn failed(py: Python<'_>, error: &PyErr, root: &Path) -> Failure {
    // The traceback is handed over beside the exception: before Python
    // 3.12 one the adapter kept does not hold its own.
    let worded = FAILED
        .import(py, MODULE, "failed")
        .and_then(|failed| failed.call1((error.value(py), error.traceback(py), root)))
        .and_then(|worded| worded.extract::<(String, String)>());
    match worded {
        Ok((message, traceback)) => Failure { message, traceback },
        Err(_) => Failure {
            message: format!("the strategy failed: {error}"),
            traceback: String::new(),
        },
    }
}
