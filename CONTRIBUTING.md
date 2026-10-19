# Contributing

Thanks for contributing to QuantForge.

## Development principles

- keep the CLI deterministic and scriptable
- avoid `unwrap` in production paths
- prefer explicit data validation over silent coercion
- document public APIs and CLI changes
- add tests for every bug fix and user-facing behavior change

## Tooling

QuantForge currently targets Rust 1.85 or newer.

## Before you open a pull request

Run:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features --locked
./scripts/release-check.sh
```

## Commit style

Small, reviewable commits are preferred.

Examples:

- `feat(cli): add download resume mode`
- `fix(storage): commit sqlite transaction after statement drop`
- `docs(readme): clarify determinism contract`

## Running the e2e tiers

Plain `cargo test` runs the offline tier (mock exchange, zero network).
Testnet-tier tests run only when `QF_BINANCE_API_KEY` and
`QF_BINANCE_API_SECRET` (Binance Spot *testnet* keys) are exported;
otherwise they skip with a `SKIP (testnet tier)` marker on stderr. Run one
tier alone with `cargo test --test e2e offline::` or
`cargo test --test e2e testnet::`.

## CLI snapshots

The `--help` output of every command is locked by committed snapshots in
`tests/snapshots/`, alongside a clap self-check and a test that the module
tree mirrors the command tree. A renamed flag, a changed default, or a
reordered argument fails CI. After an intentional change to the CLI
surface, refresh the snapshots and review the diff:

```bash
UPDATE_SNAPSHOTS=1 cargo test --test e2e snapshot::
```

## File size budget

Each `.rs` file under `src/` has a line budget: more than 400 lines prints a
warning, and more than 600 lines fails CI and `./scripts/release-check.sh`.
Lines are counted raw, so comments, blank lines, and an in-file
`#[cfg(test)] mod tests` all count; files under `tests/` are not checked. Run
the check on its own with:

```bash
./scripts/file-size-check.sh
```

To bring a file back under budget, split it along a responsibility boundary,
the way `src/engine/live/` separates the runner, order execution, and run
state. A large test module can move to its own file: keep
`#[cfg(test)] mod tests;` in the module and move the body to `tests.rs` beside
a `mod.rs`, or to `<name>/tests.rs` beside a `<name>.rs` (for example
`src/engine/live/execution/tests.rs`).

A file that cannot reasonably be split yet can get a higher ceiling in
`scripts/file-size-allowlist.txt`: one `<path> <ceiling> <justification>`
entry per line, with the ceiling above 600. The file still fails above its
ceiling, and the entry itself fails once the file is back at 600 lines or
fewer or no longer exists, so remove it in the same change as the split.

## Security and secrets

Do not commit API keys, secrets, `.env` files, or production credentials.

Every pull request is scanned for leaked secrets with gitleaks. To run the same
scan locally over the committed history:

```bash
gitleaks git .
```

The `git` subcommand scans commits only, so untracked files such as your local
`.env` are never read.
