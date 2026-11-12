//! Workers: one process per job, started from this binary, that runs the
//! job it is handed and nothing else. A server keeps a pool of them started
//! ahead of its jobs, hands each job to one over the worker's standard
//! input, reads how the job ended from its standard output, and kills the
//! worker once the job is over, so that no job ever runs where another
//! did. A worker gets a scratch directory of its own, its working and
//! temporary directory, which is removed once it stops, and an environment
//! with nothing of the server's in it: no store, no database URL or
//! credentials, and no descriptor but its three pipes. The server reads
//! everything a job runs on and writes how it ended.
//!
//! The limits are per job: the CPU time and memory a worker puts on itself
//! as it starts, and the wall-clock time the server allows before it kills
//! the worker. What a worker enforces depends on its operating system, and
//! it reports what it did as a [`Posture`]:
//!
//! | Guard | Linux | macOS | Windows |
//! | --- | --- | --- | --- |
//! | CPU time | `RLIMIT_CPU` | `RLIMIT_CPU` | none |
//! | memory | `RLIMIT_AS` | none | none |
//! | network | seccomp refuses every socket | none | none |
//! | files | Landlock leaves only the scratch directory, where the kernel has it | none | none |
//! | wall clock | the server kills the worker | the server kills the worker | the server kills the worker |
//!
//! `docs/worker-isolation.md` says what each guard covers and what none
//! does yet.
//!
//! A backtest that ran to its end sends its server its summary and the
//! rows of its trades and its equity curve, never files: the server checks
//! every row as it reads it, and writes the result files itself. While a
//! job runs, its worker sends each line the job writes to its log and how
//! many bars it has run; the server keeps the newest lines within the
//! log's limits, and never holds more than a few of a worker's frames at
//! once, so a job that writes without end fills no memory of the server's.
//!
//! - `pool`: the workers a server keeps, and a job's run on one.
//! - `config`: what a pool starts with, and what a run answers.
//! - `job`: one job's run on a worker.
//! - `process`: a worker's process, and why it stopped.
//! - `frames`: what a server and a worker say to each other.
//! - `scratch`: the scratch directories.
//! - `child`: the worker's side.
//! - `backtest`: the engine's run over a job's candles, in the worker.
//! - `report`: what a job says to its server while it runs.
//! - `isolation`: the limits and sandbox a worker puts on itself.
//! - `synthetic`: workloads the tests run in place of a strategy.

mod backtest;
mod child;
mod config;
mod frames;
mod isolation;
mod job;
mod pool;
mod process;
mod report;
mod scratch;
mod synthetic;

pub use child::{WorkerError, run_worker};
pub use config::{Execution, PoolConfig, RunningWorker, WorkerLimits, WorkerMode, WorkerProgram};
pub use isolation::{Guard, Posture, protect_server};
pub use pool::WorkerPool;

/// The command that makes this binary a worker: `quantforge worker`.
pub const WORKER_COMMAND: &str = "worker";

/// The CPU time a job may use unless the server says otherwise: 10 minutes.
pub const DEFAULT_JOB_CPU_SECS: u64 = 600;

/// The memory a job may use unless the server says otherwise, in MiB of
/// address space: 4 GiB.
pub const DEFAULT_JOB_MEMORY_MIB: u64 = 4096;

/// How long a job may run unless the server says otherwise: 30 minutes.
pub const DEFAULT_JOB_WALL_SECS: u64 = 1800;
