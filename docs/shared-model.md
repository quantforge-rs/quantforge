# The shared model

ADR 0002 §14 froze the model, wire and Python decisions that v0.3's
automation needs, before the Python bindings (ALG-377 to ALG-379) and
ALG-534. This page tracks each change:
- what it is;
- where it stands;
- who consumes it.

ALG-733 built the Rust side of the identity and the futures contract,
QUA-37 (ALG-734 in the previous workspace) the storage, the wire's
`category` and the migrations, QUA-36 (ALG-735 in the parity matrix) a
profile's policy and its store, QUA-35 (ALG-736) the sizing of an entry
from it, and QUA-207 (ALG-737) the payload of an alert and what its signal
resolves to. QUA-469 (ALG-377) bound to Python what the tables below say
reaches it, and QUA-464 (ALG-378) the candle strategy itself. The rest
lands with the stories named below.

## Changed in Rust by ALG-733

| Change | Where | Reaches the Python FFI |
| --- | --- | --- |
| `Venue` (`binance`, `bybit`) and `Category` (`spot`, `linear`, `inverse`) | `quantforge::{Venue, Category}` | `Category` only, and only as `spot` for candle strategies |
| `ExchangeId` gains the product lines `BinanceLinear`, `BinanceInverse`, `BybitSpot`, `BybitLinear` and `BybitInverse`, spelled as in ADR 0002 §2, and becomes `#[non_exhaustive]` | `quantforge::ExchangeId` | Yes: it is the exchange a market names |
| `MarketId::category()`. A market is ADR 0002's data-series key: product line, symbol and interval, with no settlement asset | `quantforge::MarketId` | Yes |
| The registry selects a venue's Spot line by the venue's name, and `connect` refuses a line without an adapter | `quantforge::exchange` | No |
| Execution identity:<ul><li>`Environment`, `InstrumentId`, `AccountRef` and `VenueAccountId`, an `IssuedAccountId` or a `KeyFingerprint`;</li><li>venue IDs as text: `VenueOrderId`, `VenueAlgoId` and `VenueTradeId`;</li><li>`OperationId`.</li></ul> | `quantforge::model` | No |
| Units:<ul><li>`Price`, `BaseQty`, `Contracts`, `Quantity` and `FaceValue`;</li><li>`Leverage` and `PositionSide`;</li><li>`Asset` and `Money`.</li></ul>Also the shared `Refusal` | `quantforge::model` | No |
| The futures contract:<ul><li>instruments and universe rule 1;</li><li>accounts and positions;</li><li>orders, stops and their coverage;</li><li>the ledger and private events;</li><li>intents and position cycles, with their state machines;</li><li>`RealizedPnl`.</li></ul> | `quantforge::model::futures` | No |
| The futures connector ports, split by capability, with `FuturesCapabilities` and `FuturesConnectors`; and the injected `Clock` | `quantforge::ports::futures`, `quantforge::ports::Clock` | No |

## Changed by QUA-37

| Change | Where | Reaches the Python FFI |
| --- | --- | --- |
| Spot's venue IDs are text, with their columns:<ul><li>`ExchangeOrder.order_id` and `Fill.trade_id`;</li><li>`AccountTrade.trade_id` and `AccountTrade.order_id`;</li><li>`ClosedTrade.entry_order_id` and `ClosedTrade.exit_order_id`;</li><li>`PositionState.entry_order_id`;</li><li>the `order_id` of a cancel or query request.</li></ul>Schema 5 rebuilt the journal columns and rewrote the JSON fields, keeping the originals | `quantforge::{VenueOrderId, VenueTradeId}` | Yes: ALG-377 binds them as `str` |
| The protocol's markets carry a required `category` beside `exchange`, which stays the venue's name: `DataSelection`, `SegmentEntry`, `Subscribe`, `BarClosed` and `GapNotice` | `quantforge::protocol` | No; remote jobs, the bar stream and provenance name markets with it |
| Product lines in storage: the `exchange` column holds their storage names, candle and segment keys stay as they are, each store is bound to one environment (`store bind`), and a SQLite database is migrated from schema 1 on, never deleted; [storage-schema.md](storage-schema.md) is the contract for both backends | `quantforge::{StoreControl, StoreIdentity, RunIdentity}` | No |
| Segment identity under the new key, with old segments and hashes still readable ([segment-spec.md](segment-spec.md#identity)) | `docs/segment-spec.md` | No |

## Changed by QUA-36

| Change | Where | Reaches the Python FFI |
| --- | --- | --- |
| An automation profile's policy, policy format 1 ([automation-policy.md](automation-policy.md)):<ul><li>`ProfilePolicy`, with its defaults, per-asset settings and capital, and `PolicyRevision`;</li><li>`PolicyChange`, each change the next revision;</li><li>`EffectivePolicy`, `PolicySnapshot` and `EntryStop`;</li><li>`PolicyProblem`, a policy held against a venue's instrument list;</li><li>the units `RiskPercent` and `StopPercent`.</li></ul> | `quantforge::model::futures` | No |
| An intent is admitted with `ExternalIntent::admit`: an entry under a snapshot of its own profile's policy, with a stop of its own or the snapshot's fallback, which its stored form keeps as `policy`, and an exit under none. `move_to(IntentState::Admitted)` is refused | `quantforge::model::futures` | No |
| `PolicyStore`, the storage port for profiles and every revision of their policies, on both backends from schema 9 ([storage-schema.md](storage-schema.md#keeping-policies)) | `quantforge::PolicyStore` | No |
| `PolicyService`, the first part of `automation`: what a command or an executor keeps a policy through | `quantforge::automation` | No |

## Changed by QUA-35

| Change | Where | Reaches the Python FFI |
| --- | --- | --- |
| Risk sizing ([risk-sizing.md](risk-sizing.md)), a pure function of what it is handed:<ul><li>`SizingRequest`, everything one sizing reads, with `Freshness`, `Observed` and `UsdIndex`;</li><li>its answers `Sizing`, `SizedEntry` and `SizingRefusal`, with the figures reached;</li><li>`SizingFault`, an input sizing cannot trust, kept apart from the refusals;</li><li>`SubmissionCheck`, a sized entry held to fresh prices, and `FillRisk`, the risk its fills took beside the risk sized;</li><li>`margin_pool`, the pool of an account an entry draws on.</li></ul> | `quantforge::model::futures` | No |

## Changed by QUA-207

| Change | Where | Reaches the Python FFI |
| --- | --- | --- |
| An external signal before it is an intent ([signal-payload.md](signal-payload.md)), each rule a pure function of what it is handed:<ul><li>`ExternalSignal` and `SignalAction`, one alert as it was delivered, with its `SignalHash`;</li><li>`SignalAgeLimit` and `SIGNAL_FUTURE_ALLOWANCE_MS`, how old an entry may be;</li><li>`ProfilePolicy::instrument_named`, the instrument a ticker names, by one of two `TickerSpelling`s;</li><li>`ExternalSignal::resolve`, the `ResolvedSignal` a store records or the `SignalRefusal` it is recorded with;</li><li>`Delivery`, a delivery sent again told from a conflicting one;</li><li>`ExitIntent::closes` and `EntryIntent::against`, a signal against the positions its profile has.</li></ul> | `quantforge::model::futures` | No |
| The alert's wire form: `Signal`, strict, with its ID, its canonical payload and its hash; `SignalProblems`; and `SignalReceipt` | `quantforge::protocol::signal` | No |

## Changed by QUA-206

| Change | Where | Reaches the Python FFI |
| --- | --- | --- |
| The execution store's records ([execution-store.md](execution-store.md)), each one versioned document with every decimal as exact text:<ul><li>`ExecutionScope`, whose record it is;</li><li>`SignalRecord` and `SignalDelivery`, the inbox;</li><li>`OperationRecord`, numbered by the store, with `Replacement` for a Spot amend or cancel-replace;</li><li>`IntentRecord`, `OrderRecord`, `FillRecord`, `CycleRecord` and `Reservation`, the ledger;</li><li>`Incident` and `OutboxEvent`;</li><li>`Lease` and `Fence`, who may act on a position.</li></ul> | `quantforge::model::execution` | No |
| `ExecutionUnit`, everything one write changes, and `Commit`, what it came to: applied, a duplicate, a conflict, stale or fenced | `quantforge::model::execution` | No |
| `ExecutionStore`, the storage port for all of it, on both backends from schema 10 ([storage-schema.md](storage-schema.md#keeping-the-execution-store)) | `quantforge::ExecutionStore` | No |
| `StoreControl::activate` and `deactivate`, with the log of both and `StoreIdentity::authority`: a store executes only while it is active, and copying a store never activates the copy | `quantforge::StoreControl` | No |
| `ExternalSignal`, `SignalAction` and `SignalRefusal` are stored as part of a signal's record, and `SignalHash::of` hashes a canonical payload | `quantforge::model::futures` | No |
| `ExternalIntent::on_fill_of` takes a fill through the order it belongs to, by the venue's order ID, since a trade record carries a client ID only on some venues | `quantforge::model::futures` | No |
| `ExecutionLedger`: a delivery admitted, an order's attempt recorded before it is sent, and an unknown outcome asked of the venue before anything is sent again | `quantforge::automation` | No |

## Bound to Python by QUA-469

What the tables above mark as reaching the Python FFI, as the Python
package has it ([python-types.md](python-types.md)):

| What | In Python |
| --- | --- |
| `ExchangeId` with every product line | `ExchangeId`, read from a line's storage name such as `bybit_linear`. A venue's name alone is refused: nothing takes it for the venue's Spot line |
| `Category`, and `MarketId::category()` | `Category`, and `MarketId.category`, which is its product line's. A market of any line can be named, as the key its candles are stored under; the backtest entry point is what refuses one that is not Spot (QUA-470) |
| Spot's venue order IDs as text | `ClosedTrade.entry_order_id` and `ClosedTrade.exit_order_id`: a `str`, or `None` where no order reached a venue |

`Venue` is not bound: a product line's name begins with its venue's, and
that is all of a venue Python holds. Every type marked `No` above stays in
Rust, and no futures execution type is public in Python.

For the binding, the crate gained:

| Change | Where |
| --- | --- |
| `Category` and `ExchangeId` are read from their names with `str::parse`, and text that is neither is refused with the names there are | `quantforge::{Category, ExchangeId}`, `ModelError::{InvalidCategory, InvalidExchange}` |
| `Interval::ALL` and `ExchangeId::all()` list every interval and every product line | `quantforge::{Interval, ExchangeId}` |
| `BacktestConfig::validate`: what a run requires of a config, before any run | `quantforge::BacktestConfig` |

## Bound to Python by QUA-464

The candle-strategy contract, which [did not change](#not-changing), as
the Python package has it ([python-types.md](python-types.md#strategies)):

| What | In Python |
| --- | --- |
| `Strategy`, with its three callbacks in the order an engine calls them | `Strategy`, a class to derive from that defines `on_bar`; `on_start` and `on_finish` do nothing unless a class defines them |
| `StrategyContext`, with the meaning each engine gives its fields | `StrategyContext`: `market`, `now_ms`, `cash` and `position_qty`, a value built anew for every callback |
| `TargetPosition`, and no decision | `TargetPosition.FLAT` and `TargetPosition.LONG_ALL_IN`, and `None` |
| `StrategyError`, a message | The exception a callback raised, as it was raised, where Python hosts the run; the engine's message is the same as text, with the traceback |

The crate did not change for it. The engine drives a class derived from
`Strategy` through one adapter, its `Strategy` trait over a Python object
([python-package.md](python-package.md#the-strategy-adapter)), and nothing
but those three callbacks crosses: no webhook, no order, no credential and
no store.

`tests/fixtures/strategy_cases.json` holds a strategy on either side of
the boundary to the same calls. The crate's tests write it from the
engine's runs of a strategy written in Rust (`tests/strategy_cases.rs`),
and the package's tests hold one written in Python to it, call for call
and digit for digit.

## Changing next

| Change | Story | Consumers |
| --- | --- | --- |
| Candle strategies refuse a linear or inverse series | QUA-37's successors: ALG-532, ALG-379, ALG-509 | Backtests, `trade run`, Python, remote jobs |
| `SignalInbox`, the server's only view of the executor | ALG-743 | The webhook route |
| `Notifier`, which delivers from the outbox | ALG-749 | ALG-751 |

## Not changing

- **The candle-strategy contract:**
  - `Strategy` and the semantics of `StrategyContext`;
  - `Candle` and `TargetPosition`;
  - `CandleBatch` and the decimal ↔ float64 policy.
- **The bundle manifest.**
- **The ALG-383 goldens.** They are never regenerated for these changes.
- **Every Spot fixture:**
  - the conformance fixtures;
  - `tests/fixtures/`;
  - the `--help` snapshots.
- **`ClosedTrade.gross_quote_pnl`, Spot's PnL field.** It keeps its two meanings: net of fees in a backtest, and gross in live trading. Futures PnL is `RealizedPnl`.

Nothing futures-specific crosses the candle-strategy FFI, and there is no
Python webhook API (ADR 0002 §14). ALG-534 is the last change to the
candle-strategy FFI.

## Brief amendments

Decided on 2026-10-22, when ALG-733 was planned:
- **The store ports.** ADR 0002 §1 lists `ExecutionStore`, `SignalInbox` and `Notifier` among ALG-733's ports. Each is designed by the story that implements it, with its inputs in hand: ALG-738, ALG-743 and ALG-749.
- **The product lines.** ALG-733 added them to `ExchangeId`, which ADR 0002's follow-ups gave to ALG-734 (now QUA-37). QUA-37 kept storage, the protocol, the environment binding and the migrations.
- **Stop ownership.** Deciding which stop covering a position is QuantForge's belongs to ALG-746: the ledger, the owner tag, the epoch, and a second writer. `CoveredStop` checks only the coverage.
- **A profile's ports.** ALG-767 hands each profile the ports it may use: no entry port and no leverage port while the profile is not armed.
- **Sizing.** ALG-736 refuses a stop on the wrong side, and converts USD margin pools at the venue's index price. It is handed that price with its source and time, and an adapter is where it is read.
- **Registration.** ALG-759 registers Binance's futures lines and decides how `--exchange` selects a category. ALG-693 registers Bybit's lines.
- **Exit attempts.** Once an exit has retried, its latest attempt decides whether it ended unfilled or rejected; an earlier attempt's order or refusal that executed nothing says nothing, and late evidence that an earlier attempt filled is reported as an invalid transition. ALG-745, which owns the retry loop, records per-attempt outcomes if it needs more than that.
