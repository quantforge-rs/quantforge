//! Candle segments, `docs/segment-spec.md`, tested from outside the crate:
//! - the shared cases every implementation of the spec passes, and every
//!   payload and stored form a reader refuses;
//! - the spec document, held to the code;
//! - data sync storing settled candles only, and packing the windows it
//!   closes.
//!
//! How a store packs, indexes and resolves segments as candles are written
//! is the shared storage conformance suite's (`tests/storage/conformance/`,
//! `docs/storage-conformance.md`), which every backend passes.

mod cases;
mod docs;
mod sync;
