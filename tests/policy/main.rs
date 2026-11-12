//! The policy contract of ADR 0002 §5 and §6, held to the shared cases of
//! `tests/fixtures/policy_cases.json`, which an independent Python model
//! writes (`spikes/futures-contract/policy_cases.py`): default inheritance,
//! per-asset overrides, reset to default, the refusals, what an admitted
//! entry carries, which stop it uses, and what a venue's instrument list
//! says about a policy. Every case runs on all four product lines.
//!
//! - `cases`: the fixture, and its changes as the model's.
//! - `model`: every case on the pure model, document by document.
//! - `service`: every case through the policy service on SQLite and, where
//!   `QUANTFORGE_TEST_POSTGRES_URL` names a server, on PostgreSQL, across a
//!   restart, with every revision read back.
//! - `races`: two changes made from one revision, on each backend.
//! - `intent`: the policy an admitted entry keeps, and what never reaches it.
//! - `docs`: `docs/automation-policy.md` held to the code and to the cases.

mod cases;
mod docs;
mod intent;
mod model;
mod races;
mod service;
mod support;

#[cfg(feature = "postgres")]
#[allow(dead_code, reason = "the storage tests use the rest of it")]
#[path = "../storage/postgres/server.rs"]
mod server_helpers;
