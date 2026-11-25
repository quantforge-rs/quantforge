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
- [The workers' sandbox](#the-workers-sandbox)
- [The Python package](#the-python-package)
- [The wheels](#the-wheels)
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
| `python` | Linux | The Python package, `quantforge-py/`: clippy for its crate with and without the default features, `cargo check` with Rust 1.85, then the wheel maturin builds, installed with its extras, NumPy and pandas, and tested on Python 3.11 and 3.15, on the oldest NumPy and the oldest pandas the extras take, with NumPy alone, and without the extras, the SQLite-only wheel, and the in-place install; the `quantforge` binary is built for the candle reader's tests, which read stores it wrote, and for the worker's, which run bundles on its server, on SQLite and on the job's PostgreSQL server: see [The Python package](#the-python-package) |
| `wheel (ubuntu-latest)`, `wheel (macos-latest)`, `wheel (windows-latest)` | Each platform | The Python package's wheel for the platform, built for release as it would be published, then installed with its extras and tested on Python 3.11 and 3.15, the candle reader's and the worker's tests on SQLite, and on Linux on the job's PostgreSQL server too, the worker's on a `serve run` of the binary built there; the wheel that passed is kept with the run: see [The wheels](#the-wheels) |
| `wheels` | Linux | Passes only when every `wheel` leg passed |

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
- `wheels`, and `wheel (ubuntu-latest)`, `wheel (macos-latest)` and
  `wheel (windows-latest)` beside it
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

**`wheels` stands for the `wheel` legs** in the same way: it passes only
when every leg passed, and runs even when one failed or was cancelled.
Branch protection names the three legs beside it, so a platform dropped
from the matrix is taken out of branch protection in the same change.

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
  Linux only, so the `wheel` legs there test the candle reader on SQLite
  alone.

## The workers' sandbox

A backtest job runs in a worker that puts limits and a sandbox on itself,
and says what held ([worker-isolation.md](worker-isolation.md)). On Linux
that is all of it; on macOS and Windows most of it does not exist. So a
test of a guard asks the workers first, and where they enforce no such
guard it has nothing to hold them to: it skips, or, for a bundle that means
harm, holds the attempt to going through, as the
[threat model](security/remote-execution.md) says it does there.

**`QUANTFORGE_TEST_SANDBOX_REQUIRED=1` keeps a sandbox that is not on from
passing as one that is.** With it, a test of a guard the workers do not
enforce fails. Every job that runs tests on Linux sets it:

<!-- begin: sandbox required -->
| Job | The variable |
| --- | --- |
| `checks` | `1` on the Linux leg, empty on macOS and Windows |
| `postgres 16`, `postgres 17`, `postgres 18` | `1` |
| `python` | `1` |
| `wheel` | `1` on the Linux leg, empty on macOS and Windows |
<!-- end: sandbox required -->

An empty value is as if unset. With it set, a Linux runner whose kernel
had no Landlock, or a build whose seccomp filter the kernel refused, fails
`tests/workers/` in the crate's suite and `test_hostile_bundles.py` in the
package's, where both would otherwise pass without having tested a guard.

What runs, on every leg:

- **`tests/workers/`**, with this build's binary as the worker: each limit
  run into, every way out of a scratch directory, every reach for another
  process, and a link planted where a bundle is written.
- **`tests/bundle/hostile.rs` and `src/server/tests/hostile.rs`**: archives
  and uploads made to do harm, or simply broken, 15,000 mutations of
  canonical tars among them, from a fixed seed. These need no sandbox, and
  run alike everywhere.
- **`test_hostile_bundles.py`**, in `python` and in each `wheel` leg: real
  bundles that mean harm, through a real `serve run` whose workers are the
  Python of the run, on SQLite and, where the job has a server, on
  PostgreSQL.

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
  reader read stores the command line itself wrote, and the worker's run
  strategy bundles on its `serve run`, whose workers are the Python of the
  run ([the Python worker](python-worker.md#testing-it)). The script tells
  every run below where it is, in `QUANTFORGE_PY_TEST_CLI`.
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
in-place install's among them, and compiles neither. Its wheels are built
in the `dev` profile, on Linux only: the wheel a release carries, for each
platform, is the `wheel` legs' ([The wheels](#the-wheels)).

## The wheels

The `wheel` legs build [the Python package](python-package.md)'s wheel for
each platform as a release would publish it, and test it on that platform.
`python` tests a wheel built in the `dev` profile, on Linux alone; these
test the wheels a release carries.

<!-- begin: wheels -->
| Leg | Built | For |
| --- | --- | --- |
| `wheel (ubuntu-latest)` | In a `manylinux_2_28` container | Linux on x86-64, with glibc 2.28 or later |
| `wheel (macos-latest)` | On the runner | macOS on Apple silicon |
| `wheel (windows-latest)` | On the runner | Windows on x86-64 |
<!-- end: wheels -->

Each leg runs, in order:

- **The build:** `maturin build --release --compatibility pypi`, through
  `PyO3/maturin-action`, which installs the maturin `pyproject.toml` asks
  for. The wheel has the crate's default features, both stores, as every
  published artifact has them, and is tagged `cp311-abi3`: one wheel for
  every Python from 3.11 on. `--compatibility pypi` fails a wheel whose
  tag PyPI would refuse, such as Linux's plain `linux` tag, which maturin
  gives a wheel no manylinux takes. The Linux wheel is built in the action's
  `manylinux_2_28` container, so that it loads on any Linux with glibc
  2.28 or later. Its target directory, `target/wheel`, is its own: before
  a build in the container the action removes the build scripts of the
  directory it builds in, which the binary's build on the runner needs.
  The leg makes `target` itself on the runner first: the container builds
  as root, and the action gives the runner back only `target/wheel` and
  `dist`, so a `target` the container made would stay root's, and the
  binary's build could write nothing in it.
- **The tests:** `scripts/python-wheel-check.sh dist <oldest> <newest>`.
  It builds the `quantforge` binary for the candle reader's tests, then
  installs the wheel with its two extras into a fresh virtualenv of each
  Python and runs the package's whole suite against it, the golden parity
  test among it. The Pythons are the oldest the package supports and the
  newest released, as in `python`, each named by its path, since Windows
  has no `python3.11` to name. On Windows the script runs in Git Bash.
- **The artifact:** the wheel that passed, kept with the run as
  `wheel-<os>`, such as `wheel-windows-latest`. A leg that fails keeps
  none.

**The Linux leg has a PostgreSQL server,** one service container of
`postgres:18`, as `python` has, and sets `QUANTFORGE_TEST_POSTGRES_URL` and
`QUANTFORGE_TEST_POSTGRES_REQUIRED=1`: there the wheel's reader reads the
stores the binary wrote on both backends, and a test that could not reach
the server fails. The macOS and Windows legs have no server, since GitHub
runs service containers on Linux alone, and both variables are empty
there: the reader's PostgreSQL tests skip, and the package is imported and
tested with no database at all.

**A wheel is held to the package as it is published.** The script never
tells the tests that a wheel is the SQLite-only build, so one built
without the `postgres` feature fails: `STORAGE_BACKENDS` has to name both
stores (`test_package.py`), and selecting PostgreSQL has to reach for the
server, where that build refuses it as not built (`test_reader.py`). The
SQLite-only build is never taken for a release's wheel.

The script ends by naming the wheel it tested and its SHA-256, by which a
record in the parity matrix's [evidence ledger](parity/README.md#evidence)
names a published file.

**Nothing here waits for remote execution.** The goldens reproduced from
candles read out of each store, run locally and as a server's job, are
QUA-458's, which extends this workflow once remote execution is ready
(ALG-530).

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

# python, with CPython 3.11 or later; name each interpreter to test on.
# with the variables of `start` exported and its servers still up, as
# here, the reader's tests run on postgresql too
scripts/python-package-check.sh python3.11 python3.15

# wheel, with maturin 1.16 or later: this platform's wheel, built for
# release into a directory that holds no other wheel, and tested as its leg
# tests it; with the servers of `start` up, on postgresql too
maturin build --release --manifest-path quantforge-py/Cargo.toml --out dist
scripts/python-wheel-check.sh dist python3.11 python3.15

scripts/postgres-test-servers.sh leftovers
scripts/postgres-test-servers.sh stop
```

On Linux, `maturin build` builds the wheel on the machine itself, not in
the leg's `manylinux_2_28` container, so its platform tag may differ from
the one a release publishes. `scripts/python-wheel-check.sh` runs on Windows
too, in Git Bash, where each interpreter is named by its path, as
`py -0p` lists them.

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

The [parity matrix](parity/README.md) names this CI (as ALG-411, and the
`wheel` legs as ALG-384) among the evidence for these capabilities. The
release gate, ALG-769, turns runs into records in `evidence.jsonl`.

| Capability | What runs it |
| --- | --- |
| `storage.conformance` | The conformance groups (`tests/storage/conformance/`) on SQLite, and on PostgreSQL in each `postgres` leg, directly and through `AnyStore`, the execution store's `execution` group among them |
| `storage.durability` | The `durability` group, `src/storage/postgres/tests/durability.rs` and the weak server; the server-crash test runs on demand |
| `storage.migrations` | `tests/storage/migration.rs`, `migration_execution.rs` and `migration_provenance.rs`, which migrate every earlier schema, 1 to 10, on SQLite; the PostgreSQL chain's tests; and `data migrate`'s copy and verification (`tests/storage/transfer/`) |
| `data.segments`, `data.backend_identity` | The segment groups and the segment spec's cases on both backends, and `both_backends_tell_the_same_story` (`tests/e2e/backends.rs`) |
| `data.sync`, `data.validate` | The command scenarios of `tests/e2e/offline.rs` on both backends, against the mock exchange |
| `data.history` | `tests/e2e/history.rs`: `serve run`'s history routes read back against the store on both backends, and `both_backends_answer_byte_for_byte_alike`; the routes' own tests in `src/server/tests/` run on SQLite |
| `config.storage`, `release.no_hidden_sqlite` | `tests/e2e/backends.rs`, and every command scenario on PostgreSQL; for Python, `test_reader.py` and `test_reader_stores.py` in `python`: an argument over its variable over the default, and no SQLite file made, opened or read where PostgreSQL is selected |
| `release.artifact` (`sqlite-build`) | The SQLite-only lines of `checks` and `package` |
| `release.artifact` (`wheel`) | The `wheel` legs: each platform's wheel, built as a release would publish it, passes the package's suite on that platform, the candle reader's tests on SQLite, and on Linux on PostgreSQL too; a wheel without the `postgres` feature fails it ([The wheels](#the-wheels)) |
| `release.wheel_surface` | `test_package.py` in the `wheel` legs: each wheel publishes the names `__all__` lists and no other, none of them a webhook or a Telegram API |
| `jobs.isolation` | `tests/workers/` in every `checks` and `postgres` leg, on each store: workers within their limits, with nothing of the store, and every way out refused where the sandbox holds, which on Linux it has to ([The workers' sandbox](#the-workers-sandbox)); and the hostile-bundle suite, `test_hostile_bundles.py`, in `python` and the `wheel` legs, each bundle through a real server on SQLite and on PostgreSQL |

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
