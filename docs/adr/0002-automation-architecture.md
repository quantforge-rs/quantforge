# ADR 0002: Automation architecture and futures capability contract for v0.3

- Status: Accepted
- Date: 2026-10-22
- Ticket: ALG-732.
- Unblocks:
  - ALG-733, the typed instrument, account, position and execution ports;
  - ALG-737, the alert payloads;
  - ALG-756, the capability matrix;
  - and through them every automation story up to ALG-769.

## Context

**What v0.3 adds.** QuantForge 0.3 implements the automation PRD (iteration 3):
- TradingView alerts become risk-sized market entries;
- each entry is protected by a stop loss held on the exchange;
- positions are monitored;
- everything is operated through the CLI and reported to Telegram.

A parity requirement added on 2026-10-21 extends the PRD from one exchange to
two. The same functions must work on Binance and Bybit, for USDT-settled linear
and coin-settled inverse futures, LONG and SHORT, with SQLite or PostgreSQL.
[automation-requirements.md](../automation-requirements.md) maps every
requirement to its stories.

**Today's code serves one Spot exchange and candle strategies:**
- **`ExchangeId::BinanceSpot` names a venue and a product at once**
  (`src/model/market.rs:49–60`). A market is
  `MarketId { exchange, symbol, interval }`, so instrument identity and candle
  interval are one key.
- **A position cannot be a short, leveraged or account-scoped.** It is
  `TargetPosition { Flat, LongAllIn }` and `PositionState { qty, … }`, open
  when `qty > 0` (`src/model/run.rs:9–13,62–83`).
- **`Strategy::on_bar` is a pure function of the bars it has seen**
  (`src/sdk/strategy.rs:67–72`). Backtests and the ALG-383 goldens rely on that.
- **IDs:** order and trade IDs are `i64`, while Bybit's are strings.
- **An exchange has three spellings:** `binance` on the wire and the CLI,
  `binance_spot` in storage columns, and `BinanceSpot` inside `state_json`.
  The wire names a market by the first alone (`src/protocol/mod.rs:42–48`),
  which cannot tell Binance's Spot, USDⓈ-M and COIN-M apart.
- **Stored data has no environment:** testnet and mainnet rows share keys.

**The venues have changed.** Both changed their derivatives APIs in the past
year:
- Binance moved conditional orders to an algo API, and its USDⓈ-M and COIN-M
  products now share account settings.
- Each venue's test environments behave differently.

[futures-venues.md](../futures-venues.md) records those facts and what is
still unverified. This record decides what QuantForge does with them.

**What this record answers, from ALG-732:**

| | Acceptance criterion | Sections |
| --- | --- | --- |
| AC1 | A requirement-to-story matrix, with the parity requirement | §13, [automation-requirements.md](../automation-requirements.md) |
| AC2 | A Rust external-signal executor on shared ports; alerts never become `on_bar` | §1 |
| AC3 | Instrument identity apart from the candle interval; account, environment, owner and position identity; the product universe and its routes | §2, §3, [futures-venues.md](../futures-venues.md) |
| AC4 | One-way mode, no pyramiding or reversal, explicit arming, unsupported modes refused, no silent mode change | §4, §5 |
| AC5 | Risk-capital currency, inverse units, margin budget, stop trigger, stale alerts, ownership and adoption, the response to a failed stop | §6 to §10 |
| AC6 | Manual changes monitored though pyramiding is off; Spot and Python acceptance kept apart | §10, §11, §13 |
| AC7 | Shared model and wire decisions frozen before the Python bindings and ALG-534; the FFI delta named | §14 |
| AC8 | Parity defined; venue differences kept; Spot gains no futures semantics | §13 |

## Decision drivers

1. **Protection.** Exposure is never left unprotected when QuantForge could
   protect it. Nothing is called protected or flat without the exchange's
   evidence.
2. **No duplicates or misroutes.** No duplicate exposure, and never the wrong
   account, venue or environment.
3. **Determinism.** Candle strategies stay deterministic, and the ALG-383
   goldens stand.
4. **Parity.** The same operations, states and outcomes on both venues and both
   stores, with real venue differences kept visible.
5. **Exact money.** `Decimal` throughout, an explicit currency on every amount,
   and no silent conversion.
6. **Trusted host.** Credentials and pools stay in the trusted host process
   (ADR 0001).
7. **Checkable.** Every rule can be checked before release. A missing
   implementation fails its cell; no flag excuses it.

## Considered options

| Option | Verdict |
| --- | --- |
| **A. A Rust `ExternalSignalExecutor` on shared connector, risk and ledger ports** | **Chosen.** |
| B. Alerts as candle-strategy input, turned into `on_bar` calls | An alert carries no bar, no stop semantics and no identity a strategy could act on. Replaying bars would replay trades, and backtest determinism and the goldens would break. |
| C. Python webhook strategies in job workers | Exchange credentials would reach untrusted workers, and protection would depend on user code. |
| D. A separate bot beside QuantForge | It duplicates connectors, storage and rules, and the two would drift apart, which parity forbids. |
| Universe: perpetuals plus dated futures | Adds delivery as a close reason, asset keys that expire, TradingView's month codes, and twice the release cells. Marko chose perpetuals only (§3). |
| Ownership: adopting positions QuantForge did not open | Adds a command, an "adopted" origin and its failure cases. Deferred; it can be added later without breaking anything (§10). |

## Decision

### 1. Architecture

**Two inputs on one execution foundation:**
- **Candle strategies are unchanged:**
  - `Strategy` keeps its pure `on_bar`, and Spot keeps `TargetPosition`;
  - backtests and `trade run` run as before, as does Python, locally and
    remotely;
  - in v0.3 they take spot data series only, and refuse a linear or inverse
    series.
- **External signals go to an `ExternalSignalExecutor`** in a new library
  module, `src/automation/`. It never builds a candle and never calls a
  `Strategy`; an alert never becomes an `on_bar` call.
- **What both share:**
  - the connectors' transports, signing and error taxonomy;
  - the `Decimal` model;
  - ADR 0001's storage bridge;
  - the exchange registry;
  - the protocol's decimal rules.

**The pipeline:**
1. Authenticate and validate the delivery, then commit it to a durable inbox.
   The HTTP 2xx means only that this commit happened.
2. Take the policy snapshot, size the entry, and reserve its risk and margin.
3. Persist the intent, with its operation and client order IDs.
4. Submit, then reconcile the outcome.
5. Confirm the fills from order queries, fill records and the private stream.
6. Place and verify the exchange-native stop.
7. Write outbox events. Notifications are delivered from the outbox.

No database transaction spans a network request.

**States:**
- An intent moves through `admitted`, `rejected` or `expired`, then
  `submitting`, `outcome_unknown` or `partially_filled`.
- A position is `protection_pending`, `protected`, `protection_unknown` or
  `unprotected`, then `closing` and `closed`.
  - `protection_unknown` means a read-back could not establish whether a stop
    exists. Entries halt, and nothing places or cancels a stop until the
    read-back succeeds.
- A close records its reason: exit signal, stop loss, manual, liquidation or
  ADL, protective, or unknown.
- An acknowledgement is not a fill, and a fill is not protection.

**Ports, built by ALG-733:**
- **Futures connector ports, split by capability:**
  - instrument metadata;
  - account, mode and position snapshots;
  - leverage;
  - an **entry** port, the only one that opens exposure;
  - a **protection** port that can only reduce: a reduce-only close, and stop
    place, read, replace and cancel;
  - query by client ID;
  - private events.
- **`ExecutionStore`:** synchronous, on SQLite and PostgreSQL through
  ADR 0001's bridge.
- **`SignalInbox`:** the server's only view of the executor.
- **`Notifier`:** delivers from the outbox.
- **A clock**, injected.
- **Sizing:** a pure function of the policy, the instrument's rules, the signal
  and an account snapshot.

**Process:**
- One owner process per store hosts admission, the executor, the monitor and
  the notifier.
- `automation start`, `status` and `stop` come with ALG-752; status reads the
  store.
- **Dependencies:**
  - `automation` uses `model` and `ports`, as the engines do;
  - `server` uses `protocol` and the `SignalInbox` port;
  - Binance futures adapters live under `src/exchange/binance/`, the only
    directory the exchange-boundary test exempts.

### 2. Identity

**Venue and category:**
- A venue is a registry name: `binance` or `bybit`.
- A category is `spot`, `linear` or `inverse`. On Binance these are Spot,
  USDⓈ-M and COIN-M.

**Product lines keep the repository's dual string channels.** Each has three
fixed spellings:

| Product line | Wire and CLI | Storage (`as_str`) | `state_json` (serde) |
| --- | --- | --- | --- |
| Binance Spot | `binance` + `spot` | `binance_spot` | `BinanceSpot` |
| Binance USDⓈ-M | `binance` + `linear` | `binance_linear` | `BinanceLinear` |
| Binance COIN-M | `binance` + `inverse` | `binance_inverse` | `BinanceInverse` |
| Bybit Spot | `bybit` + `spot` | `bybit_spot` | `BybitSpot` |
| Bybit linear | `bybit` + `linear` | `bybit_linear` | `BybitLinear` |
| Bybit inverse | `bybit` + `inverse` | `bybit_inverse` | `BybitInverse` |

- The Spot spellings are unchanged.
- `ExchangeId` becomes `#[non_exhaustive]`.
- A command names a product line by venue and category. `spot` is the default
  only where a command is Spot-only. ALG-761 and ALG-750 choose the flag.

**`InstrumentId { venue, category, symbol, settle }`** identifies what is
traded:
- `settle` is the asset the venue books the instrument's PnL in: the settle coin
  for linear, the base coin for inverse, the quote asset for Spot.
- It comes from venue metadata and is checked again at every resolution; a
  mismatch is refused.
- In Rust, as for market data below, the product line carries the venue and the
  category.

**`DataSeriesKey { venue, category, symbol, interval }`** identifies market
data: candles, segments, the protocol's markets and strategies.
- **In Rust** it stays today's `MarketId { exchange, symbol, interval }`, with
  `exchange` naming the product line, which carries both venue and category. So
  `state_json` keeps its shape and only gains new `ExchangeId` values.
- **On the wire,** the product line is spelled `exchange` plus `category` (§14).
- **It carries no `settle`,** because a symbol within a product line already
  fixes it.
- This amends the ALG-733 and ALG-761 briefs, which key data by the whole
  `InstrumentId`.

**Environment** is `mainnet`, `testnet` or `demo`. It is declared, never
inferred from a URL.
- **Store binding:**
  - Every store is bound to one environment.
  - A connector or profile for another environment is refused before any write.
  - A store created before this record is declared once by its operator, never
    guessed (ALG-734).
  - So market-data keys need no environment. Execution keys carry it anyway, as
    a second safeguard.
- **Signed requests** go only to the hosts documented for that environment
  ([futures-venues.md](../futures-venues.md#routes)). The Spot adapter's suffix
  check, which calls `demo-fapi.binance.com` production, is not used for
  futures.
- **Public data** (instrument rules, mark and book prices, candles) comes from
  the hosts the venue documents for the profile's environment. For Bybit's demo,
  those are mainnet's public hosts. Rules and prices are never taken from
  another environment: Binance's demo, for one, has its own book and its own
  steps.

**Accounts and profiles:**
- **`AccountRef { venue, environment, venue account id }`:**
  - On mainnet the ID comes from the venue: Bybit's `userID`, or Binance's
    `accountAlias` checked against the spot `uid`. ALG-759 and ALG-739 verify
    these.
  - Outside mainnet, a fingerprint of the API key may stand in where the venue
    gives no ID.
- **A profile's identity is `{ id, AccountRef, category, owner tag }`.**
  - Its policy revision and label are metadata: editing them never disarms it,
    while changing its identity does.
  - The owner tag is random, unique in its store, and checked at arming.
- **At most one armed profile per account and category.** Where the account
  ID is a key fingerprint, the limit is one per venue, environment and category
  in a store, since two keys on one account would otherwise pass as two
  accounts.
- **One active store per account.** ALG-734's activation contract deactivates
  a store's source when a copy is activated. The client-order-ID layout carries
  the store's activation epoch, so orders with the right owner tag but another
  epoch reveal a second writer, and entries halt.

**Positions and operations:**
- **`PositionKey { AccountRef, InstrumentId }`**, in one-way mode only.
  - Bybit's `positionIdx` and Binance's `positionSide` exist only inside the
    adapters.
  - Leases and serialization are keyed by `PositionKey`, never by profile, so no
    two writers enter the same position. A lease records the profile that holds
    it.
- **`PositionCycleId`:** from flat, through an owned entry, back to flat.
- **`SignalId`:** scoped to its profile, and stored with the hash of its
  canonical payload.
- **`OperationId`:** every action that changes anything on an exchange has one.

**Client order IDs:**
- **Built from** a layout version, the owner tag, the store's activation epoch,
  the operation, its leg and its attempt: at most 36 characters of
  `[A-Za-z0-9_-]`, which both venues accept.
- **Not idempotency keys:** Binance checks uniqueness only among open orders,
  and Bybit's position stops have no client ID at all.

**Venue IDs** (order, algo order, trade and execution) are opaque text in every
type, Spot's included. ALG-734 migrates today's `i64` fields and columns.

### 3. Product universe

v0.3 trades crypto perpetuals in trading status. They are selected from venue
metadata by rule, never from a fixed list:

| Venue | Family | Rule |
| --- | --- | --- |
| Binance | linear | `contractType` `PERPETUAL`, `marginAsset` `USDT`, `status` `TRADING` |
| Binance | inverse | `contractType` `PERPETUAL`, `contractStatus` `TRADING` |
| Bybit | linear | `contractType` `LinearPerpetual`, `settleCoin` `USDT`, `status` `Trading` |
| Bybit | inverse | `contractType` `InversePerpetual`, `status` `Trading` |

**Refused, each with a message saying why:**
- dated futures;
- Binance's TradFi perpetuals;
- linear contracts settled in anything but USDT;
- every other status.

**Why dated futures are refused:** that is Marko's scope decision of 2026-10-22.
Both families remain required on both venues.

**A missing family blocks the release:** if a required family is missing on a
venue or environment, the release waits; the scope never shrinks to fit.

**Where this is frozen:** ALG-756 freezes this rule, as rule version 1, with a
dated list from
[`universe.py`](../../spikes/futures-contract/universe.py). The counts, routes
and venue changes are in [futures-venues.md](../futures-venues.md).

### 4. Account and position policy

**Modes:**
- **One-way mode only.** Hedge mode is refused, with a message saying how to
  switch.
- **QuantForge never changes an account setting:** position mode (one setting
  for USDⓈ-M and COIN-M on Binance since 2026-06-30), margin mode, multi-assets
  mode or account type.

**Margin:**
- Isolated and cross margin are accepted.
- Portfolio margin is refused on both venues:
  - Binance's portfolio-margin accounts trade through a separate API or report
    on a separate stream;
  - Bybit's reports no leverage, margin or liquidation price per position.

**Accounts and leverage:**
- **Bybit accounts must be unified accounts 2.0** (`unifiedMarginStatus` 5 or
  6). They are the only kind that trades inverse contracts inside the unified
  account.
- **Leverage is set per asset,** as the PRD asks:
  - only while its position is flat and no orders are open;
  - then read back;
  - and it must lie within the venue's bracket for the notional.

**Checks:** these run at arming, at start, and after every reconnect. A failed
check blocks entries and says why.

### 5. Execution defaults and arming

**Entries refused:**
- **No pyramiding.** An entry on the same side as an open position is a
  duplicate, and is refused.
- **No reversal.** An entry on the opposite side is refused; the indicator sends
  an exit first.
- **An unowned position** on the instrument blocks entries there, because
  one-way mode would merge it.
- **An unresolved entry** (`submitting` or `outcome_unknown`) blocks every
  other entry on its `PositionKey`.
  - An exit that arrives meanwhile waits until the entry settles, then applies
    to the cycle it opened.
  - If the settle window (§9) ends without an answer, the key raises an incident
    and its entries halt until reconciliation shows the position.

**Arming:**
- **Profiles start disarmed.**
- **Arming** is a CLI action, confirmed with `--yes` or by typing `yes` at a
  terminal. It shows the production marker on mainnet. A deactivated store
  cannot arm (ALG-734).
- **A disarmed profile cannot enter:**
  - entries that arrive while it is disarmed are refused as `not armed`, and
    never execute later;
  - it is given no entry port.
- **Owned positions keep their exits, monitoring, stop repair and protective
  close** through the protection port, which can only reduce.

**Assets, policy edits and stopping:**
- **A disabled asset** behaves the same way, for that asset alone (PRD §3.3).
- **Defaults** (PRD §3.3) supply settings for assets that have none of their
  own. They never enable trading by themselves. An asset without settings
  trades only if:
  - the operator has enabled the defaults;
  - it is inside the universe;
  - its ticker uses the venue's documented TradingView spelling, for example
    `SOLUSDT.P` for a perpetual.
- **A policy edit** applies to later entries. An open cycle keeps the snapshot
  it was opened with; an edit never resizes it or removes its protection.
- **A normal stop** of the service closes no position and cancels no stop.

### 6. Risk, units and margin

**Capital:**
- **Configured** per profile and per settlement asset, with its currency: USDT
  for linear, the coin itself for inverse.
- **Never converted** between currencies in v0.3. An entry whose settlement
  asset has no capital configured is refused.
- **Not the balance:** configured capital is a setting, not the account's
  balance.

**Sizing,** with E the alert's expected entry:
- **Budget:** `R = B × r`, where B is the capital and r is the asset's risk
  percentage.
- **The stop is chosen in this order:**
  1. the alert's absolute stop, checked to lie on the protective side;
  2. otherwise the asset's percentage p: `E × (1 − p)` for LONG, `E × (1 + p)`
     for SHORT;
  3. otherwise the entry is refused.
- **Rounding:** the stop is rounded to the tick away from the entry, so it is
  never tighter than asked. That gives S′, and sizing uses S′.
- **Linear:** quantity `Q = R / |E − S′|`, in the base asset.
- **Inverse:** contracts `N = R / (F × |1/E − 1/S′|)`, where F is the
  contract's face value in USD:
  - on Binance, the `contractSize` (100 for BTC, 10 for every other coin);
  - on Bybit, 1. That is a documented rule; no metadata field carries it.
- **Quantity limits:**
  - The quantity is floored to the market-order step and its risk recomputed;
    it stays at or below R.
  - Below the minimum quantity or notional, or above the market-order maximum,
    the entry is refused.
  - A quantity is never rounded up.
- **Leverage** changes the margin, never the quantity, and the notional must fit
  its bracket.
- **Sizing uses E,** as the PRD asks. The actual fill and the actual risk are
  recorded beside it. Nominal risk is not a loss limit: gaps, fees and funding
  can exceed it.

**Margin budget:**
- **What must fit:** the initial margin (`notional / leverage`) plus taker fees
  for opening and closing must fit the available margin of the venue's pool,
  minus reservations already held against that pool for the same account.
- **The pools are the venue's own:**

  | Venue | Pool |
  | --- | --- |
  | Binance USDⓈ-M | the USDT wallet, or the USD pool in multi-assets mode |
  | Binance COIN-M | each coin |
  | Bybit isolated margin | each coin |
  | Bybit cross margin | the account's USD pool |

- **Conversion:** where a pool is in USD, the adapter converts with the venue's
  own index price and records its source and time.
- **If it does not fit:** the entry is refused, never shrunk. The venue's own
  rejection stays authoritative.

**Settlement wallets:**
- An entry is refused when the wallet of its settlement asset (USDT, or the
  coin) holds less than R.
  - Under cross margin, other collateral can fund the margin.
  - A stop-out would then leave a debt in the settlement asset.
- Below B it is allowed, with a warning.

**Worked examples** come from
[`sizing.py`](../../spikes/futures-contract/sizing.py), which checks itself;
see Evidence.

**The PRD's p.9 example fits a 150 USDT budget** (1 % of 15,000 USDT) in every
figure but its risk line, which says 15.00:
- 4.38 SOL on a 0.01 step;
- 1,025.93 of value;
- 205.19 of margin at 5×;
- p.10's −149.93 at the stop.

Its message fields stay; tests take their numbers from `sizing.py`.

### 7. Native protection

**One stop per position.** Every owned position has one stop-market order,
held by the exchange, covering the whole position. A replacement adds a second
stop only for as long as it bridges the two (below). The stop is triggered by
the **mark price**, the price that drives liquidation on both venues.

**How each venue holds it:**
- **Bybit:** the position's stop, set through `trading-stop` with
  `tpslMode=Full`, `slOrderType=Market` and `slTriggerBy=MarkPrice`.
  - It follows the position's size and ends with it.
  - Replacing it amends it in place.
  - It is attached to the entry order where ALG-741 shows that works on the test
    environments. Otherwise it is set after the first fill.
- **Binance USDⓈ-M and COIN-M:** an algo `STOP_MARKET` with
  `closePosition=true`, `workingType=MARK_PRICE` and `priceProtect=false`,
  placed after the first fill.
  - Stops are never sent to the ordinary order endpoint (ALG-763).
  - COIN-M's algo API is only partly documented. If ALG-763's demo evidence
    shows it cannot carry the whole lifecycle, the Binance inverse cells block
    the release. Falling back to the legacy endpoint would be a scope decision
    for Marko, not an implementation choice.
  - An untriggered algo order cannot be modified, and a second `closePosition`
    stop may be refused. So a replacement uses a bridge that never leaves the
    position bare:
    1. place a reduce-only stop for the current quantity;
    2. verify it;
    3. cancel the old stop;
    4. place the new stop and verify it;
    5. then cancel the bridge.

    If a step fails, the stops already in place stay, and §8 takes over.

**When a position counts as protected:**
- **The read-back:** a position is `protected` only when a read-back shows its
  stop with the recorded price, trigger basis and `priceProtect`, covering the
  whole position.
- **An operator's stop counts too.** A stop the operator placed that covers the
  whole position makes it `protected (operator)`. It is compared with
  liquidation on its own trigger basis, with a warning when that basis is not
  the mark price.
- **Cleanup** cancels only stops whose IDs QuantForge issued, never one the
  operator placed.
- **A partial fill** enters protection at once.
- **A stop that is rejected or expires when it triggers** is a protection
  failure (§8).
- **A stop refused because the price has already crossed it** is final: the
  position goes straight to the protective close.

**Liquidation:**
- It is compared with the stop on the mark basis, and QuantForge warns when the
  stop would not trigger first.
- The value is the venue's own. When the venue gives none it shows as N/A with
  the reason: Binance reports `0`, and Bybit leaves it empty.
- It is never estimated.

### 8. Protection failure

1. **Repair:** up to three placements within 10 seconds, each read back.
2. **If repair fails:**
   - record a critical incident;
   - halt the profile's entries;
   - close the remaining exposure with a reduce-only order: up to three attempts
     within 10 seconds, querying the previous attempt before each retry.
3. **If the close fails, or its outcome is unknown:**
   - the position is `unprotected`;
   - an alert goes out at once, and every five minutes after;
   - reconciliation continues;
   - repair and the protective close are retried with backoff and at every
     reconnect until the position is protected or flat.

     This does not apply when the operator removed the stop (§10).

**Guarantees:**
- Nothing is called flat or protected without the exchange's evidence.
- Cleanup never cancels protection on what remains.
- Entries stay halted while any owned position is unprotected.

**Recovery:** the operator clears the incident from the CLI once the position is
flat or protected.

### 9. Signals

**Stale entries** (Marko's choice) are refused when:
- **Age:** the alert's `{{timenow}}` is more than 60 seconds old at admission or
  at submission, or more than 5 seconds in the future. The limit can be set per
  profile, from 5 to 300 seconds.
- **The stop has been crossed:** a mark price or a touch (the bid for LONG, the
  ask for SHORT), fetched just before submission, is at or past the stop. A
  price that cannot be fetched fresh refuses the entry too. The stop itself
  triggers on the mark price.
- **Drift:** the risk at the expected fill (the ask for LONG, the bid for SHORT)
  exceeds 1.25 × R.

TradingView resends a failed delivery at most three times, five seconds apart.
So 60 seconds admits every legitimate delivery and refuses late replays.

**Exits:**
- An exit carries the side it closes.
- It closes only an owned cycle of that side whose entry's `{{timenow}}` is
  strictly earlier than its own. Otherwise it is recorded as a no-op.
- Exits never expire.

**Unknown outcomes:**
- **How they are settled:** within a bounded window, from the order query, the
  private stream and the change in the position.
- **When "not found" counts:** only inside the venue's retention window, and
  only after its propagation delay.
- **An entry is never resubmitted after an unknown outcome.**
- **Closes and stops** (reduce-only) may be retried once a query has settled the
  previous attempt.

**Duplicates and recording:**
- **Duplicates:**
  - the same signal ID with the same payload hash gets the original's outcome;
  - the same ID with a different hash is a conflict, which is refused and
    alerted.
- **Recording:** every authenticated delivery is recorded, admitted or
  rejected, before the response.

**The payload contract,** which ALG-737 writes:
- **Module:** it lives in a `protocol::signal` module.
  - It has its own `version` field and is strict: unknown fields are refused.
  - This is an exception to the protocol's ignore-unknown rule, as the bundle
    manifest is.
- **Signal ID:** stable across resends, built from fields TradingView renders
  the same way each time: the template's constant alert name, the side,
  `{{ticker}}` and `{{time}}`.
- **Profile:** the webhook route's token chooses the profile. An alias in the
  payload must agree with it.
- **Content:**
  - The ticker resolves through the profile's asset table, never by guesswork.
  - Prices are decimal strings.
  - A payload carries no credentials, no risk override and no switch to live
    trading.

### 10. Ownership and manual changes

**A profile owns the cycles its own entries opened** (Marko's choice).
- **The record:** the ledger is the record; client IDs and the stop on the
  position corroborate it.
- **Restore:** a restored store finds its positions again from these, within the
  venues' retention windows.
- **Other positions:** every other position on the account is shown read-only,
  and blocks entries on its instrument.
- **Adoption:** there is no adopt command in v0.3.

**Manual changes to an owned position:**
- **Additions and reductions** keep ownership. The full-position stop covers
  them, and the risk is recomputed and reported.
- **A stop moved to a valid price** is accepted, and compared with liquidation
  again.
- **A stop the venue reports as cancelled by the account's user** is the
  operator's intent, and is not restored. That means Bybit's `cancelType`
  `CancelByUser`, or a Binance `CANCELED` that QuantForge did not request.
  - A Binance algo stop cannot be modified, so an operator moves one by
    cancelling it and placing another. If a stop that covers the whole position
    appears within 30 seconds, the change is a move, and the position is
    `protected (operator)` (§7).
  - Otherwise the position becomes `unprotected (operator)`. Alerts repeat
    every five minutes, and entries halt.
- **A stop gone for any other reason, or with no evidence of who removed it,**
  gets §8's repair. Where a venue cannot attribute a cancel at all, ALG-746 adds
  a CLI command that leaves a position unprotected on purpose.
- **A manual close** ends the cycle, with the reason `manual`.

### 11. Monitoring and recovery

- **Sources:** private streams, with REST snapshots to fill gaps. There is no
  global order across them:
  - Bybit's events carry `seq`;
  - Binance's carry event times, ordered within one event type.
- **Reconciliation:** at start and after every reconnect, positions, orders,
  fills and stops are reconciled before the profile is ready for entries.
- **Halting:** state that is stale, or not yet reconciled, halts entries for its
  account.
- **Connection changes:** the loss and the restoration of a connection are both
  notified.

### 12. Accounting

**Each closed cycle records:**
- its gross PnL, fees, funding and net PnL;
- each with its currency (the settlement asset) and its source;
- a status: `provisional` until the venue's records are final, then `final`.

**In messages and the CLI:**
- "Realized PnL" means the **net**: gross − fees + funding received. Funding
  paid counts as negative.
- The return on a position is net ÷ entry notional, both in the settlement
  asset.
- The breakdown is shown beside them.
- The PRD's figures (+69.07, +6.73 %) are gross examples.

**Sources:**
- **Binance:** `userTrades` and income records, paged by time window.
- **Bybit:** executions, closed PnL (already net of fees and funding), and the
  transaction log, whose signs the adapter normalizes.

**Rules:**
- A summary is never added to its own components.
- Totals, such as the status's unrealized PnL and balances, are kept per
  settlement asset. USDT, BTC and ETH are never summed: a combined figure needs
  a stated conversion source and time.
- `ClosedTrade.gross_quote_pnl` is not reused.

### 13. Parity

**Equal across venues and stores:**
- the documented operations;
- the normalized states;
- errors and recovery;
- the durable outcomes.

**Different, and kept visible:**
- instruments, units, rules, prices, fees and funding;
- mark and liquidation semantics;
- which operations are atomic.

**Across stores,** the same frozen inputs give identical results on SQLite and
PostgreSQL.

**Not an oracle:** market PnL across venues.

**Spot** gains no SHORT, leverage or liquidation. Spot and Python acceptance
stay separate from the futures capability.

**Capability flags:** a required operation whose capability flag is `false`
fails its cell. A futures capability describes an implementation; it cannot
excuse a missing one.

**Evidence** comes from each venue's demo environment, with Bybit's testnet for
whatever its demo lacks.

### 14. Shared model and wire; the FFI delta

These are frozen before the Python bindings (ALG-377 to ALG-379) and ALG-534.

**Protocol v1** may still change until 0.3.0 ships:
- Every market gains a required `category`, beside `exchange`, which stays the
  registry name.
- This applies to `DataSelection`, `SegmentEntry`, `Subscribe`, `BarClosed` and
  `GapNotice`.
- ALG-734 makes the change, with its doctests.

**Storage:** product lines use their storage names in the existing `exchange`
columns, so candles and segments need no new key columns.

**The Python FFI delta:**
- **What changes:**
  - the market identity gains `category`, always `spot` for candle strategies;
  - exchange identity gains the new product lines;
  - venue IDs become `str`.
- **What does not cross:** nothing futures-specific crosses the candle-strategy
  boundary, and there is no Python webhook API.

**Unchanged:** `Strategy`, the semantics of `StrategyContext`, `Candle`,
`TargetPosition`, and the bundle manifest.
- ALG-383's goldens are never regenerated for these changes.
- ALG-534 is the last change to the candle-strategy FFI.

### 15. Storage, migrations, secrets and ingress

**Execution store** (ALG-738):
- it holds the inbox, intents, reservations, positions, protection, incidents
  and outbox, on both backends;
- no transaction spans a network request.

**No more delete-and-resync.** From ALG-734 on, a SQLite database is migrated,
never deleted:
- one chain runs to the first v0.3 schema from every schema a build has
  written:
  - 1, in QuantForge 0.1.0;
  - 2, in 0.2.0;
  - 3 and 4, in development builds;
- the policy note at `src/storage/sqlite/mod.rs:22–27` changes there;
- PostgreSQL starts at the v0.3 schema.

**Decimals** stay exact TEXT, scale included.

**Credentials and the Telegram token:**
- They live only in the trusted owner process, as per-profile references
  (ALG-750, ALG-754).
- They never appear in payloads, the database, logs, notifications or workers.

**Webhooks:**
- They arrive over HTTPS on port 443, over IPv4.
- The TLS edge verifies TradingView's client certificate: its chain to a
  trusted certificate authority, and its subject, CN
  `webhook-server@tradingview.com`.
- The route also carries a token, which is redacted wherever it is shown.
- An allowlist of TradingView's four source addresses is optional.
- Admission answers within TradingView's 3-second limit.

### When an unverified item resolves

[futures-venues.md](../futures-venues.md#unverified-items) lists what the
research could not confirm. Each item has a rule for whichever way it resolves.
The rules below are the ones that change a decision; the rest, where the
outcome changes nothing, are in that table.

| Item | If it holds | If it does not |
| --- | --- | --- |
| Binance COIN-M algo orders carry the whole lifecycle on demo | ALG-763 uses them | The Binance inverse cells block the release until Marko decides |
| Binance accepts a second `closePosition` stop in one direction | §7's bridge may be a `closePosition` stop | The bridge is a reduce-only stop for the current quantity |
| Binance's private user stream works on demo | Demo provides the Binance monitoring evidence | The Binance monitoring cells block the release until a test route is found |
| A venue reports the account ID for the keys in use | That is the account ID | Outside mainnet a key fingerprint stands in; mainnet arming is refused |
| Bybit reports a position stop's trigger basis | The read-back checks it | ALG-741 uses a stop whose basis can be read; nothing counts as `protected` unverified |
| Bybit attaches a stop to a market entry on its test environments | Attached at entry | Placed after the first fill |
| A venue tells who cancelled a stop | §10's rule for user cancels applies | §8's repair, plus ALG-746's command to leave a position unprotected on purpose |
| Bybit's demo trades inverse contracts through the API | Demo provides the inverse evidence | Testnet does |

## Evidence

**The PRD:** all 13 pages, mapped requirement by requirement in
[automation-requirements.md](../automation-requirements.md).

**The venues:** their documentation and changelogs, and their public endpoints,
recorded with sources in [futures-venues.md](../futures-venues.md). Nothing was
checked with signed requests. Those need futures keys for each venue's test
environment, which the adapter stories obtain.

**The universe:** [`universe.py`](../../spikes/futures-contract/universe.py)
applied §3's rule to every source on 2026-10-22 (UTC):

| Venue | Family | Mainnet | Test environment |
| --- | --- | --- | --- |
| Binance | linear | 524 of 931 | demo: 528 of 742 |
| Binance | inverse | 20 of 30 | demo: 20 of 51 |
| Bybit | linear | 799 of 907 | testnet: 787 of 896; demo: 799 of 907 |
| Bybit | inverse | 22 of 26 | testnet: 21 of 25; demo: 22 of 26 |

**Sizing:** [`sizing.py`](../../spikes/futures-contract/sizing.py) computes every
case below with `Decimal` at 60 digits.
- It checks each result against the invariants:
  - on the tick and the step;
  - within budget;
  - one more step would exceed the budget;
  - the stop on the protective side.
- It computes every number again in exact rational arithmetic, with code that
  shares nothing with the first computation.
- It checks the stops, the quantities and the linear amounts against values
  computed by hand.
- Five planted defects, among them a one-in-ten-million error in the inverse
  formula, each made it fail.
- It writes
  [`sizing-cases.json`](../../spikes/futures-contract/sizing-cases.json) for
  ALG-736.

| Case | Instrument | Input | Result |
| --- | --- | --- | --- |
| The PRD's p.9 trade | Binance SOLUSDT, LONG | 1 % of 15,000 USDT; E 234.23; stop 200.00 | 4.38 SOL; risk 149.9274 USDT; value 1,025.9274; margin 205.18548 at 5×. Every p.9 figure but its 15.00 risk line |
| The same on Bybit | Bybit SOLUSDT, LONG | as above | 4.3 SOL, on the 0.1 step; risk 147.189 USDT |
| The p.9 risk line read literally | Binance SOLUSDT, LONG | 1 % of 1,500 USDT; as above | 0.43 SOL (0.43821 before the step); risk 14.7189 USDT |
| A percentage stop | Binance BTCUSDT, SHORT | 1 % of 10,000 USDT; E 84,123.45; 3 % | stop 86,647.20; 0.039 BTC; risk 98.42625 USDT |
| 100 USD contracts | Binance BTCUSD_PERP, LONG | 1 % of 0.5 BTC; E 84,123.4; 3 % | stop 81,599.6; 135 contracts; risk 0.004963452356 BTC; a warning that the wallet holds less than the capital |
| 1 USD contracts | Bybit BTCUSD, LONG | as above | stop 81,599.60; 13,599 contracts; risk 0.004999851007 BTC |
| 10 USD contracts | Binance ETHUSD_PERP, SHORT | 0.5 % of 2 ETH; E 3,012.34; stop 3,105.555 | stop 3,105.56; 100 contracts; risk 0.009964721966 ETH |
| An inverse SHORT | Bybit SOLUSD, SHORT | 1 % of 20 SOL; E 187.45; 2 % | stop 191.20; 1,911 contracts; risk 0.199948717147 SOL |
| Drift at submission | Binance SOLUSDT, LONG, 0.43 SOL | ask 243.00, then 244.00 | risk 18.49, within 1.25 × 15 = 18.75: accepted; risk 18.92: refused |
| Refusals | | | a stop on the wrong side; no stop; below the minimum quantity; below the minimum notional; above the market-order maximum; margin short once a reservation is counted; a settlement wallet below R, in USDT and in the coin; capital in the wrong currency; a stop the book or the mark price has already crossed |
| The PRD's p.10 closes | 4.38 SOL from 234.23 | exits at 250.00 and at 200.00 | gross +69.0726 (+6.73 %) and −149.9274 (−14.61 %), matching the PRD's figures |

**A design review** checked this record's plan against the PRD, the venue facts
and the downstream briefs. It found four problems, and the rules above answer
each:
- two profiles could double the exposure on one position;
- an exit could close a newer cycle;
- disarming could strand positions;
- the stop replacements planned were ones neither venue allows.

**An independent review of this record** then found eight more, which are
answered too:
- entries could stack while an earlier one was unresolved;
- the protection state had no "unknown";
- an operator's stop move on Binance was read as a removal;
- the stop check used the book but not the mark price;
- an unprotected position stopped being repaired;
- totals could mix currencies;
- the schema chain left out schema 1;
- copies of a store could not tell their orders apart.

## Consequences

- **One executor** serves both venues, both families and both stores. A new
  venue adds adapters, not a workflow.
- **Candle strategies are untouched,** along with their FFI and their goldens.
  The cost is two inputs that share no state machine.
- **Positions survive QuantForge:**
  - exchange-held stops;
  - a service stop that leaves them in place;
  - recovery from the ledger.
- **Some PRD wishes become explicit refusals** instead of guesses:
  - stale or drifting entries;
  - hedge-mode and portfolio-margin accounts;
  - dated futures;
  - an asset or capital that is not configured.
- **Test and live trading stay in separate stores,** now enforced rather than
  advised.
- **The pre-1.0 delete-and-resync policy ends** for durable records. Every later
  schema change ships a migration.
- **Wire, storage and Python names change once,** before 0.3.0 ships, and are
  then fixed.

## Follow-ups

**Per story:**
- **ALG-733:**
  - the identity types of §2;
  - the ports of §1, with their capability split;
  - the states;
  - the PnL fields of §12.
- **ALG-734:**
  - the store's environment binding;
  - the product-line spellings;
  - text venue IDs;
  - the protocol's `category`;
  - the migration chain of §15;
  - the activation contract.
- **ALG-735 and ALG-736:** capital per settlement asset, defaults and policy
  snapshots; sizing to §6, starting from `sizing-cases.json`.
- **ALG-737:** the payload contract of §9.
- **ALG-738:** the execution store, reservations per account and margin pool,
  and leases per `PositionKey`.
- **ALG-739 to ALG-741 and ALG-759 to ALG-765:**
  - the adapters;
  - their account checks (§4);
  - the protection mechanisms (§7);
  - the answers to the unverified items.
- **ALG-743:** admission under §9 and §15.
- **ALG-744 and ALG-764:** monitoring under §10 and §11.
- **ALG-745 to ALG-748:** entry, protection, exit and recovery under §5 to §10.
- **ALG-749 to ALG-755:** notifications, the CLI, the lifecycle and arming,
  under §5 and §12.
- **ALG-756:** freeze §3's rule and its dated list, and build the matrix of §13.
- **ALG-761:** futures market data keyed by `DataSeriesKey`.
- **ALG-767:** routing under §2 and §5.

**Brief amendments:**
- **ALG-733 and ALG-761** take §2's `DataSeriesKey`, which has no settlement
  asset.
- **ALG-756 and ALG-761** drop dated futures from the universe (§3). For
  perpetuals, expiry means only a scheduled delisting.
- **ALG-734** takes:
  - the store environment and the activation epoch;
  - the spellings and the text IDs;
  - the schema chain from 1.
- **ALG-736** takes Bybit's face value from the venue's documented rule, since
  no metadata field carries it.
- **ALG-737** takes §9's payload rules.
- **ALG-741 and ALG-746** use §1's `protection_unknown` as their unknown
  protection state.
- **ALG-745** serializes per `PositionKey`.
- **ALG-763** follows §7: the algo API for both families, and the bridge for a
  replacement.
- **ALG-767** keys leases by `PositionKey`, recording the profile in each.
