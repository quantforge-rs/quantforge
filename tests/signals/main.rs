//! The TradingView alert payload (ADR 0002 §5, §9) held to the shared
//! cases of `tests/fixtures/signal_cases.json`, which an independent Python
//! model writes (`spikes/futures-contract/signal_cases.py`): valid and
//! invalid payloads for both venues, both families and both sides, the
//! alert templates, what each signal resolves to for the profile its URL
//! selects, which exit closes which position, which delivery is a repeat
//! and which a conflict, and the receipts.
//!
//! - `cases`: the fixture, and a product line's profile as a policy.
//! - `payloads`: every body read by the protocol, held to the fixture's
//!   answer, and the body the server makes of it.
//! - `templates`: the alert templates, rendered as TradingView would.
//! - `receipts`: the receipts the format takes and refuses.
//! - `resolution`: every signal resolved on the pure model, exits against
//!   entries, deliveries one after another, and an entry against the side
//!   held.
//! - `service`: every signal resolved through the signal service over a
//!   store of each backend, SQLite and, where
//!   `QUANTFORGE_TEST_POSTGRES_URL` names a server, PostgreSQL.
//! - `schema`: `docs/signal-schema.json` held to the format.
//! - `docs`: `docs/signal-payload.md` held to the code and to the cases.

mod cases;
mod docs;
mod payloads;
mod receipts;
mod resolution;
mod schema;
mod service;
mod templates;

/// The stores of each backend, as the policy tests open them.
#[allow(dead_code, reason = "the policy tests use the rest of it")]
#[path = "../policy/support.rs"]
mod support;

#[cfg(feature = "postgres")]
#[allow(dead_code, reason = "the storage tests use the rest of it")]
#[path = "../storage/postgres/server.rs"]
mod server_helpers;
