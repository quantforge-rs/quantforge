//! What a pool is started with, and what a job's run on it answers: the
//! program its workers run, the limits on each job, what the workers run,
//! and each run's outcome with the results it produced.

use std::ffi::OsString;
use std::fmt;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::{DEFAULT_JOB_CPU_SECS, DEFAULT_JOB_MEMORY_MIB, DEFAULT_JOB_WALL_SECS, WORKER_COMMAND};
use crate::protocol::bundle::Runtime;
use crate::protocol::jobs::ResultRef;
use crate::server::jobs::WorkerOutcome;

/// The program a worker runs: this binary, as `quantforge worker`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkerProgram {
    /// The executable.
    pub path: PathBuf,
    /// Its arguments, which make it a worker.
    pub args: Vec<OsString>,
}

impl WorkerProgram {
    /// This process's own binary, as `quantforge worker`.
    pub fn this_binary() -> io::Result<Self> {
        Ok(Self {
            path: std::env::current_exe()?,
            args: vec![OsString::from(WORKER_COMMAND)],
        })
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
    /// The synthetic workloads the tests try a worker's isolation with,
    /// offering `runtime` so that the tests' bundles are admitted.
    /// `serve run` never starts its workers in this mode.
    Synthetic {
        /// The runtime the workers say they offer.
        runtime: Runtime,
    },
}

impl WorkerMode {
    pub(super) fn runtime(&self) -> Option<Runtime> {
        match self {
            Self::NoRuntime => None,
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
    /// How it ended.
    pub outcome: WorkerOutcome,
    /// What it produced: each result's reference and bytes. A server keeps
    /// the references; the results routes, which keep the bytes, come later.
    pub results: Vec<ResultFile>,
    /// The process of the worker that ran it, if one did.
    pub pid: Option<u32>,
}

/// One result: its reference and its bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResultFile {
    /// Its name, media type, hash and size.
    pub reference: ResultRef,
    /// Its bytes.
    pub bytes: Vec<u8>,
}

/// A job running on a worker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunningWorker {
    /// The job.
    pub job_id: String,
    /// The worker's process.
    pub pid: u32,
}
