//! End-to-end CLI tests, tiered by external dependencies.
//!
//! - `offline`: driven against the scriptable mock Binance server in
//!   [`mock_binance`]; always runs, zero network
//! - `testnet`: real Binance Spot testnet; skips with a stderr marker
//!   unless `QF_API_KEY`/`QF_API_SECRET` are exported
//!
//! [`precedence`] asserts where each setting with a variable comes from:
//! flag, then `QF_*` variable, then default. No network, always runs.
//!
//! [`storage`] selects the storage backend by flag, variable and default,
//! and proves PostgreSQL is never replaced by SQLite and its password never
//! printed. No network, always runs.
//!
//! [`backends`] runs every command that reads or writes the store on each
//! backend, as [`offline`] does with its own, against the mock exchange: on
//! SQLite always, and on a real PostgreSQL when
//! `QUANTFORGE_TEST_POSTGRES_URL` names a server.
//!
//! [`serve`] runs `serve run` as a process on a loopback port: its routes,
//! and how it stops on a signal. Loopback only, always runs; the signal
//! tests run on unix.
//!
//! [`migrate`] copies a SQLite store into PostgreSQL with `data migrate`,
//! held to the acceptance criteria and to the transfer fixture
//! ([`transfer_fixture`], shared with the storage tests): its refusals
//! always, everything else on a real PostgreSQL when
//! `QUANTFORGE_TEST_POSTGRES_URL` names a server.
//!
//! [`snapshot`] locks the CLI surface itself: committed `--help`
//! snapshots plus a check that the module tree mirrors the command tree.
//! [`layout`] keeps the annotated source tree in `docs/architecture.md` in
//! step with `src/`, each command's module labeled with its command.
//!
//! Shared infrastructure lives in [`harness`]: the env-isolated runners
//! (`assert_cmd`, and `std::process` for a command that keeps running),
//! per-test temp databases, `key: value` output parsing, and the tier gate.
//!
//! Run one tier alone: `cargo test --test e2e offline::` or
//! `cargo test --test e2e testnet::`.

mod backends;
mod harness;
mod layout;
mod migrate;
mod mock_binance;
mod offline;
mod precedence;
mod serve;
#[cfg(feature = "postgres")]
#[allow(dead_code, reason = "the storage tests use the rest of it")]
#[path = "../storage/postgres/server.rs"]
mod server;
mod snapshot;
mod storage;
mod testnet;
#[allow(dead_code, reason = "the storage tests use the rest of it")]
#[path = "../storage/transfer/fixture.rs"]
mod transfer_fixture;
