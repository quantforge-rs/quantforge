# Configuration

Every QuantForge setting is a command-line flag. The ten global settings, the
seven settings of `serve run` and the token of `backtest --remote` also read a
`QF_*` environment variable as a fallback; every other per-command option is a
flag only. A job's ID, the one argument that is no flag, is not a setting and
is not listed below. QuantForge reads no
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

`serve run`'s settings and `--remote-token` follow the same order, with two
differences: their flags go after their command, and their variables are
read, and checked, only when the flag is absent, so `QF_LISTEN=nonsense` is
ignored beside `--listen 127.0.0.1:9600`.

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
  against `--listen`), and so are `QF_MAX_SPAN_BARS=`,
  `QF_JOB_CONCURRENCY=`, `QF_JOB_CPU_SECS=`, `QF_JOB_MEMORY_MIB=`,
  `QF_JOB_WALL_SECS=` and `QF_JOB_PYTHON=` (each reported against its
  flag);
- an empty or whitespace-only API key or secret counts as missing, so commands
  that need credentials fail with the missing-credentials error;
- an empty or whitespace-only `--remote-token` or `QF_REMOTE_TOKEN` counts as
  no token, and an empty flag never falls back to the variable.

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
| Most PostgreSQL connections the process opens; omitted, 2, or 8 for `serve run`; from 1 to 262143, the most a PostgreSQL server serves | `--postgres-pool-size <count>` | `QF_POSTGRES_POOL_SIZE` | none | global |
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
| SQLite database to copy into the selected PostgreSQL store, opened read-only and never changed; it must be at the schema this build writes | `--from <path>` | — | `required` | `data migrate` |
| After copying, compare every row of the copy with the source; any difference is listed and exits with status 1 | `--verify` | — | `false` | `data migrate` |
| Rows read and written at a time, at least 1; a batch holds at most 64 segments | `--batch-size <rows>` | — | `5000` | `data migrate` |
| Copy although the source records runs still starting or running, once you have checked that nothing writes to it: a process that died leaves its run running | `--assume-quiescent` | — | `false` | `data migrate` |
| Fast SMA period of the `sma_cross` strategy | `--fast <bars>` | — | `20` | `backtest`, `trade run` |
| Slow SMA period of the `sma_cross` strategy | `--slow <bars>` | — | `50` | `backtest`, `trade run` |
| Starting quote cash; a positive decimal | `--cash <decimal>` | — | `10000` | `backtest` |
| Fee per fill in basis points; a decimal of zero or more | `--fee-bps <decimal>` | — | `10` | `backtest` |
| QuantForge server to run the backtest on, `http://` or `https://` with a host, such as `http://127.0.0.1:9600`, optionally under a path: it runs the `--strategy` bundle on the candles it stores, within the same `--start` and `--end` a local run reads, and the report prints here. It needs `--strategy`, and refuses `--fast` and `--slow`; nothing local is opened. See [Remote backtests](#remote-backtests) | `--remote <url>` | — | none | `backtest` |
| QuantForge server that holds the job | `--remote <url>` | — | `required` | `backtest jobs status` |
| Token sent to the `--remote` server with every request, as `Authorization: Bearer`; no server checks one yet. Prefer the variable, since command-line values can land in shell history and process listings, and `--help` never shows its value; it goes only over `https`, or over `http` to this machine and never through a proxy then, and is refused before any request otherwise; with a token, no redirect is followed | `--remote-token <token>` | `QF_REMOTE_TOKEN` | none | `backtest`, `backtest jobs status` |
| Python strategy to run on `--remote`: a `.py` file, bundled beside a `quantforge.toml` written from `--entry` and `--params`, or a directory, bundled as the [bundle spec](bundle-spec.md) collects one, with its own `quantforge.toml` if it holds one | `--strategy <path>` | — | none | `backtest` |
| The strategy's file and class, `path/to/file.py:ClassName`, for a `--strategy` without a `quantforge.toml`; refused beside one | `--entry <file:class>` | — | none | `backtest` |
| The strategy's parameters, as the body of a TOML inline table, such as `fast=20,slow=50` or `name="sma"`, for a `--strategy` without a `quantforge.toml`; refused beside one | `--params <pairs>` | — | none | `backtest` |
| Print the job's ID once the server has queued it, and leave without following it | `--no-wait` | — | `false` | `backtest` |
| Follow the job's log until the job ends, then print its report; Ctrl-C leaves the job running | `--follow` | — | `false` | `backtest jobs status` |
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
| Most bars one `/v1/candles` answer or one backtest job may span, counted at the range's interval; at least 1, and a longer range is refused as `invalid_request`. The default is a leap year of `1m` bars | `--max-span-bars <count>` | `QF_MAX_SPAN_BARS` | `527040` | `serve run` |
| Backtest jobs that run at once; the others wait in the queue, the first submitted first. At least 1; omitted, half the machine's CPU cores, at least 1, which the `job_concurrency:` line shows. The server keeps a worker process started for each | `--job-concurrency <count>` | `QF_JOB_CONCURRENCY` | none | `serve run` |
| CPU time one backtest job may use, in seconds, at least 1; a job that uses it all is stopped and fails as `invalid_request`, saying so. Enforced on Linux and macOS; see [worker-isolation.md](worker-isolation.md) | `--job-cpu-secs <secs>` | `QF_JOB_CPU_SECS` | `600` | `serve run` |
| Memory one backtest job may use, in MiB of address space, at least 1; a job that needs more is stopped and fails as `invalid_request`, saying so. Enforced on Linux only | `--job-memory-mib <mib>` | `QF_JOB_MEMORY_MIB` | `4096` | `serve run` |
| Wall-clock time one backtest job may run, in seconds, at least 1; the server then kills its worker, and the job fails as `invalid_request`, saying so. The `job_limits:` line shows all three | `--job-wall-secs <secs>` | `QF_JOB_WALL_SECS` | `1800` | `serve run` |
| Python interpreter that runs each job's bundle: one with `quantforge-py` of this release installed, such as a virtualenv's `bin/python`, with the packages bundles may import (NumPy, pandas) installed beside it; a path that is no file is refused before anything is opened. Omitted, the server runs no bundle, and each job fails as it starts, `unavailable`. The `job_runtime:` line shows what its workers offer; see [python-worker.md](python-worker.md) | `--job-python <path>` | `QF_JOB_PYTHON` | none | `serve run` |

## Storage backends

`--storage` selects where candles, journals and segments are kept:

- **`sqlite`**, the default: one file at `--db`. Nothing about SQLite
  changes when no backend is selected.
- **`postgres`**: the database `--postgres-url` names, for every command:
  candles, segments, runs and the store's own record all live there. A build
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
directory. A command that cannot reach its database fails, with exit status
1, naming the database and why: the server refused the URL's user or
password, the database does not exist, or what stopped the connection.

Every command logs which store it uses, and `store status` and `serve run`
print it, as `storage: sqlite data/market.sqlite` or `storage: postgres
postgres://HOST:PORT/DATABASE`, never with a user or password.

**The Python package's candle reader selects its store by these rules,**
applied by the same code: its arguments are these flags by name, an
argument left out is read from the flag's variable, and with nothing set it
is SQLite at `data/market.sqlite` ([python-reader.md](python-reader.md)).
It differs from a command in one thing: it opens the store read-only, so it
creates no file and no schema, and brings no earlier schema forward.

### The PostgreSQL database

The database must exist, and hold QuantForge alone: the first command that
opens it creates the schema, in the role's default schema, and logs a
warning that it did, in case the URL named the wrong one. A schema that
holds tables of the same names but no QuantForge version is refused,
untouched.

The role needs no superuser rights, only the right to create tables there.
PostgreSQL 15 and later give that on `public` to a database's owner alone,
so the simplest setup is a database the role owns:

```bash
createdb --owner=quant market
```

One database is one store, as one `--db` file is: keep testnet and production
in separate databases, and bind each with `store bind`. Nothing moves from a
SQLite file on its own, so a new database starts empty; `data migrate --from
<file>` copies one into it ([storage-migration.md](storage-migration.md)).

`serve run` serves history from the selected store, segments included, and
keeps nothing beside it on the local disk.

### The PostgreSQL URL

The URL has libpq's form, `postgres://USER:PASSWORD@HOST:PORT/DATABASE`
(`postgresql://` works too). One host, a user and a database are required;
the port defaults to 5432; percent-encode `@`, `:`, `/` and `%` inside the
user, password or database. A database name with an `@` that is not
percent-encoded is refused: a password whose `@` and `/` were left as they
are leaves the rest of itself there, where the database's name is shown. QuantForge reads two query parameters and
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
through its environment, and none of its settings can touch an account.
`--remote-token` is the other: it is a secret, which belongs in the
environment rather than on a command line, and it touches no account.
`--remote` itself has no variable, so an exported one can never send a
backtest elsewhere unasked.

## Remote backtests

`backtest --remote <url> --strategy <path>` runs a Python strategy bundle
on a QuantForge server and prints the report a local run prints. The
client asks the server, in order, which protocol versions it speaks
(`GET /version`), what its workers run bundles on (`GET /v1/runtime`), and
which bars it stores of the market (`GET /v1/markets`); then it uploads
the bundle under the code hash it computed, submits the job, follows the
job's log, and downloads the result, checking each file against the size
and hash the server listed. Every flag is parsed and the bundle checked
against the [bundle spec](bundle-spec.md) before the first request.

- **The bundle.** A directory with its own `quantforge.toml` is bundled as
  it is, and `--entry` and `--params` are refused beside it. A `.py` file,
  or a directory without one, is bundled beside a manifest written from
  `--entry` and `--params`, whose `python` is the server's and whose
  `engine` is `==` this build's version, so the server runs the engine a
  local run would; a server running another engine refuses it, saying so.
- **The candles.** `--start` and `--end` mean what they mean locally: the
  bars the server stores between them, each one left out reaching the
  first or the last stored bar. A window outside the bars the server stores
  is refused before anything is uploaded; one that falls in a gap between
  them fails its job, with the server's message, as a local run over no
  candles fails.
- **The output.** `remote:`, `range:`, `bundle:` and `job_id:` come first,
  then the job's log as it runs: `log:` lines, with the level before the
  text unless it is `info`, and `progress:` at most once a tenth of the
  bars. Then `state:` and the report, whose first line names the strategy
  by its entry, as in `strategy: sma_cross.py:SmaCross`, since the server
  keeps no strategy name with a job; every other line is the local report's.
- **Leaving.** `--no-wait` prints `job_id:` and leaves. Ctrl-C while the log
  is followed leaves too, and the job keeps running: `quantforge backtest
  jobs status <job id> --remote <url>` prints where it stands and, once it
  has succeeded, the report, with the strategy named by its bundle's code
  hash; `--follow` follows its log to its end first.
- **Failures.** A server that cannot be reached, that is not QuantForge,
  or that speaks no protocol version this build speaks is named as such;
  a refusal prints the server's own message word for word, and a job that
  failed prints its error; each exits with status 1.

## Environment variables QuantForge does not own

These are conventions shared across Rust and Unix tools, read by dependencies
with no QuantForge-specific meaning, so they have no flags:

| Variable | Read by | Effect |
| --- | --- | --- |
| `NO_COLOR` | tracing-subscriber, clap | set and non-empty, removes ANSI colors; log lines carry them even when stdout is not a terminal |
| `CLICOLOR`, `CLICOLOR_FORCE`, `TERM`, `CI` | clap | color choice for help and usage errors |
| `HTTPS_PROXY`, `HTTP_PROXY`, `ALL_PROXY`, `NO_PROXY`, and their lowercase forms | reqwest | route exchange requests and `--remote` requests through a proxy; all of them are ignored when `REQUEST_METHOD` is set, as under CGI |
| `RUST_BACKTRACE`, `RUST_LIB_BACKTRACE` | Rust standard library, anyhow | add backtraces to panics and error reports |
| `TOKIO_WORKER_THREADS` | tokio | async runtime worker threads; a value that is not a positive integer makes QuantForge panic at startup |
| `SQLITE_TMPDIR`, `TMPDIR` | SQLite | directory for SQLite's temporary files |
| `TMPDIR`; `TMP`, `TEMP` on Windows | Rust standard library | where `serve run` makes the directory that holds its job workers' scratch directories |
| `SSL_CERT_FILE`, `SSL_CERT_DIR` | rustls-native-certs | replace the system's trust store for PostgreSQL's TLS |

`RUST_LOG` is not read: the log filter comes only from `--log-level` or
`QF_LOG`.
