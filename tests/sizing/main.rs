//! Risk sizing (ADR 0002 §6, §9) held to the shared cases of
//! `tests/fixtures/sizing_cases.json`, which an independent Python model
//! writes (`spikes/futures-contract/sizing.py`): every venue, family and
//! side of the parity matrix's `futures.sizing` cells, every refusal, every
//! limit met exactly and missed by one step, margin pools in USD, what
//! sizing cannot trust, and the risk a fill actually took.
//!
//! - `cases`: the fixture, and a case as a sizing request.
//! - `model`: every case on the pure model, and the fill reports.
//! - `rules`: what the cases leave out: each refusal's and fault's words,
//!   market-order rules that differ from the limit-order ones, a caller's
//!   own freshness limits, amounts beyond an exact decimal, and the rules'
//!   promises held over generated entries.
//! - `docs`: `docs/risk-sizing.md` held to the code and to the cases, and
//!   the cases ADR 0002 links held to the fixture.
//! - `service`: every case under a policy read back from SQLite and, where
//!   `QUANTFORGE_TEST_POSTGRES_URL` names a server, from PostgreSQL, and an
//!   admitted entry sized from its snapshot after the policy has changed.

mod cases;
mod docs;
mod model;
mod rules;
mod service;

/// The stores of each backend, as the policy tests open them.
#[allow(dead_code, reason = "the policy tests use the rest of it")]
#[path = "../policy/support.rs"]
mod support;

#[cfg(feature = "postgres")]
#[allow(dead_code, reason = "the storage tests use the rest of it")]
#[path = "../storage/postgres/server.rs"]
mod server_helpers;
