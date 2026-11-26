//! A strategy bundle as the package's client builds one, and the runtime a
//! server offers, each by the engine's own code (`docs/python-bundles.md`).
//!
//! What a client sends a server is checked there by
//! `quantforge::protocol::bundle`, the reference implementation of the
//! bundle spec. The client checks with the same code, compiled into this
//! module, before it sends anything:
//!
//! - **A bundle is collected, checked and packed here,** from a directory
//!   or from files given in memory: which files go in, every rule of the
//!   spec, the canonical tar and the code hash taken over it. The manifest
//!   is read as a server reads one, by Rust's `toml`, whatever TOML the
//!   Python that runs the client reads.
//! - **A server's runtime is read as the protocol reads it,** and a bundle
//!   is admitted against it by the check a server makes of an upload.
//! - **A manifest is written by the writer every client shares,**
//!   `written_manifest`, which the command line writes with too: the same
//!   entry, parameters and packages make the same text, and so the same
//!   bundle, from Python as from `quantforge backtest --remote`. A float is
//!   handed over as the float it is, and never as text another reader
//!   would have to round.
//!
//! What is the client's own stays in Python (`_bundles.py`): what it takes
//! as a strategy and as its parameters, and what it raises. The entries
//! are private, and none raises for a bundle that is refused: a refusal is
//! the list of its problems, each the spec's code and a message, for the
//! package to raise as its own exception.

use std::collections::BTreeMap;
use std::path::PathBuf;

use pyo3::exceptions::{PyOSError, PyOverflowError, PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyBool, PyBytes, PyDict, PyFloat, PyInt, PyList, PyString};
use quantforge::protocol::bundle::{
    Bundle, BundleError, BundleLimits, MAX_MANIFEST_DEPTH, Runtime, written_manifest,
};
use quantforge::protocol::jobs::JobRuntime;

use crate::convert::wrong_type;

/// What a parameter is, as a refusal says it.
const PARAM: &str = "a str, a bool, an int, a float, a list or a dict";

/// One thing wrong with a bundle: the spec's code, and its message.
type Problem = (&'static str, String);

/// A bundle that passed: its code hash, its canonical tar, and what its
/// manifest says.
struct Built {
    bundle: Bundle,
    tar: Vec<u8>,
    code_hash: String,
}

/// The answer of `GET /v1/runtime`, read as the protocol reads it.
fn answered(text: &str) -> PyResult<JobRuntime> {
    serde_json::from_str(text).map_err(|error| PyValueError::new_err(error.to_string()))
}

/// What a server's workers run bundles on, from the JSON `GET /v1/runtime`
/// answers: `_server_runtime(text)`.
///
/// A dict of `engine`, the engine's version, `python`, the Python's, as
/// `MAJOR.MINOR`, and `packages`, each package a bundle may import with the
/// versions the server offers of it, in order; or `None` for a server that
/// runs no bundles. An answer the protocol does not write, a version or a
/// package name that is none among it, is a `ValueError` that says what is
/// wrong with it.
#[pyfunction]
#[pyo3(name = "_server_runtime")]
pub(crate) fn runtime<'py>(py: Python<'py>, text: &str) -> PyResult<Option<Bound<'py, PyDict>>> {
    let Some(runtime) = answered(text)?.runtime else {
        return Ok(None);
    };
    let offered = PyDict::new(py);
    offered.set_item("engine", runtime.engine.to_string())?;
    offered.set_item("python", runtime.python.to_string())?;
    let packages = PyDict::new(py);
    for (name, versions) in &runtime.packages {
        let versions = PyList::new(py, versions.iter().map(|version| version.as_str()))?;
        packages.set_item(name.as_str(), versions)?;
    }
    offered.set_item("packages", packages)?;
    Ok(Some(offered))
}

/// A bundle built from files, checked by the bundle spec and packed:
/// `_bundle(root, files, limits=None, runtime=None)`.
///
/// - `root`: a directory to collect, as the spec collects one, or `None`.
/// - `files`: files given in memory, each a path in the bundle and its
///   bytes: beside the directory's, where there is one, as the manifest a
///   client writes is.
/// - `limits`: the most bytes one file may hold, the most all may hold
///   together and the most files, or `None` for the spec's defaults.
/// - `runtime`: the JSON `GET /v1/runtime` answered, to admit the bundle
///   against once it has passed the spec, or `None`.
///
/// Gives a pair. For a bundle that passed, a dict and no problem: its
/// `code_hash`, its canonical `tar`, the `paths` of its files in the
/// tar's order, and its manifest's `entry`, `engine`, `python`, `params`,
/// as the JSON a strategy is handed, and `deps`. For one that is refused,
/// `None` and every problem, in the order the spec checks, each its code
/// and its message. A file or a directory that cannot be read is an
/// `OSError`.
///
/// The interpreter is given up while the files are read and hashed.
#[pyfunction]
#[pyo3(name = "_bundle", signature = (root, files, limits=None, runtime=None))]
pub(crate) fn bundle<'py>(
    py: Python<'py>,
    root: Option<PathBuf>,
    files: Vec<(String, Bound<'py, PyBytes>)>,
    limits: Option<(u64, u64, u32)>,
    runtime: Option<&str>,
) -> PyResult<(Option<Bound<'py, PyDict>>, Vec<Problem>)> {
    let given: Vec<(String, Vec<u8>)> = files
        .into_iter()
        .map(|(path, bytes)| (path, bytes.as_bytes().to_vec()))
        .collect();
    let limits = limits.map_or_else(BundleLimits::default, |(file, total, files)| BundleLimits {
        max_file_bytes: file,
        max_total_bytes: total,
        max_files: files,
    });
    let runtime = match runtime {
        None => None,
        Some(text) => Some(answered(text)?.runtime.ok_or_else(|| {
            PyValueError::new_err(
                "the server runs no bundles: it has no runtime to admit one against",
            )
        })?),
    };
    let built = py.detach(|| build(root, given, &limits, runtime.as_ref()));
    let built = match built {
        Ok(built) => built,
        Err(BundleError::Refused(problems)) => {
            let problems = problems
                .iter()
                .map(|problem| (problem.code().as_str(), problem.message().to_string()))
                .collect();
            return Ok((None, problems));
        }
        Err(error @ BundleError::Io { .. }) => return Err(PyOSError::new_err(error.to_string())),
        Err(other) => return Err(PyRuntimeError::new_err(other.to_string())),
    };
    let manifest = built.bundle.manifest();
    let passed = PyDict::new(py);
    passed.set_item("code_hash", built.code_hash)?;
    passed.set_item("tar", PyBytes::new(py, &built.tar))?;
    let paths = built.bundle.files().iter().map(|file| file.path().as_str());
    passed.set_item("paths", PyList::new(py, paths)?)?;
    passed.set_item("entry", manifest.entry().to_string())?;
    passed.set_item("engine", manifest.engine().to_string())?;
    passed.set_item("python", manifest.python().to_string())?;
    let params = serde_json::to_string(manifest.params())
        .map_err(|error| PyRuntimeError::new_err(error.to_string()))?;
    passed.set_item("params", params)?;
    let deps = PyDict::new(py);
    for (name, version) in manifest.deps() {
        deps.set_item(name.as_str(), version.as_str())?;
    }
    passed.set_item("deps", deps)?;
    Ok((Some(passed), Vec::new()))
}

/// Collects, checks and packs a bundle, and admits it against `runtime`
/// where one is given: the spec's own checks, in the spec's order.
fn build(
    root: Option<PathBuf>,
    given: Vec<(String, Vec<u8>)>,
    limits: &BundleLimits,
    runtime: Option<&Runtime>,
) -> Result<Built, BundleError> {
    let bundle = match root {
        Some(root) => Bundle::from_dir_with(&root, given, limits)?,
        None => Bundle::from_files(given, limits)?,
    };
    if let Some(runtime) = runtime {
        bundle.admit(runtime)?;
    }
    Ok(Built {
        tar: bundle.canonical_tar(),
        code_hash: bundle.code_hash().to_string(),
        bundle,
    })
}

/// The manifest a client writes for a strategy whose files hold none, in
/// the one form every client writes it: `_manifest(entry, python, params,
/// deps)`.
///
/// - `entry`: the strategy, `path/to/module.py:ClassName`.
/// - `python`: the Python version it is written for, the server's.
/// - `params`: the strategy's parameters, a dict whose values are each a
///   `str`, a `bool`, an `int`, a `float`, a `list` or a `dict` of those.
/// - `deps`: the packages it imports, each with the version it pins.
///
/// Its `engine` is the engine this package was built with, exactly, so
/// that a server runs the engine a local run would. Nothing of it is
/// checked here but that a value is one TOML writes: the bundle's own
/// check reads it back.
#[pyfunction]
#[pyo3(name = "_manifest", signature = (entry, python, params, deps))]
pub(crate) fn manifest(
    entry: &str,
    python: &str,
    params: &Bound<'_, PyDict>,
    deps: BTreeMap<String, String>,
) -> PyResult<String> {
    let params = table(params, "params", 2)?;
    let engine = format!("=={}", quantforge::ENGINE_VERSION);
    Ok(written_manifest(entry, &engine, python, &params, &deps))
}

/// A dict as the table TOML writes of it. `depth` is how deep it lies in
/// the manifest, whose own table is the first level.
fn table(given: &Bound<'_, PyDict>, at: &str, depth: usize) -> PyResult<toml::Table> {
    let mut table = toml::Table::new();
    for (key, value) in given.iter() {
        let Ok(key) = key.cast_exact::<PyString>() else {
            return Err(wrong_type(format_args!("a key of {at}"), "a str", &key));
        };
        let key = key.to_cow()?.into_owned();
        let value = value_of(&value, &format!("{at}.{key}"), depth + 1)?;
        table.insert(key, value);
    }
    Ok(table)
}

/// A value as TOML writes it: text, a flag, a whole number, a float, a list
/// or a dict of those, each of exactly that type, and nothing that merely
/// converts to one. `depth` is the level a list or a dict lies at where the
/// value is one: no manifest holds one deeper than its limit, and none is
/// followed further.
fn value_of(value: &Bound<'_, PyAny>, at: &str, depth: usize) -> PyResult<toml::Value> {
    if let Ok(flag) = value.cast_exact::<PyBool>() {
        return Ok(toml::Value::Boolean(flag.is_true()));
    }
    if value.is_exact_instance_of::<PyInt>() {
        return match value.extract::<i64>() {
            Ok(whole) => Ok(toml::Value::Integer(whole)),
            Err(error) if error.is_instance_of::<PyOverflowError>(value.py()) => Err(
                PyValueError::new_err(format!("{at} is an integer beyond the 64 bits TOML holds")),
            ),
            Err(error) => Err(error),
        };
    }
    if value.is_exact_instance_of::<PyFloat>() {
        return Ok(toml::Value::Float(value.extract::<f64>()?));
    }
    if let Ok(text) = value.cast_exact::<PyString>() {
        return Ok(toml::Value::String(text.to_cow()?.into_owned()));
    }
    let nested = value.is_exact_instance_of::<PyList>() || value.is_exact_instance_of::<PyDict>();
    if nested && depth > MAX_MANIFEST_DEPTH {
        return Err(PyValueError::new_err(format!(
            "{at} lies more than {MAX_MANIFEST_DEPTH} levels deep in the manifest"
        )));
    }
    if let Ok(items) = value.cast_exact::<PyList>() {
        let mut array = Vec::with_capacity(items.len());
        for (index, item) in items.iter().enumerate() {
            array.push(value_of(&item, &format!("{at}[{index}]"), depth + 1)?);
        }
        return Ok(toml::Value::Array(array));
    }
    if let Ok(given) = value.cast_exact::<PyDict>() {
        return Ok(toml::Value::Table(table(given, at, depth)?));
    }
    Err(wrong_type(at, PARAM, value))
}
