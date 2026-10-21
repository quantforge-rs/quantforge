# ADR 0001: PostgreSQL client strategy for the synchronous storage ports

- Status: Accepted
- Date: 2026-10-21
- Ticket: ALG-403. Unblocks ALG-404 (schema and adapter), ALG-401
  (`--storage` and its settings), ALG-411 (CI), and ALG-729 (the server on
  PostgreSQL); reusable by ALG-391 (TimescaleDB) without being its
  prerequisite.

## Context

QuantForge 0.3.0 ships PostgreSQL as a second storage backend. SQLite stays
the default, and a SQLite-only setup needs no PostgreSQL service, setting, or
code path.

The storage ports are synchronous traits: `CandleStore`, `RunJournalStore`,
and `SegmentStore` in `src/ports.rs`, implemented by `SqliteStore`. They stay
synchronous in 0.3; rewriting them as async traits would touch every engine
and command. So the PostgreSQL adapter must serve synchronous calls, and
these are where they come from:

- **The CLI.** `main.rs` is `#[tokio::main]`, so every command handler is
  async and calls the ports from a thread that drives the runtime.
  `AppContext::init`, which builds the store, runs there too.
- **The poll loops.** `data sync` and `trade run` are async loops that call
  the ports between their awaits.
- **The HTTP server.** axum handlers are async; a blocking call made in one
  stalls a worker that every other request shares.
- **Tests.** `#[tokio::test]` runs on a current-thread runtime.
- **Trusted Python readers** (ALG-382): Python code in the same process
  reads candles through the store.
- **Untrusted job workers**: isolated processes running a user's strategy,
  which must never reach the database.

Tokio panics in four places that bear on this, all verified in its source:

1. `Runtime::block_on` and `Handle::block_on` on a thread that already
   drives a runtime: "Cannot start a runtime from within a runtime".
2. `block_in_place` on a current-thread runtime.
3. Dropping a `Runtime` inside async code, unless it is shut down with
   `shutdown_background`.
4. `oneshot::Receiver::blocking_recv` inside any runtime.

A `spawn_blocking` thread only enters its runtime's handle, so it may block,
and may even drive another runtime.

The obvious synchronous client, the `postgres` crate, is a wrapper over
tokio-postgres in which every connection owns a tokio runtime and runs
`block_on` for each operation. It panics under the first of these rules
from every async context above.

## Decision drivers

1. No nested-runtime panic from any context that calls the ports.
2. Bounded connections, with backpressure instead of unbounded waits.
3. Never block the HTTP server's executor.
4. Trusted Python in the same process works, with the GIL released while
   the database is waited on.
5. Untrusted workers inherit nothing: no URL, credentials, or connection.
6. MSRV 1.85; builds on Linux, macOS, and Windows without system OpenSSL or
   libpq.
7. SQLite-only builds and runs are unaffected.
8. The same choice serves TimescaleDB.

## Considered options

| Option | Verdict |
| --- | --- |
| **A. tokio-postgres and deadpool-postgres on a runtime the store owns** | **Chosen.** |
| B. The sync `postgres` crate with an r2d2 pool | Panics from every async context (rule 1); each connection carries its own runtime; r2d2 is in maintenance. |
| C. tokio-postgres driven by `Handle::block_on` or `block_in_place` on the caller's runtime | Panics in async code (rule 1) and on current-thread runtimes (rule 2). |
| D. sqlx | sqlx 0.9 needs Rust 1.94, above the MSRV, and brings its own pool and runtime choices. |
| E. diesel | Needs Rust 1.86 and libpq, a C library to install or build on every platform. |
| F. Async storage ports | Out of scope for 0.3: every engine and command would change. |
| TLS: native-tls or openssl | A system OpenSSL dependency on Linux and in images; rustls is already in the build. |

## Decision

### Driver and pool

- `tokio-postgres` 0.7 with `deadpool-postgres` 0.14, built with
  `Runtime::Tokio1` and an explicit `max_size`, wait timeout, create timeout,
  and recycle timeout. Without a runtime the timeouts do not work, and
  deadpool's defaults are a pool of twice the cores and no timeouts at all.
- Recycling is `Verified`: a returned connection runs a test query before
  reuse, so one that died while idle is replaced, not handed out.
- No libpq: configuration comes only from QuantForge's own settings (ALG-401).
  tokio-postgres reads no `PG*` environment variables and no `.pgpass`.
- Supported servers: PostgreSQL 16, 17, and 18, the range current
  TimescaleDB releases support. PostgreSQL 14 reaches end of life on
  2026-11-12.

### The bridge

A `PostgresStore` owns the pool and a small runtime of its own: two named
worker threads, with timers and I/O enabled.

- **Calls.** A port method spawns its work onto the bridge and blocks the
  caller on a `std::sync::mpsc` reply. It never calls `block_on` on the
  caller's thread, so it works from an async handler, a current-thread test,
  a `spawn_blocking` thread, a Python thread, or a plain one. A caller that
  blocks a current-thread runtime cannot starve the work, which runs on the
  bridge's threads.
- **The pool stays on the bridge.** deadpool-postgres starts each
  connection's driver with `tokio::spawn` on whichever runtime asks the pool
  for it. The pool is used only inside bridge tasks, so every driver lives
  on the bridge and none on a caller's runtime that may be blocked waiting.
- **Refusals.** A call from one of the bridge's own threads is refused,
  since it would wait on itself; the runtime id tells. So is a call from
  another process than the one that built the bridge, such as a forked
  child, where the bridge's threads do not exist; the pid tells.
- **Panics.** A panic in the work is raised again on the caller, rather than
  turning into a lost reply.
- **Deadlines.** Each call runs under a deadline. Past it the bridge sends a
  cancel request for the query and discards the connection instead of
  returning it to the pool, so no later call queues behind an abandoned
  query. The caller also stops waiting after a watchdog margin past the
  deadline, in case the bridge itself wedges.
- **CPU-heavy work stays off the bridge.** Packing segments with zstd, for
  example, runs on the caller's thread: the bridge's two workers drive every
  connection's I/O.
- **Where it is built.** In `AppContext::init`, as `SqliteStore` is today,
  behind an `AnyStore` enum that implements the three ports by delegation:
  MSRV 1.85 has no trait upcasting, so one trait object cannot serve as the
  others (ALG-401).

### Connections

| Setting | Value | Why |
| --- | --- | --- |
| Connect timeout | 5 s | Bounds connecting and TLS. |
| TCP keepalives | idle 30 s | tokio-postgres defaults to 2 h, which leaves a partitioned connection hanging. |
| `statement_timeout` | 30 s, set with `SET` on each new connection | The `options` startup parameter would do it too, but PgBouncer refuses it. |
| Statements | unnamed (`query_typed`) | Named prepared statements break under PgBouncer's transaction pooling. |
| `application_name` | `quantforge` | Operators can tell the connections apart in `pg_stat_activity`. |

### Pools and their owners

| Context | Owner | Pool | Calls |
| --- | --- | --- | --- |
| CLI command | `AppContext` | 2 | Directly from the handler. Blocking a CLI handler costs nothing else: it is the only task. |
| `serve run` | The server parent | 8 | Under `spawn_blocking`, behind a semaphore; see below. |
| Python reader | The Python object | 2 | With the GIL released. |
| Job worker | Nobody | None | The parent loads the candles and passes them. |

- One pool per process. Every caller in a process shares it; nothing builds
  a second store for the same database.
- A call that finds no free connection within the wait timeout, 5 s, fails
  as unavailable. It never opens a connection beyond the pool's size.
- Operators budget connections across processes against the server's
  `max_connections`: a server, a `data sync`, and a `trade run` side by side
  hold up to 8 + 2 + 2.

### The HTTP server

- Store calls run under `spawn_blocking`, gated by an async semaphore sized
  to the pool. A request that gets no permit within its timeout is answered
  503 `unavailable` with `Retry-After`. Without the gate a burst would queue
  on tokio's blocking pool, up to 512 threads shared with DNS lookups and
  SQLite, and its waits would outlive the request.
- The same pattern serves SQLite, which blocks on file I/O just the same.
- `spawn_blocking` work cannot be cancelled: a request whose client hangs
  up keeps its call, bounded by the call's deadline.

### Trusted Python readers

- The Python object owns its store, and closes it on `close` or when it is
  dropped.
- Calls release the GIL while they wait (`py.detach` in PyO3 0.30), so other
  Python threads run meanwhile. The bridge's threads never touch Python, so
  no lock is held across the two.
- One store per process, created after any fork: a forked child has none of
  the bridge's threads, and the pid check refuses its calls.

### Untrusted job workers

- A worker never builds a store and never receives a database URL or
  credentials: it is spawned with a cleared environment and an explicit
  allowlist.
- Sockets are opened close-on-exec, so no connection leaks into a worker.
- The parent reads the candles a job needs and passes them to the worker,
  as a segment payload or a stream.

### TLS

- `tokio-postgres-rustls` with its `ring` feature: the same rustls 0.23 and
  ring that reqwest already uses, so no OpenSSL and no aws-lc-rs.
- The rustls configuration is built with the `ring` provider passed in, not
  installed process-wide.
- Certificates are checked against the operating system's trust store
  (`rustls-native-certs`; `SSL_CERT_FILE` and `SSL_CERT_DIR` replace it, and
  belong in `docs/configuration.md` with the other dependency-owned
  variables), plus an optional CA file for providers whose CA it lacks.
- tokio-postgres knows three `sslmode`s and none of libpq's certificate
  options. QuantForge reads libpq's into them:

| `sslmode` | Loopback host | Any other host |
| --- | --- | --- |
| absent | `prefer` | `require` |
| `disable` | `disable` | refused |
| `prefer` | `prefer` | refused |
| `require`, `verify-ca`, `verify-full` | `require` | `require` |

  `require` always verifies the chain and the host name, because the rustls
  connector does; `sslrootcert` becomes the CA file. Plain text, or a
  fallback to it, is allowed only to loopback.

### Errors

The kinds mirror `ExchangeErrorKind`, and `StorageError` becomes
`#[non_exhaustive]` so that they can join it (ALG-404).

| Kind | When | Caller | HTTP |
| --- | --- | --- | --- |
| Unavailable | No connection, TLS refused, no free connection in time, or a read past its deadline: nothing was changed | Retry | 503 `unavailable` |
| OutcomeUnknown | A write lost its answer: past its deadline, or its connection dropped while it or its `COMMIT` ran | Reconcile before retrying: `order_events` has no idempotency key, so a blind retry can duplicate a row | 500 `internal` |
| Other | The database refused the statement; its SQLSTATE stays in the error's source | Fix the cause | 500 `internal` |

A statement timeout is treated as the deadline: unavailable for a read,
outcome unknown for a write. HTTP clients never see a connection string,
host, or database message.

### Shutdown

- Dropping the store closes the pool, which wakes its waiters with an error
  and drops idle connections, then shuts the bridge's runtime down in the
  background. Dropping it inside async code therefore never panics.
- Work still in flight is cut off. The server drains its HTTP requests
  before it drops the store.

### Cargo and SQLite

- The adapter goes behind an optional `postgres` cargo feature, on by
  default, so release binaries, images, and `cargo install` include it.
- CI adds a `--no-default-features` build and test, which proves the SQLite
  path needs no PostgreSQL code, and an MSRV job (ALG-411).
  tokio-postgres 0.7.18 needs `sha2 ^0.11`; 0.11.1 needs Rust 1.88, and
  Cargo's MSRV-aware resolver, the edition 2024 default, picks 0.11.0,
  which declares 1.85.
- With no PostgreSQL configuration nothing connects anywhere, and SQLite
  works as it does today.

### Notes for ALG-404

- SQLite's snapshot reads become read-only `REPEATABLE READ` transactions,
  and its `IMMEDIATE` write checks become transactions with
  `SELECT … FOR UPDATE`.
- `INSERT OR REPLACE`'s fresh row id for a dirty segment mark becomes
  `ON CONFLICT … DO UPDATE SET id = nextval(…)`.
- `IS NOT` comparing values becomes `IS DISTINCT FROM`.
- `init` runs its DDL under `pg_advisory_lock`, so that processes starting
  together cannot race on it.
- Decimals stay exact TEXT, written with `to_string` and read with
  `from_str`, so PostgreSQL returns the same text as SQLite. TimescaleDB's
  aggregates will need NUMERIC generated columns beside them (ALG-391).
- A storage conformance suite, in the manner of `exchange::conformance`, is
  better than one-off parity tests (ALG-411).

## Evidence

`spikes/postgres-bridge/` is a standalone Cargo project, outside the
published crate, that implements the bridge, the TLS mapping, and
`CandleStore` over PostgreSQL, and exercises them from every context above.
It ran on macOS 26.4 (arm64) against a throwaway PostgreSQL 14.22 cluster
with TLS, with rustc 1.99.0, tokio 1.54, tokio-postgres 0.7.18,
deadpool-postgres 0.14.2, tokio-postgres-rustls 0.14.0, and PyO3 0.30.1 on
Python 3.14.2. What it measures is protocol-level and does not depend on the
server's version; ALG-411's CI covers the supported ones.

| Spike | Result |
| --- | --- |
| The sync `postgres` client inside `#[tokio::test]` | Panics: "Cannot start a runtime from within a runtime". On a `spawn_blocking` thread it only fails to connect. |
| The bridge from a current-thread test, a multi-thread test, `spawn_blocking`, and a plain thread | Answers in each. |
| A call from the bridge's own thread, from another process (simulated by changing the recorded pid), a panic, a wedged bridge | Refused, refused, raised on the caller, and reported after the watchdog. |
| Dropping the bridge inside async code | No panic, while a runtime dropped in place there panics. |
| Parity with `SqliteStore` | The same text for `1.10` and `1.1`, `0.00000000`, absent and zero trade counts, and the largest decimal. |
| The real `DataSyncEngine` poll loop, on a current-thread and a multi-thread runtime | Writes the same candles as into SQLite. |
| Dropping the store inside async code | No panic; its connections are gone from `pg_stat_activity`. |
| 200 concurrent requests of a 50 ms query, gated, 8 connections | All answered in 1.31 s, against an ideal 1.25 s. The heartbeat's worst lateness was 3.9 ms, against 401 ms when the handler called the store on the executor. Connections peaked at exactly 8. |
| A pool of 2 held by two long queries | A third request is answered 503 with `Retry-After` within a second; no third connection opens. |
| A read past a 300 ms deadline | Unavailable within 2 s; the next call is served at once; the server stops the abandoned query. A write past its deadline is outcome unknown. |
| TLS | `verify-full` with the cluster's CA connects over TLS 1.3; without the CA the certificate is refused. Plain text to a remote host is refused before connecting. |
| Python embedded in a process running a tokio runtime | Reads candles from its main thread and from a tokio worker. Four threads waiting 0.3 s each finish in 0.30 s with the GIL released, against 1.21 s holding it. |
| `cargo +1.85 check --locked` | Passes, with sha2 0.11.0. |
| Controls | Nine planted defects, each caught by its spike; see below. |

Each control plants one defect the spikes exist to catch, runs the spike
that should catch it, and restores the file:

| Defect planted | Caught by |
| --- | --- |
| `block_on` on the caller's thread instead of the reply channel | The current-thread and async-drop runner tests panic. The wedged-bridge test hangs instead, since `block_on` has no watchdog. |
| The bridge's runtime dropped in place instead of in the background | The async-drop test panics. |
| No re-entrancy guard | The re-entrancy test: the nested call is served instead of refused. |
| No pid check | The forked-process test. |
| No cancel past the deadline | The cancellation test: the server still runs the abandoned query. |
| A pool twice its configured size | The exhaustion test: the third request gets a connection. |
| Plain text allowed to a remote host | The `sslmode` mapping test. |
| The server calling the store on the executor | The burst test's heartbeat. |
| Python holding the GIL while it waits | The Python spike's timing. |

To run the spikes, start a PostgreSQL cluster, then:

```bash
cd spikes/postgres-bridge
export SPIKE_DATABASE_URL='postgres://user@127.0.0.1:5432/db?sslmode=disable'
# For the TLS spike, a host name the server's certificate names, and its CA:
export SPIKE_TLS_DATABASE_URL='postgres://user@localhost:5432/db?sslmode=verify-full&sslrootcert=/path/to/ca.crt'
# For the Python spike, the interpreter to embed:
export PYO3_PYTHON=/path/to/python3
cargo test --features python -- --test-threads=1
```

Without `SPIKE_DATABASE_URL` the database spikes skip and the rest run. The
spike's README describes a throwaway cluster made from the PostgreSQL
binaries.

## Consequences

- Every context that calls the ports works unchanged; the adapter is a
  fourth implementation of the existing traits, not a new programming model.
- Each process that uses PostgreSQL runs two extra threads, and one pool
  whose size bounds its connections.
- The server pattern, a semaphore and `spawn_blocking`, is required for
  every store call, SQLite's included, and ALG-508 follows it.
- Errors become richer: callers can tell a safe retry from a write whose
  outcome must be reconciled.
- QuantForge owns the URL handling libpq would have done: the `sslmode`
  mapping and the CA file.

## Follow-ups

- **ALG-404:** the `PostgresStore` for all three ports, schema and
  migrations, the `postgres` feature, and the new `StorageError` kinds.
- **ALG-401:** `--storage`, the URL and TLS settings and their environment
  variables, the pool size flags, and `AnyStore`.
- **ALG-411:** CI against PostgreSQL 16 to 18, `--no-default-features`, an
  MSRV job, and a storage conformance suite.
- **ALG-729:** the server on PostgreSQL.
- **ALG-382:** the Python reader, following the rules above.
- **ALG-391:** TimescaleDB, reusing this decision.
