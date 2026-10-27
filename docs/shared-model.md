# The shared model

ADR 0002 §14 froze the model, wire and Python decisions that v0.3's
automation needs, before the Python bindings (ALG-377 to ALG-379) and
ALG-534. This page tracks each change:
- what it is;
- where it stands;
- who consumes it.

ALG-733 built the Rust side of the identity and the futures contract, and
QUA-37 (ALG-734 in the previous workspace) the storage, the wire's
`category` and the migrations. The rest lands with the stories named below.

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

## Changing next

| Change | Story | Consumers |
| --- | --- | --- |
| Candle strategies refuse a linear or inverse series | QUA-37's successors: ALG-532, ALG-379, ALG-509 | Backtests, `trade run`, Python, remote jobs |
| `ExecutionStore`, which persists the records above | ALG-738 | ALG-743 to ALG-748 |
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
- **Sizing.** ALG-736 refuses a stop on the wrong side, and converts USD margin pools at the venue's index price.
- **Registration.** ALG-759 registers Binance's futures lines and decides how `--exchange` selects a category. ALG-693 registers Bybit's lines.
- **Exit attempts.** Once an exit has retried, its latest attempt decides whether it ended unfilled or rejected; an earlier attempt's order or refusal that executed nothing says nothing, and late evidence that an earlier attempt filled is reported as an invalid transition. ALG-745, which owns the retry loop, records per-attempt outcomes if it needs more than that.
