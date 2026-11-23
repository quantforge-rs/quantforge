//! What a pool is started with, and what a job's run on it answers: the
//! program its workers run, the limits on each job, what the workers run,
//! and each run's outcome.

use std::ffi::OsString;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{
    DEFAULT_JOB_CPU_SECS, DEFAULT_JOB_MEMORY_MIB, DEFAULT_JOB_WALL_SECS, JOB_PACKAGES,
    PYTHON_WORKER_MODULE, WORKER_COMMAND,
};
use crate::protocol::bundle::Runtime;
use crate::server::jobs::WorkerOutcome;

/// What a Python worker's interpreter is started with, before the module it
/// runs: UTF-8 mode, so that a bundle reads its files alike on every
/// machine; Python's traceback on a crash, which ends up in what the server
/// keeps of the worker's standard error; no bytecode written; and neither
/// the user's site-packages nor the working directory on its path.
const PYTHON_FLAGS: [&str; 7] = ["-X", "utf8", "-X", "faulthandler", "-B", "-s", "-P"];

/// The variables a Python worker gets beside its temporary directory: a
/// `str` hashes the same on every run, so that a set of them is gone
/// through in the same order, and NumPy's linear algebra runs on the job's
/// own thread, so that it starts no thread before the worker's sandbox and
/// adds up a sum in one order whatever the machine.
const PYTHON_VARIABLES: [(&str, &str); 4] = [
    ("PYTHONHASHSEED", "0"),
    ("OPENBLAS_NUM_THREADS", "1"),
    ("OMP_NUM_THREADS", "1"),
    ("MKL_NUM_THREADS", "1"),
];

/// The program a worker runs: this binary, as `quantforge worker`, or a
/// Python interpreter running the worker of the `quantforge_py` installed
/// beside it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkerProgram {
    /// The executable.
    pub path: PathBuf,
    /// Its arguments, which make it a worker.
    pub args: Vec<OsString>,
    /// The variables it is started with, beside its temporary directory,
    /// which every worker gets: its environment holds nothing else.
    pub env: Vec<(OsString, OsString)>,
}

impl WorkerProgram {
    /// This process's own binary, as `quantforge worker`.
    pub fn this_binary() -> io::Result<Self> {
        Ok(Self {
            path: std::env::current_exe()?,
            args: vec![OsString::from(WORKER_COMMAND)],
            env: Vec::new(),
        })
    }

    /// The Python interpreter `python`, running the worker of the
    /// `quantforge_py` installed in its environment: `python -m
    /// quantforge_py._worker`, with the flags and variables that make a job
    /// run alike on every machine. The path is made absolute, never
    /// resolved: a virtualenv's interpreter is a link that finds its
    /// environment by where the link is.
    pub fn python(python: &Path) -> io::Result<Self> {
        let mut args: Vec<OsString> = PYTHON_FLAGS.iter().map(OsString::from).collect();
        args.extend(["-m", PYTHON_WORKER_MODULE].map(OsString::from));
        let env = PYTHON_VARIABLES
            .iter()
            .map(|(name, value)| (OsString::from(name), OsString::from(value)))
            .chain(system_root())
            .collect();
        Ok(Self {
            path: std::path::absolute(python)?,
            args,
            env,
        })
    }
}

/// On Windows, where its own libraries, which Python loads, find
/// themselves: the server's `SYSTEMROOT`, passed on. Nothing elsewhere.
fn system_root() -> Option<(OsString, OsString)> {
    #[cfg(windows)]
    {
        std::env::var_os("SYSTEMROOT").map(|root| (OsString::from("SYSTEMROOT"), root))
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// The limits on each job: its CPU time, its memory, and its wall-clock
/// time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorkerLimits {
    /// The CPU time a job may use, in seconds.
    pub cpu_secs: u64,
    /// The memory a job may use, in MiB of address space.
    pub memory_mib: u64,
    /// How long a job may run, in seconds, before the server stops it.
    pub wall_secs: u64,
}

impl Default for WorkerLimits {
    fn default() -> Self {
        Self {
            cpu_secs: DEFAULT_JOB_CPU_SECS,
            memory_mib: DEFAULT_JOB_MEMORY_MIB,
            wall_secs: DEFAULT_JOB_WALL_SECS,
        }
    }
}

/// `cpu 600 s, memory 4096 MiB, wall clock 1800 s`.
impl fmt::Display for WorkerLimits {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "cpu {} s, memory {} MiB, wall clock {} s",
            self.cpu_secs, self.memory_mib, self.wall_secs
        )
    }
}

/// What a server's workers run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkerMode {
    /// Nothing yet: the workers offer no runtime, so the server fails each
    /// job as it starts and never hands one over.
    NoRuntime,
    /// The synthetic workloads the tests try a worker's isolation and a
    /// job's results with, offering `runtime` so that the tests' bundles
    /// are admitted.
    /// `serve run` never starts its workers in this mode.
    Synthetic {
        /// The runtime the workers say they offer.
        runtime: Runtime,
    },
    /// Python bundles, run by workers that are a Python interpreter with
    /// `quantforge_py` installed ([`WorkerProgram::python`]). Each offers
    /// its own Python and engine, and those of `packages` its environment
    /// holds, at the version it holds: a bundle may import those and no
    /// other. This binary runs no Python, and offers no runtime in this
    /// mode.
    Python {
        /// The packages a bundle may import, by the names a manifest's
        /// `deps` gives them.
        packages: Vec<String>,
    },
}

impl WorkerMode {
    /// Python bundles, with the packages this build lets a bundle import,
    /// [`JOB_PACKAGES`].
    pub fn python() -> Self {
        Self::Python {
            packages: JOB_PACKAGES.map(String::from).to_vec(),
        }
    }

    /// The runtime this binary offers in this mode, as a worker.
    pub(super) fn runtime(&self) -> Option<Runtime> {
        match self {
            Self::NoRuntime | Self::Python { .. } => None,
            Self::Synthetic { runtime } => Some(runtime.clone()),
        }
    }
}

/// How a pool starts.
#[derive(Clone, Debug)]
pub struct PoolConfig {
    /// What its workers run.
    pub program: WorkerProgram,
    /// How many workers it keeps, as many as the server runs jobs at once;
    /// at least 1.
    pub size: usize,
    /// The limits on each job.
    pub limits: WorkerLimits,
    /// What its workers run.
    pub mode: WorkerMode,
    /// Where it makes its own scratch directory, which holds its workers':
    /// the system's temporary directory, say.
    pub scratch_parent: PathBuf,
}

/// A job's run on a worker.
#[derive(Debug)]
pub struct Execution {
    /// How it ended: a succeeded run with the result files it produced.
    pub outcome: WorkerOutcome,
    /// The process of the worker that ran it, if one did.
    pub pid: Option<u32>,
}

/// A job running on a worker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunningWorker {
    /// The job.
    pub job_id: String,
    /// The worker's process.
    pub pid: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_python_worker_runs_the_packages_worker_module_with_flags_and_variables_of_its_own() {
        let program = WorkerProgram::python(Path::new("venv/bin/python")).expect("a program");
        // Made absolute, and never resolved: a virtualenv's interpreter is a
        // link that finds its environment by where the link is.
        assert!(program.path.is_absolute());
        assert!(program.path.ends_with("venv/bin/python"));
        let args: Vec<&str> = program
            .args
            .iter()
            .map(|arg| arg.to_str().expect("text"))
            .collect();
        assert_eq!(
            args,
            [
                "-X",
                "utf8",
                "-X",
                "faulthandler",
                "-B",
                "-s",
                "-P",
                "-m",
                "quantforge_py._worker"
            ]
        );
        let env: Vec<(&str, &str)> = program
            .env
            .iter()
            .map(|(name, value)| (name.to_str().expect("text"), value.to_str().expect("text")))
            .filter(|(name, _)| *name != "SYSTEMROOT")
            .collect();
        assert_eq!(
            env,
            [
                ("PYTHONHASHSEED", "0"),
                ("OPENBLAS_NUM_THREADS", "1"),
                ("OMP_NUM_THREADS", "1"),
                ("MKL_NUM_THREADS", "1"),
            ]
        );
        assert_eq!(
            program.env.len(),
            if cfg!(windows) { 5 } else { 4 },
            "SYSTEMROOT on Windows alone"
        );
        // This binary's own worker gets nothing beside its scratch directory.
        assert!(
            WorkerProgram::this_binary()
                .expect("a program")
                .env
                .is_empty()
        );
    }

    #[test]
    fn the_python_mode_names_the_packages_a_bundle_may_import_and_this_binary_offers_it_nothing() {
        let mode = WorkerMode::python();
        assert_eq!(
            serde_json::to_string(&mode).expect("json"),
            r#"{"kind":"python","packages":["numpy","pandas"]}"#
        );
        assert_eq!(mode.runtime(), None);
        assert_eq!(
            serde_json::from_str::<WorkerMode>(r#"{"kind":"no_runtime"}"#).expect("a mode"),
            WorkerMode::NoRuntime
        );
    }
}
