# The storage schema

**Schema 5.** Every QuantForge store holds the same tables under one schema
version, on SQLite and on PostgreSQL. This page is the contract between
them: the identities their rows carry, the record a store keeps
about itself and the activation contract behind it, who owns which tables
and how the schema moves forward, and the DDL on both backends.
`tests/storage/docs.rs` holds it to the code, and the shared cases in
[`tests/fixtures/storage_identity_cases.json`](../tests/fixtures/storage_identity_cases.json)
hold every backend to the identities. [upgrading.md](upgrading.md) is the
operator's side of the same contract.

<!-- begin: schema version -->
5
<!-- end: schema version -->

## Contents

- [Identities](#identities)
- [The store's record](#the-stores-record)
- [Versioning and ownership](#versioning-and-ownership)
- [The chain on SQLite](#the-chain-on-sqlite)
- [The chain on PostgreSQL](#the-chain-on-postgresql)
- [Legacy runs](#legacy-runs)
- [Writing the journal](#writing-the-journal)
- [SQLite DDL](#sqlite-ddl)
- [PostgreSQL DDL](#postgresql-ddl)
- [Segments on PostgreSQL](#segments-on-postgresql)
- [Keeping segments](#keeping-segments)
- [The shared cases](#the-shared-cases)
- [Backends and ownership](#backends-and-ownership)

## Identities

**A market** is three columns, never a joined string: `exchange` holds the
product line's storage name, `<venue>_<category>` such as `binance_spot` or
`bybit_linear` (`ExchangeId::as_str`); `symbol` the exchange's symbol in
uppercase; `interval` the interval token, such as `1m`. The same symbol on
two venues, or in two product families of one venue, is two keys. On the
wire the product line is `exchange`, the venue's registry name, with
`category`; in `state_json` it is the serde token, such as `BinanceSpot`.
The three spellings never read each other.

**The environment** a store trades is a property of the store, not of a
row: `store_control.environment`, declared once by the operator
(`quantforge store bind`) and never inferred from a URL. `NULL` is a store
that was never bound: one an earlier release created, or a new one not yet
bound. Before any request, a command checks that its endpoint agrees with
the binding: mainnet is a production endpoint and nothing else is. So
market-data keys need no environment column; execution rows carry one
anyway, as a second safeguard.

**An account** is `VenueAccountId`, stored as two columns: `account_kind`
(`venue` for the ID the venue issued, `key_fingerprint` for the stand-in)
and `account_id` (the ID, or the 16 lowercase hex digits of the key's
fingerprint). For a Spot run the account is the fingerprint of the API key
on every environment; it identifies the key, not the account, and nothing
arms on it (ADR 0002 §2; ALG-759 brings the venue-issued IDs).

**A run's identity** is the environment the store was bound to when the run
started and the account that traded it, with where the identity came from:
`identity_source` is `recorded` (by the process that started the run),
`adopted` (given by an operator to a legacy run) or `legacy` (written by a
build before schema 5, which recorded neither). A run is driven again only
under the identity that recorded it: the same environment, an unbound
store included, and the same account, none included. It never inherits a
newly bound environment or another key silently, and a legacy run is never
resumed or closed until an operator adopts it.

**Venue IDs** (orders, trades) are opaque text in every column and every
JSON field, Spot's included: a number on one venue, a string on another,
never computed with.

**Decimals** are exact text that keeps its scale: `1.10` is stored and
returned as `1.10`, never `1.1` and never a `REAL` or `NUMERIC` (ADR 0001).
Every backend writes `Decimal::to_string` and reads back with
`Decimal::from_str_exact`: stored text a `Decimal` cannot hold exactly,
more than 28 decimal places or beyond its range, is refused with the text
named, never rounded. QuantForge never writes such text; it can only have
been written behind the store's back.

**Segments** are named by the hash of their bytes, which hold no market; an
index entry (`segment_index`) gives a name its market and window. The
[segment spec](segment-spec.md#identity) fixes the identity of an entry.

## The store's record

`store_control` is one row: the store's `store_id`, its `environment`, its
`activation` and `activation_epoch`. It is the control record that the
activation contract of ADR 0002 §2 and §5 reads and writes.

**States and transitions** (`StoreActivation`, pinned by the parity
matrix's `states.store`):

| From | To | When |
| --- | --- | --- |
| `inactive` | `active` | An operator activates the store; the epoch becomes one more than it was. |
| `active` | `deactivated` | An operator hands over to another store; the source refuses to arm from then on. |
| `deactivated` | `active` | Rollback: the operator reactivates the source after deactivating the target; a new epoch again. |

**Rules:**

- A store starts `inactive` with epoch 0: the migration default for every
  store brought to schema 5, and the state of every store a transfer
  creates. Epoch 0 means never activated.
- Activation is an operator's action on one store; it is never a row that
  a copy or a transfer carries over. A transfer (ALG-407) creates the
  target's own `store_control` row and never copies the source's. A byte
  copy of a file does carry the row, which is why the epoch is in every
  client order ID: orders with the right owner tag but another epoch
  reveal a second writer, and ALG-767's arming guard halts entries.
- Copying records or leases never confers execution authority: only the
  transition to `active`, on that store, does.
- Exactly one store per account is `active` at a time. Deactivate the
  source before activating the target; reactivation after a rollback
  deactivates the target first.
- `store status` shows the row. ALG-738 persists the transitions, ALG-767
  enforces them before arming, and ALG-768 verifies a transfer end to end.
  Nothing in this release moves a store between these states.

## Versioning and ownership

**One version for the whole store**, `meta.schema_version`, and one chain:
every schema change adds a step that brings version N to N + 1, never
edits an earlier step, and logs itself in `schema_migrations` (the version
reached, the version left, when, and the QuantForge version that ran it).
A fresh store runs the whole chain in one transaction and logs one step
from 0. Nothing is deleted by a step, and a step that rewrites a row keeps
the original beside it.

**Who owns which tables.** A story that adds tables adds a step and the DDL
for both backends to this page; nothing here claims the tables of a later
story exist yet.

<!-- begin: table owners -->
| Tables | Owner | Since |
| --- | --- | --- |
| `meta`, `candles` | QuantForge 0.1.0 | schema 1 |
| `bot_runs`, `order_events`, `closed_trades` | QuantForge 0.2.0; the identity columns and text IDs by QUA-37 | schema 2, 5 |
| `segments`, `segment_index`, `segment_dirty` | ALG-506 | schema 4 |
| `store_control`, `schema_migrations` | QUA-37 | schema 5 |
| Asset defaults, risk capital and per-asset policies | ALG-735 | a later schema |
| The signal inbox, intents, protection and the event outbox | ALG-738 | a later schema |
| Notifications | ALG-749 | a later schema |
| Jobs and their provenance: the server's job records and the reproducibility ledger | ALG-509, ALG-516 | a later schema |
<!-- end: table owners -->

**PostgreSQL** starts at schema 5 with the same version numbers and the
same steps from there on; a step that applies to a backend that never had
the earlier schema is the whole step's DDL for that backend.

**A later step.** A story that changes the schema, such as ALG-735's
policies, ALG-738's execution store, or the jobs and provenance of ALG-509
and ALG-516:

- adds one step for the next version on both backends in the same change:
  SQLite's to `src/storage/sqlite/migrate.rs`, PostgreSQL's to
  `src/storage/postgres/migrate.rs` with its DDL in
  `src/storage/postgres/schema.rs`; a PostgreSQL chain that stops short of
  SQLite's version fails the unit tests;
- adds its tables to the DDL of both backends below and to the owners
  table;
- never edits an earlier step, deletes nothing, and keeps a row it rewrites
  beside the original;
- proves on both backends what the chain promises: a fresh store, a repeat
  that changes nothing, stores starting together that apply it once, a
  failure that rolls back whole, and an upgrade that keeps every row it
  found;
- keeps its records in the store the host selected, beside the candles and
  runs they describe. A job's provenance names segment hashes this store
  holds, and no release deletes a segment (see
  [Keeping segments](#keeping-segments)).

## The chain on SQLite

The chain runs when a store is opened, in `src/storage/sqlite/migrate.rs`.
Each step is one `BEGIN IMMEDIATE` transaction that re-reads the version
inside the transaction, applies the step, stamps the version and logs the
step. A step that fails rolls back whole and leaves the store at the last
completed one; two processes opening the same file serialize on the lock,
and the second finds the work done. A store from a newer build is refused
untouched (`StorageError::SchemaUnsupported`).

| Step | Build | What it does |
| --- | --- | --- |
| 0 → 1 | 0.1.0 | `meta` and `candles`. |
| 1 → 2 | 0.2.0 | `bot_runs`, `order_events` and `closed_trades`, with integer venue IDs. |
| 2 → 3 | development | Nothing: `state_json` gained a required `execution_mode`, which a run 0.2.0 wrote does not have and is never given by guesswork. |
| 3 → 4 | development | The segment tables, and one dirty mark per window that holds candles, so the next refresh packs the candles the earlier build stored. |
| 4 → 5 | QUA-37 | The identity columns of `bot_runs` (every existing run `legacy`, its `state_json` copied to `legacy_state_json`); `order_events` and `closed_trades` rebuilt with text IDs, every row and its `seq` copied, `raw_json` copied to `legacy_raw_json`; the numeric IDs inside `state_json` and `raw_json` rewritten as strings; `store_control` with a fresh ID, no environment, `inactive`, epoch 0; `schema_migrations`. |

## The chain on PostgreSQL

The chain runs when `PostgresStore::init` opens a store, in
`src/storage/postgres/migrate.rs`; the DDL is in
`src/storage/postgres/schema.rs`. Each pass is one transaction whose first
statement takes the advisory lock `pg_advisory_xact_lock(5856459913882324289)`
(the bytes of `QFSCHEMA`), which PostgreSQL scopes to the database and
releases when the transaction ends. Under the lock the pass reads the
version, applies the step that leaves it (on a database without a schema,
the whole chain), stamps the version, logs the step and commits.
PostgreSQL's DDL is transactional, so a step that fails rolls back whole
and leaves the store at the last completed version, and processes starting
together wait on the lock and find the work done. The transaction is `READ
COMMITTED` whatever the server's default: a `REPEATABLE READ` snapshot
would be taken at the lock statement, before the previous holder
committed, and the waiting process would apply the step again.

| Step | Build | What it does |
| --- | --- | --- |
| 0 → 5 | QUA-125 | Every table of schema 5, as [below](#postgresql-ddl), and the store's control record: a fresh ID, no environment, `inactive`, epoch 0. Logged as one step from 0. |

Refused untouched, before anything is created:

- a newer schema (`StorageError::SchemaUnsupported`);
- a schema that holds any of the tables below but no `meta`, or a `meta`
  that records no version: it is not a QuantForge store;
- a role whose `search_path` names no schema that exists.

The tables are created unqualified, in the first schema of the role's
`search_path` that exists, normally `public`: one store per database, as
one store per SQLite file. Give QuantForge a database of its own and a role
that may create tables in it. Nothing needs an extension. A pass is one
storage call, so a step finishes within the statement timeout (30 s) and
the call's deadline; a pass that loses its answer is
`StorageError::OutcomeUnknown`, and running `init` again is always safe.

## Legacy runs

A run written before schema 5 has `identity_source = 'legacy'`, no
environment and no account; a run 0.2.0 wrote has no `execution_mode` in
its `state_json` either. The migration marks and keeps such runs; it never
rewrites their meaning. `StoreControl::legacy_runs` lists them, and
`StoreControl::adopt_run` gives one the identity an operator verified
(`quantforge store adopt-run`): the store's environment, which must be
bound, and the account behind the credentials in hand for a live run.
A run without an execution mode takes the one the operator states, and the
record it had before is kept in `legacy_state_json`. Loading such a run
before its adoption fails with the adoption command in the message, and
`trade run --run-id` and `trade close` refuse a legacy run.

## Writing the journal

What every backend's run journal promises the process writing it, and
what it leaves to that process. The shared conformance cases hold both
backends to it, and `tests/live_engine.rs` runs the engine's dry runs and
resume checks on both.

- **Each write stands alone.** Creating a run, saving its state, and
  appending an order event or a closed trade are one statement each; a
  refused or failed write leaves nothing of itself. No write covers a state
  and an event together: the engine saves a run's position before it
  appends the order event that changed it, and a failed append never rolls
  that save back (safety rail 5), so an executed order's position is never
  lost to a journal row.
- **Nothing is retried below the caller.** A store never retries a write;
  a write whose answer was lost, or that the disk failed at its commit, is
  `StorageError::OutcomeUnknown`, for the caller to look at before writing
  again. Retrying a write and submitting an order are separate decisions: a
  failed save never sends an order again.
  [storage-durability.md](storage-durability.md) says what an answer means
  and how long one may take.
- **One process drives a run at a time.** The journal does not arbitrate
  between processes driving one run: the last save wins whatever its times
  (`journal.last_save_wins`), and appends never conflict, each a row of its
  own numbered by a sequence that never reuses a value, so two writers'
  rows interleave and each writer's keep their order. Who may drive is
  decided above the journal, by the leases and the arming guard of ALG-738
  and ALG-767.
- **A run belongs to its market and identity.** The latest run is found by
  exchange, symbol, interval and strategy, never across exchanges; a run
  resumed by its ID is checked against the invocation's market, strategy,
  execution mode, environment and account before anything is driven, and a
  legacy run is never resumed. Run IDs that tie compare byte by byte, as
  SQLite compares text; PostgreSQL sorts them `COLLATE "C"` whatever the
  database's collation.
- **Raw, not semantic.** The journal records what it is given: the same
  order event twice is two rows, and nothing here deduplicates signals or
  orders. ALG-738's signal inbox, intents and execution ledger do that, in
  transactions and tables of their own.
- **Closed trades keep their Spot meaning.** A `closed_trades` row is a
  Spot round trip as QuantForge 0.2.0 defined it: its `gross_quote_pnl` is
  the exit price less the entry price, times the quantity, with no fee
  taken. A futures position's gross, fees, funding and net result belong to
  ALG-738's execution ledger, in the accounting types of ALG-733, never to
  these columns.

## SQLite DDL

The schema a fresh store ends up with, and every migrated store equals
(`tests/storage/migration.rs` compares their columns and indexes). The
step DDL is in `src/storage/sqlite/`, where the text 0.1.0 and 0.2.0 wrote
is kept verbatim.

<!-- begin: sqlite schema -->
```sql
CREATE TABLE meta (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE candles (
  exchange TEXT NOT NULL,
  symbol TEXT NOT NULL,
  interval TEXT NOT NULL,
  open_time_ms INTEGER NOT NULL,
  close_time_ms INTEGER NOT NULL,
  open TEXT NOT NULL,
  high TEXT NOT NULL,
  low TEXT NOT NULL,
  close TEXT NOT NULL,
  volume TEXT NOT NULL,
  trades INTEGER,
  PRIMARY KEY (exchange, symbol, interval, open_time_ms)
);
CREATE INDEX idx_candles_market_time
  ON candles(exchange, symbol, interval, open_time_ms);

CREATE TABLE bot_runs (
  run_id TEXT PRIMARY KEY,
  exchange TEXT NOT NULL,
  symbol TEXT NOT NULL,
  interval TEXT NOT NULL,
  strategy_name TEXT NOT NULL,
  status TEXT NOT NULL,
  state_json TEXT NOT NULL,
  started_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  stopped_at_ms INTEGER,
  last_error TEXT,
  environment TEXT,
  account_kind TEXT,
  account_id TEXT,
  identity_source TEXT NOT NULL DEFAULT 'legacy',
  identity_at_ms INTEGER,
  legacy_state_json TEXT
);
CREATE INDEX idx_bot_runs_market
  ON bot_runs(exchange, symbol, interval, strategy_name, updated_at_ms DESC);

CREATE TABLE order_events (
  seq INTEGER PRIMARY KEY AUTOINCREMENT,
  run_id TEXT NOT NULL,
  symbol TEXT NOT NULL,
  side TEXT NOT NULL,
  order_type TEXT NOT NULL,
  status TEXT NOT NULL,
  order_id TEXT,
  client_order_id TEXT,
  transact_time_ms INTEGER,
  raw_json TEXT NOT NULL,
  legacy_raw_json TEXT
);
CREATE INDEX idx_order_events_run
  ON order_events(run_id, seq DESC);

CREATE TABLE closed_trades (
  seq INTEGER PRIMARY KEY AUTOINCREMENT,
  run_id TEXT NOT NULL,
  symbol TEXT NOT NULL,
  entry_time_ms INTEGER NOT NULL,
  exit_time_ms INTEGER NOT NULL,
  entry_price TEXT NOT NULL,
  exit_price TEXT NOT NULL,
  qty TEXT NOT NULL,
  gross_quote_pnl TEXT NOT NULL,
  entry_order_id TEXT,
  exit_order_id TEXT
);
CREATE INDEX idx_closed_trades_run
  ON closed_trades(run_id, seq DESC);

CREATE TABLE segments (
  hash TEXT PRIMARY KEY,
  format INTEGER NOT NULL,
  payload_len INTEGER NOT NULL,
  data BLOB NOT NULL
);

CREATE TABLE segment_index (
  exchange TEXT NOT NULL,
  symbol TEXT NOT NULL,
  interval TEXT NOT NULL,
  window_start_ms INTEGER NOT NULL,
  first_open_ms INTEGER NOT NULL,
  last_open_ms INTEGER NOT NULL,
  bar_count INTEGER NOT NULL,
  hash TEXT NOT NULL,
  PRIMARY KEY (exchange, symbol, interval, window_start_ms)
);

CREATE TABLE segment_dirty (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  exchange TEXT NOT NULL,
  symbol TEXT NOT NULL,
  interval TEXT NOT NULL,
  window_start_ms INTEGER NOT NULL,
  UNIQUE (exchange, symbol, interval, window_start_ms)
);

CREATE TABLE store_control (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  store_id TEXT NOT NULL,
  environment TEXT,
  activation TEXT NOT NULL,
  activation_epoch INTEGER NOT NULL,
  changed_at_ms INTEGER NOT NULL
);

CREATE TABLE schema_migrations (
  version INTEGER PRIMARY KEY,
  from_version INTEGER NOT NULL,
  applied_at_ms INTEGER NOT NULL,
  quantforge_version TEXT NOT NULL
);
```
<!-- end: sqlite schema -->

## PostgreSQL DDL

The schema `PostgresStore::init` creates, as it runs it:
`src/storage/postgres/schema.rs` holds this text, and a unit test holds
the two equal line for line. `tests/storage/postgres/schema.rs` compares
the catalog of a fresh database with it, and `tests/storage/docs.rs`
checks that it names the tables and columns of the SQLite schema and
nothing else. The mapping follows ADR 0001:

- Decimals stay `TEXT`, written with `to_string`, so PostgreSQL returns
  the text SQLite returns, scale included (`1.10` is never `1.1`). No
  `NUMERIC` or `REAL` column, and no `CHECK` on a decimal, changes or
  refuses text: a store holds what was written, inexact text written
  behind its back included, and a read refuses what it cannot hold
  exactly, as on SQLite ([Identities](#identities)).
- Timestamps are `BIGINT` milliseconds, and a trade count is a `u64`
  stored in a signed column, as on SQLite.
- A sequence number is an identity column that never reuses a value, as
  `AUTOINCREMENT` never does.
- `INSERT OR REPLACE` becomes `ON CONFLICT ... DO UPDATE` with a fresh
  identity value (`SET id = DEFAULT`), and `IS NOT` becomes `IS DISTINCT
  FROM`.

<!-- begin: postgres schema -->
```sql
CREATE TABLE meta (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE candles (
  exchange TEXT NOT NULL,
  symbol TEXT NOT NULL,
  interval TEXT NOT NULL,
  open_time_ms BIGINT NOT NULL,
  close_time_ms BIGINT NOT NULL,
  open TEXT NOT NULL,
  high TEXT NOT NULL,
  low TEXT NOT NULL,
  close TEXT NOT NULL,
  volume TEXT NOT NULL,
  trades BIGINT,
  PRIMARY KEY (exchange, symbol, interval, open_time_ms)
);

CREATE TABLE bot_runs (
  run_id TEXT PRIMARY KEY,
  exchange TEXT NOT NULL,
  symbol TEXT NOT NULL,
  interval TEXT NOT NULL,
  strategy_name TEXT NOT NULL,
  status TEXT NOT NULL,
  state_json TEXT NOT NULL,
  started_at_ms BIGINT NOT NULL,
  updated_at_ms BIGINT NOT NULL,
  stopped_at_ms BIGINT,
  last_error TEXT,
  environment TEXT,
  account_kind TEXT,
  account_id TEXT,
  identity_source TEXT NOT NULL DEFAULT 'legacy',
  identity_at_ms BIGINT,
  legacy_state_json TEXT
);
CREATE INDEX idx_bot_runs_market
  ON bot_runs(exchange, symbol, interval, strategy_name, updated_at_ms DESC);

CREATE TABLE order_events (
  seq BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  run_id TEXT NOT NULL,
  symbol TEXT NOT NULL,
  side TEXT NOT NULL,
  order_type TEXT NOT NULL,
  status TEXT NOT NULL,
  order_id TEXT,
  client_order_id TEXT,
  transact_time_ms BIGINT,
  raw_json TEXT NOT NULL,
  legacy_raw_json TEXT
);
CREATE INDEX idx_order_events_run
  ON order_events(run_id, seq DESC);

CREATE TABLE closed_trades (
  seq BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  run_id TEXT NOT NULL,
  symbol TEXT NOT NULL,
  entry_time_ms BIGINT NOT NULL,
  exit_time_ms BIGINT NOT NULL,
  entry_price TEXT NOT NULL,
  exit_price TEXT NOT NULL,
  qty TEXT NOT NULL,
  gross_quote_pnl TEXT NOT NULL,
  entry_order_id TEXT,
  exit_order_id TEXT
);
CREATE INDEX idx_closed_trades_run
  ON closed_trades(run_id, seq DESC);

CREATE TABLE segments (
  hash TEXT PRIMARY KEY,
  format INTEGER NOT NULL,
  payload_len INTEGER NOT NULL,
  data BYTEA NOT NULL
);

CREATE TABLE segment_index (
  exchange TEXT NOT NULL,
  symbol TEXT NOT NULL,
  interval TEXT NOT NULL,
  window_start_ms BIGINT NOT NULL,
  first_open_ms BIGINT NOT NULL,
  last_open_ms BIGINT NOT NULL,
  bar_count INTEGER NOT NULL,
  hash TEXT NOT NULL,
  PRIMARY KEY (exchange, symbol, interval, window_start_ms)
);

CREATE TABLE segment_dirty (
  id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  exchange TEXT NOT NULL,
  symbol TEXT NOT NULL,
  interval TEXT NOT NULL,
  window_start_ms BIGINT NOT NULL,
  UNIQUE (exchange, symbol, interval, window_start_ms)
);

CREATE TABLE store_control (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  store_id TEXT NOT NULL,
  environment TEXT,
  activation TEXT NOT NULL,
  activation_epoch INTEGER NOT NULL,
  changed_at_ms BIGINT NOT NULL
);

CREATE TABLE schema_migrations (
  version INTEGER PRIMARY KEY,
  from_version INTEGER NOT NULL,
  applied_at_ms BIGINT NOT NULL,
  quantforge_version TEXT NOT NULL
);
```
<!-- end: postgres schema -->

The indexes are SQLite's, except where a query plan says otherwise;
[postgres-query-plans.md](postgres-query-plans.md) records, on
representative data, the index each query the stores run is planned on:

<!-- begin: postgres index differences -->
- `idx_candles_market_time`: not on PostgreSQL. It repeats the candles
  primary key column for column; the key serves every candle query, and a
  copy would double every upsert's index writes on the largest table
<!-- end: postgres index differences -->

A BRIN index on `candles(open_time_ms)` was evaluated and is not built: no
query the stores run picks it, since every candle query names its market
([the evaluation](postgres-query-plans.md#brin)).

## Segments on PostgreSQL

The segment tables are SQLite's, and so is the protocol over them, which
the PostgreSQL segment store (QUA-112) follows. A refresh's loop and its
packing are the same code on both backends (`src/storage/refresh.rs`), and
`src/storage/postgres/segments/` is this protocol:

- **Blobs live in the store's own database.** `segments.data` is `BYTEA`,
  so PostgreSQL mode needs no SQLite file and no blob store beside it, and a
  blob commits with its index entry. A segment's payload is at most 266,416
  bytes before compression, too small for a separate blob store to repay the
  second system to run.
- **Blobs are immutable.** A segment row is inserted with `ON CONFLICT
  (hash) DO NOTHING` and never updated: a replaced segment stays loadable,
  and a hash a job's provenance names (ALG-516) stays there. Only an index
  entry moves to another hash, or goes when its window has no candles left.
- **Every mark is a revision.** Writing candles that change a window marks
  it with `INSERT ... ON CONFLICT (exchange, symbol, interval,
  window_start_ms) DO UPDATE SET id = DEFAULT`, in the writer's
  transaction: the mark's `id` is a fresh, larger revision each time.
- **Reads are one snapshot.** A refresh reads a window, and a plan its
  tip, dirty marks and index entries, each in one read-only `REPEATABLE
  READ` transaction. A refresh then packs with no transaction open, on the
  caller's thread, never on the bridge's workers.
- **A write-back publishes only what it read.** One `READ COMMITTED`
  transaction locks the window's mark (`SELECT id ... FOR UPDATE`), which
  holds back a writer re-marking it; finds none (another refresh did it:
  `elsewhere`), another revision (a writer marked it again: `dirty`), or
  the revision it read, in which case it inserts the blob, writes or
  removes the index entry, and deletes the mark by that revision, then
  commits. A plan never sees an index entry without its blob, or a window
  packed from candles older than its mark.
- **A lost answer repeats safely.** A write-back that loses its answer is
  `OutcomeUnknown`. Refreshing again is safe: the mark decides every step,
  so the window is found done or packed once more.

## Keeping segments

No release deletes a segment, on either backend. A segment a correction
replaced stays stored and opens under its name, because a job's provenance
(ALG-516) names the segments it read by hash, and a rerun must find their
bytes. The shared suite holds this on both backends with
`segments.correction_keeps_old_segment`, `segments.backfill_keeps_old_segment`
and `segments.emptied_window`.

Replaced segments take up space in proportion to what corrections replace.
A later story may reclaim it with an operator command. Nothing in 0.3 does,
and the rules below need no schema change:

- **What counts as a reference.** A segment may go only when no
  `segment_index` row and no retained provenance record names it. Both are
  read when the command runs; no reference is counted or cached.
- **Retention belongs to the records.** ALG-516 decides how long a job's
  record is kept, and a hash stays referenced for as long as a record names
  it. A segment has no clock of its own, and without that ledger nothing can
  be collected at all.
- **The check and the delete are one transaction that keeps write-backs
  out.** A write-back can name a stored blob again at any moment, and `ON
  CONFLICT (hash) DO NOTHING` locks no row. So the transaction starts with
  `BEGIN IMMEDIATE` on SQLite and `LOCK TABLE segments IN SHARE MODE` on
  PostgreSQL. A write-back writes `segments` before its index entry, so it
  has either committed, and its entry is seen, or it waits and stores its
  blob again.
- **An operator runs it, nothing else.** A plan handed out before it ran can
  name a segment it removed: `load_segment` answers `None`, and the reader
  plans the range again.

## The shared cases

[`tests/fixtures/storage_identity_cases.json`](../tests/fixtures/storage_identity_cases.json)
holds, for every product line, the storage columns, the wire spelling and
the `state_json` token of one market, and the decimal texts a store must
return unchanged. They were written by an independent Python script,
`spikes/storage-contract/identity_cases.py`, not by the Rust code under
test, and a backend conforms when it stores and returns every case as
given. The shared storage conformance suite,
[storage-conformance.md](storage-conformance.md), runs them on every
backend beside its own scenarios: how candles are written and read, how
runs and their journals are kept, legacy runs included, and how segments
are packed and planned.

## Backends and ownership

A host selects one backend with `--storage` (`StorageSettings::resolve`)
and opens it through `StoreFactory`, which hands back an `AnyStore` that
serves every port above; [configuration.md](configuration.md#storage-backends)
is the operator's side. The factory opens only the backend selected: a
backend without a registered provider is refused, never replaced by
another. PostgreSQL's stores all exist (QUA-124, QUA-123, QUA-112), and an
`AnyStore` can hold one; QUA-116 registers its provider. Until then a
PostgreSQL selection is checked and refused.

The process that opens a store owns it, its pool and its credentials, for
its whole life, and no other process receives them (ADR 0001): a CLI
command, the server's parent process, later the Python reader. A job
worker never opens a store and never receives its settings; the parent
reads the candles a job needs and passes them on. The execution store of
ALG-738 is opened the same way, from the same settings, in the same
process.
