//! The storage contract of `docs/storage-schema.md` and
//! `docs/storage-conformance.md`, tested from outside the crate:
//! - [`conformance`]: the shared suite every backend passes, written
//!   against the ports, with every answer taken from the fixtures;
//! - [`sqlite`]: the suite on the SQLite store;
//! - [`any_store`]: the suite again through `AnyStore`, opened by the
//!   built-in store factory, with the segment and store-control ports
//!   compared against the SQLite store on the same file;
//! - `postgres`: an isolated database per fixture on the server
//!   `QUANTFORGE_TEST_POSTGRES_URL` names, ready for the PostgreSQL stores;
//! - [`migration`]: the schema chain from every schema an earlier build
//!   wrote, loaded from `tests/fixtures/sqlite/`, with every row carried
//!   forward unchanged, and the controls that prove a failed step leaves
//!   the file where it was;
//! - [`docs`]: the documents held to the code and the fixtures.

mod any_store;
mod conformance;
mod docs;
mod migration;
#[cfg(feature = "postgres")]
mod postgres;
mod sqlite;
