//! Persistence: one adapter namespace per backend, the segment format every
//! backend packs its candles into, and how a host picks a backend and opens
//! it: [`settings`] resolves the backend and its settings, [`factory`]
//! opens the store they describe as an [`AnyStore`]. SQLite has every
//! store; PostgreSQL, in `postgres` and built with the `postgres` feature,
//! has the candle store so far. `common` holds what both backends share.

mod any;
mod common;
pub mod factory;
#[cfg(feature = "postgres")]
pub mod postgres;
pub mod segments;
pub mod settings;
pub mod sqlite;

pub use any::AnyStore;
pub use sqlite::SqliteStore;
