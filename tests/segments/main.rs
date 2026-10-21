//! Candle segments, `docs/segment-spec.md`, tested from outside the crate:
//! - the shared cases every implementation of the spec passes, and every
//!   payload and stored form a reader refuses;
//! - the spec document, held to the code;
//! - the SQLite store packing, indexing, and resolving segments as candles
//!   are written;
//! - data sync storing settled candles only, and packing the windows it
//!   closes.

mod cases;
mod docs;
mod store;
mod sync;
