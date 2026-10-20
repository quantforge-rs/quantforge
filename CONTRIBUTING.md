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

## Adding a CLI command

The module tree under `src/cli/` mirrors the command tree (see
[docs/architecture.md](docs/architecture.md)), so a new command is a new file
at its mirroring path: `quantforge <group> <name>` lives in
`src/cli/<group>/<name>.rs`, with dashes in the name turned into
underscores. `src/cli/data/validate.rs` is a small complete command to copy
from; `monitor` commands such as `src/cli/monitor/orders.rs` get the
connectors they need (and the store, if they need it) from
`monitor::dispatch` instead of `AppContext`. Commands reach the exchange only
through the `ports` connector traits; `tests/exchange_boundary.rs` fails if
any code outside `src/exchange/` names a concrete adapter.

1. Create the file. Start it with a module doc naming the command
   (`` //! `quantforge <group> <name>` — what it does ``), then add:
   - an args struct, `<Group><Name>Args`, deriving `Args` and `Debug`. Take
     shared flags from `src/cli/common.rs` with `#[command(flatten)]` instead
     of declaring them again: `SymbolArgs` (`--symbol`), `MarketArgs`
     (`--symbol`, `--interval`), `ConfirmArgs` (`--yes`), `PollArgs`
     (`--poll-secs`, `--max-loops`), and `StrategyArgs` (`--strategy-name`).
     Flattened flags appear in `--help` where their field sits, so field
     order is help order. The global flags (`--db`, `--binance-base-url`,
     `--log-level`, and the credentials) are applied once by
     `AppContext::init`; a handler uses `ctx.store`, `ctx.base_url`, or
     `ctx.exchange` (the exchange's connectors), and never declares them
     again.
   - a handler, `handle_<group>_<name>`, that parses and validates every
     argument at its top, before it reads the store or calls Binance, using
     `parse_market`, `parse_positive_decimal`, and
     `parse_non_negative_decimal`. Take money amounts as `String` flags and
     parse them into `Decimal`, never a float. Print results as `key: value`
     lines, and send diagnostics through `tracing`.
2. Gate what needs gating. A command that sends signed requests gets the
   signed connectors from `ctx.require_signed("<group> <name> requires
   Binance credentials")`, which fails with a remedy naming both credential
   mechanisms; `monitor` commands skip this, because `run()` checks
   credentials once for the whole group. A command that changes exchange
   state flattens `ConfirmArgs` and, without `--yes`, prints what it would do
   (or that nothing was sent) and exits 0.
3. Register it in `src/cli/<group>/mod.rs`: declare the module, add a
   variant to the group's command enum (its `///` doc comment becomes the
   `--help` summary), and add an arm to `dispatch`. A new top-level command
   lives in `src/cli/<name>/mod.rs` and also needs `mod <name>;`, a `Command`
   variant, and a `run()` arm in `src/cli/mod.rs`. A group's `mod.rs` holds
   its enum and `dispatch`; a leaf like `backtest` holds its args and
   `handle_<name>`.
4. Update what CI checks:
   - add the command to `COMMANDS` in `tests/e2e/snapshot.rs`, then run
     `UPDATE_SNAPSHOTS=1 cargo test --test e2e snapshot::` and review the
     diff: the new `tests/snapshots/<group>-<name>.txt` (dashes kept) and
     `<group>.txt`, which now lists it;
   - add each new flag to the matrix in `docs/configuration.md`, and the
     command to the scope of each shared flag it takes;
     `configuration_doc_matches_every_flag_env_var_default_and_scope` names
     every setting that is missing from the doc or stale in it;
   - add the file to the tree in `docs/architecture.md`, with a note that
     starts with `quantforge <group> <name>:`;
   - add tests: argument validation and error text in `tests/cli.rs`, with
     `--binance-base-url http://127.0.0.1:9/` so a validation-ordering bug
     fails instead of reaching Binance, and exchange flows in
     `tests/e2e/offline.rs` against the mock server;
   - add a `CHANGELOG.md` entry.

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
