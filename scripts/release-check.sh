#!/usr/bin/env bash
# What CI checks (docs/ci.md). With the variables of
# `scripts/postgres-test-servers.sh start` exported, the tests run on
# PostgreSQL too; the MSRV lines need `rustup toolchain install 1.85`.
set -euo pipefail

"$(dirname "$0")/file-size-check.sh"
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo clippy --all-targets --no-default-features -- -D warnings
cargo test --all-features --locked
cargo test --no-default-features --locked
cargo +1.85 check --locked --all-targets
cargo +1.85 check --locked --all-targets --no-default-features
cargo package --list >/dev/null
# a target directory of its own: the dry run's verify build would otherwise
# leave target/debug's binary pointing at the packaged copy of the sources
CARGO_TARGET_DIR=target/publish cargo publish --dry-run --locked
