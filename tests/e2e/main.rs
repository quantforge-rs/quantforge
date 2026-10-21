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
//! [`serve`] runs `serve run` as a process on a loopback port: its routes,
//! and how it stops on a signal. Loopback only, always runs; the signal
//! tests run on unix.
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

mod harness;
mod layout;
mod mock_binance;
mod offline;
mod precedence;
mod serve;
mod snapshot;
mod testnet;
