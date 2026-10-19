//! End-to-end CLI tests, tiered by external dependencies.
//!
//! - `offline`: driven against the scriptable mock Binance server in
//!   [`mock_binance`]; always runs, zero network
//! - `testnet`: real Binance Spot testnet; skips with a stderr marker
//!   unless `QF_BINANCE_API_KEY`/`QF_BINANCE_API_SECRET` are exported
//!
//! [`precedence`] asserts where each global setting comes from: flag, then
//! `QF_*` variable, then default. No network, always runs.
//!
//! [`snapshot`] locks the CLI surface itself: committed `--help`
//! snapshots plus a check that the module tree mirrors the command tree.
//! [`layout`] keeps the annotated source tree in `docs/architecture.md` in
//! step with `src/`, each command's module labeled with its command.
//!
//! Shared infrastructure lives in [`harness`]: the env-isolated
//! `assert_cmd` runner, per-test temp databases, `key: value` output
//! parsing, and the tier gate.
//!
//! Run one tier alone: `cargo test --test e2e offline::` or
//! `cargo test --test e2e testnet::`.

mod harness;
mod layout;
mod mock_binance;
mod offline;
mod precedence;
mod snapshot;
mod testnet;
