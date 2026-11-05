# Configuration

Every QuantForge setting is a command-line flag. The ten global settings and
the two settings of `serve run` also read a `QF_*` environment variable as a
fallback; every other per-command option is a flag only. QuantForge reads no
other environment variable itself — the few its dependencies honor are listed
at the end.

## Precedence

For each global setting, the first source present wins:

1. the flag, before or after the subcommand — these two are the same:
   `quantforge --db a.sqlite data validate ...` and
   `quantforge data validate ... --db a.sqlite`;
2. its environment variable;
3. the default.

An exported variable is checked even when its flag is given: a value the
setting rejects, such as `QF_EXCHANGE=nosuch`, fails the command until it is
fixed or unset, whatever the flag says.

`serve run`'s settings follow the same order, with two differences: their
flags go after `serve run`, and their variables are read, and checked, only
when the flag is absent, so `QF_LISTEN=nonsense` is ignored beside
`--listen 127.0.0.1:9600`.

A variable that is exported but empty is still set, so it never falls back to
the default:

- `QF_DB=` is a usage error (exit status 2, reported against `--db`);
- `QF_STORAGE=`, `QF_POSTGRES_POOL_SIZE=` and
  `QF_POSTGRES_CONNECT_TIMEOUT_SECS=` are usage errors (exit status 2,
  reported against their flags);
- `QF_POSTGRES_URL=` is a PostgreSQL setting that is set: with `--storage
  postgres` it fails as an unusable URL, and with no backend selected it is
  refused like any other PostgreSQL setting (see [Storage
  backends](#storage-backends));
- `QF_EXCHANGE=` is a usage error (exit status 2, reported against
  `--exchange`);
- `QF_BASE_URL=` fails with `failed to parse --base-url` before the database
  is created, so a missing testnet URL can never quietly become production;
- `QF_LOG=` is an empty filter, which turns log output off, warnings included;
- without its flag, `QF_LISTEN=` is a usage error (exit status 2, reported
  against `--listen`), and so is `QF_SEGMENT_CACHE_DIR=` (reported against
  `--segment-cache-dir`);
- an empty or whitespace-only API key or secret counts as missing, so commands
  that need credentials fail with the missing-credentials error.

## Matrix

<!--
The `configuration_doc_matches_every_flag_env_var_default_and_scope` unit test
in src/cli/tests.rs compares this table with the clap definitions. One flag per
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
| SQLite database path, relative to the working directory; a missing parent directory is created; refused with `--storage postgres` | `--db <path>` | `QF_DB` | `data/market.sqlite` | global |
| Storage backend: `sqlite` or `postgres`; see [Storage backends](#storage-backends) | `--storage <backend>` | `QF_STORAGE` | `sqlite` | global |
| PostgreSQL URL, `postgres://USER:PASSWORD@HOST:PORT/DATABASE` with optional `sslmode` and `sslrootcert`; prefer the variable, since command-line values can land in shell history and process listings, and `--help` never shows its value | `--postgres-url <url>` | `QF_POSTGRES_URL` | none | global |
| Most PostgreSQL connections the process opens; omitted, 2, or 8 for `serve run`; at least 1 | `--postgres-pool-size <count>` | `QF_POSTGRES_POOL_SIZE` | none | global |
| Seconds opening a PostgreSQL connection, TLS included, may take; at least 1 | `--postgres-connect-timeout-secs <seconds>` | `QF_POSTGRES_CONNECT_TIMEOUT_SECS` | `5` | global |
| Exchange to connect to, by its registry name; `binance` (Binance Spot) is the only one, and output names it by its storage name, `binance_spot` | `--exchange <name>` | `QF_EXCHANGE` | `binance` | global |
| Exchange API base URL; omitted, the exchange's production endpoint — `https://api.binance.com/` for `binance`, whose Spot testnet is `https://testnet.binance.vision/` | `--base-url <url>` | `QF_BASE_URL` | none | global |
| Log filter: `trace`, `debug`, `info`, `warn`, `error`, or a full tracing filter expression; log lines go to stdout | `--log-level <filter>` | `QF_LOG` | `info` | global |
| API key for the exchange's signed endpoints (`monitor`, `trade close`, `trade run --mode live`); set both credentials or neither, and prefer the variable, since command-line values can land in shell history and process listings | `--api-key <key>` | `QF_API_KEY` | none | global |
| API secret paired with the key; same rules | `--api-secret <secret>` | `QF_API_SECRET` | none | global |
| Trading pair such as `BTCUSDT`, trimmed and uppercased | `--symbol <symbol>` | — | required | `data sync`, `data validate`, `backtest`, `trade run`, `trade close`, `monitor status`, `monitor watch`, `monitor orders`, `monitor trades`, `monitor cancel-order`, `monitor close-position` |
| Candle interval: `1s`, `1m`, `3m`, `5m`, `15m`, `30m`, `1h`, `2h`, `4h`, `6h`, `8h`, `12h`, `1d`, `3d`, or `1w`; with `--strategy-name` it also selects the run that `trade close`, `monitor status`, and `monitor watch` act on | `--interval <interval>` | — | `1m` | `data sync`, `data validate`, `backtest`, `trade run`, `trade close`, `monitor status`, `monitor watch` |
| RFC3339 time to start syncing from; omitted, syncing starts at the current time with no backfill | `--start <rfc3339>` | — | none | `data sync` |
| RFC3339 time to stop syncing at; omitted, syncing runs until Ctrl-C | `--end <rfc3339>` | — | none | `data sync` |
| RFC3339 start of the window over stored candles; omitted, from the first one | `--start <rfc3339>` | — | none | `data validate`, `backtest` |
| RFC3339 end of the window over stored candles; omitted, through the last one | `--end <rfc3339>` | — | none | `data validate`, `backtest` |
| Candles requested per klines call, capped at the exchange's page size | `--limit <count>` | — | `1000` | `data sync`, `trade run` |
| With `--end`, keep polling until the end time is reached instead of stopping after one pass | `--follow` | — | `false` | `data sync` |
| Seconds to wait between polls; at least 1 | `--poll-secs <seconds>` | — | `5` | `data sync`, `trade run`, `monitor watch` |
| Stop after this many loop iterations; omitted, no limit | `--max-loops <count>` | — | none | `data sync`, `trade run`, `monitor watch` |
| Fast SMA period of the `sma_cross` strategy | `--fast <bars>` | — | `20` | `backtest`, `trade run` |
| Slow SMA period of the `sma_cross` strategy | `--slow <bars>` | — | `50` | `backtest`, `trade run` |
| Starting quote cash; a positive decimal | `--cash <decimal>` | — | `10000` | `backtest` |
| Fee per fill in basis points; a decimal of zero or more | `--fee-bps <decimal>` | — | `10` | `backtest` |
| Quote amount spent per entry; a positive decimal | `--quote-order-qty <decimal>` | — | `100` | `trade run` |
| Execution mode: `dry-run` runs the strategy without any trading venue, `live` places real orders | `--mode <mode>` | — | `dry-run` | `trade run` |
| Confirm a live run, an account change, or a change to the store's record; without it the command only previews the action, except that a live `trade run` on an interactive terminal asks instead | `--yes` | — | `false` | `trade run`, `trade close`, `monitor cancel-order`, `monitor close-position`, `store bind`, `store adopt-run` |
| Closed candles loaded to warm up the strategy before the first poll | `--bootstrap-bars <count>` | — | `300` | `trade run` |
| Act on the warm-up signal in the first loop, which may place an order immediately | `--bootstrap-enter` | — | `false` | `trade run` |
| Run to resume; omitted, a new run starts with a flat position | `--run-id <id>` | — | none | `trade run` |
| Run to close; omitted, the latest run for the market and `--strategy-name` | `--run-id <id>` | — | none | `trade close` |
| Legacy run to adopt, as `store status` lists it | `--run-id <id>` | — | `required` | `store adopt-run` |
| Execution mode of a legacy run that recorded none (QuantForge 0.2.0 wrote none); refused for a run that did | `--execution-mode <mode>` | — | none | `store adopt-run` |
| Environment the store is bound to for life: `mainnet`, `testnet`, or `demo`; must agree with the endpoint `--base-url` reaches | `--environment <environment>` | — | `required` | `store bind` |
| Strategy whose journaled runs are read | `--strategy-name <name>` | — | `sma_cross` | `trade close`, `monitor status`, `monitor watch` |
| Recent account trades shown | `--recent-trades <count>` | — | `10` | `monitor status`, `monitor watch` |
| Recent account trades listed | `--limit <count>` | — | `20` | `monitor trades` |
| Exchange order id of the order to cancel, as the exchange spells it; pass it or `--client-order-id` | `--order-id <id>` | — | none | `monitor cancel-order` |
| Client order id of the order to cancel; pass it or `--order-id` | `--client-order-id <id>` | — | none | `monitor cancel-order` |
| Address to listen on: an IP address and a port, such as `0.0.0.0:9600` for every interface; a host name is refused; the default accepts connections from this machine only, and port 0 picks a free port, which the `listening:` line shows | `--listen <addr>` | `QF_LISTEN` | `127.0.0.1:9600` | `serve run` |
| Directory for cached candle segments, relative to the working directory; created at startup if missing | `--segment-cache-dir <path>` | `QF_SEGMENT_CACHE_DIR` | `data/segments` | `serve run` |

## Storage backends

`--storage` selects where candles, journals and segments are kept:

- **`sqlite`**, the default: one file at `--db`. Nothing about SQLite
  changes when no backend is selected.
- **`postgres`**: the database `--postgres-url` names. This release checks
  the PostgreSQL settings and refuses the command (`postgres storage has no
  adapter yet`); a later release adds the PostgreSQL stores. A build
  compiled without the `postgres` cargo feature, which is on by default,
  refuses it as `not in this build`.

A selection is never guessed and never replaced. These are refused before
anything is created:

- a PostgreSQL setting (`--postgres-url`, `--postgres-pool-size`,
  `--postgres-connect-timeout-secs` or their variables) while no backend is
  selected, since the command would quietly use SQLite. With `--storage
  sqlite` they are ignored;
- `--db` or `QF_DB` with `--storage postgres`;
- `--storage postgres` without a URL.

PostgreSQL never falls back to SQLite, and never creates a SQLite file or
directory.

### The PostgreSQL URL

The URL has libpq's form, `postgres://USER:PASSWORD@HOST:PORT/DATABASE`
(`postgresql://` works too). One host, a user and a database are required;
the port defaults to 5432; percent-encode `@`, `:`, `/` and `%` inside the
user, password or database. QuantForge reads two query parameters and
refuses every other one, so a setting is never dropped in silence:

| `sslmode` | This machine (`localhost`, `127.0.0.0/8`, `::1`) | Any other host |
| --- | --- | --- |
| absent | TLS if offered | TLS, verified |
| `disable` | plain text | refused |
| `prefer` | TLS if offered | refused |
| `require`, `verify-ca`, `verify-full` | TLS, verified | TLS, verified |

"Verified" checks the certificate chain against the system's trust store
and the host name, `verify-ca` included. `sslrootcert` names a CA file to
trust as well, for a provider whose CA the system lacks.

The password belongs in `QF_POSTGRES_URL`, set by whatever starts
QuantForge (a service manager, a container runtime, a shell profile with
restricted permissions). No error, log line or `--help` output shows it, or
the URL that carries it: the database appears as
`postgres://HOST:PORT/DATABASE`. A password passed with `--postgres-url` on
an interactive terminal logs a warning. libpq's variables (`PGHOST`,
`PGPASSWORD` and the rest) and `~/.pgpass` are never read.

### Connection limits

The pool opens at most `--postgres-pool-size` connections: 2 by default,
which is enough for a command, and 8 for `serve run`. Budget the database's
`max_connections` across the processes that share it. The other limits are
fixed:

| Limit | Value |
| --- | --- |
| Waiting for a free connection | 5 s, then the call fails as unavailable |
| Waiting for a lock (`lock_timeout`) | 5 s, then the call fails as busy |
| A statement on the server (`statement_timeout`) | 30 s |
| One call, end to end | 35 s, then the query is cancelled on the server |
| Idle time before TCP keepalives probe a connection | 30 s |

### Durable commits

A commit is flushed before the store answers when the store has the settings
that promise it: SQLite in WAL with `synchronous=FULL` and `fullfsync`, and
PostgreSQL committing with `synchronous_commit` at `local` or stronger, on a
server with `fsync` and `full_page_writes` on. `trade run --mode live` and
`trade close` refuse a store without them, before any request; other commands
log a warning and run, and their commits then carry no such promise.
`quantforge store status` prints `durability: met`, or `durability: weakened`,
with what it read back.
[storage-durability.md](storage-durability.md) is the contract: what a
commit survives, what it assumes of the disk, and what each failure means.

## Renamed settings

The exchange settings lost their Binance prefix when `--exchange` arrived:

| Old | New |
| --- | --- |
| `--binance-base-url`, `QF_BINANCE_BASE_URL` | `--base-url`, `QF_BASE_URL` |
| `--binance-api-key`, `QF_BINANCE_API_KEY` | `--api-key`, `QF_API_KEY` |
| `--binance-api-secret`, `QF_BINANCE_API_SECRET` | `--api-secret`, `QF_API_SECRET` |

The old base-URL flag and variable are still recognized, but only to be
refused (exit status 2, naming the new name), because a testnet URL exported
under the old name must not quietly leave the production default in charge.
The old credential flags are unknown arguments, and the old credential
variables are no longer read, so a command that needs credentials fails with
the missing-credentials error, which names the new ones.

## Why most per-command options have no environment variable

A per-command option describes one invocation. An exported variable would
outlive the command it was meant for, and one that silently changed what a
command does to an account — `--mode live`, `--yes`, a larger
`--quote-order-qty` — is exactly the mistake the guardrails exist to prevent.

`serve run`'s settings are the exception. A server is set up once for the
place it runs, often by a service manager or container that configures it
through its environment, and neither of its settings can touch an account.

## Environment variables QuantForge does not own

These are conventions shared across Rust and Unix tools, read by dependencies
with no QuantForge-specific meaning, so they have no flags:

| Variable | Read by | Effect |
| --- | --- | --- |
| `NO_COLOR` | tracing-subscriber, clap | set and non-empty, removes ANSI colors; log lines carry them even when stdout is not a terminal |
| `CLICOLOR`, `CLICOLOR_FORCE`, `TERM`, `CI` | clap | color choice for help and usage errors |
| `HTTPS_PROXY`, `HTTP_PROXY`, `ALL_PROXY`, `NO_PROXY`, and their lowercase forms | reqwest | route exchange requests through a proxy; all of them are ignored when `REQUEST_METHOD` is set, as under CGI |
| `RUST_BACKTRACE`, `RUST_LIB_BACKTRACE` | Rust standard library, anyhow | add backtraces to panics and error reports |
| `TOKIO_WORKER_THREADS` | tokio | async runtime worker threads; a value that is not a positive integer makes QuantForge panic at startup |
| `SQLITE_TMPDIR`, `TMPDIR` | SQLite | directory for SQLite's temporary files |
| `SSL_CERT_FILE`, `SSL_CERT_DIR` | rustls-native-certs | replace the system's trust store for PostgreSQL's TLS |

`RUST_LOG` is not read: the log filter comes only from `--log-level` or
`QF_LOG`.
