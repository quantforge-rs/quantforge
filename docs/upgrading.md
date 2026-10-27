# Upgrading a database

A QuantForge database is never deleted for a new release. The first time a
newer release opens an older file, it brings the file's schema forward in
place, step by step, and keeps everything the older release wrote: candles,
journals, IDs, timestamps and every decimal's exact text. This page says
which upgrades are supported, what to do before and after one, and how to
recover when one stops. [storage-schema.md](storage-schema.md) is the
contract behind it.

## Supported upgrades

| Written by | Schema | Upgrades to |
| --- | --- | --- |
| QuantForge 0.1.0 | 1 | 5 |
| QuantForge 0.2.0 | 2 | 5 |
| A development build after 0.2.0 | 3 or 4 | 5 |
| This release | 5 | — |

Every step of the chain runs forward only. A file a newer release has
already upgraded is refused by an older one, untouched:

```text
database schema version 6 is newer than this build supports (5); use a newer
quantforge release, and never an older one on this database
```

## Before the first start

1. **Stop every process that uses the file**: `data sync`, `trade run`,
   `monitor watch` and `serve run`. The upgrade takes a write lock on the
   file for each step; a running process would wait on it, and a process
   of the older release would then refuse the upgraded file.
2. **Back the file up.** The upgrade is transactional (a step either
   completes or leaves the file as it was), but a backup is what makes a
   return to the older release possible:

   ```bash
   sqlite3 data/market.sqlite ".backup data/market.sqlite.schema-2.bak"
   ```

   Without `sqlite3`, copy `market.sqlite` together with its `-wal` and
   `-shm` files while nothing has them open.

Then run any command, `store status` for one:

```bash
quantforge --db data/market.sqlite store status
```

## What the upgrade does

Each step runs in one transaction with its version stamp, and
`schema_migrations` logs it; `store status` prints the log.

| Step | What it does |
| --- | --- |
| 1 → 2 | Creates the run journal tables 0.2.0 introduced. |
| 2 → 3 | Nothing. A run 0.2.0 wrote recorded no execution mode; it stays as it is and is never given one by guesswork (see below). |
| 3 → 4 | Creates the segment tables, and marks every window that holds candles, so the next `data sync` packs the candles the older release stored into segments. |
| 4 → 5 | Turns the venue's order and trade IDs into text, in the journal columns and inside `state_json` and `raw_json`, keeping each row's original JSON in `legacy_state_json` and `legacy_raw_json`; gives every run its identity columns, with every existing run marked `legacy`; and creates the store's own record: a fresh store ID, no environment, `inactive`. |

Candles are not rewritten. The journal tables are copied row by row, so the
upgrade takes seconds for any journal and leaves the sequence numbers as
they were.

## After the upgrade

**Bind the store to its environment.** The older release recorded none, and
nothing infers it from a URL: declare it once, with the base URL the store's
credentials reach, and it holds for the file's life. From then on a
command whose base URL disagrees with it is refused before any request.

```bash
quantforge --db data/market.sqlite --base-url https://testnet.binance.vision/ \
  store bind --environment testnet --yes
```

**Adopt the runs you still need.** A run the older release wrote recorded no
environment and no account, so `trade run --run-id` and `trade close`
refuse it until an operator adopts it; `store status` lists these runs. An
adoption gives the run the store's environment and, for a live run, the
account behind the credentials in hand, which must reach that environment.

`store status` shows each legacy run's mode. A run a development build
wrote shows `dry_run` or `live`, and `--execution-mode` is refused for it,
since its mode is already recorded. A run 0.2.0 wrote shows `unrecorded`,
and its adoption states the mode with `--execution-mode`.

A dry run needs no credentials and takes the store's environment only:

```bash
# A dry run that recorded its mode
quantforge --db data/market.sqlite store adopt-run --run-id qf-... --yes
# A dry run 0.2.0 wrote, which recorded none
quantforge --db data/market.sqlite store adopt-run --run-id qf-... \
  --execution-mode dry-run --yes
```

A live run is adopted by the account that will close or resume it, so the
credentials must be in hand and the base URL must reach the environment the
store is bound to; without `--base-url` the default is the production
endpoint, and the command is refused before any request. The example is a
live run 0.2.0 wrote; for a live run that recorded its mode, leave out
`--execution-mode live`:

```bash
export QF_API_KEY="$(cat ~/.config/quantforge/testnet-api-key)"
export QF_API_SECRET="$(cat ~/.config/quantforge/testnet-api-secret)"
quantforge --db data/market.sqlite --base-url https://testnet.binance.vision/ \
  store adopt-run --run-id qf-... --execution-mode live --yes
```

A run nobody adopts stays as it was: readable, listed, never resumed. New
runs start under the store's environment without any of this.

**From 0.2.0, re-sync every period that holds a forming candle.** 0.2.0
stored the candle still forming at the end of each sync pass and resumed
after it, so it never fetched that candle again. Two kinds of period are
affected, for each market:

- **Every period a following sync covered** (`--follow`, or no `--end`). It
  kept a snapshot of nearly every candle, taken seconds after the candle
  opened.
- **The tail of every bounded sync whose `--end` had not yet passed when it
  ran**, even where no following sync covered it. It stored settled candles
  except its last one. Re-sync that tail once its last candle has settled.

Re-sync each period with `--start` and `--end` that span it: the sync writes
the settled candles over the snapshots in place, marks their windows for the
next segment refresh, and `data validate` checks the result. For example,
one day of `1m` candles, which is an example window and not the whole
repair:

```bash
quantforge --db data/market.sqlite data sync --symbol BTCUSDT --interval 1m \
  --start 2026-10-26T00:00:00Z --end 2026-10-27T00:00:00Z
```

## Recovery

**A step stopped.** The file is at the last completed step, and the error
names what stopped the next one: a column the step adds already exists,
for example, which happens only to a file edited by hand. Fix the cause and
start again; the chain continues from where it stopped. Or restore the
backup and run the older release.

**The older release is needed again.** Restore the backup. An upgraded file
is refused by the older release, and never changed by it.

**Something looks wrong in a migrated journal.** The original JSON of every
run and order event the older release wrote is in `legacy_state_json` and
`legacy_raw_json`, beside the migrated form, and `store status` shows which
steps ran and when.

**Nothing requires deleting a journal.** A live run's record is kept through
every upgrade; the only thing that is ever re-synced is candle data, and
only if you choose to.
