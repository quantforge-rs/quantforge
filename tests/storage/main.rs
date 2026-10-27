//! The storage contract of `docs/storage-schema.md`, tested from outside
//! the crate:
//! - [`contract`]: the checks every backend passes, written against the
//!   ports so the PostgreSQL store runs the same module — the shared
//!   identity cases of `tests/fixtures/storage_identity_cases.json`, the
//!   isolation of product lines, environments and accounts, decimal scale,
//!   and the rule that a legacy run is never driven again silently;
//! - [`sqlite`]: those checks on the SQLite store;
//! - [`migration`]: the schema chain from every schema an earlier build
//!   wrote, loaded from `tests/fixtures/sqlite/`, with every row carried
//!   forward unchanged, and the controls that prove a failed step leaves
//!   the file where it was;
//! - [`docs`]: the document held to the code.

mod cases;
mod contract;
mod docs;
mod migration;
mod sqlite;
