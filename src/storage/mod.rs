//! Persistence: one adapter namespace per backend, and the segment format
//! every backend packs its candles into. SQLite is the only backend today.

pub mod segments;
pub mod sqlite;

pub use sqlite::SqliteStore;
