# Parity matrix

QuantForge 0.3 promises the same functions on Binance and Bybit, with SQLite
and PostgreSQL, for every product family it claims. This directory makes that
promise measurable. ALG-756 froze it on 2026-10-22, and ALG-769 holds the
release until every cell has passing evidence.

| File | What it holds |
| --- | --- |
| [matrix.toml](matrix.toml) | The versioned, machine-readable matrix: capabilities, their cells, contracts, owners, policy and scope changes |
| [matrix.md](matrix.md) | The same, rendered for people. It is generated; never edit it |
| [cells.txt](cells.txt) | The append-only ledger of every required cell |
| [evidence.jsonl](evidence.jsonl) | The evidence ledger: one record per line, appended by the stories that produce evidence |
| [universe-v1-2026-10-22.json](universe-v1-2026-10-22.json) | What universe rule 1 selected on 2026-10-22, per venue, family and environment |

`cargo test --test parity` checks all of them. After changing `matrix.toml`,
refresh `cells.txt` and `matrix.md` with
`UPDATE_SNAPSHOTS=1 cargo test --test parity`.

This matrix is the plan of what must pass, not evidence that it does.
[automation-requirements.md](../automation-requirements.md) traces the PRD to
the stories, and every one of its requirement IDs maps to a capability here.

## Reading a cell

**A capability is one function,** such as `futures.entry`. Its `axes` say which
dimensions it spans:

| Axis | Values |
| --- | --- |
| `artifact` | `binary`, `image`, `wheel`, `sqlite-build` |
| `surface` | `cli`, `server`, `python` |
| `venue` | `binance`, `bybit` |
| `backend` | `sqlite`, `postgres` |
| `family` | `spot`, `linear`, `inverse` |
| `side` | `long`, `short` |

**A cell is one capability with one value on each axis it spans.** Its ID joins
them in the order above, for example
`futures.entry/binance/postgres/inverse/short`.

**A capability spans an axis only where the axis can change the outcome:**
- connector operations span the venue and the family, but not the store;
- end-to-end paths span both;
- store-only paths span the store alone;
- LONG and SHORT appear only where direction changes a price comparison, a sign
  or the reduce-only side.

**The 12 base product cells** of the v0.3 work order are venue × store ×
family. `matrix.md` counts how many cells fall in each. A cell that names no
venue or no store counts for each of them.

**Owners.** Each capability names implementation and evidence stories.
- They are keyed by `all`, by an axis value, or by values joined with dots,
  such as `binance.linear`.
- A cell is owned by the union of every key whose parts are all among its
  values.
- Every cell has at least one implementation owner and one evidence owner.
- ALG-769 is never a cell's only evidence owner.
- The consumers (ALG-731, ALG-755, ALG-711, ALG-535 and ALG-719) read the
  evidence and never own it.

## What fails a cell

**A required cell passes only on positive evidence.** It needs a `pass` record
of every kind its capability `requires`:
- written by a story that owns its evidence;
- against the code under test;
- at or after the matrix revision in which the capability was last revised.

**Anything else fails it:**
- a missing operation, backend, adapter or product path;
- a skipped case. In the conformance suite only `Passed` passes, so a
  `Skipped` result, `Skip::Unsupported` included, fails a required cell;
- an `Unsupported` return;
- a manual or unverified check.

**Capability flags never waive a cell.** `ExchangeCapabilities` describes an
implementation, so a required flag that is `false` fails the cell.
Capabilities name their flags under `flags`.

**A value a venue genuinely lacks** is listed under `[[optional_value]]`, with
when it is absent, how it is shown and the source that says so. Examples:
- Binance's `liquidationPrice` of 0;
- Bybit's candles without a trade count.

Anything else that is missing fails.

**Blockers.** `[[blocker]]` ties a cell to an unverified venue item in
[futures-venues.md](../futures-venues.md#unverified-items). The cell cannot pass
until the item resolves.

## Domain applicability

`only` narrows an axis for a domain reason, which it must state. It never
narrows the venue or the store: parity forbids that, and an artifact sets its
own stores.

**Spot is long or flat.** It gains no short side, no leverage and no native
futures protection.

**Candle strategies take Spot series only.**
- `strategy.refuses_futures` requires them to refuse a linear or inverse
  series.
- v0.3 adds no futures research engine.
- Futures market data is still available through the data capabilities.

**Futures and automation capabilities** require linear and inverse together,
on both venues and both stores.

## Comparing results

Each `[[area]]` in `matrix.toml` defines what parity is checked against:
- normalized inputs and outputs;
- state machines;
- error vocabularies;
- units;
- data quality and persistence.

Two rules hold throughout:
- **Across stores,** identical frozen data and scenarios give identical
  results, byte for byte: rows, hashes and Decimal scale.
- **Across venues,** economically matched fixtures give the same normalized
  states, transitions, refusals and errors. Live prices, fees and PnL are never
  compared for equality.

The state machines and error vocabularies already in code are pinned to it:
- `OrderStatus`;
- `RunStatus`;
- the protocol's job kinds;
- `ExchangeErrorKind`;
- the wire `ErrorCode`.

ALG-733 implements the rest, using the names here.

## Evidence

Each line of `evidence.jsonl` is one JSON record. Unknown fields are refused.

| Field | Meaning |
| --- | --- |
| `cell` | The cell's ID |
| `result` | `pass` or `fail`; nothing else exists |
| `story` | The story that ran it; it must own evidence for the cell |
| `matrix_revision` | The matrix revision it was run against |
| `commit` | The full SHA of the code under test |
| `kind` | `replay`, `live`, `real_store` or `artifact` |
| `environment` | Live runs only: the venue environment |
| `backend_version` | Real-store runs only: the server version, such as `16` |
| `platform`, `digest` | Artifact runs only: the published file's platform and `sha256:` digest |
| `fixture`, `link` | Optional: where the evidence can be repeated or read |
| `date` | `YYYY-MM-DD` |
| `note` | Required on a failure: why it failed |

**Which environments count** is set in `[policy.evidence]`:
- Binance Spot: its testnet.
- Binance futures: demo.
- Bybit: demo, or testnet for what demo lacks.

**Real-store evidence** for PostgreSQL runs on 16 and on 18.

**Every published file** of an artifact needs its own record.

**A record cannot name the commit that adds it,** so `commit` is the code that
was tested. The record itself may land in a later commit that changes only
evidence.

## Keeping the matrix

**Every change to the cells bumps `revision`.** A capability whose pass
condition or required evidence changes also takes the new revision as
`revised`, which makes older evidence for it stale.

**Adding coverage** needs no decision: edit `matrix.toml`, bump `revision`, and
refresh. The update appends the new cells to `cells.txt`.

**Reducing coverage needs Marko's explicit decision.** Dropping a venue, store,
family, operation or artifact goes like this:
1. Record it as a `[[scope_change]]` of kind `reduction` that lists the removed
   cells or capabilities.
2. Log it in
   [automation-requirements.md](../automation-requirements.md#scope-decisions).
3. Bump `revision` and refresh.

The update turns the removed cells into tombstones, `<cell> removed SC-N`.
- A removal that no reduction names is refused.
- A ledger line deleted by hand fails the test.
- An implementation choice, a capability flag or missing evidence can never
  reduce coverage.

**Splitting or renaming a cell** keeps its history: write
`<old> replaced-by <new>,… rN` in `cells.txt` by hand.

**Exclusions** record scope that never had cells:
- SC-1: perpetuals only;
- SC-2: no adoption.

**Owners may change** as Linear's stories change. Story roles here are
paraphrased; Linear holds the briefs.

## Frozen choices

`[policy]` freezes, from ADR 0002 §2, §4, §5 and §8 to §10:
- profile identity and the account reference;
- the declared environments;
- one-way mode, the accepted margin modes and the refused account modes;
- that no account setting is ever changed;
- the arming and routing rules;
- the signal bounds.

ALG-733 and the adapter stories implement them. **Ambiguous routing is
refused**, which `config.select` and `futures.routing` test.

**Each venue's own facts stay explicit,** and are never averaged into one:
- **Eligibility:**
  - Universe rule 1 selects trading perpetuals from venue metadata.
  - The dated list records what it selected and is not an allowlist.
  - Binance lists perpetuals whose symbols are not ASCII, such as
    `币安人生USDT`, so symbols are compared by code point.
- **Transport:**
  - Futures routes and hosts per environment are in
    [futures-venues.md](../futures-venues.md#routes).
  - Binance Spot uses `api.binance.com`, and its testnet
    `testnet.binance.vision`.
  - Bybit Spot uses the v5 hosts with `category=spot`: `api.bybit.com`,
    `api-testnet.bybit.com` and `api-demo.bybit.com`.
- **Rate budgets:** in the table below.

### Rate budgets

These were read on 2026-10-22 from each venue's documentation, and for Binance
also from the `rateLimits` of the live `exchangeInfo`. Adapters stay within
them and report a 429, 418 or 10006 as `rate_limited`.

| Venue | Product | Budget | Counted per |
| --- | --- | --- | --- |
| Binance | USDⓈ-M and COIN-M together | 2,400 request weight a minute | IP |
| Binance | USDⓈ-M and COIN-M together | 1,200 orders a minute and 300 every 10 s; algo orders count too | account |
| Binance | Spot | 6,000 request weight a minute | IP |
| Binance | Spot | 100 orders every 10 s and 200,000 a day, counting unfilled orders | account |
| Binance | Spot | 300,000 raw requests every 5 minutes | IP |
| Bybit | every product | 600 requests every 5 s | IP |
| Bybit | linear and inverse | create, amend and cancel: 10 a second each | account (UID) |
| Bybit | Spot | create and cancel 20 a second, amend 10 a second | account (UID) |
| Bybit | every product | order and execution queries: 50 a second | account (UID) |
| Bybit | demo | default limits, not upgradable; demo has its own UID | demo account |

**Shared budgets and known gaps:**
- **Binance futures:** since the 2026-06-30 integration, USDⓈ-M and COIN-M
  share one weight budget per IP and one order budget per account. The two
  `exchangeInfo` endpoints each still report the full limits, so they must
  never be added together.
- **Binance COIN-M docs:** the definitions page still shows 6,000 weight a
  minute, which contradicts both the live value and the integration notice.
- **Bybit:**
  - The docs do not say whether linear and inverse share a bucket.
  - They do not say what the IP limit is on the demo host.

**Sources:**
- [Binance USDⓈ-M general info](https://developers.binance.com/docs/derivatives/usds-margined-futures/general-info)
- [CM-UM integration notice](https://developers.binance.com/en/docs/products/derivatives-trading-coin-futures/Important-CM-UM-Integration-Notice)
- [Binance Spot limits](https://developers.binance.com/docs/binance-spot-api-docs/rest-api/limits)
- [Bybit rate limits](https://bybit-exchange.github.io/docs/v5/rate-limit)
- [Bybit demo trading](https://bybit-exchange.github.io/docs/v5/demo)

## Artifacts

`[[artifact]]` lists what each release artifact promises:
- **The binary and the OCI image** carry the CLI and the server.
- **The wheel** carries the Python SDK: local and remote candle-strategy
  workflows, with PostgreSQL pools held by the trusted host. It has no webhook
  or Telegram API.
- **Every published artifact carries both venues and both stores.** A full
  artifact built without the `postgres` feature fails.
- **The SQLite-only build** (`--no-default-features`) is supported separately,
  labelled as such and never published as a full artifact.

**No hidden SQLite state:** with PostgreSQL selected, no SQLite file may be
created, opened or read. Today `AppContext::init` always creates the `--db`
file, so ALG-401 and ALG-728 must change that.

## Who uses it

| Role | Stories |
| --- | --- |
| Implement the contracts | ALG-733, and the stories each capability names |
| Produce evidence | ALG-742, ALG-765, ALG-702, ALG-703, ALG-748, ALG-530, ALG-411, ALG-754 and the others named per cell |
| Gate the release on it | ALG-769, which generates its test cells from `cells.txt` and reads `evidence.jsonl` |
| Consume the result | ALG-731, ALG-755, ALG-711 |

## Brief amendments

Apply these in Linear:

**Provisional owners**, assigned where no story covers a cell:
- **ALG-744, ALG-747 and ALG-700** carry the Bybit side of what ALG-764 does
  for Binance: private streams, restart reconciliation, closed PnL and the
  transaction log. A dedicated Bybit story can take these cells over.
- **ALG-693** carries Bybit environment routing, including demo, and Bybit's
  rate budgets.

**New duties:**
- **ALG-401 and ALG-728** stop `AppContext::init` from creating a SQLite file
  when PostgreSQL is selected (`release.no_hidden_sqlite`).
- **ALG-411** owns the evidence for the SQLite-only build
  (`release.artifact/sqlite-build/…`).
- **ALG-742** owns `futures.venue_equivalence` and `futures.backend_identity`:
  matched fixtures across venues, and identical ledgers across stores.
- **ALG-769** generates its cells from `cells.txt`, appends to `evidence.jsonl`,
  and treats evidence older than a capability's `revised` as stale.
