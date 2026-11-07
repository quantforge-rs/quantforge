# Moving a store to PostgreSQL

`quantforge data migrate` copies a SQLite store into PostgreSQL, and with
`--verify` compares every row of the copy with its source. It is a one-way
copy of a store at rest, for a planned switch: there is no replication, and
no switch without downtime. This page says what is copied and what stays
behind, what to do before, during and after a copy, how to go back, and the
canonical text a verification's checksums are taken over.
[storage-schema.md](storage-schema.md) is the contract the copy keeps, and
[configuration.md](configuration.md#storage-backends) the selection it
writes into.

## Contents

- [What is copied](#what-is-copied)
- [Before you start](#before-you-start)
- [Running a copy](#running-a-copy)
- [A copy cut off, or run again](#a-copy-cut-off-or-run-again)
- [Verification](#verification)
- [Switching over](#switching-over)
- [Going back](#going-back)
- [The canonical text](#the-canonical-text)
- [Extending a transfer](#extending-a-transfer)

## What is copied

Every row of these tables, in this order, each read in order of its key:
a run before its journals, a segment's bytes before the index entry that
names them, candles before the marks on their windows. Each table's columns
are those of the [DDL](storage-schema.md#sqlite-ddl).

<!-- begin: transfer tables -->
- `candles`: market by market, by `open_time_ms`
- `bot_runs`: by `run_id`, byte by byte
- `order_events`: by `seq`, a number the store gave
- `closed_trades`: by `seq`, a number the store gave
- `segments`: by `hash`, byte by byte
- `segment_index`: market by market, by `window_start_ms`
- `segment_dirty`: by `id`, a number the store gave
<!-- end: transfer tables -->

Nothing is parsed or rewritten on the way: a decimal keeps its exact text
and scale (`1.10` stays `1.10`, and text no `Decimal` holds is copied as it
is and refused alike by both stores when read), IDs and sequence numbers
keep their values, timestamps their milliseconds, run state and order JSON
their bytes, and a segment its bytes, a replaced one too. A dirty mark keeps
its revision. The target's own numbering moves past the numbers copied, so
the next event, trade or mark it writes takes a new one.

Identities are copied as they are, never guessed: a run keeps the
environment, account and identity source it recorded; a legacy run stays
legacy, with no environment and no account, until an operator adopts it
with `store adopt-run` against the target, as on the source.

These stay behind, because every store keeps its own:

<!-- begin: kept by each store -->
- `meta`
- `store_control`
- `schema_migrations`
<!-- end: kept by each store -->

The target has its own schema version and migration log, and its own
record: a new store ID, `inactive`, epoch 0. Copying never gives a store
execution authority (storage-schema.md, [The store's
record](storage-schema.md#the-stores-record)). The source's environment is
carried over on its own: a target bound to no environment is bound to the
source's, and one bound to another is refused. On the target, `meta` holds
one more row, `transfer`, the record of the copy it holds.

## Before you start

1. **Stop every process that writes to the source**: `trade run`, `data
   sync` and `serve run`, and every `monitor` or `trade close` action.
   `data migrate` reads one snapshot of the source, so a writer still
   running cannot make the copy inconsistent, but what it writes after the
   snapshot is not in the copy. It refuses a source whose runs recorded no
   end, `starting` or `running`, and names them. A process that died leaves
   its run `running`: once you have checked that nothing writes to the
   source, pass `--assume-quiescent`. A source that changes while it is
   copied or verified fails the command, whatever the flag says.
2. **Bring the source to this build's schema.** A copy reads the schema
   this build writes, and changes nothing in the source, so it never
   migrates it. A store an earlier release wrote is refused; any command of
   this build brings it forward in place, `quantforge --db
   data/market.sqlite store status` for one, after the backup
   [upgrading.md](upgrading.md) describes.
3. **Create an empty database** that the role owns
   ([The PostgreSQL database](configuration.md#the-postgresql-database)).
   `data migrate` refuses a target that holds rows of its own, or the copy
   of another store; it looks as the copy begins, with every writer held
   off, so that nothing slips in between.

The source is opened read-only. Its database file keeps every byte, and so
does its `-wal` file. SQLite may leave an empty `-wal` file and a `-shm`
file beside a database a reader opened, as any reader does; the next
process that writes to the file uses them.

## Running a copy

The target is the store `--storage postgres` and `--postgres-url` select, as
for every other command; the source is `--from`:

```bash
export QF_POSTGRES_URL=postgres://USER:PASSWORD@HOST:5432/DATABASE
quantforge --storage postgres data migrate --from data/market.sqlite --verify
```

It prints the store it writes to, the source and its store ID, how the copy
began (`started`, `resumed`, or `already copied`), what it read from each
table and wrote of it, and the record the target keeps, such as `transfer:
copied from 3f2… at 2026-11-07T13:35:10.506Z`. Progress goes to the log
every ten seconds and after each table, with no credential in it. The
warning that the database held no schema and was created is expected on
the first copy into a new database.

`--batch-size` sets the rows read and written at a time, 5000 unless given;
a batch holds at most 64 segments, whose bytes are large. A smaller batch
gives up less work when a copy is cut off, and a larger one makes fewer
round trips.

## A copy cut off, or run again

Every batch is one statement that writes only the rows the target does not
hold yet. A copy cut off, by a failed statement, a lost connection or a
killed process, leaves whole batches behind, and the same command run again
finishes it, writing each row once. The error says where it stopped:

```text
Error: the copy stopped in closed_trades; the rows it wrote stay, and running it again finishes it
```

Until a copy finishes, the target is part of a store: every other command
refuses it, a process that had it open before the copy began has every
write refused, and `store status` shows `transfer: copying from …`. A finished
copy is final: running `data migrate` again writes nothing, and with
`--verify` only verifies.

Keep the source unchanged between attempts. An earlier attempt read an
earlier snapshot, and a copy never rewrites a row the target holds, so a
copy that resumes first compares the rows the target holds, those already
copied, with the source, as a verification does. Only those are checked:
a row not copied yet may have been changed or deleted since without the
command noticing, and needs no check, because the copy takes the source as
it is now. A source that only gained rows passes, and the copy takes them.
A row already copied that the source has since changed or deleted fails
the command before anything is written, naming the first:

```text
Error: transfer refused: the source changed since this copy began: the target holds 1 row(s) the source has since changed or deleted (first: …); a copy never rewrites a row it holds, so this one cannot be finished, but the source is untouched: give the copy a new, empty database and run the command again
```

That copy stays unfinished, and running the command again refuses it
again. Give the copy a new, empty database, by dropping this one and
creating it again or by creating another, and copy into it.

## Verification

`--verify` reads both stores side by side, after the copy, in key order,
and compares every row. For each table, and each market of a market table,
it prints what each side read: the rows, the first and last key when the
key is an integer, and the checksum of [the canonical text](#the-canonical-text):

```text
verified: candles binance_spot BTCUSDT 1m rows=5 open_time_ms=1792454400000..1792540860000 checksum=sha256:3f52…
```

A table or market that differs prints both sides as `mismatch:`, then the
differences, the first 20 of them in full: a row only in the source, a row
only in the target, or a row whose columns differ, with the first column's
values; a long text is shown around where it differs, a blob by its length.

```text
differences: 2
  candles binance_spot BTCUSDT 1m open_time_ms=1792454520000: close differs: source "67013.5", target "67013.50"
  order_events seq=9: only in the source
verify: failed
```

Then the command exits with status 1.

Every table is read through an index except where text keys must come in
byte order on PostgreSQL: no index of the schema serves `COLLATE "C"`, so
each batch of `segments` and `bot_runs` sorts the table's keys. On
PostgreSQL 16, verifying 100,000 segments took 11 seconds that way, and the
time grows with the square of their number. A copy reads only the SQLite
source, in its own order, except when it resumes: then it reads the rows
the target holds as a verification does. A difference means the target is
not a copy of the source as it is now: the source was written after the
copy began, the target was written after it finished, or one of them was
changed behind QuantForge's back. The source is untouched, so a new copy
into an empty database starts over cleanly.

## Switching over

1. Stop every process that uses the source, and check `store status` on it.
2. Copy and verify: `data migrate --from <the file> --verify`, until it
   prints `verify: ok`.
3. Select the target in every process: `--storage postgres` or `QF_STORAGE=postgres`,
   and `QF_POSTGRES_URL`; unset `QF_DB`, which PostgreSQL refuses.
4. Before trading again, reconcile with the exchange: `monitor status`
   shows a run's position beside the account's balances, open orders and
   recent trades. A run resumes only under the identity it recorded, so a run
   recorded on the source resumes on the target unchanged, and a legacy run
   stays refused until it is adopted.
5. Keep the SQLite file as an archive of the store as it was.

## Going back

Before the target has accepted a write, selecting the SQLite file again is
safe: nothing was written that it lacks.

Once the target has accepted writes, such as a candle synced, a run's state
saved, or an order event appended, the SQLite file is no longer the store's
current state, and selecting it again is not a rollback. It lacks every
write since the switch: a run resumed from it starts from a position the
exchange no longer holds, and orders the target journaled are missing from
it. Going back then needs reconciliation, not a switch: stop the processes
that use the target, compare each run's position and journal on both
stores with the exchange's orders and trades, and decide which store holds
the truth for each run before anything trades again. A copy from
PostgreSQL back to SQLite is the work of ALG-768, which this command does
not do.

## The canonical text

A checksum is `sha256:` and the lowercase hex of the SHA-256 of a scope's
canonical text: a table, or one market of a market table, as a
verification reads it. The text is, in this format:

<!-- begin: transfer format -->
1
<!-- end: transfer format -->

- a header of three lines, each ending in a newline: `quantforge transfer`
  and the format, the table's name, and its columns in the order of the DDL
  joined by commas;
- then each row, in order of its key, as its values followed by a newline.
  NULL is `n`; any other value is its tag, its length in bytes in decimal,
  a colon, and its bytes: `i` and an integer's decimal digits, `t` and a
  text's UTF-8, `b` and a blob's own bytes.

Integer keys order by value, text keys by their UTF-8 bytes, as SQLite
compares text; PostgreSQL sorts them `COLLATE "C"`, whatever the database's
collation. The scope of a market is the rows whose first three columns are
its `exchange`, `symbol` and `interval`, and markets are reported in the
order of those texts, byte by byte.

A worked example, the canonical text of one row of `closed_trades`, and its
checksum:

<!-- begin: transfer worked example -->
```text
quantforge transfer 1
closed_trades
seq,run_id,symbol,entry_time_ms,exit_time_ms,entry_price,exit_price,qty,gross_quote_pnl,entry_order_id,exit_order_id
i1:4t10:run-legacyt6:BTCUSDi13:1792454404000i13:1792454464000t6:1100.0t6:1050.0t5:0.010t4:-0.5nn
```

sha256:f334507638bb188f7f323a1895d78aa292eeef987789beaa64d7e0ac4ebf4f79
<!-- end: transfer worked example -->

[`tests/fixtures/storage_transfer_cases.json`](../tests/fixtures/storage_transfer_cases.json)
holds a store's tables as rows, three product lines on two venues, a run of
every identity source, and segments with their index and marks, with the
checksum of every scope. An independent Python script,
`spikes/storage-contract/transfer_cases.py`, wrote it, not the Rust code
under test, and both stores read every checksum from it.

## Extending a transfer

The transfer is versioned twice. `TRANSFER_SCHEMA`, in
`src/model/transfer/mod.rs`, is the schema whose tables it copies, and a
source and a target are both at it; `TRANSFER_FORMAT` is the format of the
canonical text and of the target's record, and a copy begun under another
format is refused. A story that adds or changes tables, such as ALG-735's
policies or ALG-738's signal inbox, execution ledger and outbox:

- lists each new table in `TRANSFER_TABLES`, in copy order with its key, or
  among the tables each store keeps for itself, and here in the lists
  above: `tests/storage/docs.rs` fails until every table of the schema is
  in one list or the other, with its columns;
- moves `TRANSFER_SCHEMA` with the schema version, which a unit test holds
  equal;
- moves `TRANSFER_FORMAT` when the canonical text or the record changes;
- adds rows of its tables to the oracle's fixture, so both stores are held
  to their checksums.

ALG-768 adds the other directions behind the same ports
(`TransferSnapshot`, `TransferTarget`): a PostgreSQL snapshot as a source,
SQLite as a target, and the backup and artifact manifest of a complete
cutover. A TimescaleDB target (ALG-395) is one more `TransferTarget`, which
the store the host selects hands over the same way.
