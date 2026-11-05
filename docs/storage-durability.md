# Storage durability

What an acknowledgement from a store means, how long a call may take to
give one, and what a caller does when it doesn't get one. SQLite and
PostgreSQL mean the same by all of it, so the code above the storage ports
never asks which backend it has. Webhook admission (ALG-743) and the
execution ledger (ALG-738) build on this contract; they verify their own
inbox and fencing semantics on top of it.

## Contents

- [The contract](#the-contract)
- [What a commit survives](#what-a-commit-survives)
- [What it assumes](#what-it-assumes)
- [The settings every connection gets](#the-settings-every-connection-gets)
- [How long a call may take](#how-long-a-call-may-take)
- [When a call fails](#when-a-call-fails)
- [What differs by design](#what-differs-by-design)
- [What was exercised](#what-was-exercised)

## The contract

- **`Ok` from a write means committed and flushed,** on a store with [the
  settings below](#the-settings-every-connection-gets): it answers only after
  the backend's commit returned. A store without them is refused, unless it
  runs in `Durability::Reported` mode, where an `Ok` promises the commit and
  not the flush. No path turns a failure, a timeout or a lost answer into
  `Ok`.
- **Anything else is a failure of a known kind.** Every failure but
  `outcome_unknown` means the write changed nothing.
- **A store never retries.** A caller may, within the rules of
  [retrying](#retrying), and what it repeats is the transaction and nothing
  else. An exchange request is never part of it.

The contract covers every write of every port. Candles could be fetched
again, but one rule for every commit is simpler to hold than a list of
which rows matter.

## What a commit survives

Four failures that are easy to mistake for one another:

| Failure | What it takes away | SQLite | PostgreSQL |
| --- | --- | --- | --- |
| Application crash: QuantForge is killed, panics or runs out of memory | The process. The operating system and its caches live on | A commit survives, whatever `synchronous` says | A commit survives: the server is untouched. A write in flight has an unknown outcome |
| Database crash: the server's processes die | The server's memory | Not applicable: SQLite is a library, so this is the row above | A commit survives: recovery replays the write-ahead log |
| Operating system crash: a kernel panic | Everything not yet flushed to the drive | A commit survives, because it was flushed before the answer | The same |
| Power loss | Also whatever the drive itself had not made stable | A commit survives if the drive honors a flush | The same |

Each row is a stronger claim than the one above it, and a test of one says
nothing about the next. Killing a process proves the first row only: the
page cache survives it, so a kill test passes at `synchronous=OFF` too.
[What was exercised](#what-was-exercised) says which rows have been tried.

## What it assumes

The last two rows rest on things QuantForge cannot check from inside:

- **The storage honors a flush.** A drive or a RAID card that acknowledges a
  flush from a volatile cache loses commits on power loss. Enterprise SSDs
  with power-loss protection and ordinary local disks are fine; a cache that
  lies is not.
- **SQLite lives on a local disk.** WAL needs shared memory, and locks and
  flushes over a network share cannot be trusted. A file on NFS or SMB is not
  supported. A container volume is supported when it is a real local
  filesystem, and not a host folder passed through a virtual machine's file
  sharing, as Docker Desktop does.
- **PostgreSQL's data directory has the same properties,** and its
  `wal_sync_method` reaches stable storage. On Linux the default does. On
  macOS it takes `fsync_writethrough`; a PostgreSQL server on macOS is for
  development.
- **A standby is the operator's own guarantee.** QuantForge relies on the
  local flush alone. `synchronous_commit = on` with a synchronous standby is
  kept, but failover is not part of this contract.

## The settings every connection gets

Each connection is set up when it opens, and the settings it ends up with
are read back rather than assumed: asking SQLite for WAL where the file
cannot have it leaves the old journal in place, and no error says so. SQLite
opens a connection per call. PostgreSQL's pool checks a connection again each
time it hands one out, in the round trip it makes anyway, so a server
weakened during a long run is found at the next call.

| Backend | Setting | Value | Why |
| --- | --- | --- | --- |
| SQLite | `journal_mode` | `wal` | Readers pass a writer, and the store's snapshot reads rely on it |
| SQLite | `synchronous` | `full` | The log is flushed at every commit; `normal` can lose recent commits on power loss |
| SQLite | `fullfsync` | `on` | On macOS `fsync` stops at the drive's cache without it; other systems ignore it |
| PostgreSQL | `synchronous_commit` | `local` or stronger | A commit waits for its own flush. `off` is raised to `local`; `on` and the standby settings stay as the operator set them |
| PostgreSQL | `fsync` | `on` | A server setting. A session cannot raise it |
| PostgreSQL | `full_page_writes` | `on` | A server setting, which keeps a torn page from corrupting recovery |

No table relies on a connection setting for its integrity: neither schema
declares a foreign key, so nothing needs SQLite's `foreign_keys`. A test
fails if that changes, and `SqliteStore::open` is where the setting goes.

**A store in `Durability::Enforced` mode refuses a connection that falls
short,** as `StorageError::DurabilityUnmet`, so no write ever goes through
it. It is every store's default, and what `trade run --mode live` and `trade
close` open: on a weaker store they stop before any request. Every other
command opens in `Durability::Reported` mode, which applies the same settings
and logs a warning, so a backtest against a development server with `fsync`
off still runs. `quantforge store status` prints `durability: met` or
`durability: weakened` with what it read back.

A refusal names the setting, its value and what to do about it. It holds
setting names and values only, never a URL.

## How long a call may take

A call never waits without limit on another writer or on a connection, and
on PostgreSQL never on the server either:

| Wait | SQLite | PostgreSQL |
| --- | --- | --- |
| A free connection | None: a connection per call | `StorageLimits.wait_budget`, 5 s |
| A lock | `wait_budget`, 5 s, by the clock | `wait_budget`, 5 s, as `lock_timeout` |
| A statement | Not bounded: it runs on the caller's thread | `statement_timeout`, 30 s |
| The whole call | Not bounded, for the same reason | 35 s, then the query is cancelled and the connection discarded |

**A route with a deadline of its own uses `AnyStore::within(budget)`:** the
same store, its file or pool shared, with each call through it bounded by
`budget`, waits and statements together. A call past its budget answers
with a failure; it never answers `Ok` for a write it could not prove.
`StorageLimits.route_budget` is 2 s, for webhook admission: TradingView drops
a delivery after 3 s, and the other second is for TLS, authentication and
the answer. A host sets another in `StorageRequest.route_budget` and reads it
from `StorageSettings::limits`; ALG-743 passes it to `within` and decides
whether an operator may change it. A budget only ever tightens a store's
limits.

What is left of a call's budget divides so that the backend's own answer
arrives first: a lock wait may take three quarters of it and a statement
seven eighths, and only then does the bridge give up. So a held lock is
`contention`, which changed nothing, rather than `outcome_unknown`.

A budget covers one port call. `refresh_segments` is many calls, one or
more per window, and each is bounded on its own.

SQLite's busy timeout cannot keep a budget: it adds up the sleeps it meant to
take and never reads a clock, so late timers stretch it. A 1.5 s wait took
3.7 s in a process at macOS's background priority, and on a macOS CI runner
a call with a 2 s budget took 3.3 s. So a connection waits with a busy handler
of its own, which reads the clock and sleeps at most 10 ms between tries, where
SQLite's sleeps grow to 100 ms and lose the lock to writers that keep
committing.

SQLite's budget bounds waiting for locks, not the disk. A call runs on the
caller's thread, and a stalled flush cannot be interrupted, so a dying disk
can hold a SQLite call past its budget. PostgreSQL's calls run on the bridge,
so the caller stops waiting at the deadline whatever the server is doing.

## When a call fails

`StorageError::kind()` gives every failure one category:

| Kind | Meaning | What a caller does | SQLite | PostgreSQL |
| --- | --- | --- | --- | --- |
| `unavailable` | Nothing changed: the store could not be reached or ran out of time | Retry later | Cannot open the file; a disk failure outside a commit | No connection, no free one in time, a read past its deadline, the server out of resources (SQLSTATE class 53) |
| `contention` | Nothing changed: another writer held a lock past the budget | Retry once it is done | `SQLITE_BUSY`, `SQLITE_LOCKED` or `SQLITE_PROTOCOL` | 55P03, from `lock_timeout` |
| `conflict` | Nothing changed: the transaction lost to another | Run the whole transaction again | `SQLITE_BUSY_SNAPSHOT` | 40001 and 40P01 |
| `outcome_unknown` | A write may or may not have committed | Find out before anything else; never report it as done | A disk failure at a commit | A write that lost its answer: past its deadline, its connection gone, or a statement timeout |
| `durability_unmet` | The store's settings fall short | Fix the settings | See [the settings](#the-settings-every-connection-gets) | The same |
| `invalid_argument`, `schema_unsupported`, `other` | The call was refused, or the database is from a newer build, or the backend refused a statement | Fix the cause | | |

A statement timeout on a write stays `outcome_unknown`, as ADR 0001 decided,
though the server rolled it back. It errs on the side of looking first.

### Retrying

`storage::retry_conflicts` is the one retry the storage layer offers, and it
is the caller's to use:

```rust,ignore
retry_conflicts(budget, |left| store.within(left).upsert_candles(&market, &candles))
```

It repeats only `conflict`, hands each attempt what is left of the budget, and
stops inside it. `outcome_unknown` ends it at once, since a write that may
have committed must never be made twice. The closure holds storage calls and
nothing else: an exchange request inside it would be sent again.

An HTTP route answers `unavailable`, `contention` and `conflict` with a 503
and `outcome_unknown` with a 500, in the terms of ADR 0001. None of them is
a 2xx: a delivery that was not committed is not acknowledged, and the sender
tries again.

## What differs by design

The contract is the same; the machinery under it is not, and no shared
performance threshold pretends otherwise.

- **Writers.** SQLite has one writer per file at a time, so `contention` means
  someone else is committing anywhere in the database. PostgreSQL locks rows,
  so it means someone holds the same rows or table.
- **Readers.** Neither backend makes a reader wait for a writer. A SQLite
  call can still wait on a quiet file: the last connection to close
  checkpoints it, and one opening meanwhile waits for that, for as long as a
  flush takes. So `contention` can reach a read as well.
- **Commit cost.** SQLite pays one flush per commit on the caller's thread:
  about 5 ms on the machine below. PostgreSQL pays a network round trip and
  a flush it can share between concurrent commits, so it gains from many
  writers where SQLite queues them.
- **Connections.** SQLite opens one per call and has none to lose. PostgreSQL
  keeps a pool, which can run out, and whose connections can die between
  calls; a dead one is replaced when it is next asked for.
- **Deadlocks.** SQLite's one writer leaves nothing to deadlock on. PostgreSQL
  detects one after `deadlock_timeout`, 1 s by default, so a budget shorter
  than that sees `contention` first.

## What was exercised

On PostgreSQL 16.15 and SQLite 3.46 on macOS 26.4.1 (arm64), on 2026-11-04.

| Case | How | Result |
| --- | --- | --- |
| Application crash, SQLite and PostgreSQL | A writer killed with `SIGKILL` after 200 acknowledgements (`tests/storage/crash.rs`) | Every acknowledged candle stored |
| Database crash, PostgreSQL | `pg_ctl stop -m immediate` under a writer, then recovery on restart (the same file, on demand) | Every acknowledged candle stored, in 12 runs of 12. With `synchronous_commit = off` planted, the same test lost 3 to 6 acknowledged candles in 3 runs of 4, so it can tell the difference |
| A held writer lock, both backends | `durability.contention_is_bounded`, also at macOS's background priority, where timers run late, on 2026-11-05 | `contention` inside the budget, nothing written |
| A full pool, a slow statement, PostgreSQL | `src/storage/postgres/tests/durability.rs` | `unavailable` and `outcome_unknown` inside the budget |
| A deadlock, a snapshot conflict | A real one on each backend | `conflict`, and a retry goes through on PostgreSQL |
| A connection ended mid-write, PostgreSQL | `pg_terminate_backend` on a write waiting for a lock | `outcome_unknown`, and the store reconnects |
| A silently weakened setting | SQLite on a VFS with no shared memory; a PostgreSQL server with `fsync = off` | Refused when enforced, reported otherwise |

**Not exercised:** an operating system crash, a power loss, a flush that
fails or lies, a disk filling up at a commit, a stalled synchronous standby,
and a network partition. Nothing here shows the last two rows of [the
table](#what-a-commit-survives) hold on any particular machine: they follow
from the settings and [the assumptions](#what-it-assumes). The disk failures
are classified by table tests over SQLite's error codes, never provoked.
QUA-114 goes further on PostgreSQL's restart recovery.
