# Architecture

QuantForge is one Cargo package with two targets: the `quantforge` library,
rooted at `src/lib.rs`, and the `quantforge` binary, rooted at `src/main.rs`,
whose command tree lives in `src/cli/`. The binary reaches the library only
through its crate root, exactly as an outside caller would.

## Finding the code for a command

The module tree under `src/cli/` mirrors the command tree. Each word of a
command is a directory, the last word is the file, and dashes become
underscores:

| Command | File |
| --- | --- |
| `quantforge data sync` | `src/cli/data/sync.rs` |
| `quantforge monitor cancel-order` | `src/cli/monitor/cancel_order.rs` |
| `quantforge backtest` | `src/cli/backtest/mod.rs` |

A command group such as `quantforge monitor` lives in its directory's
`mod.rs`, which holds the subcommand enum and the dispatch. A top-level
command without subcommands, such as `backtest`, takes the directory form
too, so subcommands can join it without moving files. The root parser, the
global flags, and the dispatch into each group are in `src/cli/mod.rs`.

Each command's `--help` output is locked in `tests/snapshots/`, named by its
words after `quantforge` joined with dashes: the help of
`quantforge monitor cancel-order` is `tests/snapshots/monitor-cancel-order.txt`,
and the root's is `root.txt`.

## Source tree

<!--
tests/e2e/layout.rs compares this tree with src/. One entry per line: four
characters of tree drawing per level, the name (directories end with /), then
a note. Every .rs file under src/ appears exactly once, every entry has a
note, and the note of a command's module starts with the command and a colon,
as in `quantforge data sync: ...`. No other note may start with `quantforge`.
-->

```text
src/
├── main.rs                     binary entry point: parse the arguments, run the command
├── lib.rs                      library root: crate docs and the public re-exports
├── cli/                        the command tree, binary only
│   ├── mod.rs                  quantforge: root parser, global flags, dispatch into groups
│   ├── common.rs               shared flattened args, execution-mode enum, parse and print helpers
│   ├── context.rs              AppContext: tracing, SQLite store, exchange connectors, prompts
│   ├── tests.rs                root-parser unit tests and the config-matrix and README drift checks
│   ├── data/                   market-data commands
│   │   ├── mod.rs              quantforge data: subcommand enum and dispatch
│   │   ├── sync.rs             quantforge data sync: pull klines from the exchange into SQLite
│   │   └── validate.rs         quantforge data validate: check stored candles
│   ├── backtest/               a leaf command in directory form
│   │   └── mod.rs              quantforge backtest: simulate the strategy on stored candles
│   ├── trade/                  strategy-bot commands
│   │   ├── mod.rs              quantforge trade: subcommand enum and dispatch
│   │   ├── run.rs              quantforge trade run: the polling bot, dry-run by default
│   │   └── close.rs            quantforge trade close: market-sell the bot's position
│   └── monitor/                account commands; every one needs credentials
│       ├── mod.rs              quantforge monitor: subcommand enum and dispatch
│       ├── status.rs           quantforge monitor status: balances, run, orders, trades
│       ├── watch.rs            quantforge monitor watch: repeat the status report
│       ├── orders.rs           quantforge monitor orders: open orders on a symbol
│       ├── trades.rs           quantforge monitor trades: recent account trades
│       ├── cancel_order.rs     quantforge monitor cancel-order: cancel one order
│       └── close_position.rs   quantforge monitor close-position: market-sell the free balance
├── protocol/                   the wire format between instances; uses no other module
│   ├── mod.rs                  the wire spec: encoding, versions, compatibility; ProtocolError
│   ├── version.rs              ProtoVersion, PROTO_VERSION, MIN_SUPPORTED, VersionRange negotiation
│   ├── decimal.rs              ExactDecimal: exact decimals as JSON strings, never numbers
│   ├── hash.rs                 ContentHash: the sha256: names of stored content
│   ├── errors.rs               the error envelope every surface answers with, and its HTTP statuses
│   ├── segments.rs             candle segment index entries and their range metadata
│   ├── bundle/                 Python strategy bundles: the files, the manifest, the canonical tar
│   │   ├── mod.rs              Bundle: build, read back, hash, and admit a bundle; the spec's constants
│   │   ├── path.rs             BundlePath and BundleFile: the path rules that extract anywhere
│   │   ├── files.rs            BundleLimits, and the checks each file and the whole set pass
│   │   ├── dir.rs              collecting a bundle from a directory, never following a link
│   │   ├── archive.rs          the canonical tar writer and the code hash
│   │   ├── reader.rs           the strict reader a server checks an upload with
│   │   ├── manifest.rs         Manifest: quantforge.toml as bundle TOML, checked key by key
│   │   ├── entry.rs            Entry: the module and class the worker runs
│   │   ├── params.rs           Params: the strategy's parameters as JSON
│   │   ├── requirement.rs      EngineVersion, EngineRequirement, PythonVersion
│   │   ├── package.rs          PackageName and PackageVersion, normalized and canonical
│   │   ├── runtime.rs          Runtime: what a server runs bundles on, and admission
│   │   └── problems.rs         BundleProblem, ProblemCode, BundleError: why a bundle is refused
│   ├── jobs/                   remote backtests of a Python bundle, over HTTP
│   │   ├── mod.rs              the job surface and JobId
│   │   ├── submission.rs       JobSubmission: the bundle's hash, the candles, the backtest settings
│   │   └── status.rs           JobStatus, JobState, and the references to a job's results
│   └── stream/                 bars as they close, over a WebSocket
│       ├── mod.rs              the session flow, ClientFrame and ServerFrame, Ping and Pong
│       ├── handshake.rs        Hello and HelloVersions, Welcome, AuthToken, SessionId, SessionLimits
│       └── bars.rs             Subscribe, BarClosed, Bar, GapNotice, GapReason
├── model/                      domain types every layer speaks
│   ├── mod.rs                  ModelError and the re-exports
│   ├── candle.rs               Candle and validate_candles
│   ├── candle_batch.rs         CandleBatch: candles as columns, the Python interchange
│   ├── decimal.rs              round_down_to_step and the decimal ↔ float64 conversions
│   ├── interval.rs             Interval: wire form, step duration, parsing
│   ├── market.rs               Symbol, ExchangeId, MarketId
│   ├── order.rs                Side, OrderStatus, Fill, ExchangeOrder, AccountTrade
│   ├── request.rs              the exchange-agnostic requests a connector serves
│   ├── rules.rs                SymbolRules and AssetBalance
│   ├── run.rs                  BotRunState, PositionState, ClosedTrade, TargetPosition
│   └── timestamp.rs            TimestampMs, now_utc_ms, RFC 3339 conversions
├── ports.rs                    the seams: connector and storage traits, capabilities, error kinds
├── exchange/                   the connector registry, one adapter per exchange, and the conformance suite
│   ├── mod.rs                  the registry API: lookup, name, connect, ExchangeConnectors
│   ├── credentials.rs          ApiCredentials: an API key and secret, never printed
│   ├── registry.rs             ExchangeId to name, default endpoint, factory; the only id-to-adapter map
│   ├── conformance/            the contract every exchange connector meets, written as checks
│   │   ├── mod.rs              the contract itself, Target, Outcome, run, and the safety refusals
│   │   ├── check.rs            Check: each check's id, summary, and what it needs to run
│   │   ├── support.rs          the check context, how a check fails, and the requests checks share
│   │   ├── clock.rs            the exchange clock and closed-bar checks
│   │   ├── klines.rs           candle shape, window bounds and limit, paging, and gaps
│   │   ├── rules.rs            the symbol rules, and the trading rules orders must meet
│   │   ├── orders.rs           a filled market order, from the buy to the sell-back
│   │   ├── errors.rs           the error kinds, provoked live or scripted
│   │   └── matrix.rs           the feature matrix: capabilities beside the results
│   └── binance/                Binance Spot REST adapter
│       ├── mod.rs              module root and Binance's registry entry
│       ├── client.rs           HTTP transport, signed requests, production-host check
│       ├── convert.rs          wire DTOs to domain types
│       ├── errors.rs           Binance error codes and HTTP statuses to ExchangeErrorKind
│       ├── signing.rs          HMAC-SHA256 query signing
│       ├── types.rs            wire DTOs, the shapes the REST API returns
│       └── venue.rs            the connector implementations and declared capabilities
├── storage/                    persistence adapters, one namespace per backend
│   ├── mod.rs                  re-exports SqliteStore
│   └── sqlite/                 SQLite backend
│       ├── mod.rs              SqliteStore: connections, schema version, row helpers
│       ├── candles.rs          candle table and the CandleStore implementation
│       └── journal.rs          run, order-event, and trade tables: RunJournalStore
├── sdk/                        the strategy boundary
│   ├── mod.rs                  StrategyError and the re-exports
│   ├── strategy.rs             the Strategy trait and its StrategyContext
│   ├── indicators.rs           the Indicator trait and Sma
│   └── builtin/                strategies that ship with the crate
│       ├── mod.rs              BuiltInStrategyConfig: names and builds one
│       └── sma_cross.rs        SmaCrossStrategy, the strategy the CLI runs
└── engine/                     the drivers: backtest, ingestion, live trading
    ├── mod.rs                  EngineError, which every engine returns
    ├── backtest.rs             BacktestEngine: fills at the next bar's open
    ├── data_sync.rs            DataSyncEngine: historical and incremental ingestion
    └── live/                   LiveTradeEngine: the polling live and dry-run trader
        ├── mod.rs              public surface and the run lifecycle
        ├── runner.rs           the poll loop: bootstrap, bars, closed-bar detection
        ├── execution.rs        order placement, exchange rules, dry-run vs live
        └── state.rs            run identity: create, resume, and resume checks
```

After `cli/`, the seven library modules follow innermost first, as in the
`src/lib.rs` docs. `protocol` stands apart: it uses no other module, and no
module uses it yet. Then `model` uses no other module, `ports` uses `model`,
`exchange` and `storage` use `model` and `ports`, `sdk` uses `model`, and
`engine` uses `model`, `ports`, and `sdk`. The engines reach the exchange
and storage adapters only through the `ports` traits; the binary chooses
which adapters to pass in, building the connectors of the exchange that
`--exchange` names through `exchange::connect`. Nothing outside
`src/exchange/` names a concrete exchange adapter, nothing outside the
Binance adapter hardcodes Binance beyond the registry entry and the
`ExchangeId` variant, and `tests/exchange_boundary.rs` fails if anything
does. What a connector must do for the engines is written down once, as
the checks in `exchange::conformance`; every registered exchange passes
them, and [exchange-feature-matrix.md](exchange-feature-matrix.md) shows
the results beside each exchange's capabilities. `tests/protocol_boundary.rs`
compiles `protocol` on its own and scans its sources, so it fails if
`protocol` uses anything else in the crate, or a dependency beyond `serde`,
`serde_json`, `rust_decimal`, `sha2`, `thiserror`, and `toml`: the module is
meant to become a crate of its own. The strategy bundle format it carries
is written down in [bundle-spec.md](bundle-spec.md).

## Tests

- `tests/cli.rs`: black-box CLI tests of argument validation, error text,
  and exit codes
- `tests/e2e/`: end-to-end tiers — `offline.rs` against a mock Binance
  server, `testnet.rs` against Binance Spot testnet, `precedence.rs` for
  flag, environment, and default precedence, and `snapshot.rs` and
  `layout.rs` for the CLI surface and this document
- `tests/snapshots/`: the committed `--help` output of every command, plus
  `version.txt` for `--version`
- `tests/backtest_engine.rs`, `tests/candle_validation.rs`,
  `tests/candle_batch.rs`, `tests/decimal_float.rs`, `tests/sqlite_store.rs`,
  and `tests/live_engine.rs`: library integration tests
- `tests/exchange_boundary.rs`: keeps every module outside `src/exchange/`
  from naming a concrete exchange adapter, and the code and `--help`
  outside the Binance adapter from hardcoding Binance
- `tests/protocol_boundary.rs`: compiles `src/protocol/` as the only module
  of a crate and scans its sources, so the wire format never comes to
  depend on the rest of QuantForge
- `tests/bundle/`: the strategy bundle spec — the shared cases of
  `tests/fixtures/bundle_cases.json`, bundles built from real directories,
  the messages a refused bundle shows, a canonical tar read by the system
  `tar`, and [bundle-spec.md](bundle-spec.md) held to the code
- `tests/conformance/`: the conformance suite against every registered
  exchange — replayed from the recorded or hand-written fixtures in
  `tests/conformance/fixtures/`, or live against a testnet when
  `CONFORMANCE_MODE` asks for it — plus the controls that prove each check
  can fail, the recorder, and the generated feature matrix
- `tests/fixtures/`: data shared with the Python bindings' tests, starting
  with the decimal ↔ float64 conformance cases of
  [decimal-float-policy.md](decimal-float-policy.md)
- unit tests sit at the bottom of the module they test, in
  `#[cfg(test)] mod tests`, or in a `tests.rs` beside it once they would
  push the file over its line budget (`src/cli/tests.rs`)

To add a command, follow the recipe in
[CONTRIBUTING.md](../CONTRIBUTING.md#adding-a-cli-command).
