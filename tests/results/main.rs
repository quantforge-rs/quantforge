//! A backtest job's results, `docs/result-spec.md`, tested from outside the
//! crate:
//! - the shared cases every implementation of the spec passes, whose files
//!   and hashes come from an independent Python implementation;
//! - the spec document, held to the code and the cases;
//! - an engine's result written as its three files and read back, every
//!   decimal digit for digit.
//!
//! How a server keeps and serves them is `tests/backtest_jobs.rs` and
//! `tests/e2e/jobs.rs`.

mod cases;
mod docs;
mod engine;
mod measure;
