# PostgreSQL query plans

The evidence behind the PostgreSQL schema's indexes (QUA-125): on
representative data, the index every query the stores run is planned on,
the one SQLite index PostgreSQL leaves out, and the BRIN index it does not
build; and how long a batch of 100,000 candles takes to write on each
backend (QUA-124). [storage-schema.md](storage-schema.md#postgresql-ddl)
holds the schema; this page holds the measurements.

`tests/storage/postgres/plans.rs` reproduces it. It seeds the data below in
a database of its own, asserts that every query is planned on the indexes
listed here and never scans whole a table its `never_scanned` list names
(every table it reads, except `bot_runs` for `runs.legacy`, which scans it
by design), asserts that no query picks the BRIN index, and prints every
plan with its timings. The list between the markers is held to the test's
own.

## Contents

- [The run](#the-run)
- [The data](#the-data)
- [The queries](#the-queries)
- [The candles key and its copy](#the-candles-key-and-its-copy)
- [BRIN](#brin)
- [Writing candles](#writing-candles)
- [Reproducing it](#reproducing-it)

## The run

PostgreSQL 16.15 from Homebrew, the oldest version ADR 0001 supports, on
macOS 26.4.1 (arm64), with `initdb`'s default settings (128 MB of shared
buffers), on 2026-11-01; the segment store's queries were measured again
on 2026-11-04, when QUA-112 gave them the store's own shape, and the two
steps of the candle store's walk over its markets on 2026-11-08, when
QUA-359 added it, against PostgreSQL 16.15 in Docker
([ci.md](ci.md#postgresql)'s local servers). Each query was
explained with literal values, reads with `EXPLAIN (ANALYZE, BUFFERS)` and
writes without running them.
The PostgreSQL stores send the same values as parameters of unnamed
statements (tokio-postgres' `query_typed`), which PostgreSQL plans when
they are bound, for those values, as it plans the literals.
CI runs the test on PostgreSQL 16, 17 and 18 ([ci.md](ci.md#postgresql)):
every query kept the indexes listed here on 17.11 and 18.6 too, on
2026-11-07.

## The data

A store some years in:

- 8 markets of `1m` candles: BTCUSDT on Binance and Bybit Spot and linear,
  BTCUSD on both venues' inverse lines, and ETHUSDT on Binance Spot and
  Bybit linear; 30 days each from `1790208000000`, and 2 years of `1h`
  candles for 4 of them: 415,680 candles;
- 2,000 runs over the 8 markets and 3 strategies, with ties in
  `updated_at_ms`, and 100,000 order events and 30,000 closed trades over
  them;
- 3 years of daily segments for each `1m` market before its candles: 8,760
  segments and index entries, and a dirty mark on every window, as a deep
  backfill leaves them before its refresh.

The data is written twice, each time to a database of its own: market by
market, as a backfill writes candles, and minute by minute across the
markets, as following them live does. The order decides where a market's
rows land in the table, and so what a scan by time costs.

## The queries

Every statement shape the SQLite stores run, as the PostgreSQL stores run
it, with the indexes its plan uses:

<!-- begin: queries -->
- `candles.load`: `candles_pkey`
- `candles.recent`: `candles_pkey`
- `candles.max_open_time`: `candles_pkey`
- `candles.first_market`: `candles_pkey`
- `candles.next_market`: `candles_pkey`
- `candles.upsert`: `candles_pkey`
- `candles.window`: `candles_pkey`
- `runs.latest`: `idx_bot_runs_market`
- `runs.by_id`: `bot_runs_pkey`
- `runs.legacy`: none
- `order_events.latest`: `idx_order_events_run`
- `closed_trades.latest`: `idx_closed_trades_run`
- `segments.lowest_dirty`: `segment_dirty_exchange_symbol_interval_window_start_ms_key`
- `segments.mark`: `segment_dirty_exchange_symbol_interval_window_start_ms_key`
- `segments.mark_of_window`: `segment_dirty_exchange_symbol_interval_window_start_ms_key`
- `segments.lock_mark`: `segment_dirty_exchange_symbol_interval_window_start_ms_key`
- `segments.entry_of_window`: `segment_index_pkey`
- `segments.plan`: `segment_index_pkey`, `segments_pkey`
- `segments.blob`: `segments_pkey`
- `segments.store`: `segments_pkey`
- `segments.index`: `segment_index_pkey`
- `segments.unindex`: `segment_index_pkey`
- `segments.unmark`: `segment_dirty_pkey`
<!-- end: queries -->

Both orders gave the same plans. The figures are from the data written
market by market; where the other order differs, it is in brackets.

| Query | Run by | Plan | Rows | Buffers | Time |
| --- | --- | --- | --- | --- | --- |
| `candles.load` | `load_candles` with a range and a limit | Index Scan on `candles_pkey`, under the limit | 1,000 | 24 (131) | 0.26 ms (0.31) |
| `candles.recent` | `load_recent_candles` | Index Scan Backward on `candles_pkey` | 300 | 13 (43) | 0.09 ms (0.12) |
| `candles.max_open_time` | `max_open_time_ms`, and a segment refresh's tip | Index Only Scan Backward on `candles_pkey` | 1 | 4 | 0.04 ms (0.06) |
| `candles.first_market` | `market_coverage`, for the first market | Index Only Scan on `candles_pkey`, under the limit | 1 | 4 | 0.03 ms |
| `candles.next_market` | `market_coverage`, for each market after it | Index Only Scan on `candles_pkey` from the key before, under the limit | 1 | 4 (5) | 0.05 ms (0.04) |
| `candles.upsert` | `upsert_candles` | The batch read from its unnested arrays, conflicts checked on `candles_pkey` | | | |
| `candles.window` | a refresh reading one window | Bitmap Index Scan on `candles_pkey`, then a sort | 1,440 | 36 (190) | 0.52 ms (0.78) |
| `runs.latest` | `latest_run_for_market` | Index Scan on `idx_bot_runs_market`, the tie-breakers sorted incrementally, run IDs in byte order | 1 of 3 | 13 | 0.04 ms |
| `runs.by_id` | `load_run_state`, `run_identity` | Index Scan on `bot_runs_pkey` | 1 | 3 | 0.03 ms (0.02) |
| `runs.legacy` | `StoreControl::legacy_runs` (`store status`) | Seq Scan of `bot_runs`, then a sort | 0 of 2,000 | 54 | 0.23 ms (0.20) |
| `order_events.latest` | `list_order_events` | Bitmap Index Scan on `idx_order_events_run`, then a sort | 50 | 56 | 0.15 ms (0.13) |
| `closed_trades.latest` | `list_closed_trades` | Bitmap Index Scan on `idx_closed_trades_run`, then a sort | 15 | 20 | 0.06 ms |
| `segments.lowest_dirty` | a refresh's next window, and a plan's effective tip | Index Only Scan on the dirty marks' unique key | 1 | 3 | 0.06 ms (0.05) |
| `segments.mark` | `upsert_candles` marking the windows it changed | The windows unnested from one array, conflicts checked on the dirty marks' unique key | | | |
| `segments.mark_of_window` | a refresh's read | Index Scan on the dirty marks' unique key | 1 | 3 | 0.02 ms |
| `segments.lock_mark` | a write-back, before it decides anything | The same scan, under `LockRows` | 1 | 4 | 0.04 ms |
| `segments.entry_of_window` | a refresh comparing the hash it packed | Index Scan on `segment_index_pkey` | 1 | 3 | 0.03 ms |
| `segments.plan` | `resolve_segments` | Bitmap Index Scan on `segment_index_pkey`, then a nested loop on `segments_pkey` | 30 | 127 | 0.10 ms |
| `segments.blob` | `load_segment` | Index Scan on `segments_pkey` | 1 | 4 | 0.03 ms |
| `segments.store` | a write-back storing its blob | One row, conflicts checked on `segments_pkey` | | | |
| `segments.index` | a write-back writing its index entry | One row, conflicts checked on `segment_index_pkey` | | | |
| `segments.unindex` | a write-back for a window with no candles left | Index Scan on `segment_index_pkey` | | | |
| `segments.unmark` | a write-back ending the mark by its revision | Index Scan on `segment_dirty_pkey` | | | |

Two plans are worth knowing:

- `runs.legacy` is the one query no index serves. `store status` lists the
  legacy runs once per command by reading every run; a store holds
  thousands of runs at most, and SQLite has no index for it either.
- `segments.plan` bounds `window_start_ms` from below as well as above: an
  entry that overlaps the range starts no earlier than the window of the
  range's first open time, so the bound changes no answer, and the key reads
  only the 30 entries in range. Without it the query read the market's 1,095
  entries below the tip and kept those 30, at 154 buffers and 0.28 ms.

## The candles key and its copy

SQLite's `idx_candles_market_time` indexes the candles' key columns in the
key's order, so it repeats the primary key's own index column for column;
PostgreSQL leaves it out ([the
difference](storage-schema.md#postgresql-ddl)). Every candle query above is
planned on `candles_pkey`. The copy's cost, inserting the 345,600 `1m`
candles minute by minute across the markets into the candles table alone,
three runs each:

| Indexes | Insert | Index size | WAL written |
| --- | --- | --- | --- |
| `candles_pkey` | 1.21 to 1.22 s | 29 MB | about 91 MB |
| `candles_pkey` and the copy | 2.01 to 2.05 s | 59 MB | about 134 MB |

## BRIN

A BRIN index on `candles(open_time_ms)` was built over both orders of the
data and is not part of the schema:

- It is small: 24 kB, against 34 to 36 MB for `candles_pkey` and 47 MB
  for the table.
- No query above picks it, in either order; the test asserts this, with
  the index in place.
- The one shape it serves is a scan of every market's candles by time,
  which no store runs. For one hour across the 8 markets it read 2,048
  lossy blocks (2,056 buffers, 9.5 ms) with the rows written market by
  market, and 256 blocks (264 buffers, 1.2 ms) minute by minute.

Every candle query names its market, and the key serves it from that
market's rows alone. A BRIN index earns its place once something reads
candles by time across markets, such as a retention job; TimescaleDB's
chunks do that job in its follow-up.

## Writing candles

One `upsert_candles` of 100,000 candles, through the port, on each backend,
with no threshold behind it: the numbers describe this machine and this
dataset, nothing more.

- **The batch:** 100,000 consecutive `1m` candles of one market, about 69
  days from `1792454400000`; prices with two decimal places, volumes with
  eight, a trade count on every candle.
- **The writes,** each on a fresh store: into the empty market; the same
  batch again, which changes no row and marks no window; and the batch with
  every close a cent higher, which changes every row and marks all 70
  windows.
- **The environment:** an Apple M2 Max with 96 GB, macOS 26.4.1, rustc
  1.99.0 in release, on 2026-11-01, and SQLite again on 2026-11-04 with its
  durable settings. PostgreSQL 16.15 from Homebrew with `initdb`'s default
  settings, over TCP to `127.0.0.1`; SQLite in WAL with `synchronous = FULL`
  and `fullfsync`, on the internal SSD.

| Backend | Into the empty market | The same batch again | Every close changed |
| --- | --- | --- | --- |
| SQLite | 0.274 to 0.291 s | 0.149 to 0.151 s | 0.202 to 0.206 s |
| PostgreSQL | 0.585 to 0.619 s | 0.435 to 0.442 s | 1.077 to 1.129 s |

PostgreSQL takes the batch as one statement, its columns sent as arrays,
and the marks as a second, in one transaction; SQLite writes it row by row
in one transaction on a local file. Changing every row costs PostgreSQL
most: each update writes a new row version and a new key entry, since the
freshly filled pages leave no room to update in place (`pg_stat_user_tables`
counts none of 100,000 such updates as in place). A batch is one commit, so
SQLite's flush adds little here: at `synchronous = NORMAL` the first and last
columns took 0.260 to 0.266 s and 0.187 to 0.197 s. Many small commits are
where a flush shows ([what differs by
design](storage-durability.md#what-differs-by-design)).

## Reproducing it

With a server as in [storage-conformance.md](storage-conformance.md#postgresql):

```bash
QUANTFORGE_TEST_POSTGRES_URL='postgres://quant:local-pw@127.0.0.1:54329/postgres?sslmode=disable' \
  cargo test --test storage postgres::plans -- --nocapture --test-threads=1
```

The test prints every plan, for both orders of the data, then the BRIN
index's size and its one plan. The batch measurement runs on demand, in
release, on SQLite and on the server the variable names:

```bash
QUANTFORGE_TEST_POSTGRES_URL='postgres://quant:local-pw@127.0.0.1:54329/postgres?sslmode=disable' \
  cargo test --release --test storage measure -- --ignored --nocapture
```
