//! Black-box tests for the job workers: this build's binary, started as
//! `quantforge worker` by a `WorkerPool` in the synthetic mode, whose
//! workloads stand in for a strategy. Each limit is run into and named, a
//! crash or a kill fails only its own job, scratch directories are kept
//! apart and removed, every job gets a fresh worker, and a worker sees
//! nothing of the store its server holds, with a SQLite parent and, where
//! `QUANTFORGE_TEST_POSTGRES_URL` names a server, a PostgreSQL one. A
//! backtest run in a worker answers the result files a local run writes,
//! and a server keeps and serves them from either store.
//!
//! - `support`: the pool, bundles naming a workload, jobs, and reports.
//! - `limits`: CPU time, memory and wall-clock time, crashes, kills and
//!   cancels.
//! - `isolation`: scratch directories, the network, the environment and
//!   descriptors a worker gets, recycling, and the worker command itself.
//! - `results`: a backtest in a worker, and the files its rows become.
//! - `backends`: jobs through the queue to real workers on each backend.

mod backends;
mod isolation;
mod limits;
mod results;
mod support;
