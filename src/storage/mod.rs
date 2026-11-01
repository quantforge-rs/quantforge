//! Persistence: one adapter namespace per backend, the segment format every
//! backend packs its candles into, and how a host picks a backend and opens
//! it: [`settings`] resolves the backend and its settings, [`factory`]
//! opens the store they describe as an [`AnyStore`]. SQLite is the only
//! backend with stores today; PostgreSQL's connection bridge is in
//! `postgres`, built with the `postgres` feature.

mod any;
pub mod factory;
#[cfg(feature = "postgres")]
pub mod postgres;
pub mod segments;
pub mod settings;
pub mod sqlite;

pub use any::AnyStore;
pub use sqlite::SqliteStore;
