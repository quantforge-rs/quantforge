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
│   ├── context.rs              AppContext: tracing, SQLite store, Binance clients, prompts
│   ├── data/                   market-data commands
│   │   ├── mod.rs              quantforge data: subcommand enum and dispatch
│   │   ├── sync.rs             quantforge data sync: pull klines from Binance into SQLite
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
├── model/                      domain types every layer speaks
│   ├── mod.rs                  ModelError and the re-exports
│   ├── candle.rs               Candle and validate_candles
│   ├── candle_batch.rs         CandleBatch: candles as columns, the Python interchange
│   ├── decimal.rs              round_down_to_step and the decimal ↔ float64 conversions
│   ├── interval.rs             Interval: wire form, step duration, parsing
│   ├── market.rs               Symbol, ExchangeId, MarketId
│   ├── order.rs                Side, OrderStatus, Fill, ExchangeOrder, AccountTrade
│   ├── rules.rs                SymbolRules and AssetBalance
│   ├── run.rs                  BotRunState, PositionState, ClosedTrade, TargetPosition
│   └── timestamp.rs            TimestampMs, now_utc_ms, RFC 3339 conversions
├── ports.rs                    the seams: venue and storage traits, requests, errors
├── exchange/                   venue adapters, one namespace per venue
│   ├── mod.rs                  re-exports the Binance client and credentials
│   └── binance/                Binance Spot REST adapter
│       ├── mod.rs              module root; only the client and credentials leave it
│       ├── client.rs           HTTP transport: base URL, public and signed requests
│       ├── convert.rs          wire DTOs to domain types
│       ├── credentials.rs      BinanceCredentials: resolving the API key and secret
│       ├── signing.rs          HMAC-SHA256 query signing
│       ├── types.rs            wire DTOs, the shapes the REST API returns
│       └── venue.rs            the MarketDataSource and TradingVenue implementations
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

After `cli/`, the six library modules follow innermost first, as in the
`src/lib.rs` docs: `model` uses no other module, `ports` uses `model`,
`exchange` and `storage` use `model` and `ports`, `sdk` uses `model`, and
`engine` uses `model`, `ports`, and `sdk`. The engines reach the exchange
and storage adapters only through the `ports` traits; the binary chooses
which adapters to pass in.

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
- `tests/fixtures/`: data shared with the Python bindings' tests, starting
  with the decimal ↔ float64 conformance cases of
  [decimal-float-policy.md](decimal-float-policy.md)
- unit tests sit at the bottom of the module they test, in
  `#[cfg(test)] mod tests`

To add a command, follow the recipe in
[CONTRIBUTING.md](../CONTRIBUTING.md#adding-a-cli-command).
