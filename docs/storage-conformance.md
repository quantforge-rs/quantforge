# The storage conformance suite

Every QuantForge storage backend passes one suite: the same scenarios, with
the same answers, whatever keeps the data. SQLite passes it today, directly
and through `AnyStore`; the PostgreSQL stores pass it as they land, and so
would any later backend, such as TimescaleDB. This page lists what the suite
holds a backend to, where its answers come from, and how a backend joins.
[storage-schema.md](storage-schema.md) is the schema the backends share; the
suite is the behavior behind the ports of `src/ports/storage.rs`. The
parity matrix's `storage.conformance` capability names it (ALG-402 there).

The answers are never another backend's. They are written in
[`tests/fixtures/storage_conformance_cases.json`](../tests/fixtures/storage_conformance_cases.json)
by an independent reference model, so a backend that agrees with SQLite on a
wrong answer still fails. `tests/storage/docs.rs` holds the lists below to
the fixture and to the checks coded in `tests/storage/conformance/`.

## Contents

- [What the suite holds](#what-the-suite-holds)
- [The fixtures](#the-fixtures)
- [Registering a backend](#registering-a-backend)
- [PostgreSQL](#postgresql)
- [The races](#the-races)
- [What the suite leaves open](#what-the-suite-leaves-open)

## What the suite holds

Each scenario runs on a fresh, empty store and names the answer of every
step; a group fails listing every scenario that failed. Most are written in
the fixture; the last of each list are coded in `tests/storage/conformance/`
because they read the shared cases of other documents: the identity cases of
[storage-schema.md](storage-schema.md#the-shared-cases) and the segment
cases of [segment-spec.md](segment-spec.md#the-shared-cases).

### The candle store

<!-- begin: candles -->
- `candles.empty_market`: a market without candles reads nothing
- `candles.written_counts_statements`: an upsert counts the candles it was
  given, not the rows it added
- `candles.duplicate_overwrites`: a candle written again under its open time
  replaces every other column
- `candles.later_duplicate_wins`: of two candles with one open time in a
  batch, the later is kept
- `candles.bounds_order_and_limit`: a load is ascending with both bounds
  included and keeps the oldest; recent keeps the newest, ascending
- `candles.empty_ranges`: a range holding no candle, a zero limit and a
  reversed range read nothing
- `candles.exact_bounds`: a bound on a candle's open time includes it, and
  one past it does not
- `candles.markets_isolated`: the same open time on every product line,
  symbol and interval reads back only its own candle
- `candles.decimal_scale`: every decimal column keeps the text it was
  written with, scale included
- `candles.unknown_trades`: an unknown trade count stays unknown and is not
  zero
- `candles.trade_count_range`: every trade count from 0 to the largest
  unsigned 64-bit one reads back unchanged
- `candles.unwritable_batch`: a batch holding a candle no segment window can
  hold is refused whole
- `candles.reopened`: a store opened again on the same database resumes from
  its newest candle
- `candles.identity_product_lines`: every product line of the identity cases
  stores its market under the storage spelling the cases give, and reads
  back only its own candle
- `candles.identity_decimals`: every decimal text of the identity cases is
  stored and read back unchanged
<!-- end: candles -->

### The run journal

<!-- begin: journal -->
- `journal.round_trip`: a run reads back in full: position, cursor, error
  and configuration
- `journal.unknown_run`: a run never created reads nothing
- `journal.created_once`: a run is created once, and its first identity
  stays
- `journal.saved_after_created`: a run is saved only after it was created
- `journal.save_keeps_identity`: saving replaces the state and never the
  identity
- `journal.latest_per_market`: the latest run is the last updated of its
  market and strategy; another strategy, interval, venue or family never
  wins
- `journal.latest_tie`: of runs updated in the same millisecond the one
  started later is the latest, then the greater run ID
- `journal.order_events`: order events read back newest first, up to the
  limit, exactly as appended
- `journal.duplicate_event`: an order event appended twice is listed twice
- `journal.closed_trades`: closed trades read back newest first with their
  scale and their missing order IDs
- `journal.runs_apart`: two runs' journals never mix
- `journal.reopened`: a store opened again on the same database loads a run
  for resume
- `journal.wire_forms`: columns hold the wire forms and the JSON the serde
  tokens
- `journal.legacy_unresolved`: a legacy run loads with no environment and no
  account, is found as the latest, and is never driven
- `journal.legacy_without_mode`: a legacy run that recorded no execution
  mode is refused on load, never given one
- `journal.identity_venue_ids`: venue IDs of every shape in the identity
  cases round-trip as text through both journals
- `journal.environments_and_accounts`: runs recorded under two environments
  or two accounts are never one another's, and binding a store later adopts
  no run
<!-- end: journal -->

### The segment store

Its scenarios write candles too, so a backend runs them once its candle and
segment stores both exist.

<!-- begin: segments -->
- `segments.no_candles`: a market without candles is all tip
- `segments.packs_closed_windows`: a refresh packs every closed window and
  leaves the tip
- `segments.fill_order`: candles written in any order pack to the same
  segments
- `segments.nothing_changed`: a second refresh, and an identical resync,
  change nothing
- `segments.changed_back`: a window changed and changed back keeps its
  segment
- `segments.tip_until_next_window`: the window of the newest candle stays in
  the tip until a later window starts
- `segments.backfill_keeps_old_segment`: a backfill reopens its window at
  once, the next refresh packs only that window, and the segment it replaced
  still loads
- `segments.scale_is_a_change`: a candle written again with another scale
  packs to a new segment
- `segments.plan_ranges`: a plan lists the segments a range overlaps, oldest
  first, then the tip
- `segments.long_windows`: three-day and weekly candles stay in the tip of
  their long windows
- `segments.crowded_window`: a window holding more candles than a segment
  keeps itself and every later window in the tip
- `segments.inexact_text`: stored text not in its exact form is left
  unpacked rather than corrected
- `segments.opens_to_its_candles`: a stored segment opens under its name to
  the candles it packs; an unknown name loads nothing
- `segments.shared_name`: the same candles under two product lines share one
  segment, listed by two index entries
- `segments.spec_cases`: every case of the segment spec, stored and closed
  by a later candle, packs to the case's name
<!-- end: segments -->

## The fixtures

`tests/fixtures/storage_conformance_cases.json` (format 1) holds:

- `markets`: the markets the scenarios name, by storage spelling: the same
  symbol on both venues and every family, beside another symbol and other
  intervals;
- `series_rule`: how a `series` generates its candles, so that thousands of
  candles take one line;
- `runs`, `orders` and `trades`: the records the journal scenarios write,
  every decimal as its exact text;
- `candles`, `journal` and `segments`: the scenarios, each an `id`, the
  statement listed above, and its `steps`;
- `races`: the interleavings each backend steps through itself.

A step is one operation and its answer: a result, or a refusal of a kind
(`invalid_argument`) whose message contains a given text.

| Steps | What they check |
| --- | --- |
| `upsert`, `load`, `recent`, `max_open_time` | the candle store, every candle compared as text |
| `create_run`, `save_run`, `load_run`, `run_identity`, `latest_run`, `identity_matches` | runs and their identities, every state compared as its JSON |
| `append_order_event`, `list_order_events`, `append_closed_trade`, `list_closed_trades` | the two journals |
| `refresh`, `resolve`, `load_segment` | the segment store; every listed segment must also load, open under its name, and be as long as listed |
| `raw_select`, `raw_contains`, `raw_execute`, `value_types` | the backend's own tables, for what the ports do not show: the text a column holds, a row written behind the store's back, the type a value is stored as |
| `reopen` | the same database opened again, as a restarted process opens it |

The reference model is
[`spikes/storage-contract/conformance_cases.py`](../spikes/storage-contract/conformance_cases.py),
written from this page and [segment-spec.md](segment-spec.md), stdlib Python
only. It runs every scenario on its model and records each answer. Its
segment packer is the one the spec prints, and before it writes anything it
reproduces every case of `tests/fixtures/segment_cases.json`, whose payloads
and names an earlier independent implementation computed. To change a
scenario, change the script and run it from the repository root:

```bash
python3 -I spikes/storage-contract/conformance_cases.py
```

The rules it models, beyond what the statements above say:

- a candle is keyed by its market and open time; a batch is refused whole
  when one of its candles lies too close to either end of the timestamp
  range for a segment window to hold it;
- a candle counts as changed when any column's text differs, a trade count
  known or unknown included, and a change marks its window dirty;
- a refresh packs each dirty window below the tip, the window of the newest
  candle: a window that cannot be packed (more than 1,440 candles, or a
  decimal not in its exact form) is counted blocked and stays dirty; one
  that packs to the name it is indexed under is unchanged; any other is
  built, and the segment it had stays stored;
- a plan lists the indexed windows below the lower of the tip window and
  the lowest dirty window that overlap the range, then the rest of the range
  as the tip;
- the latest run of a market and strategy is the one updated last, then the
  one started last, then the one with the greatest run ID;
- order events and closed trades are listed newest first, in the order they
  were appended, duplicates kept.

## Registering a backend

A backend joins the suite in its own file under `tests/storage/`, the way
`sqlite.rs`, `any_store.rs` and `postgres.rs` do:

1. Implement `conformance::Backend`. Its `fresh` returns a `Fixture`: a store
   that is initialized, empty, and isolated from every other store the suite
   opens; a `reopen` that opens the same database again, initialized, as a
   restarted process would; the `Raw` view below; and a guard, dropped last,
   that keeps the database alive and removes it afterwards.
2. Implement `conformance::Raw` over the backend's own client. The
   scenarios' SQL is portable: plain `SELECT`, `INSERT` and `UPDATE` on the
   tables of [storage-schema.md](storage-schema.md), every selected column
   text or `CAST(… AS TEXT)`, no function of one dialect. Only `value_types`
   is written per backend: SQLite's `typeof`, PostgreSQL's `pg_typeof`.
3. Add one test per port the backend implements, calling its group:
   `conformance::candles::<B>()` once it implements `CandleStore`,
   `conformance::journal::<B>()` with `RunJournalStore`, and
   `conformance::segments::<B>()` with `CandleStore` and `SegmentStore`.
   The groups are generic over the ports, so a backend switches each on as
   its store lands.
4. Step through the races in the backend's own unit tests, as
   `src/storage/sqlite/segments/tests.rs` does; see [the races](#the-races).

A later backend, such as TimescaleDB, registers the same way and reuses the
fixture as it is. A behavior a new backend adds is a new scenario, written
in the reference model under an `id` of its own; an existing answer never
changes for one backend.

## PostgreSQL

`tests/storage/postgres.rs` registers PostgreSQL. Every fixture creates a
database of its own, `qf_conformance_<pid>_<n>`, on the server
`QUANTFORGE_TEST_POSTGRES_URL` names, opens it through `PostgresStore`, and
drops it with `DROP DATABASE … WITH (FORCE)` (PostgreSQL 13 or later) when
the fixture goes, so scenarios never share rows and a run leaves nothing
behind. Without the variable its test skips.

The URL connects as a user allowed to create databases, over plain TCP; the
store connects to each isolated database through the checks of
[configuration.md](configuration.md#storage-backends), which allow plain
text only to a loopback host. CI's service container and a local cluster
both serve that; a local one, with Homebrew:

```bash
initdb -D pg -U quant --auth=scram-sha-256 --pwfile=<(printf 'local-pw\n')
printf "port = 54329\nlisten_addresses = 'localhost'\nunix_socket_directories = ''\n" >> pg/postgresql.conf
pg_ctl -D pg -l pg.log -w start
QUANTFORGE_TEST_POSTGRES_URL='postgres://quant:local-pw@127.0.0.1:54329/postgres?sslmode=disable' \
  cargo test --test storage postgres
pg_ctl -D pg -w stop
```

No group runs on PostgreSQL yet, because `PostgresStore` implements no port:
QUA-124 switches on the candle group, QUA-123 the journal group, and QUA-112
the segment group and the PostgreSQL race stepper, each with the schema of
QUA-125 initialized in `fresh`. QUA-118 runs them in CI against a PostgreSQL
service container.

## The races

A refresh reads a dirty window, packs it with no transaction open, then
writes it back only if no writer marked the window again meanwhile. The
ports cannot pause a refresh between those steps, so the fixture lists the
interleavings, and each backend steps through them with its own read, pack
and write-back:

<!-- begin: races -->
- `races.write_between_read_and_write_back`: a write between a refresh's
  read and its write-back keeps the window dirty, and the next refresh packs
  the new candles
- `races.two_refreshes`: of two refreshes packing one window, the later
  finds it done elsewhere
<!-- end: races -->

A write-back answers `done`, `elsewhere` when another refresh unmarked the
window first, or `dirty` when a writer marked it again; the plan and refresh
counts after each race are in the fixture too. SQLite steps through them in
`src/storage/sqlite/segments/tests.rs`.

## What the suite leaves open

- Appending an order event or a closed trade to a run that was never
  created: both backends' schemas allow it, and no command does it.
- A segment's stored bytes: their length depends on the compression level,
  which is not part of the [segment format](segment-spec.md#compression).
  The suite checks only that a plan lists the length its bytes have.
- Concurrency beyond the races above, and how fast a backend answers.
- The rest of an error's message, beyond the text a refusal names.
