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
  `ubuntu-latest-m`, with 4 cores and 16 GB of memory. The check is named
  after the job, so its name does not depend on the runner. Setting
  `runs-on` back to `ubuntu-latest` puts the job back on the standard
  runner.

Both runners belong to the organization's `Default Larger Runners` group,
which must allow public repositories; where it does not, or in a copy of
the repository without such runners, these jobs wait for a runner that
never comes.

## Required checks

Branch protection on `main` requires these checks by name, `storage` once a
maintainer adds it:

- `checks (ubuntu-latest)`, `checks (macos-latest)`, `checks (windows-latest)`
- `package`
- `storage`
- `analyze` and `CodeQL`, from `.github/workflows/codeql.yml`
- `scan` and `gitleaks`, from `.github/workflows/gitleaks.yml`
- `CodeRabbit`

`storage` is new, and needs adding once:

```bash
gh api --method POST \
  repos/quantforge-rs/quantforge/branches/main/protection/required_status_checks/contexts \
  -f 'contexts[]=storage'
```

The `postgres` legs are not required themselves: `storage` stands for them.
A major added to the matrix, or dropped from it, then needs no change to
branch protection, and a dropped leg cannot leave a required check waiting
forever. `storage` runs even when a leg failed or was cancelled
(`if: always()`), because GitHub counts a skipped required check as passed.
A renamed job is different: every merge waits until its new name replaces
the old one in branch protection. That is why the runner of a `checks` leg
is changed through the matrix's `runner`, never by changing the `os` its
check is named after.

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
```

`start` runs the two servers on `127.0.0.1:54329` and `127.0.0.1:54330`, set
up as in CI, and prints the four variables to export. A server of your own
works too, such as the Homebrew one in
[storage-conformance.md](storage-conformance.md#postgresql).

`./scripts/release-check.sh` runs the `checks` and `package` lines in one go,
and the PostgreSQL tests too when the variables are exported. It gives the
publish dry run a target directory of its own: the dry run's verify build
would otherwise leave `target/debug`'s binary pointing at the packaged copy
of the sources, and later test runs would run that copy.

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
| `storage.conformance` | The conformance groups (`tests/storage/conformance/`) on SQLite, and on PostgreSQL in each `postgres` leg, directly and through `AnyStore`; the execution store joins with ALG-738 |
| `storage.durability` | The `durability` group, `src/storage/postgres/tests/durability.rs` and the weak server; the server-crash test runs on demand |
| `storage.migrations` | `tests/storage/migration.rs`, which migrates every earlier schema, 1 to 7, on SQLite; the PostgreSQL chain's tests; and `data migrate`'s copy and verification (`tests/storage/transfer/`) |
| `data.segments`, `data.backend_identity` | The segment groups and the segment spec's cases on both backends, and `both_backends_tell_the_same_story` (`tests/e2e/backends.rs`) |
| `data.sync`, `data.validate` | The command scenarios of `tests/e2e/offline.rs` on both backends, against the mock exchange |
| `data.history` | `tests/e2e/history.rs`: `serve run`'s history routes read back against the store on both backends, and `both_backends_answer_byte_for_byte_alike`; the routes' own tests in `src/server/tests/` run on SQLite |
| `config.storage`, `release.no_hidden_sqlite` | `tests/e2e/backends.rs`, and every command scenario on PostgreSQL |
| `release.artifact` (`sqlite-build`) | The SQLite-only lines of `checks` and `package` |

`data.history`'s release evidence on both exchanges is ALG-703's, and the
server and Python surfaces of `config.storage` wait for their stories.
