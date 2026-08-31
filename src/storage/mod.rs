//! Persistence adapters. SQLite is the only backend today.

pub mod sqlite;

pub use sqlite::SqliteStore;
