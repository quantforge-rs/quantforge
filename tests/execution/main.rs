//! The execution store of ADR 0002 §15, from outside the crate
//! (`docs/execution-store.md`): the inbox, the ledger, the outbox and the
//! leases, on SQLite and, where `QUANTFORGE_TEST_POSTGRES_URL` names a
//! server, on PostgreSQL.
//!
//! - `support`: a store of each backend that can be opened again, as a
//!   process starting on it would, and a raw view of its tables.
//! - `fixtures`: two profiles on two venues that share everything a
//!   signal names, with their signals, intents and orders.
//! - `venue`: a venue that answers queries and takes no order.
//! - `smoke`: one signal from its delivery to its first fill, unit by
//!   unit.
//! - `admission`: deliveries through the ledger: recorded once, sent again
//!   and in conflict, judged in place, and what the store's records refuse.
//! - `orders`: an attempt recorded before it is sent, the venue's evidence,
//!   and a restart that asks the venue and sends nothing.
//! - `between`: what another writer does between admission's look and its
//!   write, and the second look an entry gets before its order is recorded.
//! - `workers`: two workers on one intent, a lease that was taken, and a
//!   store that is not active.
//! - `isolation`: the same alert on two venues.
//! - `crash`: a writer that dies right after each step of a signal's
//!   life, and one killed at no step in particular, with what each left
//!   resumed.
//! - `docs`: `docs/execution-store.md` held to the code and to the shared
//!   cases.

mod admission;
mod between;
mod crash;
mod docs;
mod fixtures;
mod isolation;
mod orders;
mod smoke;
mod support;
mod venue;
mod workers;

#[cfg(feature = "postgres")]
#[allow(dead_code, reason = "the storage tests use the rest of it")]
#[path = "../storage/postgres/server.rs"]
mod server_helpers;
