//! A spike for ADR 0001, `docs/adr/0001-postgres-client-strategy.md`: how
//! QuantForge's synchronous storage ports reach PostgreSQL through an async
//! driver, from every context that calls them, without a nested runtime,
//! without unbounded connections, and without blocking an async executor.
//! It is not part of the published crate; ALG-404 builds the real adapter.
//!
//! - [`runner`]: the bridge's own runtime, and how synchronous code waits on
//!   it.
//! - [`bridge`]: the connection pool on that runtime, with deadlines,
//!   cancellation, and error kinds.
//! - [`tls`]: rustls, and libpq's `sslmode`s mapped onto tokio-postgres.
//! - [`store`]: `quantforge::CandleStore` over the bridge.

pub mod bridge;
pub mod runner;
pub mod store;
pub mod tls;
