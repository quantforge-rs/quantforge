# Capability parity matrix

<!-- Generated from matrix.toml by `UPDATE_SNAPSHOTS=1 cargo test --test parity`. Edit matrix.toml, never this page. -->

This page renders [matrix.toml](matrix.toml): revision 1, frozen on 2026-10-22 for QuantForge 0.3.0. [README.md](README.md) explains how to read it and keep it.

It requires **458 cells** from 64 capabilities in 10 areas.

## Product cells

These are the work order's 12 base product cells. A cell counts in each product cell its venue, store and family fit; a cell that names no venue or no store counts for each.

| Venue | Store | Family | Cells |
| --- | --- | --- | --- |
| binance | sqlite | spot | 27 |
| binance | sqlite | linear | 49 |
| binance | sqlite | inverse | 49 |
| binance | postgres | spot | 27 |
| binance | postgres | linear | 49 |
| binance | postgres | inverse | 49 |
| bybit | sqlite | spot | 27 |
| bybit | sqlite | linear | 49 |
| bybit | sqlite | inverse | 49 |
| bybit | postgres | spot | 27 |
| bybit | postgres | linear | 49 |
| bybit | postgres | inverse | 49 |

Another 64 cells name no family: they cover stores, surfaces and artifacts.

## Configuration and routing

The `config` area.

- **Inputs:** Flags, then environment variables, then a persisted profile: venue, product family, environment, account, storage backend and its URL, and secrets by reference
- **Outputs:** One resolved selection (venue, family, environment, account, store) that every later request and row carries
- **States:** profile
- **Errors:** refusal, storage
- **Units:** none
- **Data quality:** Nothing is inferred: the environment is declared, never read from a URL; a missing, conflicting or ambiguous selection is refused before any network or storage access; secret values are never shown
- **Persistence:** A store is bound to one environment for its life; profiles keep their identity and settings and refer to secrets without holding them
- **Compared:** The same flags, precedence and refusals on every venue and store

### `config.select`

Select the venue, family, environment, account and store.

- **Cells:** 12, over venue × backend × family
- **Passes when:** Selecting each cell by flag, by environment variable and by profile reaches that venue, family and store, with the flag winning; a missing, conflicting or ambiguous selection is refused before any network or storage access
- **Evidence required:** replay, real_store
- **Implementation:** all: ALG-401, ALG-750; binance: ALG-684; binance.inverse: ALG-759; binance.linear: ALG-759; bybit: ALG-693, ALG-694; inverse: ALG-767; linear: ALG-767
- **Evidence:** all: ALG-769; inverse: ALG-742; linear: ALG-742; spot: ALG-702
- **Requirements:** OBS-R008
- **Revised in:** revision 1

### `config.secrets`

Hold credentials and tokens by reference, redacted.

- **Cells:** 4, over venue × backend
- **Passes when:** Credentials and tokens resolve only from flags, environment variables or a profile's secret reference; no value appears in output, logs, the database, notifications or workers, and every one shown is redacted
- **Evidence required:** replay, real_store, artifact
- **Implementation:** all: ALG-750, ALG-754; binance: ALG-499, ALG-759; bybit: ALG-694
- **Evidence:** all: ALG-754
- **Requirements:** OBS-R072, D23
- **Revised in:** revision 1

### `config.environment`

Bind each store to one environment.

- **Cells:** 2, over backend
- **Passes when:** A store records its environment once; a connector or profile for another environment is refused before any write, and a store from an older release is declared by its operator, never guessed
- **Evidence required:** real_store
- **Implementation:** all: ALG-734
- **Evidence:** all: ALG-748
- **Revised in:** revision 1

### `config.storage`

Select the storage backend on every surface.

- **Cells:** 6, over surface × backend
- **Passes when:** Selecting SQLite or PostgreSQL by flag or environment variable on each surface reaches that backend, and the other backend is never touched
- **Evidence required:** real_store
- **Implementation:** all: ALG-401; cli: ALG-728; python: ALG-382; server: ALG-729
- **Evidence:** all: ALG-411, ALG-530
- **Revised in:** revision 1

### `futures.routing`

Route every request by its persisted profile.

- **Cells:** 8, over venue × backend × family
- **Only family:** linear, inverse. Profiles route the futures automation; Spot keeps the global selection (ADR 0002 §2)
- **Passes when:** Every request and row carries the profile's venue, environment, account and family; same-named symbols on another venue, family or profile never reach the wrong account, and an ambiguous route is refused
- **Evidence required:** replay, real_store
- **Implementation:** all: ALG-767; binance: ALG-759; bybit: ALG-693
- **Evidence:** all: ALG-742, ALG-748
- **Requirements:** OBS-R008, D05, P-001
- **Revised in:** revision 1

## Candle strategies

The `strategy` area.

- **Inputs:** A strategy bundle or Python strategy, a Spot data series with its stored candles, and a backtest or trading configuration in Decimal
- **Outputs:** Trades, equity and metrics, with provenance: code hash, data hashes and configuration
- **States:** strategy_run, job
- **Errors:** exchange, storage, wire
- **Units:** price, base_qty, quote_amount, fee, timestamp, interval, exactness
- **Data quality:** Closed bars only; in a backtest a target set on bar N fills at bar N+1's open, with no clock, randomness or I/O; linear and inverse series are refused
- **Persistence:** Runs, journals and job results live in the selected store and survive restarts
- **Compared:** Byte-identical results across stores and between local and remote runs (the ALG-383 goldens); across venues a result depends only on that venue's frozen candles

### `strategy.backtest_local`

Backtest a strategy locally.

- **Cells:** 8, over surface × venue × backend × family
- **Only family:** spot. Local backtests run from the CLI and from Python; candle strategies take Spot series only (ADR 0002 §1)
- **Only surface:** cli, python. Local backtests run from the CLI and from Python; candle strategies take Spot series only (ADR 0002 §1)
- **Passes when:** The frozen SMA-cross goldens reproduce byte for byte from candles stored for the venue in each store, from the CLI and from Python
- **Evidence required:** replay, real_store
- **Implementation:** cli: ALG-375, ALG-532, ALG-534; postgres: ALG-728; python: ALG-376, ALG-377, ALG-378, ALG-379, ALG-380, ALG-381
- **Evidence:** all: ALG-383, ALG-530
- **Revised in:** revision 1

### `strategy.backtest_remote`

Backtest a strategy as a server job.

- **Cells:** 8, over surface × venue × backend × family
- **Only family:** spot. Remote backtests are submitted from the CLI and from Python; candle strategies take Spot series only (ADR 0002 §1)
- **Only surface:** cli, python. Remote backtests are submitted from the CLI and from Python; candle strategies take Spot series only (ADR 0002 §1)
- **Passes when:** The same bundle submitted from the CLI and from Python returns results identical to the local run and the goldens, with provenance naming the code and data hashes
- **Evidence required:** replay, real_store
- **Implementation:** all: ALG-509, ALG-510, ALG-511, ALG-512, ALG-515, ALG-516; cli: ALG-513; postgres: ALG-729; python: ALG-527, ALG-528, ALG-529
- **Evidence:** all: ALG-530, ALG-703
- **Revised in:** revision 1

### `strategy.trade_run`

Trade a Spot strategy in dry-run and live mode.

- **Cells:** 4, over surface × venue × backend × family
- **Only family:** spot. trade run and trade close are CLI commands; candle strategies trade Spot only (ADR 0002 §1)
- **Only surface:** cli. trade run and trade close are CLI commands; candle strategies trade Spot only (ADR 0002 §1)
- **Passes when:** A Python strategy runs and closes a Spot position in dry-run and in live mode on the venue's test environment, with the same journal on both stores
- **Evidence required:** replay, live, real_store
- **Implementation:** all: ALG-388, ALG-533; bybit: ALG-699; postgres: ALG-728
- **Evidence:** all: ALG-703
- **Revised in:** revision 1

### `strategy.python_reader`

Read stored candles from Python.

- **Cells:** 4, over venue × backend × family
- **Only family:** spot. The Python SDK serves candle strategies, which take Spot series only (ADR 0002 §1)
- **Passes when:** Python reads the same candles, decimals and timestamps from both stores as the Rust store returns
- **Evidence required:** real_store
- **Implementation:** all: ALG-381, ALG-382
- **Evidence:** all: ALG-530
- **Revised in:** revision 1

### `strategy.refuses_futures`

Refuse a futures series for a candle strategy.

- **Cells:** 6, over surface × family
- **Only family:** linear, inverse. The refusal is the requirement: candle strategies take Spot series only, and no futures research engine is added (ADR 0002 §1)
- **A refusal:** the cell passes when the request is refused
- **Passes when:** A candle strategy given a linear or inverse series is refused with a typed error before it runs, on every surface
- **Evidence required:** replay
- **Implementation:** all: ALG-734; cli: ALG-532; python: ALG-379; server: ALG-509
- **Evidence:** all: ALG-703
- **Revised in:** revision 1

## Server jobs

The `jobs` area.

- **Inputs:** A canonical bundle (bundle format 1) and a data selection
- **Outputs:** Job status, logs, results and a provenance record
- **States:** job
- **Errors:** wire, storage
- **Units:** hash, timestamp
- **Data quality:** Workers never receive store credentials; a bundle is verified before it runs; results can be reproduced from their provenance
- **Persistence:** Jobs and results live in the selected store; a restart neither loses nor repeats a finished job
- **Compared:** Identical results and provenance on both stores

### `jobs.lifecycle`

Submit, follow and cancel jobs.

- **Cells:** 4, over surface × backend
- **Only surface:** cli, python. Jobs are submitted from the CLI and from Python; the server runs them
- **Passes when:** A job is submitted, queued, run, cancelled and reported through the job states, from the CLI and from Python, on both stores
- **Evidence required:** real_store
- **Implementation:** all: ALG-509, ALG-510, ALG-512; cli: ALG-513; postgres: ALG-729; python: ALG-527, ALG-528, ALG-529
- **Evidence:** all: ALG-530, ALG-703
- **Revised in:** revision 1

### `jobs.results`

Fetch job results and provenance.

- **Cells:** 4, over surface × backend
- **Only surface:** cli, python. Results are fetched from the CLI and from Python
- **Passes when:** Results, logs and provenance download intact, and a re-run from the provenance gives identical results
- **Evidence required:** real_store
- **Implementation:** all: ALG-511, ALG-516; postgres: ALG-729; python: ALG-529
- **Evidence:** all: ALG-530
- **Revised in:** revision 1

### `jobs.isolation`

Isolate workers from credentials and each other.

- **Cells:** 2, over backend
- **Passes when:** Workers run within their limits, without store credentials or access to the store; the hostile-payload suite passes
- **Evidence required:** real_store
- **Implementation:** all: ALG-510, ALG-517, ALG-525; postgres: ALG-729
- **Evidence:** all: ALG-517
- **Revised in:** revision 1

### `jobs.recovery`

Keep jobs through a server restart.

- **Cells:** 2, over backend
- **Passes when:** A server restart neither loses nor repeats a queued, running or finished job
- **Evidence required:** real_store
- **Implementation:** all: ALG-509, ALG-510; postgres: ALG-729
- **Evidence:** all: ALG-703; postgres: ALG-730
- **Revised in:** revision 1

## Market data

The `data` area.

- **Inputs:** A data series key (venue, family, symbol, interval) and a time range
- **Outputs:** Settled candles, validation issues, immutable segments with their hashes, history responses, live closed bars and current prices
- **States:** none
- **Errors:** exchange, storage, wire
- **Units:** price, base_qty, contracts, quote_amount, timestamp, interval, hash, exactness
- **Data quality:** Only settled, closed bars are stored, ascending and without duplicates; gaps are reported, never filled with invented bars; inverse volume stays in contracts; mark-price series never mix with trade-price candles; a family is never inferred from a symbol
- **Persistence:** Decimals keep their exact text; segments are append-only; syncing the same data again changes nothing
- **Compared:** Identical bytes and segment hashes across stores; matched page shapes, gaps and duplicates across venues

### `data.instruments`

Read instrument rules from venue metadata.

- **Cells:** 6, over venue × family
- **Passes when:** Instrument rules (tick, step, minimums, maximums, face value and status) come from the traded environment's metadata and normalize to the shared rules
- **Evidence required:** replay, live
- **Implementation:** binance.inverse: ALG-760; binance.linear: ALG-760; binance.spot: ALG-686; bybit.inverse: ALG-739; bybit.linear: ALG-739; bybit.spot: ALG-695; inverse: ALG-761; linear: ALG-761
- **Evidence:** inverse: ALG-742; linear: ALG-742; spot: ALG-702
- **Capability flags:** spot: market_data.symbol_rules
- **Requirements:** OBS-R002
- **Revised in:** revision 1

### `data.sync`

Sync settled candles into the store.

- **Cells:** 12, over venue × backend × family
- **Passes when:** Syncing a range stores exactly the settled candles of the matched fixture, handling gaps, duplicates and page edges; a repeat sync changes nothing
- **Evidence required:** replay, live, real_store
- **Implementation:** binance.spot: ALG-687; bybit.spot: ALG-696; inverse: ALG-761; linear: ALG-761; postgres: ALG-405, ALG-728
- **Evidence:** inverse: ALG-742; linear: ALG-742; postgres: ALG-411; spot: ALG-702
- **Capability flags:** spot: market_data.server_time
- **Revised in:** revision 1

### `data.validate`

Validate stored candles.

- **Cells:** 12, over venue × backend × family
- **Passes when:** Validation reports the fixture's seeded gaps, duplicates and invalid candles, and nothing else
- **Evidence required:** replay, real_store
- **Implementation:** binance.spot: ALG-488; bybit.spot: ALG-696; inverse: ALG-761; linear: ALG-761; postgres: ALG-405, ALG-728
- **Evidence:** inverse: ALG-742; linear: ALG-742; postgres: ALG-411; spot: ALG-702
- **Revised in:** revision 1

### `data.segments`

Build immutable segments.

- **Cells:** 12, over venue × backend × family
- **Passes when:** Segments built from the same stored candles have identical bytes and hashes, and a refresh after a backfill rebuilds only the changed windows
- **Evidence required:** replay, real_store
- **Implementation:** all: ALG-734; inverse: ALG-761; linear: ALG-761; postgres: ALG-757; sqlite: ALG-506
- **Evidence:** all: ALG-402, ALG-411
- **Revised in:** revision 1

### `data.history`

Serve history over HTTP.

- **Cells:** 12, over venue × backend × family
- **Passes when:** History requests return the stored candles, and segments by hash, identically from both stores
- **Evidence required:** real_store
- **Implementation:** all: ALG-508; inverse: ALG-761; linear: ALG-761; postgres: ALG-729
- **Evidence:** all: ALG-703; postgres: ALG-411
- **Revised in:** revision 1

### `data.live`

Stream closed bars.

- **Cells:** 12, over venue × backend × family
- **Passes when:** Closed bars arrive once each and in order, with gaps filled from REST after a reconnect, both locally and through the live plane
- **Evidence required:** replay, live, real_store
- **Implementation:** all: ALG-518, ALG-519, ALG-520, ALG-521; binance.spot: ALG-688; bybit.spot: ALG-697; inverse: ALG-761; linear: ALG-761
- **Evidence:** all: ALG-521, ALG-703; inverse: ALG-742; linear: ALG-742
- **Capability flags:** spot: streams.klines
- **Revised in:** revision 1

### `data.prices`

Read current mark and book prices.

- **Cells:** 4, over venue × family
- **Only family:** linear, inverse. Mark and book prices feed futures monitoring and submission checks (ADR 0002 §9)
- **Passes when:** The mark and best bid and ask used by monitoring and submission checks come fresh from the venue with their source and time; a price that cannot be fetched fresh refuses the entry
- **Evidence required:** replay, live
- **Implementation:** all: ALG-761; binance: ALG-760; bybit: ALG-739
- **Evidence:** all: ALG-742
- **Requirements:** OBS-R042
- **Revised in:** revision 1

### `data.backend_identity`

Store identical data identically.

- **Cells:** 6, over venue × family
- **Passes when:** Identical frozen candles give byte-identical candle rows, segments and hashes in SQLite and PostgreSQL
- **Evidence required:** real_store
- **Implementation:** all: ALG-402, ALG-757
- **Evidence:** all: ALG-411
- **Revised in:** revision 1

## Spot orders and accounts

The `spot` area.

- **Inputs:** Symbol, side, base quantity or quote amount, limit price, time in force (GTC, IOC or FOK), client order ID, and replacement intents
- **Outputs:** Normalized orders (venue ID as text, client ID, status, executed quantity and quote amount, fills with their fees), balances and trades
- **States:** spot_order, spot_replace, strategy_run
- **Errors:** exchange, refusal, storage
- **Units:** price, base_qty, quote_amount, fee, timestamp, exactness
- **Data quality:** An acknowledgement is not a fill; an unknown outcome is queried before any retry; Spot never gains short positions, leverage or native futures protection
- **Persistence:** The position change is persisted before its journal event; journal rows are identical on both stores; PnL is never fabricated
- **Compared:** Identical journals across stores; matched order scenarios across venues

### `spot.order.market`

Market orders by base quantity or quote amount.

- **Cells:** 2, over venue × family
- **Only family:** spot. Spot order types; futures entries are futures.entry (ADR 0002 §13)
- **Passes when:** Market orders by base quantity and by quote amount pass the shared scenarios (full fill, partial fill, rejection) with the normalized states and fills
- **Evidence required:** replay, live
- **Implementation:** binance: ALG-689; bybit: ALG-698
- **Evidence:** all: ALG-702
- **Capability flags:** all: orders.market_by_base_quantity, orders.market_by_quote_quantity, orders.client_order_ids
- **Revised in:** revision 1

### `spot.order.limit`

Limit orders.

- **Cells:** 2, over venue × family
- **Only family:** spot. Spot order types; futures enter at market (ADR 0002 §13)
- **Passes when:** Limit orders rest, fill partially, fill and are rejected with the normalized states and fills
- **Evidence required:** replay, live
- **Implementation:** binance: ALG-689; bybit: ALG-698
- **Evidence:** all: ALG-702
- **Capability flags:** all: orders.client_order_ids
- **Revised in:** revision 1

### `spot.order.time_in_force`

Time in force: GTC, IOC and FOK.

- **Cells:** 2, over venue × family
- **Only family:** spot. Spot order types (ADR 0002 §13)
- **Passes when:** GTC orders rest, IOC orders cancel what does not fill at once, and FOK orders fill completely or not at all
- **Evidence required:** replay, live
- **Implementation:** binance: ALG-689; bybit: ALG-698
- **Evidence:** all: ALG-702
- **Revised in:** revision 1

### `spot.order.cancel`

Cancel an order.

- **Cells:** 2, over venue × family
- **Only family:** spot. Spot orders; futures closes are futures.exit (ADR 0002 §13)
- **Passes when:** Cancelling by venue ID and by client ID ends a resting order as CANCELED and keeps any fill
- **Evidence required:** replay, live
- **Implementation:** binance: ALG-689; bybit: ALG-698
- **Evidence:** all: ALG-702
- **Revised in:** revision 1

### `spot.order.replace`

Change a resting limit order.

- **Cells:** 2, over venue × family
- **Only family:** spot. Spot orders (ADR 0002 §13)
- **Passes when:** Changing a resting limit order's price or quantity gives the normalized replace states, including partial application and fills during the change; an Unsupported answer fails
- **Evidence required:** replay, live
- **Implementation:** all: ALG-766; binance: ALG-689
- **Evidence:** all: ALG-702
- **Revised in:** revision 1

### `spot.order.query`

Query orders.

- **Cells:** 2, over venue × family
- **Only family:** spot. Spot orders (ADR 0002 §13)
- **Passes when:** Querying by venue ID and by client ID, and listing open orders, return the normalized order
- **Evidence required:** replay, live
- **Implementation:** binance: ALG-689; bybit: ALG-698
- **Evidence:** all: ALG-702
- **Revised in:** revision 1

### `spot.account`

Balances and trades.

- **Cells:** 2, over venue × family
- **Only family:** spot. Spot accounts (ADR 0002 §13)
- **Passes when:** Balances and trades return normalized amounts in their own assets; what the venue does not report stays unknown
- **Evidence required:** replay, live
- **Implementation:** binance: ALG-683; bybit: ALG-700
- **Evidence:** all: ALG-702
- **Revised in:** revision 1

### `spot.user_stream`

Private order and balance events.

- **Cells:** 2, over venue × family
- **Only family:** spot. Spot accounts; futures events are futures.monitoring (ADR 0002 §13)
- **Passes when:** Execution reports and balance updates arrive from the private stream and agree with order queries
- **Evidence required:** replay, live
- **Implementation:** binance: ALG-691; bybit: ALG-700
- **Evidence:** all: ALG-702
- **Capability flags:** all: streams.user_data
- **Revised in:** revision 1

### `spot.journal`

Journal every order event.

- **Cells:** 4, over venue × backend × family
- **Only family:** spot. The Spot run journal; futures use the execution ledger (ADR 0002 §15)
- **Passes when:** Every order event is journaled before and after its request, identically in both stores, and a failed journal write never loses an executed fill
- **Evidence required:** replay, real_store
- **Implementation:** all: ALG-690, ALG-738; postgres: ALG-406
- **Evidence:** all: ALG-402, ALG-703
- **Revised in:** revision 1

### `spot.reconciliation`

Reconcile the journal with the venue.

- **Cells:** 4, over venue × backend × family
- **Only family:** spot. Spot runs; futures recovery is futures.crash_recovery (ADR 0002 §11)
- **Passes when:** At start and after a reconnect the journal matches the venue, and any drift is reported before trading resumes
- **Evidence required:** replay, real_store
- **Implementation:** binance: ALG-692; bybit: ALG-701; postgres: ALG-728
- **Evidence:** all: ALG-703
- **Revised in:** revision 1

### `spot.operator_cli`

Operate Spot runs from the CLI.

- **Cells:** 4, over venue × backend × family
- **Only family:** spot. The monitor and trade close commands serve Spot runs; futures status is automation.status_cli
- **Passes when:** The monitor commands and trade close work on the venue and both stores, behind their confirmation gates and without fabricated PnL
- **Evidence required:** replay, live, real_store
- **Implementation:** binance: ALG-491; bybit: ALG-699; postgres: ALG-728
- **Evidence:** all: ALG-703; bybit: ALG-699
- **Revised in:** revision 1

## Transport and errors

The `transport` area.

- **Inputs:** Requests to the hosts documented for the profile's environment
- **Outputs:** A response, or one typed error per failure
- **States:** none
- **Errors:** exchange
- **Units:** timestamp
- **Data quality:** Each venue's rate budget is respected and stays explicit; every venue error maps to exactly one kind; a request that may have executed is outcome_unknown
- **Persistence:** None
- **Compared:** The same error kinds for matched failures on both venues

### `transport.rate_budget`

Stay within each venue's rate budget.

- **Cells:** 6, over venue × family
- **Passes when:** Requests stay within the venue's documented budget for the product; a 429, 418 or 10006 answer backs off and is reported as rate_limited, never retried blindly
- **Evidence required:** replay, live
- **Implementation:** binance.inverse: ALG-759; binance.linear: ALG-759; binance.spot: ALG-687; bybit: ALG-693
- **Evidence:** inverse: ALG-742; linear: ALG-742; spot: ALG-702
- **Revised in:** revision 1

### `errors.taxonomy`

Map every venue error to one kind.

- **Cells:** 6, over venue × family
- **Passes when:** Every venue error in the shared error fixtures maps to exactly one exchange error kind, and an error after which the request may have executed maps to outcome_unknown
- **Evidence required:** replay
- **Implementation:** binance.inverse: ALG-759; binance.linear: ALG-759; binance.spot: ALG-685; bybit: ALG-693
- **Evidence:** inverse: ALG-742; linear: ALG-742; spot: ALG-702
- **Revised in:** revision 1

## Futures adapters and the execution ledger

The `futures` area.

- **Inputs:** Instrument, account and position identities, the policy snapshot, an account snapshot and admitted signals
- **Outputs:** Instruments with their units; account and mode facts; positions with liquidation; leverage; sizing results; orders and fills; protection state; realized PnL
- **States:** signal_intent, futures_position, incident, accounting
- **Errors:** exchange, refusal, storage
- **Units:** price, base_qty, contracts, money, fee, funding, pnl, leverage, percent, fraction, timestamp, rounding, exactness
- **Data quality:** Universe rule 1; rules come from the environment being traded; stops trigger on the mark price; liquidation is the venue's value or N/A with a reason, never an estimate; nothing is called protected or flat without the exchange's evidence
- **Persistence:** Intents, fills, positions, protection and incidents live in the execution store; no transaction spans a network request; state survives a restart
- **Compared:** Identical ledger rows across stores for identical frozen inputs; matched fixtures across venues give the same states, transitions and errors, never the same prices, fees or PnL

### `futures.universe`

Select the futures universe by rule 1.

- **Cells:** 4, over venue × family
- **Only family:** linear, inverse. Futures semantics never reach Spot (ADR 0002 §13)
- **Passes when:** Rule 1 selects the venue's trading perpetuals from its metadata, and everything else is refused with a reason (SC-1)
- **Evidence required:** replay, live
- **Implementation:** all: ALG-733; binance: ALG-760; bybit: ALG-739
- **Evidence:** all: ALG-742; binance: ALG-765
- **Requirements:** OBS-R002, D01
- **Revised in:** revision 1

### `futures.account_checks`

Check the account type, position mode and margin mode.

- **Cells:** 4, over venue × family
- **Only family:** linear, inverse. Futures semantics never reach Spot (ADR 0002 §13)
- **Passes when:** Hedge mode, portfolio margin and unsupported account types are refused with an actionable message; one-way mode with isolated or cross margin passes; no account setting is changed
- **Evidence required:** replay, live
- **Implementation:** all: ALG-767; binance: ALG-759, ALG-760; bybit: ALG-739, ALG-740
- **Evidence:** all: ALG-742; binance: ALG-765
- **Requirements:** D05, D15
- **Revised in:** revision 1

### `futures.leverage`

Set and read back leverage.

- **Cells:** 4, over venue × family
- **Only family:** linear, inverse. Spot has no leverage (ADR 0002 §13)
- **Passes when:** Leverage is set only while flat with no open orders, read back, and refused outside the venue's bracket for the notional
- **Evidence required:** replay, live
- **Implementation:** binance: ALG-760; bybit: ALG-740
- **Evidence:** all: ALG-742; binance: ALG-765
- **Requirements:** OBS-R035, D15
- **Revised in:** revision 1

### `futures.position`

Read positions with their liquidation price.

- **Cells:** 8, over venue × family × side
- **Only family:** linear, inverse. Spot has no short side or liquidation (ADR 0002 §13)
- **Passes when:** Positions normalize to side, size, entry, mark, margin, leverage and liquidation (the venue's value, or N/A with a reason) on both sides
- **Evidence required:** replay, live
- **Implementation:** binance: ALG-760, ALG-763; bybit: ALG-739, ALG-741
- **Evidence:** all: ALG-742; binance: ALG-765
- **Requirements:** OBS-R039, OBS-R040, D17
- **Revised in:** revision 1

### `futures.sizing`

Size an entry from risk.

- **Cells:** 8, over venue × family × side
- **Only family:** linear, inverse. Risk sizing serves futures entries (ADR 0002 §6)
- **Passes when:** Every case in sizing-cases.json reproduces exactly, every refusal included
- **Evidence required:** replay
- **Implementation:** all: ALG-735, ALG-736
- **Evidence:** all: ALG-742
- **Requirements:** OBS-R009, OBS-R017, OBS-R018, OBS-R019, OBS-R020, OBS-R021, OBS-R022, OBS-R023, OBS-R024, OBS-R025, OBS-R026, OBS-R027, OBS-R028, D04, D07, D10, D13, D14
- **Revised in:** revision 1

### `futures.entry`

Enter at market through confirmed fill states.

- **Cells:** 16, over venue × backend × family × side
- **Only family:** linear, inverse. Futures semantics never reach Spot (ADR 0002 §13)
- **Passes when:** An admitted entry reaches a confirmed fill or a typed refusal through the intent states; a duplicate, a pyramid or a reversal never adds exposure
- **Evidence required:** replay, live, real_store
- **Implementation:** all: ALG-738, ALG-745; binance: ALG-762; bybit: ALG-740
- **Evidence:** all: ALG-742; binance: ALG-765
- **Requirements:** OBS-R003, OBS-R007, OBS-R010, OBS-R011, OBS-R019, D06, D09
- **Revised in:** revision 1

### `futures.unknown_outcome`

Settle unknown entry outcomes.

- **Cells:** 8, over venue × backend × family
- **Only family:** linear, inverse. Futures semantics never reach Spot (ADR 0002 §13)
- **Passes when:** An entry with an unknown outcome is never resubmitted; not found settles it only after 30 s with the position flat, and at 60 s an unsettled entry raises the incident (ADR 0002 §9)
- **Evidence required:** replay, real_store
- **Implementation:** all: ALG-745; binance: ALG-762; bybit: ALG-740
- **Evidence:** all: ALG-748
- **Requirements:** D09
- **Revised in:** revision 1

### `futures.native_stop`

Hold an exchange-native stop on every position.

- **Cells:** 16, over venue × backend × family × side
- **Only family:** linear, inverse. Spot gains no native futures protection (ADR 0002 §13)
- **Passes when:** A full-position mark-price stop is placed after the first fill, read back before the position counts as protected, and replaced without leaving the position bare
- **Evidence required:** replay, live, real_store
- **Implementation:** all: ALG-746; binance: ALG-763; bybit: ALG-741
- **Evidence:** all: ALG-742; binance: ALG-765
- **Requirements:** OBS-R003, OBS-R043, OBS-R044, OBS-R045, D10
- **Revised in:** revision 1

### `futures.protection_failure`

Respond to a failed stop.

- **Cells:** 16, over venue × backend × family × side
- **Only family:** linear, inverse. Spot gains no native futures protection (ADR 0002 §13)
- **Passes when:** A failed stop goes through three repairs in 10 s, then an incident, a halt and a reduce-only close, then alerts and retries until the position is protected or flat (ADR 0002 §8)
- **Evidence required:** replay, real_store
- **Implementation:** all: ALG-746
- **Evidence:** all: ALG-748
- **Requirements:** OBS-R046, D19
- **Revised in:** revision 1

### `futures.exit`

Close on an exit signal.

- **Cells:** 16, over venue × backend × family × side
- **Only family:** linear, inverse. Futures semantics never reach Spot (ADR 0002 §13)
- **Passes when:** An exit closes only an owned cycle of its side entered strictly earlier, with a reduce-only market close that never opens or reverses exposure; a duplicate exit is a no-op
- **Evidence required:** replay, live, real_store
- **Implementation:** all: ALG-747; binance: ALG-762; bybit: ALG-740
- **Evidence:** all: ALG-742; binance: ALG-765
- **Requirements:** OBS-R003, OBS-R012, OBS-R013, OBS-R014, OBS-R015, D11
- **Revised in:** revision 1

### `futures.monitoring`

Monitor positions, orders and stops.

- **Cells:** 16, over venue × backend × family × side
- **Only family:** linear, inverse. Futures semantics never reach Spot (ADR 0002 §13)
- **Passes when:** Private events and REST snapshots keep positions, orders, stops and liquidation current, and a stale or unreconciled stream halts entries
- **Evidence required:** replay, live, real_store
- **Implementation:** all: ALG-744; binance: ALG-764; bybit: ALG-700
- **Evidence:** all: ALG-748; binance: ALG-765
- **Requirements:** OBS-R003, OBS-R042, OBS-R047, OBS-R048, OBS-R050, OBS-R053, OBS-R054, OBS-R055, OBS-R056, D18
- **Revised in:** revision 1

### `futures.manual_changes`

Handle manual changes to owned positions.

- **Cells:** 16, over venue × backend × family × side
- **Only family:** linear, inverse. Futures semantics never reach Spot (ADR 0002 §13)
- **Passes when:** Manual additions, reductions, stop moves, stop removals and closes are detected and handled as ADR 0002 §10 decides
- **Evidence required:** replay, live, real_store
- **Implementation:** all: ALG-744, ALG-746; binance: ALG-764
- **Evidence:** all: ALG-748
- **Requirements:** OBS-R049, OBS-R051, OBS-R052, D05, D20
- **Revised in:** revision 1

### `futures.accounting`

Record realized PnL with fees and funding.

- **Cells:** 16, over venue × backend × family × side
- **Only family:** linear, inverse. Futures semantics never reach Spot (ADR 0002 §13)
- **Passes when:** Each closed cycle records gross, fees, funding and net in the settlement asset, provisional and then final, from the venue's own records
- **Evidence required:** replay, live, real_store
- **Implementation:** all: ALG-747; binance: ALG-764
- **Evidence:** all: ALG-742
- **Requirements:** OBS-R016, D12
- **Revised in:** revision 1

### `futures.crash_recovery`

Recover from crashes and reconnects.

- **Cells:** 16, over venue × backend × family × side
- **Only family:** linear, inverse. Futures semantics never reach Spot (ADR 0002 §13)
- **Passes when:** A restart at each intent, fill, protection and outbox boundary recovers without a second order and without losing protection
- **Evidence required:** replay, real_store
- **Implementation:** all: ALG-738, ALG-744; binance: ALG-764
- **Evidence:** all: ALG-748
- **Requirements:** OBS-R056, OBS-R057
- **Revised in:** revision 1

### `futures.venue_equivalence`

Behave the same on both venues.

- **Cells:** 8, over backend × family × side
- **Only family:** linear, inverse. Compares the venues' futures paths with each other (ADR 0002 §13)
- **Passes when:** Economically matched fixtures give the same normalized states, transitions, refusals and errors on both venues; prices, fees and PnL are not compared
- **Evidence required:** replay, real_store
- **Implementation:** all: ALG-742
- **Evidence:** all: ALG-742
- **Requirements:** OBS-R001, P-001
- **Revised in:** revision 1

### `futures.backend_identity`

Keep identical ledgers on both stores.

- **Cells:** 8, over venue × family × side
- **Only family:** linear, inverse. Compares the stores' futures ledgers with each other (ADR 0002 §13)
- **Passes when:** Identical frozen inputs give identical intents, fills, positions, protection rows and outbox records in SQLite and PostgreSQL
- **Evidence required:** real_store
- **Implementation:** all: ALG-738, ALG-742
- **Evidence:** all: ALG-742
- **Requirements:** P-001
- **Revised in:** revision 1

## Automation service

The `automation` area.

- **Inputs:** Authenticated TradingView deliveries, CLI commands and profile settings
- **Outputs:** Inbox and outbox records, Telegram messages and CLI status
- **States:** signal_intent, notification, profile, service, incident
- **Errors:** refusal, storage, wire
- **Units:** money, pnl, percent, timestamp
- **Data quality:** A 2xx answer means only that the inbox commit happened, within TradingView's 3 s; an acknowledgement is not a fill and a fill is not protection; secrets are held by reference
- **Persistence:** Inbox, outbox and profiles are durable; a normal stop leaves positions and native stops in place
- **Compared:** Identical records across stores; the same workflow on both venues

### `automation.admission`

Admit webhook deliveries durably.

- **Cells:** 8, over venue × backend × family
- **Only family:** linear, inverse. External signals drive the futures automation (ADR 0002 §1)
- **Passes when:** An authenticated, valid delivery is committed to the inbox before a 2xx within 3 s; a duplicate gets the original outcome, and conflicting, stale or unauthenticated deliveries are refused and recorded
- **Evidence required:** replay, real_store
- **Implementation:** all: ALG-737, ALG-738, ALG-743, ALG-758
- **Evidence:** all: ALG-748
- **Requirements:** OBS-R004, OBS-R005, OBS-R006, OBS-R007, OBS-R008, OBS-R024, D02, D03, D06, D24
- **Revised in:** revision 1

### `automation.profiles`

Keep profiles, asset policies and capital.

- **Cells:** 8, over venue × backend × family
- **Only family:** linear, inverse. Profiles configure the futures automation (ADR 0002 §2, §5)
- **Passes when:** Profiles, asset policies, defaults and capital persist, start disarmed, arm only with confirmation, and an edit never resizes or unprotects an open cycle
- **Evidence required:** replay, real_store
- **Implementation:** all: ALG-735, ALG-750, ALG-767; binance: ALG-759
- **Evidence:** all: ALG-748
- **Requirements:** OBS-R009, OBS-R017, OBS-R025, OBS-R029, OBS-R030, OBS-R031, OBS-R032, OBS-R033, OBS-R034, OBS-R035, OBS-R036, OBS-R037, OBS-R038, D07, D08, D13
- **Revised in:** revision 1

### `automation.status_cli`

Show account and position status.

- **Cells:** 8, over venue × backend × family
- **Only family:** linear, inverse. The futures automation's status (ADR 0002 §12)
- **Passes when:** Status shows the running state, connections, balances per asset and the 11 position fields, all from verified state
- **Evidence required:** replay, live, real_store
- **Implementation:** all: ALG-753
- **Evidence:** all: ALG-754, ALG-769
- **Requirements:** OBS-R041, OBS-R068, OBS-R069
- **Revised in:** revision 1

### `automation.notifications`

Send Telegram messages from the outbox.

- **Cells:** 16, over venue × backend × family × side
- **Only family:** linear, inverse. The futures automation's messages (ADR 0002 §1, §12)
- **Passes when:** Each message type is sent from the outbox once its state is verified, never claiming an unconfirmed fill or protection
- **Evidence required:** replay, real_store
- **Implementation:** all: ALG-749, ALG-751
- **Evidence:** all: ALG-754, ALG-769
- **Requirements:** OBS-R015, OBS-R041, OBS-R058, OBS-R059, OBS-R060, OBS-R061, OBS-R062, OBS-R063, OBS-R064, OBS-R065, D21
- **Revised in:** revision 1

### `automation.lifecycle`

Start, report and stop the service.

- **Cells:** 8, over venue × backend × family
- **Only family:** linear, inverse. The futures automation's service (ADR 0002 §1, §5)
- **Passes when:** Start, status and a graceful stop work from the CLI, and a normal stop leaves positions and native stops in place
- **Evidence required:** replay, live, real_store
- **Implementation:** all: ALG-752
- **Evidence:** all: ALG-754, ALG-769
- **Requirements:** OBS-R045, OBS-R066, OBS-R067, OBS-R070, OBS-R071, D22
- **Revised in:** revision 1

### `automation.deployment`

Deploy and recover the service.

- **Cells:** 4, over artifact × backend
- **Only artifact:** binary, image. The service runs from the binary or the image; the wheel carries no service (ADR 0002 §14)
- **Passes when:** The documented deployment starts the service from the artifact with its store, secrets and webhook edge, and the recovery drill passes
- **Evidence required:** artifact, real_store
- **Implementation:** all: ALG-754; image: ALG-409
- **Evidence:** all: ALG-754
- **Requirements:** D22
- **Revised in:** revision 1

## Storage backends

The `storage` area.

- **Inputs:** The storage ports' operations
- **Outputs:** The same results from SQLite and PostgreSQL
- **States:** none
- **Errors:** storage
- **Units:** timestamp, hash, exactness
- **Data quality:** Durable commits and bounded waits; never a hidden fallback to another backend
- **Persistence:** Migrations never delete data; every earlier schema moves forward; transfers are verified
- **Compared:** One shared conformance suite gives identical results on both backends

### `storage.conformance`

Pass the shared storage conformance suite.

- **Cells:** 2, over backend
- **Passes when:** The shared conformance suite passes for the candle, journal, segment and execution stores
- **Evidence required:** real_store
- **Implementation:** all: ALG-402; postgres: ALG-404, ALG-405, ALG-406, ALG-738, ALG-757; sqlite: ALG-496, ALG-738
- **Evidence:** all: ALG-411
- **Revised in:** revision 1

### `storage.durability`

Commit durably within bounded waits.

- **Cells:** 2, over backend
- **Passes when:** Commits are durable under the documented settings, and waits stay within the admission budget
- **Evidence required:** real_store
- **Implementation:** all: ALG-758
- **Evidence:** all: ALG-411, ALG-748
- **Requirements:** D16
- **Revised in:** revision 1

### `storage.migrations`

Migrate every earlier schema.

- **Cells:** 2, over backend
- **Passes when:** Every earlier schema (1 to 4 on SQLite) migrates to the current one without losing or changing data
- **Evidence required:** real_store
- **Implementation:** postgres: ALG-404; sqlite: ALG-734
- **Evidence:** all: ALG-411, ALG-768
- **Requirements:** D16
- **Revised in:** revision 1

### `storage.transfer`

Transfer a store to the other backend.

- **Cells:** 2, over backend
- **Passes when:** A full transfer into this backend verifies counts, hashes, decimal scale and IDs, and backup and restore round-trip
- **Evidence required:** real_store
- **Implementation:** all: ALG-768; postgres: ALG-407
- **Evidence:** all: ALG-754, ALG-768
- **Requirements:** D16
- **Revised in:** revision 1

## Release artifacts

The `release` area.

- **Inputs:** Candidate binaries, wheels and OCI images, and the SQLite-only build
- **Outputs:** Smoke results per surface, venue and backend, for every published file
- **States:** none
- **Errors:** none
- **Units:** hash
- **Data quality:** Full artifacts carry the postgres feature and both venues; a PostgreSQL deployment leaves no SQLite file; the Python SDK has no webhook or Telegram API
- **Persistence:** None
- **Compared:** Every published file of an artifact passes the same cells

### `release.artifact`

Carry every promised venue and backend.

- **Cells:** 24, over artifact × surface × venue × backend
- **Passes when:** The candidate artifact runs its surface's smoke scenarios with each venue on each backend it promises; a full artifact without the postgres feature fails
- **Evidence required:** artifact
- **Implementation:** binary: ALG-535; image: ALG-408, ALG-412; sqlite-build: ALG-404, ALG-411; wheel: ALG-376, ALG-387
- **Evidence:** all: ALG-769; binary: ALG-703; image: ALG-412; sqlite-build: ALG-411; wheel: ALG-384
- **Revised in:** revision 1

### `release.no_hidden_sqlite`

Leave no SQLite state under PostgreSQL.

- **Cells:** 3, over surface
- **Passes when:** With PostgreSQL selected, no SQLite file is created, opened or read
- **Evidence required:** artifact
- **Implementation:** all: ALG-401; cli: ALG-728; python: ALG-382; server: ALG-729
- **Evidence:** all: ALG-411, ALG-769
- **Revised in:** revision 1

### `release.wheel_surface`

Keep the Python SDK to candle strategies and jobs.

- **Cells:** 1, over artifact
- **Only artifact:** wheel. Only the wheel carries the Python SDK (ADR 0002 §14)
- **Passes when:** The wheel exposes the candle-strategy and job APIs, and no webhook or Telegram API
- **Evidence required:** artifact
- **Implementation:** all: ALG-376, ALG-387
- **Evidence:** all: ALG-384
- **Revised in:** revision 1

## State machines

### `accounting`

- **States:** provisional, final
- **Initial:** provisional
- **Terminal:** final
- **Transitions:**
  - provisional → final

### `futures_position`

- **States:** protection_pending, protected, protected_by_operator, protection_unknown, unprotected, unprotected_by_operator, closing, closed
- **Initial:** protection_pending
- **Terminal:** closed
- **Transitions:**
  - protection_pending → protected, protected_by_operator, protection_unknown, unprotected, closing, closed
  - protected → protection_pending, protected_by_operator, protection_unknown, unprotected, unprotected_by_operator, closing, closed
  - protected_by_operator → protection_pending, protection_unknown, unprotected_by_operator, closing, closed
  - protection_unknown → protection_pending, protected, protected_by_operator, unprotected, unprotected_by_operator, closing, closed
  - unprotected → protection_pending, protected, closing, closed
  - unprotected_by_operator → protected_by_operator, closing, closed
  - closing → protected, protection_unknown, unprotected, closed

### `incident`

- **States:** open, cleared
- **Initial:** open
- **Terminal:** cleared
- **Transitions:**
  - open → cleared

### `job`

- **States:** queued, running, succeeded, failed, cancelled
- **Initial:** queued
- **Terminal:** succeeded, failed, cancelled
- **Transitions:**
  - queued → running, failed, cancelled
  - running → succeeded, failed, cancelled

### `notification`

- **States:** pending, delivered, outcome_unknown
- **Initial:** pending
- **Terminal:** delivered, outcome_unknown
- **Transitions:**
  - pending → delivered, outcome_unknown

### `profile`

- **States:** disarmed, armed
- **Initial:** disarmed
- **Terminal:** none; the machine cycles
- **Transitions:**
  - disarmed → armed
  - armed → disarmed

### `service`

- **States:** stopped, starting, running, stopping
- **Initial:** stopped
- **Terminal:** none; the machine cycles
- **Transitions:**
  - stopped → starting
  - starting → running, stopped
  - running → stopping
  - stopping → stopped

### `signal_intent`

- **States:** received, admitted, rejected, duplicate, noop, expired, submitting, outcome_unknown, not_placed, partially_filled, filled, unfilled
- **Initial:** received
- **Terminal:** rejected, duplicate, noop, expired, not_placed, filled, unfilled
- **Transitions:**
  - received → admitted, rejected, duplicate, noop
  - admitted → submitting, expired, rejected, noop
  - submitting → partially_filled, filled, unfilled, rejected, outcome_unknown
  - outcome_unknown → partially_filled, filled, unfilled, rejected, not_placed
  - partially_filled → filled

### `spot_order`

- **States:** PENDING_NEW, NEW, PARTIALLY_FILLED, FILLED, CANCELED, REJECTED, EXPIRED
- **Initial:** PENDING_NEW
- **Terminal:** FILLED, CANCELED, REJECTED, EXPIRED
- **Transitions:**
  - PENDING_NEW → NEW, PARTIALLY_FILLED, FILLED, CANCELED, REJECTED, EXPIRED
  - NEW → PARTIALLY_FILLED, FILLED, CANCELED, EXPIRED
  - PARTIALLY_FILLED → FILLED, CANCELED, EXPIRED

### `spot_replace`

- **States:** pending, accepted, partially_applied, rejected, outcome_unknown
- **Initial:** pending
- **Terminal:** accepted, partially_applied, rejected
- **Transitions:**
  - pending → accepted, partially_applied, rejected, outcome_unknown
  - outcome_unknown → accepted, partially_applied, rejected

### `strategy_run`

- **States:** starting, running, stopped, failed
- **Initial:** starting
- **Terminal:** stopped, failed
- **Transitions:**
  - starting → running, stopped, failed
  - running → stopped, failed

## Vocabularies

| Errors | Names |
| --- | --- |
| exchange | transport, invalid_response, invalid_request, missing_credentials, unauthorized, clock_skew, rate_limited, unavailable, outcome_unknown, unknown_symbol, unknown_order, insufficient_balance, rule_violation, rejected, other |
| storage | unavailable, outcome_unknown, invalid_argument, schema_unsupported, other |
| wire | invalid_request, unauthorized, not_found, conflict, unsupported_version, limit_exceeded, rate_limited, unavailable, internal |
| refusal | no_capital_in_settlement_asset, stop_wrong_side, no_stop, below_min_qty, above_market_max, below_min_notional, above_leverage_bracket, insufficient_margin, settlement_wallet_below_risk, stop_crossed, risk_drift, stale, future_timestamp, not_armed, conflict, pyramiding, reversal, unowned_position, unresolved_entry, incident_open, hedge_mode, portfolio_margin, unsupported_account, ambiguous_selection, environment_mismatch, unknown_ticker, outside_universe |

| Unit | Meaning |
| --- | --- |
| base_qty | Decimal in the base asset, on the instrument's step (Spot and linear) |
| contracts | Whole inverse contracts; face value F is the venue's (Binance contractSize, 100 or 10 USD; Bybit 1 USD) |
| exactness | Decimals are exact text that keeps its scale; an amount the venue did not report stays unknown, never zero |
| fee | Decimal with the asset the venue charged it in |
| fraction | Decimal fraction: taker_fee 0.0005 means 0.05 % |
| funding | Decimal in the settlement asset: received is positive, paid negative |
| hash | sha256 followed by 64 lowercase hex digits |
| interval | Interval token, such as 1m or 1d |
| leverage | Decimal multiplier, within the venue's bracket for the notional |
| money | Decimal with its asset code; amounts in different assets are never summed |
| percent | Decimal percent: risk_percent 1 means 1 % |
| pnl | Realized PnL in the settlement asset, net: gross minus fees plus funding |
| price | Decimal in the quote asset, on the instrument's tick |
| quote_amount | Decimal in the quote asset |
| rounding | Stops round to the tick away from the entry; quantities floor to the step; coin amounts round half-even to 12 places; returns round half-even to 8 places |
| timestamp | UTC epoch milliseconds |

## Policy

From ADR 0002 §2, §4 and §5.

| Setting | Value |
| --- | --- |
| Environments | mainnet, testnet, demo |
| Profile identity | id, account_ref, category, owner_tag |
| Account reference | venue, environment, venue_account_id |
| Position mode | one-way |
| Margin modes | isolated, cross |
| Refused modes | hedge, portfolio_margin, bybit_unified_1_0 |
| Changes account settings | no |

- The environment is declared, never inferred from a URL, and every store is bound to one.
- Outside mainnet a key fingerprint may stand in for the venue account ID; mainnet arming needs the venue's ID.
- Leverage is set only while the position is flat with no open orders, then read back.
- At most one armed profile per account and category.
- One active store per account; client order IDs carry the store's activation epoch.
- An ambiguous venue, account, environment or product selection is refused.

### Signals

From ADR 0002 §8, §9 and §10.

| Setting | Value |
| --- | --- |
| Alert age limit | 60 s by default, settable from 5 to 300 s |
| Future allowance | 5 s |
| Drift limit | 1.25 × R |
| Propagation delay | 30 s |
| Settle window | 60 s |
| Stop repair | 3 attempts within 10 s |
| Protective close | 3 attempts within 10 s |
| Unprotected alerts | every 300 s |
| Operator stop-move window | 30 s |

### Evidence

From ADR 0001; ADR 0002 §13.

| Setting | Value |
| --- | --- |
| Live environments | binance.inverse: demo; binance.linear: demo; binance.spot: testnet; bybit: demo, testnet |
| PostgreSQL versions | 16, 18 |
| Artifacts | One passing record for every published file of an artifact, named by its digest |

## Product universe

Rule 1 (ADR 0002 §3), recorded on 2026-10-22T10:52:11Z in [universe-v1-2026-10-22.json](universe-v1-2026-10-22.json). The list is what the rule selected that day, never an allowlist.

| Venue | Family | Selects |
| --- | --- | --- |
| binance | linear | contractType PERPETUAL, marginAsset USDT, status TRADING |
| binance | inverse | contractType PERPETUAL, contractStatus TRADING |
| bybit | linear | contractType LinearPerpetual, settleCoin USDT, status Trading |
| bybit | inverse | contractType InversePerpetual, status Trading |

| Venue | Family | Environment | Perpetuals |
| --- | --- | --- | --- |
| binance | linear | mainnet | 524 |
| binance | inverse | mainnet | 20 |
| binance | linear | demo | 528 |
| binance | inverse | demo | 20 |
| bybit | linear | mainnet | 802 |
| bybit | inverse | mainnet | 22 |
| bybit | linear | testnet | 790 |
| bybit | inverse | testnet | 21 |
| bybit | linear | demo | 802 |
| bybit | inverse | demo | 22 |

## Artifacts

| Artifact | Surfaces | Backends | Published | Platforms | Requirement |
| --- | --- | --- | --- | --- | --- |
| `binary`: The quantforge binary | cli, server | sqlite, postgres | yes | linux, macos, windows | Built with default features: the postgres feature is on and both venues are registered |
| `image`: The OCI image | server, cli | sqlite, postgres | yes | linux/amd64, linux/arm64 | The binary with default features, running non-root, with its store and secrets supplied by the deployment |
| `wheel`: The Python wheel | python | sqlite, postgres | yes | linux, macos, windows | Local and remote Python workflows on both stores, with PostgreSQL pools held by the trusted host; no webhook or Telegram API |
| `sqlite-build`: The SQLite-only build | cli, server | sqlite | no | none | Built with --no-default-features; supported separately and labelled as such, never published as a full artifact |

## Optional values

| Venue | Value | Absent when | Shown as | Source |
| --- | --- | --- | --- | --- |
| binance | liquidationPrice | The position has no liquidation price; Binance reports 0 | N/A with the reason; never estimated | docs/futures-venues.md#stops-and-liquidation |
| bybit | liqPrice | Portfolio margin, or outside the price limits; Bybit leaves it empty | N/A with the reason; never estimated | docs/futures-venues.md#stops-and-liquidation |
| bybit | kline trade count | Bybit's klines carry no trade count | Unknown: None in candles, and known = 0 in segments | docs/segment-spec.md |
| bybit | position stop client ID | Bybit's position stops carry no client ID | Identified by the position's stopLoss; ownership comes from the ledger | docs/futures-venues.md#order-identity-and-retention |
| bybit | position stop trigger basis | /v5/position/list reports the stop's price but not its trigger basis | Read from the order view, or ALG-741 uses a stop whose basis can be read; never assumed (unverified item 5) | docs/futures-venues.md#unverified-items |
| bybit | inverse face value | No instrument field carries it | F = 1 USD, from Bybit's documented rule | docs/futures-venues.md#units |
| binance | COIN-M minimum notional | COIN-M instruments have no minimum-notional filter | No notional check; the minimum quantity still applies | docs/futures-venues.md#units |
| binance | acknowledgement fills | Since 2026-06-30 Binance's futures acknowledgements carry no fills | Fills come from order queries, fill records or private events | docs/futures-venues.md#order-identity-and-retention |
| binance | futures account ID | A key may expose no stable account ID (unverified item 3) | Outside mainnet a key fingerprint stands in; mainnet arming is refused | docs/adr/0002-automation-architecture.md |

## Blockers

| Item | Capability | Cells | Effect |
| --- | --- | --- | --- |
| [1](../futures-venues.md#unverified-items) | `futures.native_stop` | family inverse, venue binance | These cells cannot pass until ALG-763 proves COIN-M algo stops on demo; if it cannot, the release waits on Marko's decision |
| [11](../futures-venues.md#unverified-items) | `futures.monitoring` | venue binance | These cells cannot pass until ALG-764 finds a working private-stream route on demo |

## Scope changes

| ID | Kind | Date | By | Summary | Reference | Removes |
| --- | --- | --- | --- | --- | --- | --- |
| SC-1 | exclusion | 2026-10-22 | Marko | Crypto perpetuals only: dated futures and TradFi perpetuals are refused, and both families stay required on both venues | ADR 0002 §3 | none |
| SC-2 | exclusion | 2026-10-22 | Marko | A profile owns only the positions it opened; there is no adoption in v0.3 | ADR 0002 §10 | none |

## Stories

Cells each story owns as implementation or evidence. Consumers read the evidence and own none.

| Story | Role | Implements | Evidences |
| --- | --- | --- | --- |
| ALG-375 | Backtest engine as a library API driven by external strategies | 4 | 0 |
| ALG-376 | Python package scaffold and abi3 wheels | 9 | 0 |
| ALG-377 | Core types exposed to Python | 4 | 0 |
| ALG-378 | Python strategy adapter | 4 | 0 |
| ALG-379 | Python backtest entry point | 6 | 0 |
| ALG-380 | NumPy interop for candle batches | 4 | 0 |
| ALG-381 | Pandas helpers for candles and results | 8 | 0 |
| ALG-382 | Python candle reader for SQLite and PostgreSQL | 7 | 0 |
| ALG-383 | Golden parity fixtures from the Rust SMA cross | 0 | 8 |
| ALG-384 | Wheel build matrix and Python test job in CI | 0 | 5 |
| ALG-387 | Python package release | 5 | 0 |
| ALG-388 | Python strategies driving dry-run live trading | 4 | 0 |
| ALG-401 | Storage backend selection and connection settings | 21 | 0 |
| ALG-402 | Shared storage conformance suite | 8 | 16 |
| ALG-404 | PostgreSQL schema, migrations and the postgres feature | 6 | 0 |
| ALG-405 | PostgreSQL candle store | 13 | 0 |
| ALG-406 | PostgreSQL run journal store | 3 | 0 |
| ALG-407 | SQLite to PostgreSQL migration with verification | 1 | 0 |
| ALG-408 | OCI container image | 8 | 0 |
| ALG-409 | Compose file with PostgreSQL | 2 | 0 |
| ALG-411 | CI against a PostgreSQL service, and the SQLite-only build | 4 | 55 |
| ALG-412 | Multi-arch OCI validation and release preparation | 8 | 8 |
| ALG-488 | Data sync and validate commands | 2 | 0 |
| ALG-491 | Monitor commands | 2 | 0 |
| ALG-496 | SQLite candle and journal stores | 1 | 0 |
| ALG-499 | Binance credential flags and environment variables | 2 | 0 |
| ALG-506 | Immutable content-addressed candle segments | 6 | 0 |
| ALG-508 | HTTP history plane | 12 | 0 |
| ALG-509 | Backtest job API | 16 | 0 |
| ALG-510 | Process-per-job worker pool | 16 | 0 |
| ALG-511 | Job results and downloads | 12 | 0 |
| ALG-512 | Job logs and progress streaming | 12 | 0 |
| ALG-513 | Remote backtests from the CLI | 6 | 0 |
| ALG-515 | Python worker runtime | 8 | 0 |
| ALG-516 | Reproducibility ledger and verified re-runs | 12 | 0 |
| ALG-517 | Bundle execution threat model and hostile-payload suite | 2 | 2 |
| ALG-518 | WebSocket live plane | 12 | 0 |
| ALG-519 | Publish-after-persist fan-out | 12 | 0 |
| ALG-520 | Remote market data source with catch-up | 12 | 0 |
| ALG-521 | Dry-run trading on remote data, end to end | 12 | 12 |
| ALG-525 | Container isolation for hosted workers | 2 | 0 |
| ALG-527 | Python client: connection, auth and job primitives | 6 | 0 |
| ALG-528 | Python bundle builder and submission | 6 | 0 |
| ALG-529 | Python job handles, results and provenance | 8 | 0 |
| ALG-530 | Parity of goldens, Python local and Python remote on both stores | 0 | 34 |
| ALG-532 | Python runtime embedded in the binary | 6 | 0 |
| ALG-533 | Python strategies in trade run and trade close | 4 | 0 |
| ALG-534 | Removal of the Rust built-in strategies | 4 | 0 |
| ALG-535 | Coordinated 0.3.0 release | 8 | consumer |
| ALG-683 | Multi-exchange connector ports and registry | 1 | 0 |
| ALG-684 | Exchange selection through the CLI, storage and protocol | 6 | 0 |
| ALG-685 | Exchange conformance suite | 1 | 0 |
| ALG-686 | Binance Spot symbol rules | 1 | 0 |
| ALG-687 | Binance Spot REST hardening and rate-limit budget | 3 | 0 |
| ALG-688 | Binance Spot kline stream with gap-fill | 2 | 0 |
| ALG-689 | Binance Spot limit orders, time in force, cancel and replace | 6 | 0 |
| ALG-690 | Unified Spot order lifecycle and journal | 4 | 0 |
| ALG-691 | Binance Spot user-data stream | 1 | 0 |
| ALG-692 | Spot reconciliation at start and reconnect | 2 | 0 |
| ALG-693 | Bybit v5 client, signing, errors and environments | 16 | 0 |
| ALG-694 | Bybit credentials | 8 | 0 |
| ALG-695 | Bybit Spot instrument rules | 1 | 0 |
| ALG-696 | Bybit Spot klines, sync and validate | 4 | 0 |
| ALG-697 | Bybit Spot kline stream with gap-fill | 2 | 0 |
| ALG-698 | Bybit Spot market and limit orders, time in force, cancel and status | 5 | 0 |
| ALG-699 | Full CLI on the Bybit testnet | 4 | 2 |
| ALG-700 | Bybit account balances and private streams | 10 | 0 |
| ALG-701 | Bybit reconciliation through the shared reconciler | 2 | 0 |
| ALG-702 | Conformance on both exchanges in CI | 0 | 34 |
| ALG-703 | End-to-end release evidence on both exchanges and stores | 0 | 68 |
| ALG-711 | Website claims checked against the matrix | 0 | consumer |
| ALG-719 | Released-state website activation | 0 | consumer |
| ALG-728 | PostgreSQL through the CLI and local execution | 25 | 0 |
| ALG-729 | PostgreSQL in the server, jobs, workers and provenance | 19 | 0 |
| ALG-730 | PostgreSQL concurrency, failures and restart recovery | 0 | 1 |
| ALG-731 | Release gate for store parity across both exchanges | 0 | consumer |
| ALG-733 | Typed futures instrument, account, position and execution ports | 4 | 0 |
| ALG-734 | Identity migration and storage and protocol compatibility | 21 | 0 |
| ALG-735 | Asset defaults, risk capital and per-asset policies | 16 | 0 |
| ALG-736 | Decimal risk sizing for linear and inverse futures | 8 | 0 |
| ALG-737 | Versioned TradingView payloads and alert templates | 8 | 0 |
| ALG-738 | Signal inbox, execution ledger and event outbox | 54 | 0 |
| ALG-739 | Bybit futures instruments, accounts and positions | 12 | 0 |
| ALG-740 | Bybit futures leverage, entry and reduce-only close | 24 | 0 |
| ALG-741 | Bybit native stop loss and liquidation | 12 | 0 |
| ALG-742 | Futures conformance matrix for adapters and sizing | 16 | 164 |
| ALG-743 | Authenticated webhook admission | 8 | 0 |
| ALG-744 | Futures private-stream monitoring and REST reconciliation | 48 | 0 |
| ALG-745 | Risk-sized entries through confirmed fills | 24 | 0 |
| ALG-746 | Native stop verification and protective failure handling | 48 | 0 |
| ALG-747 | Exit, stop-loss and manual-close races, with realized PnL | 32 | 0 |
| ALG-748 | Crash recovery, liquidation warnings and safety faults | 0 | 100 |
| ALG-749 | Telegram transport and notification outbox | 16 | 0 |
| ALG-750 | CLI asset settings and automation profiles | 24 | 0 |
| ALG-751 | Telegram message types | 16 | 0 |
| ALG-752 | Automation start, status and graceful stop | 8 | 0 |
| ALG-753 | Account and futures position status in the CLI | 8 | 0 |
| ALG-754 | Deployment, alert setup and recovery drills | 8 | 42 |
| ALG-755 | Final requirement and readiness gate | 0 | consumer |
| ALG-757 | PostgreSQL segment store | 13 | 0 |
| ALG-758 | Durable commits and bounded waits on both stores | 10 | 0 |
| ALG-759 | Binance futures transport, routing and account profiles | 20 | 0 |
| ALG-760 | Binance futures instruments, accounts, positions and leverage | 14 | 0 |
| ALG-761 | Futures market data on both exchanges | 48 | 0 |
| ALG-762 | Binance futures entry, queries and reduce-only close | 20 | 0 |
| ALG-763 | Binance algo stop loss and liquidation | 12 | 0 |
| ALG-764 | Binance futures private streams, reconciliation and accounting | 32 | 0 |
| ALG-765 | Binance futures conformance gate | 0 | 42 |
| ALG-766 | Spot amend and cancel-replace on both exchanges | 2 | 0 |
| ALG-767 | Routing by persisted exchange, account and product profile | 28 | 0 |
| ALG-768 | SQLite and PostgreSQL cutover, backup and recovery | 2 | 4 |
| ALG-769 | Release block until every parity cell passes | 0 | 71 |
