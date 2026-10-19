#!/usr/bin/env bash
set -euo pipefail

"$(dirname "$0")/file-size-check.sh"
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features --locked
cargo package --list >/dev/null
cargo publish --dry-run --locked
