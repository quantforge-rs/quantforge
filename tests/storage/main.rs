//! The storage contract of `docs/storage-schema.md` and
//! `docs/storage-conformance.md`, tested from outside the crate:
//! - [`conformance`]: the shared suite every backend passes, written
//!   against the ports, with every answer taken from the fixtures;
//! - [`sqlite`]: the suite on the SQLite store;
//! - [`any_store`]: the suite again through `AnyStore`, opened by the
//!   built-in store factory, with the segment and store-control ports
//!   compared against the SQLite store on the same file;
//! - `postgres`: the suite on the PostgreSQL stores, directly and through
//!   `AnyStore`, an isolated database per fixture on the server
//!   `QUANTFORGE_TEST_POSTGRES_URL` names, with the schema, the candle
//!   store's writes against the segment protocol, and the query plans;
//! - [`crash`]: acknowledged writes outliving a killed process, and on
//!   demand a crashed server;
//! - [`migration`]: the schema chain from every schema an earlier build
//!   wrote, loaded from `tests/fixtures/sqlite/`, with every row carried
//!   forward unchanged, and the controls that prove a failed step leaves
//!   the file where it was;
//! - [`migration_execution`]: the step that gives a store its execution
//!   tables, from a store the build before it wrote, with every profile
//!   and every revision of its policy read back word for word;
//! - [`migration_provenance`]: the step that gives a store its jobs'
//!   provenance, from a store the build before it wrote, with every job
//!   and execution row kept and no provenance made up for an earlier job;
//! - [`docs`]: the documents held to the code and the fixtures;
//! - [`boundary`]: nothing opens a store but the factory, and nothing asks
//!   the factory but the CLI's context;
//! - [`reader`]: the reader a host opens to read candles and nothing else,
//!   on each backend: what it reads, what it says of a range that held
//!   nothing, and what it refuses and leaves as it was;
//! - [`transfer`]: a store copied into another and verified, from a
//!   read-only snapshot of SQLite into PostgreSQL, held to the checksums an
//!   independent oracle wrote;
//! - [`measure`]: a 100,000-candle batch on each backend, run on demand.

mod any_store;
mod boundary;
mod conformance;
mod crash;
mod docs;
mod measure;
mod migration;
mod migration_execution;
mod migration_provenance;
#[cfg(feature = "postgres")]
mod postgres;
mod reader;
mod sqlite;
mod transfer;
