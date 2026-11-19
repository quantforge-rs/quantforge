# Continuous integration

What a pull request to `main` must pass, what each check runs, and how to
run the same locally. The checks live in `.github/workflows/ci.yml`. CodeQL,
gitleaks and CodeRabbit check every pull request beside them, and the
conformance suite runs live against the exchanges on a schedule
([CONTRIBUTING.md](../CONTRIBUTING.md#the-conformance-suite)).

## Contents

- [The checks](#the-checks)
- [Required checks](#required-checks)
- [PostgreSQL](#postgresql)
- [The Python package](#the-python-package)
- [Running them locally](#running-them-locally)
- [How long they take](#how-long-they-take)
- [What they are evidence for](#what-they-are-evidence-for)

## The checks

| Check | Runs on | What it runs |
| --- | --- | --- |
| `checks (ubuntu-latest)`, `checks (macos-latest)`, `checks (windows-latest)` | Each platform | The file-size budget (Linux only), `cargo fmt`, then clippy and the tests twice: with every feature, and as the SQLite-only build (`--no-default-features`), which holds no PostgreSQL code. No server is given here, so the PostgreSQL tests skip |
| `package` | Linux | `cargo check` with Rust 1.85, the oldest the crate declares (`rust-version`), with and without the default features; then `cargo publish --dry-run` |
| `postgres 16`, `postgres 17`, `postgres 18` | Linux | The whole suite against each PostgreSQL major [ADR 0001](adr/0001-postgres-client-strategy.md) supports: see [PostgreSQL](#postgresql) |
| `storage` | Linux | Passes only when every `postgres` leg passed |
| `python` | Linux | The Python package, `quantforge-py/`: clippy for its crate with and without the default features, `cargo check` with Rust 1.85, then the wheel maturin builds, installed with its extras, NumPy and pandas, and tested on Python 3.11 and 3.15, on the oldest NumPy and the oldest pandas the extras take, with NumPy alone, and without the extras, the SQLite-only wheel, and the in-place install; the `quantforge` binary is built for the candle reader's tests, which read stores it wrote, on SQLite and on the job's PostgreSQL server: see [The Python package](#the-python-package) |

Every check runs on every pull request; none has a path filter. A pull
request that changes only documentation can still break the PostgreSQL
legs, which compare the server's catalog to the DDL in
[storage-schema.md](storage-schema.md).

Two jobs run on larger runners of the organization, where every other job
runs on GitHub's standard runners:

- `checks (windows-latest)`, the slowest leg, runs on `windows-latest-l`,
  with 8 cores. The check keeps its name: the job names its checks after
  the platform (`name: checks (${{ matrix.os }})`), and the matrix gives
  Windows its runner beside it. Taking the `include` entry out of the
  matrix puts the leg back on `windows-latest`.
- `analyze`, CodeQL's job in `.github/workflows/codeql.yml`, runs on
  `ubuntu-latest-l`, with 8 cores and 32 GB of memory. The check is named
  after the job, so its name does not depend on the runner. Setting
  `runs-on` back to `ubuntu-latest` puts the job back on the standard
  runner.

Both runners belong to the organization's `Default Larger Runners` group,
which must allow public repositories; where it does not, or in a copy of
the repository without such runners, these jobs wait for a runner that
never comes.

## Required checks

Branch protection on `main` requires these checks by name:

- `checks (ubuntu-latest)`, `checks (macos-latest)`, `checks (windows-latest)`
- `package`
- `storage`, and `postgres 16`, `postgres 17` and `postgres 18` beside it
- `python`
- `analyze` and `CodeQL`, from `.github/workflows/codeql.yml`
- `scan` and `gitleaks`, from `.github/workflows/gitleaks.yml`
- `CodeRabbit`

A maintainer adds a check to them once, by its name:

```bash
gh api --method POST \
  repos/quantforge-rs/quantforge/branches/main/protection/required_status_checks/contexts \
  -f 'contexts[]=python'
```

**`python` is required for the golden parity test,** which runs in it: an
SMA cross written in Python has to make the trades, the final equity and
the drawdown of the Rust one's frozen run, digit for digit
([golden-parity.md](golden-parity.md)). The crate's half of the goldens,
the engine and the Rust strategy held to the same files, runs in the three
`checks` legs.

**`storage` stands for the `postgres` legs.** It passes only when every leg
passed, and it runs even when a leg failed or was cancelled
(`if: always()`), because GitHub counts a skipped required check as passed.
With `storage` alone required, a major added to the matrix or dropped from
it would need no change to branch protection. Branch protection names the
three legs as well, so a major that is dropped has to be taken out of
branch protection in the same change: a required check that no job reports
is waited for forever.

A renamed job is waited for the same way: every merge waits until its new
name replaces the old one in branch protection. That is why the runner of
a `checks` leg is changed through the matrix's `runner`, never by changing
the `os` its check is named after.

## PostgreSQL

Each `postgres` leg starts two official `postgres:<major>` images as service
containers. `scripts/postgres-test-servers.sh configure` then sets them up:

- **The server** serves TLS to `localhost`, with a certificate from a CA made
  for the run.
- **The weak server** has `fsync = off` in its configuration file. The
  durability tests show its refusal, turn `fsync` on with `ALTER SYSTEM`, and
  reset it.

Then `cargo test --all-features --locked --no-fail-fast` runs every test,
the SQLite ones included, with these variables:

| Variable | Value |
| --- | --- |
| `QUANTFORGE_TEST_POSTGRES_URL` | The server over plain TCP (`sslmode=disable`), on which each test creates a database of its own |
| `QUANTFORGE_TEST_POSTGRES_TLS_URL` | The same server over TLS: `sslmode=verify-full`, with the run's CA as `sslrootcert` |
| `QUANTFORGE_TEST_POSTGRES_WEAK_URL` | The weak server |
| `QUANTFORGE_TEST_POSTGRES_REQUIRED` | `1` |

**`QUANTFORGE_TEST_POSTGRES_REQUIRED=1` keeps a PostgreSQL failure from
passing as a skip.** Without it, a test that lacks its server skips, as on
the `checks` legs and on a machine with no server. With it, the test fails
instead. The same goes for a test that needs what a server
lacks, such as an `en_US.UTF-8` locale or a password in the URL. A test
finds its server through `server_url()` or `url()` in
`tests/storage/postgres/server.rs`, or `database_url()` in
`src/storage/postgres/tests/database.rs`, so the variable holds for every
test; a test that read the environment itself would skip in CI unseen.

**Selecting PostgreSQL never falls back to SQLite**, as these tests show on
every pull request:
- in each `postgres` leg, `a_connection_the_server_refuses_says_why_and_never_falls_back`
  and `a_file_at_the_default_sqlite_path_is_never_opened`
  (`tests/e2e/backends.rs`), and every command scenario on PostgreSQL, which
  ends by finding no SQLite file;
- in the SQLite-only lines of `checks`, the settings test that refuses
  PostgreSQL as not built (`src/storage/settings/tests/`).

**Runs leave nothing behind.** Each test creates what it uses, databases
and roles, under a name of its own (the process ID and a count), and drops
it when it ends. So tests run in parallel, and a run can follow another on
the same server. After the tests, `scripts/postgres-test-servers.sh
leftovers` fails the leg if a server holds any database or role beyond the
image's own.

Two things stay off CI:
- **The server-crash test** (`crash::postgres::acknowledged_writes_survive_the_server_crashing`)
  stops the server with `pg_ctl` and needs its data directory, so it runs on
  demand ([storage-conformance.md](storage-conformance.md#postgresql)).
- **PostgreSQL on macOS and Windows:** GitHub runs service containers on
  Linux only.

## The Python package

The `python` job checks [the Python package](python-package.md), whose
crate the other jobs do not build: a cargo command at the repository's root
acts on the `quantforge` crate alone. In `checks`, `cargo fmt --all` and
the file-size budget read it, and neither builds it.

The job is one script, `scripts/python-package-check.sh python3.11
python3.15`, which runs, in order:

- **Clippy for the crate**, with and without the default features, and
  `cargo check` with Rust 1.85.
- **The `quantforge` binary**: `cargo build`, since the tests of the candle
  reader read stores the command line itself wrote. The script tells every
  run below where it is, in `QUANTFORGE_PY_TEST_CLI`.
- **The wheel**: `maturin build` once, then the tests against it in a fresh
  virtualenv of each Python named, where it is installed with its two
  extras, as `pip install "quantforge-py[numpy,pandas]"` installs it.
  Nothing is rebuilt between them: the package promises one wheel for
  Python 3.11 and every later version, and the two are the oldest it
  supports and the newest released. The newest is raised by hand as a new
  Python is released.
- **The wheel on the oldest NumPy and the oldest pandas** the extras take,
  2.0.0 and 2.2.2, each of which `pyproject.toml` pins in a dependency
  group, with `QUANTFORGE_PY_TEST_OLDEST_NUMPY=1` and
  `QUANTFORGE_PY_TEST_OLDEST_PANDAS=1` telling the tests so. Every other
  run with them has the newest `pip` finds for its Python. Neither Python
  named here has a wheel of both the oldest and the newest NumPy: NumPy 2.0
  and pandas 2.2.2 run on Python up to 3.12, and the newest NumPy from 3.12
  on. So this run is on the first Python named, where that is the oldest
  the package supports, as it is here: the script says so where it is not,
  and tests the two oldest nowhere.
- **The wheel with its `numpy` extra alone**, as
  `pip install "quantforge-py[numpy]"` installs it, on the first Python
  named, with `QUANTFORGE_PY_TEST_WITHOUT_PANDAS=1` telling the tests that
  there is no pandas. The tests of the frames are skipped there, and every
  other test, those of the arrays among them, holds the package to working
  with NumPy and without pandas.
- **The wheel without its extras**, as `pip install quantforge-py` installs
  it, on the first Python named, with `QUANTFORGE_PY_TEST_WITHOUT_NUMPY=1`
  and `QUANTFORGE_PY_TEST_WITHOUT_PANDAS=1` telling the tests that there is
  no NumPy and no pandas. The tests of the arrays and of the frames are
  skipped there, and every other test holds the package to working without
  either.
- **The SQLite-only wheel** (`--no-default-features`), with
  `QUANTFORGE_PY_TEST_SQLITE_ONLY=1` telling the tests which build it is.
- **The in-place install**: `maturin develop --extras numpy,pandas` in a
  virtualenv, the import, and the tests again, as a contributor runs them.

The tests are the package's whole suite each time, but for the tests of
the frames where there is no pandas, and of the arrays too where there is
no NumPy. The golden parity test is among them
([golden-parity.md](golden-parity.md)), so a strategy written in Python is
held to the Rust one's frozen results on the oldest Python and on the
newest, without the extras, and in the SQLite-only wheel as well. A run
that is not what the tests were told, one with a NumPy or a pandas that was
said to have none or one without that was not, stops before its first test.

**The job has a PostgreSQL server,** one service container of
`postgres:18`, for the tests of the candle reader
([python-reader.md](python-reader.md)): each writes a store with the
binary, a SQLite file and a database of its own on the server, and reads
both from Python. The step sets `QUANTFORGE_TEST_POSTGRES_URL`, the
variable the crate's tests read, and `QUANTFORGE_TEST_POSTGRES_REQUIRED=1`,
so a test that could not reach the server fails where it would otherwise
skip. The SQLite-only wheel opens no PostgreSQL store, and its run skips
those tests whatever it is told. The server is the newest major of the
three the `postgres` legs cover: the reader's statements are the store's
own, which those legs run on every major, and this job holds the Python
side of them.

maturin, pytest, NumPy and pandas come from PyPI, at the versions
`pyproject.toml` asks for, and NumPy and pandas as wheels or not at all:
the script installs them into every virtualenv that has them itself, the
in-place install's among them, and compiles neither. The wheels are built
in the `dev` profile, on Linux only: release wheels for Linux, macOS and
Windows are QUA-474's.

## Running them locally

The same commands as CI:

```bash
# checks
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo clippy --all-targets --no-default-features -- -D warnings
cargo test --all-features --locked
cargo test --no-default-features --locked

# package, after `rustup toolchain install 1.85`
cargo +1.85 check --locked --all-targets
cargo +1.85 check --locked --all-targets --no-default-features
CARGO_TARGET_DIR=target/publish cargo publish --dry-run --locked

# postgres 16, 17 or 18, with Docker
eval "$(scripts/postgres-test-servers.sh start 16)"
cargo test --all-features --locked --no-fail-fast
scripts/postgres-test-servers.sh leftovers
scripts/postgres-test-servers.sh stop

# python, with CPython 3.11 or later; name each interpreter to test on.
# with the variables of `start` exported, the reader's tests run on
# postgresql too
scripts/python-package-check.sh python3.11 python3.15
```

`start` runs the two servers on `127.0.0.1:54329` and `127.0.0.1:54330`, set
up as in CI, and prints the four variables to export. A server of your own
works too, such as the Homebrew one in
[storage-conformance.md](storage-conformance.md#postgresql).

`./scripts/release-check.sh` runs the `checks` and `package` lines in one go,
and the PostgreSQL tests too when the variables are exported. It gives the
publish dry run a target directory of its own: the dry run's verify build
would otherwise leave `target/debug`'s binary pointing at the packaged copy
of the sources, and later test runs would run that copy. It also fails if
the crate's package list names a file of `quantforge-py/`. It does not run
the `python` job's script: the crate is released without the Python
package's tools.

`scripts/python-package-check.sh` without arguments tests on `python3`
alone, with its extras, with NumPy alone and without either, and on the
oldest NumPy and the oldest pandas only where that `python3` is Python
3.11.
It makes its virtualenvs with `python -m venv`, which on Debian and Ubuntu
needs the `python3-venv` package.

## How long they take

The PostgreSQL legs, recorded on 2026-11-07 on an Apple M2 Max with Docker
Desktop and a warm build, 934 tests each:

| Major | Image | Servers up | `cargo test` |
| --- | --- | --- | --- |
| 16 | 16.15 | 2.5 s | 40 s |
| 17 | 17.11 | 2.8 s | 38 s |
| 18 | 18.6 | 2.8 s | 38 s |

At macOS's background priority, where timers run late, the suite took
102 s on 16.

## What they are evidence for

The [parity matrix](parity/README.md) names this CI (as ALG-411) among the
evidence for these capabilities. The release gate, ALG-769, turns runs into
records in `evidence.jsonl`.

| Capability | What runs it |
| --- | --- |
| `storage.conformance` | The conformance groups (`tests/storage/conformance/`) on SQLite, and on PostgreSQL in each `postgres` leg, directly and through `AnyStore`, the execution store's `execution` group among them |
| `storage.durability` | The `durability` group, `src/storage/postgres/tests/durability.rs` and the weak server; the server-crash test runs on demand |
| `storage.migrations` | `tests/storage/migration.rs` and `migration_execution.rs`, which migrate every earlier schema, 1 to 9, on SQLite; the PostgreSQL chain's tests; and `data migrate`'s copy and verification (`tests/storage/transfer/`) |
| `data.segments`, `data.backend_identity` | The segment groups and the segment spec's cases on both backends, and `both_backends_tell_the_same_story` (`tests/e2e/backends.rs`) |
| `data.sync`, `data.validate` | The command scenarios of `tests/e2e/offline.rs` on both backends, against the mock exchange |
| `data.history` | `tests/e2e/history.rs`: `serve run`'s history routes read back against the store on both backends, and `both_backends_answer_byte_for_byte_alike`; the routes' own tests in `src/server/tests/` run on SQLite |
| `config.storage`, `release.no_hidden_sqlite` | `tests/e2e/backends.rs`, and every command scenario on PostgreSQL; for Python, `test_reader.py` and `test_reader_stores.py` in `python`: an argument over its variable over the default, and no SQLite file made, opened or read where PostgreSQL is selected |
| `release.artifact` (`sqlite-build`) | The SQLite-only lines of `checks` and `package` |

`data.history`'s release evidence on both exchanges is ALG-703's, and the
server surface of `config.storage` waits for its story.

**The candle reader's own capability, `strategy.python_reader`, has ALG-530
for its evidence,** the parity run that reads the goldens' candles out of
each store. Until it lands, what holds the reader is `tests/storage/reader.rs`
on both backends in every leg, and `test_reader_stores.py` in `python`:
stores the command line wrote, read from Python on SQLite and on the job's
PostgreSQL, the same batch from either. They are on Binance's Spot line,
the one a command stores candles of today.

**The goldens are evidence of their own.** The matrix names them, as
ALG-383, for `strategy.backtest_local`: the frozen SMA-cross results
reproduced from candles stored for each venue in each store, from the
command line and from Python ([golden-parity.md](golden-parity.md)).
`tests/golden/` holds the engine to them in every `checks` leg, and the
Rust strategy for as long as it exists; `test_golden_parity.py` holds a
strategy written in Python to them in `python`. Neither reads the candles
from a store: the same results from candles stored for each venue in each
store are QUA-458's.
