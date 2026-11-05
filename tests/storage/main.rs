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
//! - [`docs`]: the documents held to the code and the fixtures;
//! - [`measure`]: a 100,000-candle batch on each backend, run on demand.

mod any_store;
mod conformance;
mod crash;
mod docs;
mod measure;
mod migration;
#[cfg(feature = "postgres")]
mod postgres;
mod sqlite;
