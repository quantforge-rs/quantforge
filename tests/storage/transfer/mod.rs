//! The transfer of `docs/storage-migration.md` from outside the crate, held
//! to `tests/fixtures/storage_transfer_cases.json`, whose every checksum an
//! independent Python oracle wrote:
//! - `sqlite`: the source, a read-only snapshot of a SQLite store: what it
//!   reads, that the file keeps every byte, that a writer cannot change what
//!   it reads, and what it refuses;
//! - `postgres`: the fixture copied into PostgreSQL and verified, on either
//!   collation; the copy read back through every port; the target's
//!   numbering after it; a copy cut off and finished; and the refusals;
//! - `execution`: the execution store's records in the copy: each read
//!   back as the source reads it and as the oracle wrote it, the inbox
//!   still telling a delivery it has seen, and no authority brought along.

#[cfg(feature = "postgres")]
mod execution;
pub mod fixture;
#[cfg(feature = "postgres")]
mod postgres;
mod sqlite;
