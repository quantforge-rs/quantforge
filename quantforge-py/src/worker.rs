//! The worker a QuantForge server runs Python bundles in: `python -m
//! quantforge_py._worker` (`python/quantforge_py/_worker.py`), which
//! `quantforge serve run --job-python` starts for each job. Its frames,
//! limits and sandbox are the server's own, compiled into this module from
//! the `quantforge` crate, so a server speaks to it as to any worker of its
//! build; its job runs in the shared runtime (`runtime.rs`).
//!
//! - **It offers what its environment holds:** its Python, the engine it
//!   was built with, and those of the packages the server allows that are
//!   installed, each at its version. NumPy among them is imported as the
//!   worker readies itself, before the sandbox and before any job.
//! - **What the job writes is the job's log:** a line printed is an `info`
//!   line, one written to standard error a `warning` line, and a failure's
//!   traceback `error` lines, after which the job fails as its request's
//!   fault.
//! - **Its frames go out on a descriptor of their own,** which
//!   `_worker.py` moved them to, written through Python's own `os.write`:
//!   what anything else writes to standard output goes where standard error
//!   goes. Each frame is written whole before the next begins, a job's
//!   lines and the run's progress in the order they are said, since all of
//!   it is said on the one thread that runs the job.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use pyo3::sync::PyOnceLock;
use pyo3::types::PyBytes;
use quantforge::protocol::ErrorCode;
use quantforge::protocol::bundle::{PackageName, PackageVersion, Runtime};
use quantforge::protocol::jobs::LogLevel;
use quantforge::server::{
    JobEnd, Prepared, Reporter, WorkerJob, WorkerMode, WorkerRuntime, run_backtest, run_worker_with,
};

use crate::runtime::{self, BundleSpec, Failure};

/// The private module the runtime's Python half is.
const MODULE: &str = "quantforge_py._runtime";

static WRITE: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static PREPARE: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static CAPTURE: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

/// Runs this process as a server's job worker, its frames read from
/// standard input and written to the descriptor `frames`, until it has said
/// how its one job ended, or its server goes away. Private: `_worker.py`
/// calls it, once the process holds no descriptor of its server's.
#[pyfunction]
#[pyo3(name = "_run_worker")]
pub(crate) fn run_worker(frames: i32) -> PyResult<()> {
    let mut runtime = PythonRuntime {
        frames,
        offered: BTreeMap::new(),
    };
    run_worker_with(io::stdin(), FramesOut { fd: frames }, &mut runtime)
        .map_err(|error| PyRuntimeError::new_err(error.to_string()))
}

/// What a Python worker runs its job in.
struct PythonRuntime {
    /// The descriptor its frames go out on.
    frames: i32,
    /// What it offers, once ready: each package a bundle may import, by its
    /// name, at the version installed.
    offered: BTreeMap<String, String>,
}

impl WorkerRuntime for PythonRuntime {
    fn prepare(&mut self, mode: &WorkerMode, _scratch: &Path) -> Result<Prepared, String> {
        // A server that asks for anything but Python bundles gets a worker
        // that offers no runtime, as this binary's own does.
        let WorkerMode::Python { packages } = mode else {
            return Ok(Prepared::default());
        };
        Python::attach(|py| self.ready(py, packages)).map_err(|error| error.to_string())
    }

    fn run(&mut self, job: &WorkerJob<'_>, reporter: &mut Reporter<'_>) -> JobEnd {
        Python::attach(|py| self.job(py, job, reporter))
    }
}

impl PythonRuntime {
    /// Readies the environment and says what it offers.
    fn ready(&mut self, py: Python<'_>, packages: &[String]) -> PyResult<Prepared> {
        let found = PREPARE.import(py, MODULE, "prepare")?.call1((packages,))?;
        let python: String = found.get_item("python")?.extract()?;
        let offered: BTreeMap<String, String> = found.get_item("packages")?.extract()?;
        let readable: Vec<PathBuf> = found.get_item("readable")?.extract()?;
        let mut packages = BTreeMap::new();
        for (name, version) in &offered {
            let pinned = PackageVersion::new(version.as_str()).map_err(|_| {
                PyRuntimeError::new_err(format!(
                    "{name} {version} is installed, and a manifest cannot pin that version: \
                     install a release of {name} in the environment jobs run in"
                ))
            })?;
            packages.insert(
                PackageName::new(name.as_str()).map_err(unreadable)?,
                BTreeSet::from([pinned]),
            );
        }
        let runtime = Runtime {
            engine: quantforge::ENGINE_VERSION.parse().map_err(unreadable)?,
            python: python.parse().map_err(unreadable)?,
            packages,
        };
        self.offered = offered;
        Ok(Prepared {
            runtime: Some(runtime),
            readable,
        })
    }

    /// Runs the job, what it writes taken into its log, and says how it
    /// ended.
    fn job(&self, py: Python<'_>, job: &WorkerJob<'_>, reporter: &mut Reporter<'_>) -> JobEnd {
        let root = job.bundle_dir();
        let streams = match capture(py, self.frames) {
            Ok(streams) => streams,
            Err(error) => {
                return JobEnd::failed(
                    ErrorCode::Internal,
                    format!("the worker could not take in what the job writes: {error}"),
                );
            }
        };
        let ran = self.ran(py, job, reporter, &root);
        // A last line without its end, before anything said after it.
        for stream in [&streams.0, &streams.1] {
            let _ = stream.call_method0("finish");
        }
        match ran {
            Ok(ended) => ended,
            Err(Failure { message, traceback }) => {
                for line in traceback.lines() {
                    reporter.line(LogLevel::Error, line);
                }
                JobEnd::failed(ErrorCode::InvalidRequest, message)
            }
        }
    }

    /// The bundle's strategy, built and run over the job's candles.
    fn ran(
        &self,
        py: Python<'_>,
        job: &WorkerJob<'_>,
        reporter: &mut Reporter<'_>,
        root: &Path,
    ) -> Result<JobEnd, Failure> {
        let bundle = BundleSpec {
            root,
            entry: job.entry(),
            module: job.module(),
            class: job.class(),
            params: job.params_json(),
            deps: job.deps(),
            offered: &self.offered,
        };
        let mut strategy = runtime::strategy(py, &bundle)?;
        let ended = run_backtest(job, &mut strategy, reporter);
        match strategy.failure() {
            Some(error) => Err(runtime::failed(py, &error, root)),
            None => Ok(ended),
        }
    }
}

/// What the runtime found that this build cannot say in its runtime.
fn unreadable(error: impl std::fmt::Display) -> PyErr {
    PyRuntimeError::new_err(format!(
        "the worker's environment cannot be offered: {error}"
    ))
}

/// Takes what the job writes to standard output and standard error into
/// its log, a line at a time: the two streams, to be finished as the job
/// ends.
fn capture(py: Python<'_>, frames: i32) -> PyResult<(Bound<'_, PyAny>, Bound<'_, PyAny>)> {
    let out = Bound::new(
        py,
        JobLines {
            frames,
            level: LogLevel::Info,
        },
    )?;
    let err = Bound::new(
        py,
        JobLines {
            frames,
            level: LogLevel::Warning,
        },
    )?;
    CAPTURE
        .import(py, MODULE, "capture")?
        .call1((out, err))?
        .extract()
}

/// Where a line the job writes goes: a `log` frame at one level, on the
/// worker's frames.
#[pyclass(frozen, module = "quantforge_py")]
struct JobLines {
    frames: i32,
    level: LogLevel,
}

#[pymethods]
impl JobLines {
    fn __call__(&self, line: &str) {
        Reporter::new(&mut FramesOut { fd: self.frames }).line(self.level, line);
    }
}

/// The worker's frames, written to the descriptor `_worker.py` moved them
/// to, through Python's own `os.write`, which works the same on every
/// platform and needs no unsafe code here.
#[derive(Clone, Copy, Debug)]
struct FramesOut {
    fd: i32,
}

impl Write for FramesOut {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        Python::attach(|py| {
            WRITE
                .import(py, "os", "write")?
                .call1((self.fd, PyBytes::new(py, bytes)))?
                .extract::<usize>()
        })
        .map_err(|error| io::Error::other(error.to_string()))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
