# The storage schema

**Schema 5.** Every QuantForge store holds the same tables under one schema
version: SQLite today, PostgreSQL from ALG-404 on. This page is the contract
between them: the identities their rows carry, the record a store keeps
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
- [Legacy runs](#legacy-runs)
- [SQLite DDL](#sqlite-ddl)
- [PostgreSQL DDL](#postgresql-ddl)
- [The shared cases](#the-shared-cases)

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
<!-- end: table owners -->

**PostgreSQL** starts at schema 5 with the same version numbers and the
same steps from there on; a step that applies to a backend that never had
the earlier schema is the whole step's DDL for that backend.

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

The same tables and columns for ALG-404, reviewed here and not yet
executed by any build; `tests/storage/docs.rs` checks that it names the
tables and columns of the SQLite schema and nothing else. The mapping
follows ADR 0001: decimals stay `TEXT`, written with `to_string` and read
with `from_str`, so PostgreSQL returns the same text as SQLite; timestamps
are `BIGINT` milliseconds; a sequence number is an identity column that
never reuses a value, as `AUTOINCREMENT` never does; `init` runs the chain
under `pg_advisory_lock`, so processes starting together cannot race on it;
`INSERT OR REPLACE` becomes `ON CONFLICT ... DO UPDATE` with a fresh
identity value, and `IS NOT` becomes `IS DISTINCT FROM`.

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

## The shared cases

[`tests/fixtures/storage_identity_cases.json`](../tests/fixtures/storage_identity_cases.json)
holds, for every product line, the storage columns, the wire spelling and
the `state_json` token of one market, and the decimal texts a store must
return unchanged. They were written by an independent Python script,
`spikes/storage-contract/identity_cases.py`, not by the Rust code under
test, and a backend conforms when it stores and returns every case as
given: `tests/storage/contract.rs` runs them against SQLite, and the
PostgreSQL store runs the same module.
