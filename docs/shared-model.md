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
reaches it, QUA-464 (ALG-378) the candle strategy itself, and QUA-470
(ALG-379) the function that runs one, with the backtest's refusal of a
series that is not Spot, QUA-475 (ALG-380) a batch of candles as NumPy
arrays, with the market a batch names, QUA-473 (ALG-381) candles and a
backtest's result as pandas DataFrames, and QUA-472 (ALG-382) the reader of
the candles a store holds. The rest lands with the stories named below.

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
| `Category`, and `MarketId::category()` | `Category`, and `MarketId.category`, which is its product line's. A market of any line can be named, as the key its candles are stored under; `run_backtest` is what refuses one that is not Spot ([below](#bound-to-python-by-qua-470)) |
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

## Bound to Python by QUA-470

The candle strategy's backtest, as the Python package runs it
([python-types.md](python-types.md#running-a-backtest)):

| What | In Python |
| --- | --- |
| `BacktestEngine::run`, with the market, the candles and the strategy of a run, under a `BacktestConfig` | `run_backtest(candles, strategy, config, *, market=None)`: the engine's own run, on the thread that called. `candles` is a `CandleBatch` or anything that iterates `Candle`s |
| The market of a run, a `MarketId` | `market`, given by name alone and never assumed: it names its product line, and so its family. Since QUA-475 a batch that names its market stands in for it ([below](#bound-to-python-by-qua-475)) |
| `BacktestResult`, with its `ClosedTrade`s and `EquityPoint`s | The result of a run, with the engine's fields under the engine's names, each meaning what it means in Rust |
| `EngineError` | A `ValueError` in the engine's words for what it refuses before the strategy is called, a market that is not Spot or no candles; an `OverflowError` for an amount beyond its decimal; and, for a `StrategyError`, the exception the callback raised |

A linear or an inverse series is refused by the engine itself, so no host
of a backtest runs one by Spot's rules. For it, the crate gained:

| Change | Where |
| --- | --- |
| `BacktestEngine::run` refuses a market that is not Spot, before it looks at the candles or the config and before the strategy is called | `quantforge::EngineError::NotSpot` |

Nothing else of the candle-strategy contract changed, and no futures type
crossed: the release adds no futures backtest, and execution on futures
stays the engine's own, on external signals.

`tests/fixtures/strategy_cases.json` gained a case a rule decides, where
every other lists its decisions: the crate's tests run the rule written in
Rust over four hours of generated candles, and the package's tests hold
the same rule written in Python to it through `run_backtest`, decision for
decision and digit for digit.

## Bound to Python by QUA-475

A batch of candles as NumPy arrays, as the Python package has it
([python-numpy.md](python-numpy.md)):

| What | In Python |
| --- | --- |
| `CandleBatch`'s columns, each one array | `CandleBatch.to_numpy()`, a dict of eight arrays of one dimension, and `CandleBatch.from_numpy(...)`, which takes eight back. A time is int64 and a trade count uint64, both exact, and a count the exchange did not report is a masked entry, never a number |
| `decimal_to_f64` and `decimal_from_f64`, the decimal ↔ float64 policy | Applied to a batch's prices and volumes as a whole, and nowhere else: a float64 array is the float nearest to each decimal, and a float is read as the decimal of its shortest text, or refused by its row |
| The data-series key a batch's candles are of, a `MarketId` | `CandleBatch.market`, given by `market=` where a batch is built and `None` where it is not. Two batches of the same candles are equal only where they name the same market. The arrays hold no market: whoever builds a batch from arrays names what they are of |
| The market of a run | `run_backtest` runs a batch on the market it names, so `market=` may be left out beside one, and refuses a market other than that one: a linear series is never run as its Spot namesake. Candles that name no market still need `market=` |

`CandleBatch` did not change in Rust: it holds columns and no market. The
market a batch names is the Python class's own, kept beside the engine's
batch, and the policy is the one it [was](#not-changing).

One thing of the engine's was corrected for it:

| Change | Where |
| --- | --- |
| A float64 that lies exactly halfway between two shortest decimal strings becomes the one whose last digit is even, as Python's `repr` writes it, which the policy names as what the conversion is. It became the one of larger magnitude, which is what Rust's `Display` wrote there | `quantforge::decimal_from_f64` |

`tests/fixtures/decimal_float_cases.csv` gained nine floats that lie
halfway, with the decimals Python gives them: eight that become the one of
the even digit, and 2^-24, of whose two candidates only the odd one reads
back as it, so that it keeps the odd one. The package's tests hold a whole
column to the same cases, and to Python's own `float(str(d))` and
`Decimal(repr(x))` over thousands of values. They take the fixtures there
were to NumPy and back as well: every product line of
`storage_identity_cases.json` kept with its batch and apart from its
namesakes, and the goldens' frozen day, which comes back the same batch by
value and backtests to the frozen result by value.

## Bound to Python by QUA-473

Candles and a backtest's result as pandas DataFrames, as the Python package
has them ([python-pandas.md](python-pandas.md)):

| What | In Python |
| --- | --- |
| `CandleBatch`'s columns, as a table | `candles_to_dataframe(batch)`, a frame with one row for each candle, indexed by the time each bar opened, and `dataframe_to_candles(frame, market=...)`, which builds a batch from one. A frame is made of the arrays `to_numpy` gives and read back through `from_numpy`, so the same contract holds of it cell for cell |
| A time, int64 milliseconds since the Unix epoch, UTC | A `datetime64[ms, UTC]`: the same integer, shown as a UTC time, on every pandas. Read back from any time zone and any of pandas' units, exactly, and refused where it has no zone, is missing, is no whole millisecond or, held in seconds, has milliseconds that do not fit 64 bits |
| A trade count the exchange did not report | A missing value, `<NA>`, of a `UInt64` column, never NaN, 0 or -1 |
| The data-series key a batch's candles are of, a `MarketId` | Stays with the batch: a frame holds no market, in a column or in its `attrs`, and whoever builds a batch from a frame names what it is of, by `market=` |
| `BacktestResult`, with its `ClosedTrade`s and `EquityPoint`s | `trades_to_dataframe(result)`, every field of a trade as a column under the field's own name, and `equity_to_dataframe(result)`, indexed as a frame of candles is. Each amount is the float nearest to its decimal, by `decimal_to_f64` |
| `ClosedTrade.gross_quote_pnl`, Spot's PnL field | The column `gross_quote_pnl`, under that name and with the meanings it [keeps](#not-changing). No column is a net PnL, a fee or a futures PnL, and there is no frame of futures execution |

The crate did not change for it: the four functions are written in Python,
over the bound types, and what they are built on is in the extension
module alone. Identity reaches a frame nowhere: a frame of
`binance_spot` candles and one of the same candles of `binance_linear` are
equal, and the batches built from them are of the market each is told.

The package's tests take the Spot fixtures there were through frames, and
change none: the goldens' frozen day comes back the same batch by value and
backtests to the frozen result by value, the frames of that result hold
Python's own reading of the frozen file's text, and the candles and the
results of `tests/fixtures/strategy_cases.json` do the same. A frame is for
computing with, and is never what is compared for parity.

## Bound to Python by QUA-472

The candles a store holds, as the Python package reads them
([python-reader.md](python-reader.md)):

| What | In Python |
| --- | --- |
| The data-series key candles are stored under, a `MarketId`: product line, symbol and interval, with no settlement asset | What `CandleReader.load` takes, and nothing that stands for one. A product line names its venue and its family, so the Spot, linear and inverse candles of one symbol, and one symbol's candles on two venues, are each read alone, on either backend |
| The Spot line's storage name, such as `binance_spot`, which a store has kept Spot candles under since before there were other families | The same key in Python: a store an earlier release wrote is read under it. A venue's name alone still names no market, and no family is assumed for one |
| How a host selects a backend and opens it: `StorageSettings::resolve` and `StoreFactory` | `CandleReader(...)`: the command line's flags as arguments, its variables where an argument is left out, and SQLite at `data/market.sqlite` where nothing is set, resolved by the same code |
| `CandleBatch`, with the market it names | What `load` gives: every decimal with the digits the store holds, oldest first, and the market that was asked for |
| The decimal ↔ float64 policy | Unchanged: a store keeps a decimal as text, a reader reads the text, and nothing is a float until a batch is asked for arrays or a frame |

For it, the crate gained:

| Change | Where |
| --- | --- |
| A store opened to read its candles and nothing else: no file or schema created, no schema migrated, every connection read-only on both backends, and an unfinished copy refused; with the sentence that says what a store holds where a range held none | `quantforge::CandleReader`, `StoreFactory::open_reader`, `StoreOpenError::NoSuchFile` |
| A reader as an owner of a store, with a command's pool of two connections (ADR 0001) | `quantforge::StoreOwner::Reader` |
| Each backend's store opened to read | `SqliteStore::read_only`, `PostgresStore::connect_read_only` |

Nothing of the model changed for it, and no futures type crossed. A reader
reads the candles of any product line a store holds; a candle strategy
still runs on Spot alone, which `run_backtest`
[refuses otherwise](#bound-to-python-by-qua-470). A server's job worker is
handed its candles by the server and opens no store, so no reader, URL or
credential reaches a strategy that runs remotely.

The crate's tests hold the reader on both backends
(`tests/storage/reader.rs`), and the package's tests read stores the
`quantforge` command line itself wrote, a SQLite file and a PostgreSQL
database, from an exchange that answers on the test's own machine.

## Changing next

| Change | Story | Consumers |
| --- | --- | --- |
| `trade run` refuses a linear or inverse series, and a remote job on one is refused when it is submitted. A submission for one is still taken today; the engine's refusal waits for it in the job's worker, once workers run backtests | QUA-37's successors: ALG-532, ALG-509 | `trade run`, remote jobs |
| `SignalInbox`, the server's only view of the executor | ALG-743 | The webhook route |
| `Notifier`, which delivers from the outbox | ALG-749 | ALG-751 |

## Not changing

- **The candle-strategy contract:**
  - `Strategy` and the semantics of `StrategyContext`;
  - `Candle` and `TargetPosition`;
  - `CandleBatch` and the decimal ↔ float64 policy.
- **The bundle manifest.**
- **The ALG-383 goldens** ([golden-parity.md](golden-parity.md)). They are
  never regenerated for these changes.
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
