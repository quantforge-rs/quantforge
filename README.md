# QuantForge

[![ci](https://github.com/quantforge-rs/quantforge/actions/workflows/ci.yml/badge.svg)](https://github.com/quantforge-rs/quantforge/actions/workflows/ci.yml)
[![codeql](https://github.com/quantforge-rs/quantforge/actions/workflows/codeql.yml/badge.svg?branch=main)](https://github.com/quantforge-rs/quantforge/actions/workflows/codeql.yml)
[![gitleaks](https://github.com/quantforge-rs/quantforge/actions/workflows/gitleaks.yml/badge.svg?branch=main)](https://github.com/quantforge-rs/quantforge/actions/workflows/gitleaks.yml)

QuantForge is a CLI-first trading systems framework in Rust for deterministic research,
SQLite-backed market data, and controlled strategy execution.

Version `0.2.0` keeps the repository as a **single Cargo package**, following the
`0.1.0` contributor experience, while adding the first live-trading and monitoring
path for **Binance Spot**.

There is **no UI**. The CLI is the product.

## What changed in 0.2.0

- single-crate repository layout with internal Rust modules
- `data` command group for historical and incremental candle ingestion into SQLite
- `trade run` command for a polling strategy bot that reads closed bars from SQLite,
  evaluates a strategy, and can place or close spot market orders
- `trade close` command for operator-driven position exits
- `monitor` command group for balances, open orders, recent trades, manual cancel,
  and manual close
- bot run state and order or trade journal persisted in SQLite

QuantForge is engineering software. It is **not** investment advice, does not
recommend trades, and does not promise profitability.

## Why keep a single crate?

This repository stays as a single Cargo package in `0.2.0` so versioning,
publishing, installation, and contributor onboarding remain simple. The one
planned exception is `protocol`, which uses nothing else in the crate so
that it can become the `quantforge-protocol` crate once it is stable.

The public surface is layered as Rust modules, innermost first:

- `model` - normalized market types, validation, the columnar candle batch, the exchange-agnostic requests, timestamp and decimal helpers
- `ports` - the exchange connector and storage traits every adapter implements, and the capability descriptor that says what an exchange supports
- `exchange` - the connector registry, which names each exchange (`--exchange binance`) and builds its connectors, and one adapter namespace per exchange; `binance` is the only one today, and nothing outside `exchange` names it. `exchange::conformance` writes down what every adapter must do as checks, and the [feature matrix](https://github.com/quantforge-rs/quantforge/blob/main/docs/exchange-feature-matrix.md) shows each exchange's results
- `storage` - persistence adapters, one namespace per backend, with `sqlite` the only one; and `segments`, the [segment format](https://github.com/quantforge-rs/quantforge/blob/main/docs/segment-spec.md) every backend packs a market's closed candles into, window by window, as immutable segments named by their hash
- `sdk` - the strategy trait and its context, indicator math, and built-in strategies
- `engine` - the drivers: `engine::backtest`, `engine::data_sync`, and `engine::live`
- `protocol` - stands apart from the layers above: the versioned wire types between QuantForge instances, for remote backtest jobs, candle segments, and the live bar stream, and the [strategy bundle](https://github.com/quantforge-rs/quantforge/blob/main/docs/bundle-spec.md) a remote backtest runs; its rustdoc is the wire specification
- `server` - the HTTP server `serve run` hosts for other QuantForge instances, speaking `protocol`: its routes, and a shutdown that finishes the requests in progress

The `quantforge` binary adds `cli`, a module tree that mirrors the command
tree: `quantforge monitor cancel-order` lives in
`src/cli/monitor/cancel_order.rs`. The
[architecture overview](https://github.com/quantforge-rs/quantforge/blob/main/docs/architecture.md)
annotates every source file and maps each command to its file.

## Scope and guardrails

- exchange in 0.2.0: **Binance Spot**
- storage in 0.2.0: **SQLite**
- strategy execution model: closed-bar, event-driven
- built-in strategy in 0.2.0: SMA crossover example
- default execution mode: **dry-run**
- live order placement requires explicit `--mode live`, a confirmation (`--yes`
  or an interactive prompt), and the exchange's API credentials (flags or
  environment variables)

## Dry-run vs live

`trade run` has two execution modes selected with `--mode`; the default is
**dry-run**.

- **dry-run** evaluates the strategy on real synced candles, but the engine is
  constructed without a trading venue, so no order can be sent at all. It needs
  no credentials and no confirmation.
- **live** places real spot market orders. It requires `--mode live`, API
  credentials (see Credentials below), and an explicit confirmation:
  - on an interactive terminal, the CLI shows the strategy, market, and
    endpoint, then asks you to type `yes`;
  - with `--yes`, the prompt is skipped (for scripts and supervisors);
  - non-interactively without `--yes`, the CLI prints a preview of the would-be
    run and exits with status 0 without trading.

Automation must pass `--yes` and should treat output without a `run_id:` line
as "the bot did not run". When the endpoint is a production Binance host, the
confirmation and logs mark it with `(PRODUCTION)`; this is a guardrail against
operator mistakes, not a security boundary — prefer a dedicated `--db` and
`https://testnet.binance.vision/` while testing.

## Configuration

Every setting is a command-line flag. The global ones — `--db`,
`--exchange`, `--base-url`, `--log-level`, `--api-key`, and `--api-secret` —
fall back to `QF_DB`, `QF_EXCHANGE`, `QF_BASE_URL`, `QF_LOG`, `QF_API_KEY`,
and `QF_API_SECRET`, and a flag always wins over its variable. So do
`serve run`'s `--listen` and `--segment-cache-dir`, with `QF_LISTEN` and
`QF_SEGMENT_CACHE_DIR`. `--exchange` names the exchange (`binance`, the
default and so far the only one), and an omitted `--base-url` is that
exchange's production endpoint. The
[configuration matrix](https://github.com/quantforge-rs/quantforge/blob/main/docs/configuration.md)
lists every flag with its environment variable, default, and scope, and the
settings renamed from their earlier `--binance-*` and `QF_BINANCE_*` names.

## Credentials

Signed endpoints — every `monitor` command, `trade close`, and
`trade run --mode live` — need the exchange's API key and secret, passed as
flags or as environment variables:

| Credential | Flag | Environment variable |
| --- | --- | --- |
| API key | `--api-key` | `QF_API_KEY` |
| API secret | `--api-secret` | `QF_API_SECRET` |

A flag wins over its variable, one credential at a time: with both variables
exported, `--api-secret` replaces only the secret. Supply both
credentials or neither — a lone or empty value counts as missing, and the
command stops with an error that names both mechanisms.

Use Binance Spot testnet first: create testnet keys at
<https://testnet.binance.vision/> and point QuantForge there with
`--base-url https://testnet.binance.vision/` or `QF_BASE_URL`.

Keep each secret in a file only you can read (`chmod 600`) and load it with
command substitution. The environment form, set once per shell, is the
recommended one:

```bash
export QF_API_KEY="$(cat ~/.config/quantforge/testnet-api-key)"
export QF_API_SECRET="$(cat ~/.config/quantforge/testnet-api-secret)"
export QF_BASE_URL="https://testnet.binance.vision/"
```

The flag form covers a single command with no variable set, for example
`--api-secret "$(cat ~/.config/quantforge/testnet-api-secret)"`; the
quickstart below includes a fully flag-driven `monitor status`.

Security notes:

- a secret typed literally — as a flag or in an `export` — is saved in your
  shell history; with command substitution, history records only `$(cat ...)`
- command-line arguments appear in process listings (`ps`) while QuantForge
  runs, and environment variables are not shown there by default, so prefer
  the variables, especially for long-running commands such as `trade run` and
  `monitor watch`; on an interactive terminal QuantForge logs a warning when a
  credential arrives as a flag
- if the file is missing, `"$(cat ...)"` expands to an empty value, which
  counts as missing — an empty flag never falls back to its variable — so the
  command fails instead of quietly using another key
- a value that loses its flag is echoed back in the usage error
  (`unexpected argument '...'` or `unrecognized subcommand '...'`), on screen
  and in any captured log

## Quickstart

Sync candles into SQLite:

```bash
cargo run -- \
  --db data/market.sqlite \
  data sync \
  --symbol BTCUSDT \
  --interval 1m
```

Boundary semantics: omitting `--start` begins at the current time (no
historical backfill — pass `--start` to backfill), and omitting `--end` keeps
the sync running until interrupted. A bounded sync reports the covered range
in its summary and warns when the exchange returned no candles for part of
the requested window.

Only settled candles are stored: a candle is written once a second has
passed since its close, and the one still forming is fetched again on a later
pass, so every stored candle is final. After each pass, `data sync` packs
each closed window, such as one UTC day of `1m` candles, into an immutable
segment for the server to serve. It packs a window again only when its
candles change, and its summary counts the windows it packed as
`segments_built`.

Validate the stored series:

```bash
cargo run -- \
  --db data/market.sqlite \
  data validate \
  --symbol BTCUSDT \
  --interval 1m
```

Backtest the SMA crossover example. The strategy needs more than `--slow`
candles (50 here) before it can trade, so backfill with `--start` first — a
sync that starts at the current time adds only one `1m` candle per minute:

```bash
cargo run -- \
  --db data/market.sqlite \
  backtest \
  --symbol BTCUSDT \
  --interval 1m \
  --fast 20 \
  --slow 50 \
  --cash 10000 \
  --fee-bps 10
```

Run the strategy against live-updating database candles without sending real orders:

```bash
cargo run -- \
  --db data/market.sqlite \
  trade run \
  --symbol BTCUSDT \
  --interval 1m \
  --fast 20 \
  --slow 50 \
  --quote-order-qty 100 \
  --mode dry-run \
  --poll-secs 5
```

Run against Binance Spot testnet with live order placement:

```bash
export QF_BASE_URL="https://testnet.binance.vision/"

cargo run -- \
  --db data/market.sqlite \
  trade run \
  --symbol BTCUSDT \
  --interval 1m \
  --fast 20 \
  --slow 50 \
  --quote-order-qty 100 \
  --mode live \
  --yes \
  --bootstrap-enter \
  --poll-secs 5
```

Inspect balances, open orders, and recent trades:

```bash
cargo run -- \
  --db data/market.sqlite \
  monitor status \
  --symbol BTCUSDT
```

The same report, fully flag-driven, with no `QF_*` variable set:

```bash
cargo run -- \
  --db data/market.sqlite \
  --base-url https://testnet.binance.vision/ \
  --api-key "$(cat ~/.config/quantforge/testnet-api-key)" \
  --api-secret "$(cat ~/.config/quantforge/testnet-api-secret)" \
  monitor status \
  --symbol BTCUSDT
```

Cancel an order manually:

```bash
cargo run -- \
  --db data/market.sqlite \
  monitor cancel-order \
  --symbol BTCUSDT \
  --order-id 123456789 \
  --yes
```

Close the bot-managed position manually:

```bash
cargo run -- \
  --db data/market.sqlite \
  trade close \
  --symbol BTCUSDT \
  --yes
```

Run the server other QuantForge instances connect to. It prints the address
it listens on and answers `GET /healthz` and `GET /version` there until
Ctrl-C or SIGTERM; then it stops accepting connections and finishes the
requests in progress, unless a second signal tells it not to wait:

```bash
cargo run -- \
  --db data/market.sqlite \
  serve run \
  --listen 127.0.0.1:9600
curl http://127.0.0.1:9600/version
```

## Determinism contract

- timestamps are stored as UTC epoch milliseconds
- prices and volumes use decimal arithmetic, not floating-point math
- strategies evaluate only closed bars
- backtests execute intent on the **next bar open**
- live trading polls closed bars and records run state into SQLite

## Local development

Run the full quality gate before opening a PR:

```bash
cargo generate-lockfile
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
./scripts/release-check.sh
```

`cargo test` also runs the exchange conformance suite with no network,
replaying recorded testnet traffic for most checks and hand-written
responses for the fault checks no testnet produces on demand. A scheduled
workflow runs every check except those fault checks live against each
testnet; the
[feature matrix](https://github.com/quantforge-rs/quantforge/blob/main/docs/exchange-feature-matrix.md)
marks which is which.
[CONTRIBUTING.md](https://github.com/quantforge-rs/quantforge/blob/main/CONTRIBUTING.md#the-conformance-suite)
explains the live modes and how to re-record.

## Roadmap

- additional exchanges such as Bybit
- restart reconciliation between local bot state and exchange state
- richer indicators and more built-in strategies
- PostgreSQL as a second storage backend, with SQLite staying the default; how the storage ports reach it is decided in [ADR 0001](https://github.com/quantforge-rs/quantforge/blob/main/docs/adr/0001-postgres-client-strategy.md)
- research-oriented parameter sweeps and snapshot reports

## License

Licensed under MIT.
