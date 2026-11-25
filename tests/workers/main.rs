//! Black-box tests for the job workers: this build's binary, started as
//! `quantforge worker` by a `WorkerPool` in the synthetic mode, whose
//! workloads stand in for a strategy. Each limit is run into and named, a
//! crash or a kill fails only its own job, scratch directories are kept
//! apart and removed, every job gets a fresh worker, and a worker sees
//! nothing of the store its server holds, with a SQLite parent and, where
//! `QUANTFORGE_TEST_POSTGRES_URL` names a server, a PostgreSQL one. A
//! backtest run in a worker answers the result files a local run writes,
//! and a server keeps and serves them from either store. What a job writes
//! while it runs reaches its log, within the log's limits however much it
//! writes, and a client follows it from either store.
//!
//! - `support`: the pool, bundles naming a workload, jobs, and reports.
//! - `limits`: CPU time, memory and wall-clock time, crashes, kills and
//!   cancels.
//! - `isolation`: scratch directories, the network, the environment and
//!   descriptors a worker gets, recycling, and the worker command itself.
//! - `hostile`: what a job that means harm tries, and is refused: a way
//!   out of its scratch directory, and the processes around it.
//! - `results`: a backtest in a worker, and the files its rows become.
//! - `logs`: what a job writes to its log, a line too long, and a flood.
//! - `docs`: the threat model, held to the tests it names and to what a
//!   worker started here enforces.
//! - `backends`: jobs through the queue to real workers on each backend.
//! - `provenance`: what a job records it ran on, and its rerun, on each
//!   backend, across both, and on a store copied or restored.

mod backends;
mod docs;
mod hostile;
mod isolation;
mod limits;
mod logs;
mod provenance;
mod results;
mod support;
