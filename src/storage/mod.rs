//! Persistence: one adapter namespace per backend, the segment format every
//! backend packs its candles into, and how a host picks a backend and opens
//! it: [`settings`] resolves the backend and its settings, [`factory`]
//! opens the store they describe as an [`AnyStore`]. SQLite and PostgreSQL,
//! the latter in `postgres` and built with the `postgres` feature, have
//! every store. `common`, `runs`, `jobs` and `refresh` hold what both
//! backends share, and [`retry_conflicts`] is how a caller runs a
//! transaction again.

mod any;
mod common;
pub mod factory;
mod jobs;
#[cfg(feature = "postgres")]
pub mod postgres;
#[cfg(test)]
mod races;
mod refresh;
mod retry;
mod runs;
pub mod segments;
pub mod settings;
pub mod sqlite;

pub use any::AnyStore;
pub use retry::retry_conflicts;
pub use sqlite::SqliteStore;
