# Configuration

Every QuantForge setting is a command-line flag. The five global settings also
read a `QF_*` environment variable as a fallback; per-command options are flags
only. QuantForge reads no other environment variable itself — the few its
dependencies honor are listed at the end.

## Precedence

For each global setting, the first source present wins:

1. the flag, before or after the subcommand — these two are the same:
   `quantforge --db a.sqlite data validate ...` and
   `quantforge data validate ... --db a.sqlite`;
2. its environment variable;
3. the default.

A variable that is exported but empty is still set, so it never falls back to
the default:

- `QF_DB=` is a usage error (exit status 2, reported against `--db`);
- `QF_BINANCE_BASE_URL=` fails with `failed to parse --binance-base-url`, so a
  missing testnet URL can never quietly become production;
- `QF_LOG=` is an empty filter, which turns log output off, warnings included;
- an empty or whitespace-only API key or secret counts as missing, so commands
  that need credentials fail with the missing-credentials error.

## Matrix

<!--
The `configuration_doc_matches_every_flag_env_var_default_and_scope` unit test
in src/cli/mod.rs compares this table with the clap definitions. One flag per
row, and no pipe inside a cell:
  Flag     a backticked `--name`, optionally followed by a <placeholder>
  Env var  a backticked variable name, or —
  Default  one backticked value, `required`, or `none`
  Scope    global, every command, or backticked command paths separated by
           commas
Explanations belong in the Setting cell.
-->

| Setting | Flag | Env var | Default | Scope |
| --- | --- | --- | --- | --- |
| SQLite database path, relative to the working directory; a missing parent directory is created | `--db <path>` | `QF_DB` | `data/market.sqlite` | global |
| Binance API base URL; `https://testnet.binance.vision/` is the Spot testnet | `--binance-base-url <url>` | `QF_BINANCE_BASE_URL` | `https://api.binance.com/` | global |
| Log filter: `trace`, `debug`, `info`, `warn`, `error`, or a full tracing filter expression; log lines go to stdout | `--log-level <filter>` | `QF_LOG` | `info` | global |
| Binance API key for signed endpoints (`monitor`, `trade close`, `trade run --mode live`); set both credentials or neither, and prefer the variable, since command-line values can land in shell history and process listings | `--binance-api-key <key>` | `QF_BINANCE_API_KEY` | none | global |
| Binance API secret paired with the key; same rules | `--binance-api-secret <secret>` | `QF_BINANCE_API_SECRET` | none | global |
| Trading pair such as `BTCUSDT`, trimmed and uppercased | `--symbol <symbol>` | — | required | every command |
| Candle interval: `1s`, `1m`, `3m`, `5m`, `15m`, `30m`, `1h`, `2h`, `4h`, `6h`, `8h`, `12h`, `1d`, `3d`, or `1w`; with `--strategy-name` it also selects the run that `trade close`, `monitor status`, and `monitor watch` act on | `--interval <interval>` | — | `1m` | `data sync`, `data validate`, `backtest`, `trade run`, `trade close`, `monitor status`, `monitor watch` |
| RFC3339 time to start syncing from; omitted, syncing starts at the current time with no backfill | `--start <rfc3339>` | — | none | `data sync` |
| RFC3339 time to stop syncing at; omitted, syncing runs until Ctrl-C | `--end <rfc3339>` | — | none | `data sync` |
| RFC3339 start of the window over stored candles; omitted, from the first one | `--start <rfc3339>` | — | none | `data validate`, `backtest` |
| RFC3339 end of the window over stored candles; omitted, through the last one | `--end <rfc3339>` | — | none | `data validate`, `backtest` |
| Candles requested per Binance klines call | `--limit <count>` | — | `1000` | `data sync`, `trade run` |
| With `--end`, keep polling until the end time is reached instead of stopping after one pass | `--follow` | — | `false` | `data sync` |
| Seconds to wait between polls; at least 1 | `--poll-secs <seconds>` | — | `5` | `data sync`, `trade run`, `monitor watch` |
| Stop after this many loop iterations; omitted, no limit | `--max-loops <count>` | — | none | `data sync`, `trade run`, `monitor watch` |
| Fast SMA period of the `sma_cross` strategy | `--fast <bars>` | — | `20` | `backtest`, `trade run` |
| Slow SMA period of the `sma_cross` strategy | `--slow <bars>` | — | `50` | `backtest`, `trade run` |
| Starting quote cash; a positive decimal | `--cash <decimal>` | — | `10000` | `backtest` |
| Fee per fill in basis points; a decimal of zero or more | `--fee-bps <decimal>` | — | `10` | `backtest` |
| Quote amount spent per entry; a positive decimal | `--quote-order-qty <decimal>` | — | `100` | `trade run` |
| Execution mode: `dry-run` runs the strategy without any trading venue, `live` places real orders | `--mode <mode>` | — | `dry-run` | `trade run` |
| Confirm a live run or an account change; without it the command only previews the action, except that a live `trade run` on an interactive terminal asks instead | `--yes` | — | `false` | `trade run`, `trade close`, `monitor cancel-order`, `monitor close-position` |
| Closed candles loaded to warm up the strategy before the first poll | `--bootstrap-bars <count>` | — | `300` | `trade run` |
| Act on the warm-up signal in the first loop, which may place an order immediately | `--bootstrap-enter` | — | `false` | `trade run` |
| Run to resume; omitted, a new run starts with a flat position | `--run-id <id>` | — | none | `trade run` |
| Run to close; omitted, the latest run for the market and `--strategy-name` | `--run-id <id>` | — | none | `trade close` |
| Strategy whose journaled runs are read | `--strategy-name <name>` | — | `sma_cross` | `trade close`, `monitor status`, `monitor watch` |
| Recent Binance trades shown | `--recent-trades <count>` | — | `10` | `monitor status`, `monitor watch` |
| Recent Binance trades listed | `--limit <count>` | — | `20` | `monitor trades` |
| Exchange order id of the order to cancel; pass it or `--client-order-id` | `--order-id <id>` | — | none | `monitor cancel-order` |
| Client order id of the order to cancel; pass it or `--order-id` | `--client-order-id <id>` | — | none | `monitor cancel-order` |

## Why per-command options have no environment variable

A per-command option describes one invocation. An exported variable would
outlive the command it was meant for, and one that silently changed what a
command does to an account — `--mode live`, `--yes`, a larger
`--quote-order-qty` — is exactly the mistake the guardrails exist to prevent.

## Environment variables QuantForge does not own

These are conventions shared across Rust and Unix tools, read by dependencies
with no QuantForge-specific meaning, so they have no flags:

| Variable | Read by | Effect |
| --- | --- | --- |
| `NO_COLOR` | tracing-subscriber, clap | set and non-empty, removes ANSI colors; log lines carry them even when stdout is not a terminal |
| `CLICOLOR`, `CLICOLOR_FORCE`, `TERM`, `CI` | clap | color choice for help and usage errors |
| `HTTPS_PROXY`, `HTTP_PROXY`, `ALL_PROXY`, `NO_PROXY`, and their lowercase forms | reqwest | route Binance requests through a proxy; all of them are ignored when `REQUEST_METHOD` is set, as under CGI |
| `RUST_BACKTRACE`, `RUST_LIB_BACKTRACE` | Rust standard library, anyhow | add backtraces to panics and error reports |
| `TOKIO_WORKER_THREADS` | tokio | async runtime worker threads; a value that is not a positive integer makes QuantForge panic at startup |
| `SQLITE_TMPDIR`, `TMPDIR` | SQLite | directory for SQLite's temporary files |

`RUST_LOG` is not read: the log filter comes only from `--log-level` or
`QF_LOG`.
