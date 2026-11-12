# QuantForge

[![ci](https://github.com/quantforge-rs/quantforge/actions/workflows/ci.yml/badge.svg)](https://github.com/quantforge-rs/quantforge/actions/workflows/ci.yml)
[![codeql](https://github.com/quantforge-rs/quantforge/actions/workflows/codeql.yml/badge.svg?branch=main)](https://github.com/quantforge-rs/quantforge/actions/workflows/codeql.yml)
[![gitleaks](https://github.com/quantforge-rs/quantforge/actions/workflows/gitleaks.yml/badge.svg?branch=main)](https://github.com/quantforge-rs/quantforge/actions/workflows/gitleaks.yml)
[![crates.io](https://img.shields.io/crates/v/quantforge.svg)](https://crates.io/crates/quantforge)
[![docs.rs](https://img.shields.io/docsrs/quantforge)](https://docs.rs/quantforge)
[![license](https://img.shields.io/crates/l/quantforge.svg)](https://github.com/quantforge-rs/quantforge/blob/main/LICENSE)

**Deterministic market data, backtests and controlled live trading for
Binance Spot, from one command line.**

There is no UI: the CLI is the product, and every engine behind it is a Rust
library too.

> 🚨 QuantForge is engineering software. It is **not** investment advice,
> recommends no trades and promises no profit.

- 📥 **Sync** settled candles into SQLite or PostgreSQL, and validate them
- 🧪 **Backtest** bar by bar in decimal arithmetic: the same inputs give the
  same result on every run and every machine
- 🤖 **Trade** the same strategy on live bars: dry-run by default, real
  orders only after an explicit confirmation
- 👀 **Monitor** balances, orders and trades, and cancel or close by hand
- 🌐 **Serve** candle history to other instances over HTTP; the backtest job
  queue beside it has no worker runtime yet
- 🦀 **Embed** it: the engines are plain Rust APIs, and a strategy is one
  trait

[Quickstart](#-quickstart) ·
[Cookbook](#-cookbook) ·
[Dry-run and live](#-dry-run-and-live) ·
[Credentials](#-credentials) ·
[Library](#-use-it-as-a-library) ·
[Documentation](#-documentation)

## 🚀 Quickstart

You need Rust 1.85 or newer. There is no account and no API key to set up:
market data is public, and the bot starts in dry-run.

```bash
git clone https://github.com/quantforge-rs/quantforge
cd quantforge
```

### 1. Download two days of 1-minute candles

```bash
cargo run -- data sync --symbol BTCUSDT --interval 1m \
  --start 2026-11-10T00:00:00Z --end 2026-11-12T00:00:00Z
```

```text
iterations: 1
written: 2881
first_synced_open_time: 2026-11-10T00:00:00Z
last_open_time: 2026-11-12T00:00:00Z
segments_built: 2
```

### 2. Check the series for gaps, duplicates and impossible prices

```bash
cargo run -- data validate --symbol BTCUSDT --interval 1m
```

```text
market: binance_spot BTCUSDT 1m
candles: 2881
issues: 0
```

### 3. Backtest the built-in SMA crossover

```bash
cargo run -- backtest --symbol BTCUSDT --interval 1m
```

```text
strategy: sma_cross
final_equity: 9023.052458195077526031947817
total_return_pct: -9.769475418049224739680521830
max_drawdown_pct: 9.938864734276031155165049330
trade_count: 35
```

The last five trades follow. Run it again: every digit is the same.

### 4. Let the bot watch live bars, in dry-run

```bash
cargo run -- trade run --symbol BTCUSDT --interval 1m
```

It downloads the bars it needs, warms the strategy up on them, then acts on
each newly closed bar, with no way to send an order. Ctrl-C stops it:

```text
run_id: qf-f317a3e7039943029b1664da6b42f34b
processed_bars: 1
submitted_orders: 0
closed_trades: 0
last_processed_open_time: 2026-11-12T13:25:00Z
```

The outputs above leave out the log lines; `--log-level warn` hides them.

## 📚 Cookbook

Every example runs from a checkout through `cargo run`. With the binary
installed (`cargo install --path . --locked`), `quantforge` takes the same
arguments. The examples follow `main`, which can be ahead of the release on
crates.io.

Data goes to `data/market.sqlite` unless `--db` or `QF_DB` names another
file.

### 📥 Market data

```bash
# from now on, until Ctrl-C
cargo run -- data sync --symbol BTCUSDT --interval 1m

# backfill a window, then stop
cargo run -- data sync --symbol BTCUSDT --interval 1m \
  --start 2026-11-01T00:00:00Z --end 2026-11-08T00:00:00Z

# backfill from a date, then keep following new candles
cargo run -- data sync --symbol BTCUSDT --interval 1m --start 2026-11-01T00:00:00Z

# catch up to now in one pass and exit: for cron and CI
cargo run -- data sync --symbol BTCUSDT --interval 1m \
  --start 2026-11-01T00:00:00Z --max-loops 1

# follow until a set end, then stop
cargo run -- data sync --symbol BTCUSDT --interval 1m \
  --start 2026-11-01T00:00:00Z --end 2027-01-01T00:00:00Z --follow

# hourly candles of another market, in the same database
cargo run -- data sync --symbol ETHUSDT --interval 1h \
  --start 2026-10-01T00:00:00Z --end 2026-11-01T00:00:00Z

# testnet candles, kept apart in a database of their own
cargo run -- --db data/testnet.sqlite --base-url https://testnet.binance.vision/ \
  data sync --symbol BTCUSDT --interval 1m

# check a whole series, or one window of it
cargo run -- data validate --symbol BTCUSDT --interval 1m
cargo run -- data validate --symbol BTCUSDT --interval 1m \
  --start 2026-11-01T00:00:00Z --end 2026-11-02T00:00:00Z
```

- Intervals: `1s 1m 3m 5m 15m 30m 1h 2h 4h 6h 8h 12h 1d 3d 1w`. Times are
  RFC 3339, such as `2026-11-01T00:00:00Z`.
- Without `--start` a sync begins now, with no backfill. Without `--end` it
  runs until Ctrl-C.
- Only settled candles are stored: a candle is written a second after it
  closes, so every stored candle is final.
- `written` counts the candles sent to the store, so repeating a sync
  reports them again and changes nothing.
- Each pass packs closed windows, such as one UTC day of `1m` candles, into
  immutable segments for the server; `segments_built` counts them.

A sync or a bot that starts later leaves a hole behind it. `data validate`
names the hole and exits non-zero:

```text
market: binance_spot BTCUSDT 1m
candles: 3180
issues: 1
  00: Gap { expected_open_time_ms: 1794441660000, open_time_ms: 1794471480000 }
Error: data validate found 1 issue(s)
```

A bounded sync over the hole fills it:

```bash
cargo run -- data sync --symbol BTCUSDT --interval 1m \
  --start 2026-11-12T00:00:00Z --end 2026-11-12T09:00:00Z
```

### 🧪 Backtests

```bash
# the defaults: --fast 20 --slow 50 --cash 10000 --fee-bps 10
cargo run -- backtest --symbol BTCUSDT --interval 1m

# other averages, starting cash and fee
cargo run -- backtest --symbol BTCUSDT --interval 1m \
  --fast 10 --slow 30 --cash 5000 --fee-bps 7.5

# one slice of the stored history
cargo run -- backtest --symbol BTCUSDT --interval 1m \
  --start 2026-11-11T00:00:00Z --end 2026-11-11T12:00:00Z

# no fees
cargo run -- backtest --symbol BTCUSDT --interval 1m --fee-bps 0

# hourly bars of another market
cargo run -- backtest --symbol ETHUSDT --interval 1h --fast 12 --slow 48
```

- A decision made on one bar fills at the **next bar's open**, a price the
  strategy had not seen when it decided: there is no look-ahead.
- Every fill pays `--fee-bps` basis points, and a position still open at the
  end is sold at the last close.
- The strategy needs more than `--slow` candles before it can trade.
- A backtest replays what the store holds and does not check it: run
  `data validate` first.

The
[backtest module](https://github.com/quantforge-rs/quantforge/blob/main/src/engine/backtest.rs)
states the whole execution model.

### 🤖 Trading bot

```bash
# dry-run is the default: real candles, and no order can be sent
cargo run -- trade run --symbol BTCUSDT --interval 1m

# other averages on 5-minute bars, 250 of the quote currency per entry
cargo run -- trade run --symbol BTCUSDT --interval 5m \
  --fast 10 --slow 30 --quote-order-qty 250

# stop by itself after 12 polls, 10 seconds apart
cargo run -- trade run --symbol BTCUSDT --interval 1m --poll-secs 10 --max-loops 12

# name a run; the same command later picks it up where it stopped
cargo run -- trade run --symbol BTCUSDT --interval 1m --run-id btc-sma-dry

# act at once on what the warm-up decided, instead of waiting for the next signal
cargo run -- trade run --symbol BTCUSDT --interval 1m --bootstrap-enter
```

- A new run downloads the last `--bootstrap-bars` (300) closed bars and
  replays them through the strategy before it acts.
- Without `--run-id` every command starts a new run with a flat position.
- A run resumes only under the market, strategy, parameters, mode,
  environment and account it started with.

Live trading, on the Binance Spot testnet first:

```bash
export QF_DB=data/testnet.sqlite
export QF_BASE_URL=https://testnet.binance.vision/
export QF_API_KEY="$(cat ~/.config/quantforge/testnet-api-key)"
export QF_API_SECRET="$(cat ~/.config/quantforge/testnet-api-secret)"

# once: tie this database to the testnet for life
cargo run -- store bind --environment testnet --yes

# without --yes a terminal asks for a typed yes, and a script only gets a preview
cargo run -- trade run --symbol BTCUSDT --interval 1m --mode live

# with --yes: real orders
cargo run -- trade run --symbol BTCUSDT --interval 1m --mode live --yes

# sell what the bot holds: look first, then confirm
cargo run -- trade close --symbol BTCUSDT
cargo run -- trade close --symbol BTCUSDT --yes
```

The preview a script gets, with exit status 0 and nothing sent:

```text
refusing to start live trading without --yes
would run strategy sma_cross on binance_spot BTCUSDT 1m
with REAL orders via https://testnet.binance.vision/
re-run with --yes to confirm, or use --mode dry-run
```

### 👀 Monitor and intervene

Every `monitor` command signs its requests, so it needs
[credentials](#-credentials).

```bash
# balances, the latest run, open orders and recent trades
cargo run -- monitor status --symbol BTCUSDT

# the same report every 10 seconds, until Ctrl-C
cargo run -- monitor watch --symbol BTCUSDT --poll-secs 10

# a bot that ran with other settings is found by them
cargo run -- monitor status --symbol BTCUSDT --interval 5m --strategy-name sma_cross

cargo run -- monitor orders --symbol BTCUSDT
cargo run -- monitor trades --symbol BTCUSDT --limit 50

# cancel one order: without --yes nothing is sent
cargo run -- monitor cancel-order --symbol BTCUSDT --order-id 123456789
cargo run -- monitor cancel-order --symbol BTCUSDT --order-id 123456789 --yes

# sell the whole free base balance at market
cargo run -- monitor close-position --symbol BTCUSDT
cargo run -- monitor close-position --symbol BTCUSDT --yes
```

`trade close` sells what a run holds and records the trade in its journal;
`monitor close-position` sells the account's whole free balance of the base
asset and records nothing.

The same report with every setting as a flag and no `QF_*` variable set:

```bash
cargo run -- \
  --db data/testnet.sqlite \
  --base-url https://testnet.binance.vision/ \
  --api-key "$(cat ~/.config/quantforge/testnet-api-key)" \
  --api-secret "$(cat ~/.config/quantforge/testnet-api-secret)" \
  monitor status --symbol BTCUSDT
```

### 💾 The store

```bash
# identity, schema, environment, durability, migrations and legacy runs
cargo run -- store status
```

```text
storage: sqlite data/market.sqlite
store_id: 7abd29cecea04835a8ee00807789e9ed
schema_version: 9
environment: undeclared
activation: inactive
activation_epoch: 0
durability: met (journal_mode=wal synchronous=full fullfsync=on)
transfer: none
migrations: 1
  0 -> 9 at 2026-11-12T13:17:14.766Z by quantforge 0.2.0
legacy_runs: 0
```

```bash
# bind a database to the environment it trades, once and for life:
# look first, then confirm
cargo run -- --db data/testnet.sqlite --base-url https://testnet.binance.vision/ \
  store bind --environment testnet
cargo run -- --db data/testnet.sqlite --base-url https://testnet.binance.vision/ \
  store bind --environment testnet --yes

# a database of production candles is bound to mainnet the same way
cargo run -- store bind --environment mainnet --yes

# bring a run an earlier release wrote back under a verified identity
cargo run -- store adopt-run --run-id qf-legacy-run --execution-mode dry-run --yes
```

A store bound to `testnet` refuses a production URL before any request, and
the other way round:

```text
Error: store aa17f12592e24eed8f8c6a5e63f7362d is bound to testnet, but https://api.binance.com/ is a production endpoint; refusing before any request (pass the testnet base URL, or use another --db)
```

Testnet and production candles look alike in one database, so give each
environment a database of its own and bind it.

### 🐘 PostgreSQL

SQLite is the default. `--storage postgres` moves everything, candles,
segments, runs and jobs, into one PostgreSQL database (16 to 18):

```bash
export QF_STORAGE=postgres
export QF_POSTGRES_URL=postgres://USER:PASSWORD@HOST:5432/DATABASE

# coming from SQLite: copy the store in first, while the database is empty,
# then compare every row
cargo run -- data migrate --from data/market.sqlite --verify

# every command now reads and writes that database
cargo run -- store status
cargo run -- data sync --symbol BTCUSDT --interval 1m \
  --start 2026-11-01T00:00:00Z --end 2026-11-08T00:00:00Z
cargo run -- backtest --symbol BTCUSDT --interval 1m
```

- The password lives only in the URL: keep it in `QF_POSTGRES_URL`, not on
  the command line. No log line, error or `--help` output shows it.
- The database must exist and hold QuantForge alone; the first command
  creates the schema.
- A connection to another machine is always verified TLS; `sslrootcert` in
  the URL names one more CA to trust.
- `data migrate` refuses a database that already holds rows of its own, so
  it comes before any other write. It never changes the SQLite file and can
  be resumed;
  [storage-migration.md](https://github.com/quantforge-rs/quantforge/blob/main/docs/storage-migration.md)
  covers the switch and the way back.

### 🌐 HTTP server

```bash
cargo run -- serve run
```

```text
storage: sqlite data/market.sqlite
job_concurrency: 4
job_limits: cpu 600 s, memory 4096 MiB, wall clock 1800 s
worker_isolation: cpu rlimit, memory rlimit, network seccomp, files landlock
listening: http://127.0.0.1:9600
```

From another terminal:

```bash
curl http://127.0.0.1:9600/healthz
curl http://127.0.0.1:9600/version
curl http://127.0.0.1:9600/v1/markets

# which segments hold 2026-11-10, with the newest candles inline
DAY='http://127.0.0.1:9600/v1/candles?exchange=binance&category=spot&symbol=BTCUSDT&interval=1m&start_open_ms=1794268800000&end_open_ms=1794355140000'
curl "$DAY"

# a segment is named by the SHA-256 of its payload: fetch one and check it
HASH=$(curl -s "$DAY" | jq -r '.segments[0].hash')
curl -s -o segment.zst "http://127.0.0.1:9600/v1/segments/$HASH"
zstd -dc segment.zst | sha256sum

# the backtest jobs the server holds
curl http://127.0.0.1:9600/v1/backtests
```

`/v1/markets` answers what the store holds:

```json
{
  "markets": [
    {
      "exchange": "binance",
      "category": "spot",
      "symbol": "BTCUSDT",
      "interval": "1m",
      "first_open_ms": 1794268800000,
      "last_open_ms": 1794489360000
    }
  ]
}
```

Another address, a smaller range limit and two jobs at a time:

```bash
cargo run -- serve run --listen 127.0.0.1:8080 --max-span-bars 100000 --job-concurrency 2
```

| Route | Answers |
| --- | --- |
| `GET /healthz` | `ok` |
| `GET /version` | the server's version and the protocol versions it speaks |
| `GET /v1/markets` | every market held, with the span of its candles |
| `GET /v1/candles` | the segments that hold a range, and its newest candles inline |
| `GET /v1/segments/{hash}` | one segment; it never changes, so any cache may keep it for a year |
| `PUT /v1/bundles/{code_hash}` | takes a strategy bundle as its canonical tar |
| `POST /v1/backtests` | queues a backtest job on candles the store holds |
| `GET /v1/backtests`, `GET /v1/backtests/{job_id}` | where jobs stand |
| `POST /v1/backtests/{job_id}/cancel` | cancels a job |
| `GET /v1/backtests/{job_id}/result` | a succeeded job's metrics and its files: `metrics.json`, `trades.arrow`, `equity.arrow` |
| `GET /v1/backtests/{job_id}/logs` | a job's log, a page at a time or as a WebSocket stream |

- The routes need no credentials yet, and the default address takes
  connections from this machine only.
- Jobs run `--job-concurrency` at a time, half the CPU cores unless set,
  each in a worker process of its own, held to `--job-cpu-secs`,
  `--job-memory-mib` and `--job-wall-secs`; on Linux a worker reaches no
  network and no file outside its scratch directory
  ([worker isolation](https://github.com/quantforge-rs/quantforge/blob/main/docs/worker-isolation.md)).
- This release's workers have no runtime yet, so each job fails as it
  starts, saying so.
- On Ctrl-C or SIGTERM the server finishes the requests in progress and
  fails the jobs that run, so that their clients submit them again; a second
  signal stops it without waiting.

The rustdoc of
[`protocol`](https://github.com/quantforge-rs/quantforge/blob/main/src/protocol/mod.rs)
is the wire specification, for
[history](https://github.com/quantforge-rs/quantforge/blob/main/src/protocol/segments/mod.rs)
and for
[jobs](https://github.com/quantforge-rs/quantforge/blob/main/src/protocol/jobs/mod.rs);
the
[bundle](https://github.com/quantforge-rs/quantforge/blob/main/docs/bundle-spec.md)
and
[result](https://github.com/quantforge-rs/quantforge/blob/main/docs/result-spec.md)
specs cover what a job takes and gives back.

### 🔧 Configuration

Every setting is a flag. The global ones, and those of `serve run`, also
read a `QF_*` variable; a flag wins over its variable, which wins over the
default.

| Flag | Variable | Default |
| --- | --- | --- |
| `--db` | `QF_DB` | `data/market.sqlite` |
| `--storage` | `QF_STORAGE` | `sqlite` |
| `--postgres-url` | `QF_POSTGRES_URL` | none |
| `--exchange` | `QF_EXCHANGE` | `binance`, so far the only one |
| `--base-url` | `QF_BASE_URL` | the exchange's production endpoint |
| `--log-level` | `QF_LOG` | `info` |
| `--api-key` | `QF_API_KEY` | none |
| `--api-secret` | `QF_API_SECRET` | none |

```bash
# a global flag goes before or after the subcommand
cargo run -- --db data/research.sqlite store status
cargo run -- store status --db data/research.sqlite

# or set it once per shell
export QF_DB=data/research.sqlite
cargo run -- store status

# result lines only
cargo run -- --log-level warn data validate --symbol BTCUSDT --interval 1m

# everything, or a filter per target
cargo run -- --log-level debug store status
export QF_LOG=warn,quantforge=debug
```

The
[configuration matrix](https://github.com/quantforge-rs/quantforge/blob/main/docs/configuration.md)
lists every flag with its variable, default and scope, and the settings
renamed from their earlier `--binance-*` and `QF_BINANCE_*` names.

### 🦀 Use it as a library

The CLI runs the built-in strategy; your own runs through the library. The
strategy API changed after `0.2.0`, so depend on the repository until the
next release:

```toml
[dependencies]
quantforge = { git = "https://github.com/quantforge-rs/quantforge" }
rust_decimal = "1"
```

A strategy is one trait. It sees closed bars and answers with the position
it wants:

```rust
use quantforge::{
    BacktestConfig, BacktestEngine, Candle, ExchangeId, Indicator, Interval, MarketId, Sma,
    Strategy, StrategyContext, StrategyError, Symbol, TargetPosition,
};
use rust_decimal::Decimal;

/// Long while the close is above its moving average, flat while below it.
struct AboveAverage {
    average: Sma,
}

impl Strategy for AboveAverage {
    fn name(&self) -> &str {
        "above_average"
    }

    fn on_bar(
        &mut self,
        _ctx: &StrategyContext,
        bar: &Candle,
    ) -> Result<Option<TargetPosition>, StrategyError> {
        let Some(average) = self.average.update(bar.close) else {
            return Ok(None); // still warming up
        };
        Ok(if bar.close > average {
            Some(TargetPosition::LongAllIn)
        } else if bar.close < average {
            Some(TargetPosition::Flat)
        } else {
            None
        })
    }
}

/// A one-minute bar that opens and closes at `price`.
fn bar(minute: i64, price: i64) -> Candle {
    let price = Decimal::from(price);
    Candle {
        open_time_ms: minute * 60_000,
        close_time_ms: minute * 60_000 + 59_999,
        open: price,
        high: price,
        low: price,
        close: price,
        volume: Decimal::ONE,
        trades: None,
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Prices climb from 100 to 119, then fall back to 100.
    let prices = (100..120).chain((100..120).rev());
    let candles: Vec<Candle> = (0..)
        .zip(prices)
        .map(|(minute, price)| bar(minute, price))
        .collect();

    let market = MarketId::new(ExchangeId::BinanceSpot, Symbol::new("BTCUSDT")?, Interval::M1);
    let engine = BacktestEngine::new(BacktestConfig {
        initial_cash: Decimal::from(10_000),
        fee_bps: Decimal::from(10),
        close_out_at_end: true,
    });
    let mut strategy = AboveAverage { average: Sma::new(5)? };
    let result = engine.run(&market, &candles, &mut strategy)?;

    // A decision fills at the next bar's open: in at 105, out at 117.
    assert_eq!(result.trade_count, 1);
    assert_eq!(result.trades[0].entry_price, Decimal::from(105));
    assert_eq!(result.trades[0].exit_price, Decimal::from(117));
    println!("final equity: {}", result.final_equity);
    Ok(())
}
```

Backtest on the candles `data sync` stored:

```rust,no_run
use quantforge::{
    BacktestConfig, BacktestEngine, CandleQuery, CandleStore, ExchangeId, Interval, MarketId,
    SmaCrossStrategy, SqliteStore, Symbol, validate_candles,
};
use rust_decimal::Decimal;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let market = MarketId::new(ExchangeId::BinanceSpot, Symbol::new("BTCUSDT")?, Interval::M1);

    let store = SqliteStore::new("data/market.sqlite");
    CandleStore::init(&store)?;
    let candles = store.load_candles(&market, CandleQuery::default())?;

    let report = validate_candles(&market, &candles);
    assert!(report.is_ok(), "{} issue(s) in the series", report.issues.len());

    let engine = BacktestEngine::new(BacktestConfig {
        initial_cash: Decimal::from(10_000),
        fee_bps: Decimal::from(10),
        close_out_at_end: true,
    });
    let result = engine.run(&market, &candles, &mut SmaCrossStrategy::new(20, 50)?)?;

    println!("return: {} %", result.total_return_pct);
    println!("max drawdown: {} %", result.max_drawdown_pct);
    for trade in &result.trades {
        println!("{} -> {}: {}", trade.entry_price, trade.exit_price, trade.gross_quote_pnl);
    }
    Ok(())
}
```

Decimals, timestamps and rounding, the way the engines use them:

```rust
use quantforge::{Indicator, Interval, Sma, ms_to_rfc3339, parse_rfc3339_to_ms, round_down_to_step};
use rust_decimal::Decimal;
use std::str::FromStr;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Decimal arithmetic: the average of 0.1, 0.2 and 0.3 is exactly 0.2.
    let mut sma = Sma::new(3)?;
    assert_eq!(sma.update(Decimal::from_str("0.1")?), None);
    assert_eq!(sma.update(Decimal::from_str("0.2")?), None);
    assert_eq!(sma.update(Decimal::from_str("0.3")?), Some(Decimal::from_str("0.2")?));

    // Timestamps are UTC epoch milliseconds.
    let open_ms = parse_rfc3339_to_ms("2026-11-10T00:00:00Z")?;
    assert_eq!(open_ms, 1_794_268_800_000);
    assert_eq!(ms_to_rfc3339(open_ms + Interval::H1.step_ms()), "2026-11-10T01:00:00Z");

    // A quantity is rounded down to the exchange's step, never up.
    let qty = round_down_to_step(Decimal::from_str("0.123456")?, Decimal::from_str("0.001")?);
    assert_eq!(qty, Decimal::from_str("0.123")?);
    Ok(())
}
```

## 🚦 Dry-run and live

`trade run` has two modes, selected with `--mode`.

| | dry-run, the default | live |
| --- | --- | --- |
| Candles | real | real |
| Orders | none: the engine is built without a trading connector | real spot market orders |
| Needs | nothing | `--mode live`, credentials, and a confirmation |

- **Confirmation** is `--yes`, or a typed `yes` at the prompt an interactive
  terminal shows. A script without `--yes` gets a preview and exit status 0.
- **Automation** must pass `--yes`, and should treat output without a
  `run_id:` line as "the bot did not run".
- **Production** endpoints are marked `(PRODUCTION)` in the confirmation and
  in the logs. That guards against operator mistakes; it is not a security
  boundary.
- **Every command that changes something** asks the same way: `trade close`,
  `monitor cancel-order`, `monitor close-position`, `store bind` and
  `store adopt-run` say what they would do and exit 0 without `--yes`.
- **Test on the testnet**, with a database of its own, bound to it.

## 🔐 Credentials

The signed commands need the exchange's API key and secret: every `monitor`
command, `trade close`, `trade run --mode live`, and `store adopt-run` for a
live run. Everything else runs without them.

| Credential | Flag | Environment variable |
| --- | --- | --- |
| API key | `--api-key` | `QF_API_KEY` |
| API secret | `--api-secret` | `QF_API_SECRET` |

Keep each secret in a file only you can read (`chmod 600`) and load it with
command substitution, once per shell:

```bash
export QF_API_KEY="$(cat ~/.config/quantforge/testnet-api-key)"
export QF_API_SECRET="$(cat ~/.config/quantforge/testnet-api-secret)"
export QF_BASE_URL=https://testnet.binance.vision/
```

- **Start on the testnet**: create keys at <https://testnet.binance.vision/>.
- **Both or neither**: a lone or empty value counts as missing, and the
  command stops with an error that names both mechanisms.
- **A flag wins over its variable**, one credential at a time: with both
  variables exported, `--api-secret` replaces only the secret. An empty flag
  never falls back to its variable, so a missing file fails the command
  instead of quietly using another key.
- **Prefer the variables**: arguments show in process listings (`ps`) while
  QuantForge runs, and a secret typed literally lands in your shell history;
  with command substitution, history records only `$(cat ...)`. On an
  interactive terminal QuantForge warns when a credential arrives as a flag.
- **Mind usage errors**: a value that loses its flag is echoed back
  (`unexpected argument '...'`), on screen and in any captured log.

## 🎯 Determinism

- timestamps are UTC epoch milliseconds
- prices and volumes use decimal arithmetic, never floating point
- strategies see only closed bars
- backtests fill a decision at the **next bar's open**
- live trading polls closed bars and records run state in the selected store

## 🧱 Architecture

One Cargo package, layered as modules, innermost first:

| Module | Holds |
| --- | --- |
| `model` | markets, candles, orders, decimals and timestamps; `model::futures`, the futures contract, with its [risk sizing](https://github.com/quantforge-rs/quantforge/blob/main/docs/risk-sizing.md) |
| `ports` | the connector and storage traits every adapter implements |
| `exchange` | the registry behind `--exchange`, one adapter per exchange (`binance` today), and the conformance checks every adapter passes ([feature matrix](https://github.com/quantforge-rs/quantforge/blob/main/docs/exchange-feature-matrix.md)) |
| `storage` | the `sqlite` and `postgres` backends, and the [segment format](https://github.com/quantforge-rs/quantforge/blob/main/docs/segment-spec.md) both pack candles into |
| `sdk` | the `Strategy` trait, indicator math, and the built-in SMA crossover |
| `engine` | the drivers: `backtest`, `data_sync` and `live` |
| `automation` | execution on external signals; so far the [policy service](https://github.com/quantforge-rs/quantforge/blob/main/docs/automation-policy.md), which no command reaches yet |
| `protocol` | the versioned wire types between instances; it uses nothing else in the crate, so that it can become a crate of its own |
| `server` | what `serve run` hosts: history, the job queue and its workers |

The binary adds `cli`, a module tree that mirrors the command tree:
`quantforge monitor cancel-order` lives in `src/cli/monitor/cancel_order.rs`.
The
[architecture overview](https://github.com/quantforge-rs/quantforge/blob/main/docs/architecture.md)
annotates every source file and maps each command to its file.

## 📖 Documentation

| Topic | Read |
| --- | --- |
| Every flag, variable and default | [configuration.md](https://github.com/quantforge-rs/quantforge/blob/main/docs/configuration.md) |
| The source tree, command by command | [architecture.md](https://github.com/quantforge-rs/quantforge/blob/main/docs/architecture.md) |
| Upgrading a database | [upgrading.md](https://github.com/quantforge-rs/quantforge/blob/main/docs/upgrading.md) |
| Moving a store to PostgreSQL | [storage-migration.md](https://github.com/quantforge-rs/quantforge/blob/main/docs/storage-migration.md) |
| What a commit survives | [storage-durability.md](https://github.com/quantforge-rs/quantforge/blob/main/docs/storage-durability.md) |
| Tables and migrations | [storage-schema.md](https://github.com/quantforge-rs/quantforge/blob/main/docs/storage-schema.md) |
| Candle segments | [segment-spec.md](https://github.com/quantforge-rs/quantforge/blob/main/docs/segment-spec.md) |
| Strategy bundles and job results | [bundle-spec.md](https://github.com/quantforge-rs/quantforge/blob/main/docs/bundle-spec.md), [result-spec.md](https://github.com/quantforge-rs/quantforge/blob/main/docs/result-spec.md) |
| What a job's worker may do | [worker-isolation.md](https://github.com/quantforge-rs/quantforge/blob/main/docs/worker-isolation.md) |
| Exchanges: what each supports, adding one | [exchange-feature-matrix.md](https://github.com/quantforge-rs/quantforge/blob/main/docs/exchange-feature-matrix.md), [adding-an-exchange.md](https://github.com/quantforge-rs/quantforge/blob/main/docs/adding-an-exchange.md) |
| Decimals and float64 | [decimal-float-policy.md](https://github.com/quantforge-rs/quantforge/blob/main/docs/decimal-float-policy.md) |
| Futures automation: decisions, sizing, policies | [ADR 0002](https://github.com/quantforge-rs/quantforge/blob/main/docs/adr/0002-automation-architecture.md), [risk-sizing.md](https://github.com/quantforge-rs/quantforge/blob/main/docs/risk-sizing.md), [automation-policy.md](https://github.com/quantforge-rs/quantforge/blob/main/docs/automation-policy.md) |
| Contributing and CI | [CONTRIBUTING.md](https://github.com/quantforge-rs/quantforge/blob/main/CONTRIBUTING.md), [ci.md](https://github.com/quantforge-rs/quantforge/blob/main/docs/ci.md) |
| What changed | [CHANGELOG.md](https://github.com/quantforge-rs/quantforge/blob/main/CHANGELOG.md) |

## ⏫ Upgrading

A database from an earlier release is migrated in place the first time a
newer release opens it, never deleted: candles, journals, IDs, timestamps
and every decimal's exact text stay as they were. Back the file up first,
then follow the
[upgrade guide](https://github.com/quantforge-rs/quantforge/blob/main/docs/upgrading.md),
which also says how `store status` and `store adopt-run` bring a run an
earlier release wrote back under a verified environment and account.

## 🧰 Development

Run the checks CI runs before opening a pull request:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo clippy --all-targets --no-default-features -- -D warnings
cargo test --all-features --locked
cargo test --no-default-features --locked
./scripts/release-check.sh
```

`cargo test` needs no network and no keys: the exchange conformance suite
replays recorded testnet traffic, and a scheduled workflow runs it live
against each testnet.
[CONTRIBUTING.md](https://github.com/quantforge-rs/quantforge/blob/main/CONTRIBUTING.md)
has the rest: the PostgreSQL legs, the
[conformance suite](https://github.com/quantforge-rs/quantforge/blob/main/CONTRIBUTING.md#the-conformance-suite),
the CLI snapshots, and how to add a command or an exchange.

## 🧭 Roadmap

- additional exchanges such as Bybit
- restart reconciliation between local bot state and exchange state
- richer indicators and more built-in strategies
- TradingView alerts executed as risk-sized trades in USDT-settled linear
  and coin-settled inverse futures on Binance and Bybit, with stop losses
  held by the exchange, monitoring, and Telegram notifications
  ([ADR 0002](https://github.com/quantforge-rs/quantforge/blob/main/docs/adr/0002-automation-architecture.md))
- research-oriented parameter sweeps and snapshot reports

## 📄 License

Licensed under MIT.
