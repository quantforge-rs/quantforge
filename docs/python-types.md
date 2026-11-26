# The Python types

`quantforge_py` holds the engine's types as Python classes: what a market
is, a candle and a batch of candles, a backtest's settings and result, and
the strategy a class implements. Each class holds the engine's own value.
It is built by the engine's own rule and refused in the engine's own
words, so that a value Python built is a value the engine takes. And it
has the function that runs a backtest, `run_backtest`: the engine's own
run of a strategy written in Python.

This page is what each type takes and gives, and how a backtest is run.
A batch of candles as NumPy arrays has a page of its own,
[candles as NumPy arrays](python-numpy.md), and so have candles and a
backtest's result as pandas DataFrames,
[candles and results as pandas DataFrames](python-pandas.md), the reader
of the candles a store holds, [the candle reader](python-reader.md), the
client of a QuantForge server, [the Python client](python-client.md), and
a backtest submitted to one,
[submitting a backtest from Python](python-bundles.md). How the package is named, built and
tested is [the Python package](python-package.md).

```python
>>> from decimal import Decimal
>>> import quantforge_py as qf
```

`qf` and `Decimal` are what the examples use.

## Contents

- [What is bound](#what-is-bound)
- [How a value crosses](#how-a-value-crosses)
- [Signatures](#signatures)
- [Market identity](#market-identity)
- [Candles](#candles)
- [Backtests](#backtests)
- [Strategies](#strategies)
- [Running a backtest](#running-a-backtest)
- [Decimal and float](#decimal-and-float)
- [Fields](#fields)
- [Equality, keys and repr](#equality-keys-and-repr)
- [Refusals](#refusals)
- [What is not bound](#what-is-not-bound)

## What is bound

<!-- begin: bound types -->
| Python | Rust | What it is |
| --- | --- | --- |
| `Symbol` | `quantforge::Symbol` | A trading symbol, such as `BTCUSDT` |
| `Interval` | `quantforge::Interval` | A candle interval, such as `1m` |
| `Category` | `quantforge::Category` | A product family: `spot`, `linear` or `inverse` |
| `ExchangeId` | `quantforge::ExchangeId` | A product line: one venue's market of one family |
| `MarketId` | `quantforge::MarketId` | The key market data is stored under: a product line, a symbol and an interval |
| `Candle` | `quantforge::Candle` | One bar: its times, its prices, its volume and its trade count |
| `CandleBatch` | `quantforge::CandleBatch` | Candles held column by column |
| `BacktestConfig` | `quantforge::BacktestConfig` | How a backtest starts and what it charges |
| `ClosedTrade` | `quantforge::ClosedTrade` | One closed round trip of a candle strategy on Spot |
| `EquityPoint` | `quantforge::EquityPoint` | Equity at one bar's close |
| `BacktestResult` | `quantforge::BacktestResult` | What a backtest produced |
| `TargetPosition` | `quantforge::TargetPosition` | The position a strategy asks for |
| `StrategyContext` | `quantforge::StrategyContext` | The engine's state as a strategy's callback is handed it |
| `Strategy` | `quantforge::Strategy` | The strategy a class implements: three callbacks an engine drives |
| `run_backtest` | `quantforge::BacktestEngine::run` | Runs a strategy over candles in the engine, and gives the result |
| `decimal_to_f64` | `quantforge::decimal_to_f64` | The float nearest to a decimal |
| `decimal_from_f64` | `quantforge::decimal_from_f64` | The decimal of a float's shortest text |
<!-- end: bound types -->

A class cannot be derived from, and a value cannot be changed once it is
built: there is no setter, and nothing can be added to one. `Strategy` is
the exception to both. It exists to be derived from, and an object of a
class derived from it holds a state of its own.

**The package publishes four functions more, which bind no type of the
engine's:** `candles_to_dataframe`, `dataframe_to_candles`,
`trades_to_dataframe` and `equity_to_dataframe` give a batch's candles and
a result's trades and equity curve as pandas DataFrames, and take a frame
of candles back. They are written in Python, over the types of this page,
and [candles and results as pandas DataFrames](python-pandas.md) is their
contract, with their signatures.

**And one class that is no value:** `CandleReader` is a store that is open,
which reads the candles the command line stored into a `CandleBatch`. It is
the engine's own reader, `quantforge::CandleReader`, and
[the candle reader](python-reader.md) is its contract, with its signatures.

**And the client of a QuantForge server, with what it gives and what it
raises:** `Client`, the `Job` it gives for each backtest job the server
holds, a job's `JobStatus`, `RemoteError` with the six exceptions derived
from it, and `BundleError`, for a strategy bundle that is refused before it
is sent. They are written in Python, with the standard library alone, and
bind no type of the engine's: a status gives what its job runs in the types
of this page, a `MarketId` and a `BacktestConfig`, and a job is submitted
with the same two. [The Python client](python-client.md) is their contract,
with their signatures, and
[submitting a backtest from Python](python-bundles.md) that of a
submission.

## How a value crosses

**A price, a quantity or an amount is a `decimal.Decimal`,** on the way in
and on the way out, and never a float. An `int` is taken as the whole
number it is.

- **Every digit and every decimal place crosses.** `81940.01000000` comes
  back with its eight places, and `1.10` is not turned into `1.1`.
- **What does not fit is refused, never rounded.** The engine's decimal is
  a whole number of at most 96 bits, up to 79228162514264337593543950335,
  with its point at most 28 places from the right. A `Decimal` with a 29th
  decimal place or a 97th bit of digits is a `ValueError`, and so are NaN
  and the infinities.
- **An exponent above zero is written out,** since the engine's decimal
  has none: `Decimal("1E+3")` comes back as `Decimal("1000")`, the same
  number.
- **A negative zero becomes zero,** as everywhere in the engine, and keeps
  its decimal places.
- **No decimal context has a say.** A value crosses as it is whatever the
  context's precision or rounding.
- **Nor has a type derived from `Decimal` or from `int`.** A value of one
  crosses by the number it holds, whatever its own `__format__` or
  `__str__` writes of it, and comes back a plain `Decimal`.
- **A float is never read as a decimal.** It is a `TypeError` that names
  [`decimal_from_f64`](#decimal-and-float), the one way across for a value.
  A batch's columns cross to float64 arrays and back as a whole, by the
  same conversion ([python-numpy.md](python-numpy.md)), and so do the
  columns of a pandas DataFrame ([python-pandas.md](python-pandas.md)).

```python
>>> candle = qf.Candle(
...     open_time_ms=1_700_000_000_000,
...     close_time_ms=1_700_000_059_999,
...     open=Decimal("81940.01000000"),
...     high=Decimal("81990.5"),
...     low=81900,
...     close=Decimal("81955.25"),
...     volume=Decimal("0.17235000"),
... )
>>> candle.open
Decimal('81940.01000000')
>>> candle.low
Decimal('81900')
>>> qf.Candle(open_time_ms=0, close_time_ms=59_999, open=81940.01, high=1, low=1, close=1, volume=1)
Traceback (most recent call last):
    ...
TypeError: open must be a decimal.Decimal or an int, not float: a float is never read as a decimal; convert it with decimal_from_f64
```

**A time is an `int`,** milliseconds since the Unix epoch, UTC, within 64
bits. **A count is an `int`** that is not negative, within 64 bits.
Anything that says it is a whole number, by Python's `__index__`, is taken
for one, here and where a decimal takes an `int`; a `bool` and a number
that would have to be cut to a whole one are not.

**A value the source did not report is `None`,** and stays `None`: a
candle's trade count where the exchange gave none, and a trade's order IDs
where no order reached a venue. It is never read or written as zero.

**A venue's order ID is a `str`,** the venue's own text, 1 to 128
characters without spaces. It is a number on one venue and not on
another, and is never computed with.

**Text is a `str` and a flag is a `bool`,** and nothing that merely
converts to one.

**A refusal is one of two exceptions:**

- **`ValueError`**: a value of the right type that the engine does not
  take. Where the engine has a rule for it, the message is the engine's
  own, word for word, such as `invalid interval: 7m`.
- **`TypeError`**: a value of a type the field does not take, a field left
  out, or one that does not exist.

A message of the package's own names the field it refuses, and a
column's names the row: `open[3]`. [Refusals](#refusals) lists one of each
kind.

**Where a field takes anything that iterates its values,** a list, a
tuple or a generator will do, and text will not: a `str` iterates too, one
character at a time, and is never taken for a sequence of values. What
reading the values raises is raised as it is, so a source that fails to
open fails with its own error.

## Signatures

Every way to build a value, and every method there is, as Python's
`inspect.signature` gives it:

<!-- begin: signatures -->
```text
Symbol(value, /)
Interval(value, /)
Category(value, /)
ExchangeId(value, /)
MarketId(exchange, symbol, interval)
Candle(*, open_time_ms, close_time_ms, open, high, low, close, volume, trades=None)
CandleBatch.from_candles(candles, /, *, market=None)
CandleBatch.from_columns(*, open_time_ms, close_time_ms, open, high, low, close, volume, trades, market=None)
CandleBatch.from_numpy(*, open_time_ms, close_time_ms, open, high, low, close, volume, trades, market=None)
CandleBatch.to_candles()
CandleBatch.to_numpy()
BacktestConfig(*, initial_cash, fee_bps, close_out_at_end)
ClosedTrade(*, symbol, entry_time_ms, exit_time_ms, entry_price, exit_price, qty, gross_quote_pnl, entry_order_id=None, exit_order_id=None)
EquityPoint(*, open_time_ms, equity)
BacktestResult(*, initial_cash, final_equity, total_return_pct, trade_count, max_drawdown_pct, trades, equity_curve)
StrategyContext(*, market, now_ms, cash, position_qty)
Strategy(*args, **kwargs)
Strategy.on_start(ctx)
Strategy.on_bar(ctx, bar)
Strategy.on_finish(ctx)
run_backtest(candles, strategy, config, *, market=None)
decimal_to_f64(value, /)
decimal_from_f64(value, /)
```
<!-- end: signatures -->

A type of one part takes it by position. A record takes its fields by
name, so that no price is taken for another, and every field is required
but the three that may be unknown. A batch takes the market its candles
are of by name alone, and names none unless it is given one. `Strategy`
takes whatever the class derived from it does, and a `TargetPosition` is
not built at all: it is one of two constants. `run_backtest` takes what a
run is made of by position, and the market the candles are of by name
alone, which a batch that names its own stands in for.

## Market identity

A market is a product line, a symbol and an interval. Each part is a type
of its own, built from its text.

```python
>>> qf.Symbol(" btcusdt ")
Symbol('BTCUSDT')
>>> qf.Interval("8h").step_ms
28800000
>>> market = qf.MarketId("binance_spot", "btcusdt", "1m")
>>> market
MarketId(ExchangeId('binance_spot'), Symbol('BTCUSDT'), Interval('1m'))
>>> market.category
Category('spot')
>>> market == qf.MarketId(qf.ExchangeId.BINANCE_SPOT, qf.Symbol("BTCUSDT"), qf.Interval.M1)
True
```

- **`Symbol`** trims its text and writes it in capitals, as the engine
  does everywhere, and refuses text that holds nothing.
- **`Interval`** reads an interval's text with the space around it
  trimmed. Case counts: `8H` is no interval.
- **`Category` and `ExchangeId`** read their names exactly: nothing is
  trimmed, and case counts.
- **`MarketId`** takes each part as its own type or as the text of one,
  read by that type's rule.

`str()` of each of the first four gives its text back. A `MarketId` has
no text of its own, and is written as its `repr`.

**A product line is a venue and a family together,** by the name its
candles are stored under: `binance_spot`, `bybit_linear`. **A venue's name
alone is not a product line.** `ExchangeId("binance")` is refused, and
nothing takes it for the venue's Spot line: a market's family is its
product line's, and is never assumed.

```python
>>> qf.ExchangeId("bybit_inverse") == qf.ExchangeId.BYBIT_INVERSE
True
>>> qf.ExchangeId.BYBIT_INVERSE.category
Category('inverse')
>>> qf.ExchangeId.BYBIT_INVERSE.category.is_futures
True
>>> qf.ExchangeId("binance")
Traceback (most recent call last):
    ...
ValueError: invalid exchange "binance": expected one of binance_spot, binance_linear, binance_inverse, bybit_spot, bybit_linear, bybit_inverse
```

**A market of any product line can be named,** since it is the key that
line's candles are stored under. That says nothing of what runs on it: a
candle strategy is a Spot strategy, and
[`run_backtest`](#running-a-backtest) refuses a linear or an inverse
market.

Every interval, family and product line is a constant of its class, as
is each [target a strategy asks for](#strategies), and `ALL` is a tuple of
them in this order:

<!-- begin: constants -->
| Type | Constants | Their text |
| --- | --- | --- |
| `Interval` | `S1`, `M1`, `M3`, `M5`, `M15`, `M30`, `H1`, `H2`, `H4`, `H6`, `H8`, `H12`, `D1`, `D3`, `W1` | `1s`, `1m`, `3m`, `5m`, `15m`, `30m`, `1h`, `2h`, `4h`, `6h`, `8h`, `12h`, `1d`, `3d`, `1w` |
| `Category` | `SPOT`, `LINEAR`, `INVERSE` | `spot`, `linear`, `inverse` |
| `ExchangeId` | `BINANCE_SPOT`, `BINANCE_LINEAR`, `BINANCE_INVERSE`, `BYBIT_SPOT`, `BYBIT_LINEAR`, `BYBIT_INVERSE` | `binance_spot`, `binance_linear`, `binance_inverse`, `bybit_spot`, `bybit_linear`, `bybit_inverse` |
| `TargetPosition` | `FLAT`, `LONG_ALL_IN` | `TargetPosition.FLAT`, `TargetPosition.LONG_ALL_IN` |
<!-- end: constants -->

An interval's constant is the engine's name for it, and a family's and a
product line's are their text in capitals. The three are listed from the
engine when the package is imported, so a product line the engine gains is
a constant here without a change to the package. A target's constant is
the engine's name for it too, in capitals with its words apart. It has no
text of its own, and is written as the constant it is.

**A venue is not a type of its own in Python.** A product line's name
begins with its venue's, and that is all of a venue the package holds:
[shared-model.md](shared-model.md) says which parts of the market's
identity reach Python, and a venue apart from its lines is not one.

## Candles

**A `Candle` holds what it is given.** Nothing checks that its prices make
a bar: a low above the high is kept as it is. Checking candles for gaps,
duplicates and impossible prices is the engine's own separate pass, which
is not bound yet.

**A `CandleBatch` is candles held column by column,** the shape candles
are exchanged in. It is built from candles, from eight columns or from
eight NumPy arrays, and never directly. The first two convert no value,
either way:

```python
>>> batch = qf.CandleBatch.from_candles([candle, candle])
>>> batch
CandleBatch(<2 candles>)
>>> len(batch)
2
>>> batch.open
(Decimal('81940.01000000'), Decimal('81940.01000000'))
>>> batch.trades
(None, None)
>>> batch.to_candles() == [candle, candle]
True
```

- **`from_candles`** takes anything that iterates `Candle`s, and keeps
  their order.
- **`from_columns`** takes the eight columns, each anything that iterates
  its values, and holds every column to the length of `open_time_ms`. The
  first that differs is refused, in the engine's words.
- **`to_candles`** gives the candles back as a list.
- **Each column reads as a tuple,** built anew each time it is read: keep
  it in a name, and do not read it in a loop.
- **`len()`** is the number of candles, and an empty batch is false.

**A batch names the market its candles are of, or none.** Each way of
building one takes `market=`, a `MarketId` given by name alone, and
`market` gives it back: `None` where none was given.

```python
>>> named = qf.CandleBatch.from_candles([candle, candle], market=market)
>>> named
CandleBatch(<2 candles>, market=MarketId(ExchangeId('binance_spot'), Symbol('BTCUSDT'), Interval('1m')))
>>> named.market == market
True
>>> batch.market is None
True
>>> named == batch
False
```

- **The market is part of what a batch is.** Two batches of the same
  candles are equal when they name the same market, or both none. The same
  candles of `binance_spot` and of `binance_linear` are two batches, and
  so are those of two intervals.
- **A batch of any product line can be built,** as a market of any line
  can be named. What runs on it is another matter:
  [`run_backtest`](#running-a-backtest) runs a batch on the market it
  names, and the engine refuses one that is not Spot.
- **Nothing is checked against the market,** nor anything else about the
  candles: a batch does not hold their times to its market's interval, and
  does not check their order.

**A batch's columns cross to NumPy arrays and back.** `to_numpy()` gives a
dict of eight arrays, and `from_numpy` builds a batch from eight. It is the
one place a batch's decimals become floats, and
[candles as NumPy arrays](python-numpy.md) is its contract: what each array
is, what crosses exactly, and what is refused. The two need NumPy, an extra
of the package: `pip install "quantforge-py[numpy]"`. Without it they raise
an `ImportError` that says so, and everything else on this page works.

**A batch's candles are a pandas DataFrame through the same arrays.**
`candles_to_dataframe(batch)` gives a frame indexed by the UTC time each
bar opened, and `dataframe_to_candles(frame, market=...)` builds a batch
from one ([candles and results as pandas DataFrames](python-pandas.md)).
They need pandas, the package's other extra:
`pip install "quantforge-py[pandas]"`.

## Backtests

These four are the candle strategy's backtest on Spot, as the engine has
it: what [`run_backtest`](#running-a-backtest) is given and what it gives.
Each is built by hand as well, as the code around a backtest builds one in
its tests.

**A `BacktestConfig` is refused where the engine would refuse to run
it,** when it is built and in the engine's words: `initial_cash` must be
above zero, and `fee_bps`, the fee on every fill in basis points of its
notional, zero or more. All three settings are required, and none has a
default.

```python
>>> qf.BacktestConfig(initial_cash=Decimal("10000"), fee_bps=10, close_out_at_end=True)
BacktestConfig(initial_cash=Decimal('10000'), fee_bps=Decimal('10'), close_out_at_end=True)
>>> qf.BacktestConfig(initial_cash=0, fee_bps=10, close_out_at_end=True)
Traceback (most recent call last):
    ...
ValueError: invalid configuration: initial_cash must be greater than 0, got 0
```

**A `ClosedTrade`, an `EquityPoint` and a `BacktestResult` hold what they
are given,** as the engine's own types do: nothing recounts a result's
trades or checks a trade's arithmetic.

```python
>>> trade = qf.ClosedTrade(
...     symbol="btcusdt",
...     entry_time_ms=60_000,
...     exit_time_ms=180_000,
...     entry_price=Decimal("100"),
...     exit_price=Decimal("125"),
...     qty=Decimal("10"),
...     gross_quote_pnl=Decimal("250"),
... )
>>> trade.symbol
Symbol('BTCUSDT')
>>> trade.entry_order_id is None
True
>>> result = qf.BacktestResult(
...     initial_cash=Decimal("1000"),
...     final_equity=Decimal("1250"),
...     total_return_pct=Decimal("25"),
...     trade_count=1,
...     max_drawdown_pct=Decimal("0"),
...     trades=[trade],
...     equity_curve=[qf.EquityPoint(open_time_ms=0, equity=Decimal("1000"))],
... )
>>> result
BacktestResult(initial_cash=Decimal('1000'), final_equity=Decimal('1250'), total_return_pct=Decimal('25'), trade_count=1, max_drawdown_pct=Decimal('0'), trades=<1 trade>, equity_curve=<1 point>)
>>> result.trades[0] == trade
True
```

**`gross_quote_pnl` is Spot's PnL field, in the quote asset, and keeps
the two meanings it has in the engine.** In a backtest it is the change in
cash over the trade, so both fees are already taken off; in live trading
it is the difference of the two prices times the quantity, before fees. It
is not a futures PnL, which the engine keeps apart with its fees and its
funding ([shared-model.md](shared-model.md#not-changing)).

**A result's equity curve has one point for every bar,** and its trades
and its curve read as tuples, built anew each time they are read.

**For computing with, a result's trades and its curve are pandas
DataFrames:** `trades_to_dataframe(result)` and
`equity_to_dataframe(result)`, each amount as the float nearest to its
decimal ([candles and results as pandas DataFrames](python-pandas.md)).
The exact values are the ones read here.

## Strategies

**A strategy is a class derived from `Strategy` that defines `on_bar`.**
An engine drives it bar by bar, through three callbacks:

<!-- begin: callbacks -->
| Callback | Called | Returns |
| --- | --- | --- |
| `on_start(ctx)` | Once, before the first bar | `None` |
| `on_bar(ctx, bar)` | Once for every candle, in the order the run was given them | A `TargetPosition`, or `None` to leave the position as it is |
| `on_finish(ctx)` | Once, after the last bar, unless a callback failed | `None` |
<!-- end: callbacks -->

`ctx` is a `StrategyContext`, the engine's state at that moment, and `bar`
a `Candle`. A class defines `on_bar` and, where it has a use for them, the
other two, which otherwise do nothing.

```python
>>> class EnterThenExit(qf.Strategy):
...     """Goes long on the first bar and flat on the third."""
...
...     def __init__(self):
...         self.bars = 0
...
...     def on_bar(self, ctx, bar):
...         self.bars += 1
...         if self.bars == 1:
...             return qf.TargetPosition.LONG_ALL_IN
...         if self.bars == 3:
...             return qf.TargetPosition.FLAT
...         return None
...
>>> strategy = EnterThenExit()
>>> ctx = qf.StrategyContext(market=market, now_ms=candle.open_time_ms, cash=Decimal("1000"), position_qty=0)
>>> strategy.on_bar(ctx, candle)
TargetPosition.LONG_ALL_IN
>>> strategy.on_bar(ctx, candle) is None
True
>>> strategy.on_bar(ctx, candle)
TargetPosition.FLAT
```

The callbacks are called here by hand, with a context built by hand, as a
test of a strategy calls them. The engine calls them in
[`run_backtest`](#running-a-backtest), and the package's tests hold every
rule of this section to it there.

**The class is the strategy's own.** It takes whatever arguments its
`__init__` does, which is how a [strategy bundle's](bundle-spec.md#params)
parameters reach it, and it keeps its state in itself. Two things are
asked of it:

- **It defines `on_bar`.** A class that has none cannot be built, and nor
  can `Strategy` itself.
- **It is named by its `name`, a `str`, where it has one,** on the class or
  on the object, and by its class's name otherwise. The name is how a
  failure names the strategy, and how a host records a run of it.

**A context is the engine's state as the callback is called,** four
values and nothing of the engine to reach through them:

- **`market`** is the market the run is on.
- **`now_ms`** is the time of the callback: in a backtest, the open time
  of the bar, and for `on_start` and `on_finish` that of the first bar and
  of the last.
- **`cash`** is the quote currency held and **`position_qty`** the base
  asset: in a backtest, after any fill at that bar's open.

An engine says what its contexts hold, and these are a backtest's, the
one engine a Python strategy runs in so far. Each callback is handed a
context and a bar of its own, built anew. What a context said it still
says when the run has moved on, and a bar is equal to the candle the run
was given, digit for digit.

**`on_bar` returns the position to hold after the bar:**
`TargetPosition.LONG_ALL_IN`, `TargetPosition.FLAT`, or `None` to leave
the position as it is. A backtest fills a decision at the next bar's open,
and asking for the position already held does nothing. **Nothing else is
read as a decision,** and `on_start` and `on_finish` return `None`, where
a decision returned from either would otherwise be dropped without a word:

<!-- begin: returns -->
| Returned | From | The run fails with |
| --- | --- | --- |
| `"FLAT"` | `on_bar` | `TypeError: on_bar must return a TargetPosition or None, not str` |
| `True` | `on_bar` | `TypeError: on_bar must return a TargetPosition or None, not bool` |
| `1` | `on_bar` | `TypeError: on_bar must return a TargetPosition or None, not int` |
| `qf.TargetPosition` | `on_bar` | `TypeError: on_bar must return a TargetPosition or None, not type` |
| `qf.TargetPosition.FLAT` | `on_start` | `TypeError: on_start must return None, not TargetPosition` |
| `0` | `on_finish` | `TypeError: on_finish must return None, not int` |
<!-- end: returns -->

An `async def` returns a coroutine, which is refused the same way.

**A callback that fails ends the run.** No callback follows it, so
`on_finish` is not called after a failure, and a callback that returns
what it may not has failed as one that raised has.

- **Python is raised the exception itself:** the object the callback
  raised, with its traceback as it was, and one note that says where the
  strategy failed. Nothing is wrapped around it, so `except
  ZeroDivisionError` catches a division by zero, and `KeyboardInterrupt`
  interrupts a run as it interrupts any code.
- **A host that is not Python is told the same as text:** the command line
  and a server's worker have the engine's error, which is that line
  followed by the exception as Python itself writes it, with its
  traceback and the exceptions it came from.

<!-- begin: failure as told -->
```text
the strategy "EnterThenExit" failed in on_bar, at the bar that opened at 1700000000000
Traceback (most recent call last):
  File "strategy.py", line 12, in on_bar
    average = self.total / self.count
              ~~~~~~~~~~~^~~~~~~~~~~~
ZeroDivisionError: division by zero
```
<!-- end: failure as told -->

The first line is the note, and names the bar by its open time.

**A strategy decides from the bars it has been handed and from its own
state,** as the engine asks of every strategy: no clock, no randomness and
no I/O, so that the same bars give the same decisions on every run. Python
adds one thing to mind. Arithmetic on `Decimal` follows the decimal
context of the thread a callback runs on, which the engine neither sets
nor reads: a strategy that needs another precision or rounding sets it
itself. [The goldens](golden-parity.md#decimals-in-the-strategy) show what
that comes to for one strategy that divides, an SMA cross: at its windows,
the same decisions as the engine's own decimal makes in any context that
holds the sums it adds up.

**Three callbacks, a context of four values and two targets are all of
the boundary.** No callback is called for a webhook, an order, a fill or a
store, and a strategy is handed no exchange, no account and no credential
([what is not bound](#what-is-not-bound)).

The package's tests hold a strategy written in Python to the cases the
engine's own tests write from one written in Rust,
`tests/fixtures/strategy_cases.json`: every callback in order, with the
state it was handed digit for digit, and the result.

## Running a backtest

**`run_backtest` runs a strategy over candles in the engine, and gives the
result.** The run is the engine's own, `BacktestEngine::run`, which the
`quantforge backtest` command runs too: nothing of it is computed in
Python but what the strategy's callbacks compute.

```python
>>> def minute(index, open, close):
...     return qf.Candle(
...         open_time_ms=index * 60_000,
...         close_time_ms=index * 60_000 + 59_999,
...         open=open,
...         high=max(open, close),
...         low=min(open, close),
...         close=close,
...         volume=1,
...     )
...
>>> candles = [minute(0, 100, 100), minute(1, 100, 110), minute(2, 110, 120), minute(3, 125, 125)]
>>> config = qf.BacktestConfig(initial_cash=1000, fee_bps=0, close_out_at_end=True)
>>> result = qf.run_backtest(candles, EnterThenExit(), config, market=market)
>>> result
BacktestResult(initial_cash=Decimal('1000'), final_equity=Decimal('1250'), total_return_pct=Decimal('25.00'), trade_count=1, max_drawdown_pct=Decimal('0'), trades=<1 trade>, equity_curve=<4 points>)
>>> result.trades[0].entry_price, result.trades[0].exit_price
(Decimal('100'), Decimal('125'))
>>> [point.equity for point in result.equity_curve]
[Decimal('1000'), Decimal('1100'), Decimal('1200'), Decimal('1250')]
```

The strategy went long on the first bar, which filled at the second bar's
open, 100, and flat on the third, which filled at the fourth's, 125.

<!-- begin: run arguments -->
| Argument | Given | Takes | What it is |
| --- | --- | --- | --- |
| `candles` | By position or by name | A `CandleBatch`, or anything that iterates `Candle`s | The bars of the run, replayed in the order given |
| `strategy` | By position or by name | An object of a class derived from `Strategy` | What decides, called as [Strategies](#strategies) says |
| `config` | By position or by name | A `BacktestConfig` | The cash the run starts with, the fee on every fill, and whether a position left open is closed out |
| `market` | By name alone | A `MarketId` of a Spot product line, or nothing where `candles` is a batch that names its market | The market the candles are of: every callback is handed it, and every trade takes its symbol from it |
<!-- end: run arguments -->

**The market is named, and never assumed.** It is a `MarketId`, which
names its product line and with it its family: `binance_spot`, never a
venue's name alone. It is named by `market=`, or by the candles where they
are [a batch that names its market](#candles): such a batch is run on the
market it names, and needs none beside it. Candles that name no market, a
list of them or a batch built without one, are a `TypeError` without
`market=`. Nothing is read from the candles themselves.

```python
>>> qf.run_backtest(qf.CandleBatch.from_candles(candles, market=market), EnterThenExit(), config) == result
True
```

**A batch is never run on a market other than the one it names.** Where
`market=` is given beside such a batch, the two are the same market:
product line, symbol and interval. Another is refused before the engine is
asked anything, so a linear series is never run as its Spot namesake, nor
one symbol's candles as another's:

```python
>>> qf.run_backtest(named, EnterThenExit(), config, market=qf.MarketId("binance_linear", "BTCUSDT", "1m"))
Traceback (most recent call last):
    ...
ValueError: the candles are of binance_spot BTCUSDT 1m, and market names binance_linear BTCUSDT 1m
```

**A candle strategy runs on Spot,** and the engine refuses any other
market before a callback is called:

```python
>>> qf.run_backtest(candles, EnterThenExit(), config, market=qf.MarketId("bybit_linear", "BTCUSDT", "1m"))
Traceback (most recent call last):
    ...
ValueError: candle strategies run on spot markets only, and bybit_linear trades linear futures
```

**There is no futures backtest.** The run fills, charges and marks by
Spot's rules: one position, long or flat, bought with all the cash. A
linear or an inverse series run by them would be a result that means
nothing, with no contract, no margin and no funding in it, so it is
refused and never run. Execution on futures is the engine's own, on
external signals, and is not reached from Python
([what is not bound](#what-is-not-bound)).

**The execution model is the engine's,** and
[`src/engine/backtest.rs`](../src/engine/backtest.rs) states it in full:

- **A decision is made at a bar's close.** `on_bar` is called once for
  every candle, in the order given, with the complete bar.
- **It is filled at the next bar's open,** a price the strategy had not
  seen when it decided: there is no look-ahead. A decision on the last bar
  never fills.
- **One position, long or flat.** `TargetPosition.LONG_ALL_IN` spends all
  the cash, and `TargetPosition.FLAT` sells all that is held. There is no
  shorting, leverage, partial size or slippage, and no exchange rule such
  as a step size or a minimum notional.
- **Every fill pays the config's fee,** `fee_bps` basis points of its
  notional, in the quote currency.
- **A position still open after the last bar** is sold at that bar's
  close, fee included, where `close_out_at_end` is `True`, and is only
  valued at that close otherwise.
- **Equity is marked at every bar's close,** after the bar's fill: the
  curve has one point for every bar.

**The candles are replayed as they are given.** Nothing checks their
order, their spacing or their prices: a bar out of place is handed over
out of place, and the bar after it is the one a decision fills at. What
iterates once, such as a generator, is read to its end before the run
starts, and what reading it raises is raised as it is.

**The result is the engine's `BacktestResult`, field for field.** It has
the fields of the engine's own type, by the same names in the same order,
and each means what it means there ([Fields](#fields)); so have the
`ClosedTrade`s and the `EquityPoint`s it holds. Nothing is renamed, left
out, or worked out again in Python.

**The same run gives the same result,** on every run and on every
machine, digit for digit: a run is a function of its candles, its config
and its strategy, and the engine reads no clock, draws no random number
and does no I/O. That holds for a strategy that starts each run in the
same state and itself
[decides from its bars and its own state](#strategies), with no clock and
no randomness, in what it is built with as in what it decides. The engine
keeps nothing between two runs, but a strategy keeps its state: each run
is given a strategy built for it, the same way each time.

**A run that is not made, or does not end, raises:**

<!-- begin: run failures -->
| What | Raised | When |
| --- | --- | --- |
| An argument of a type it does not take | `TypeError` | Before any callback, each argument in the order written |
| No market named, by `market=` or by a batch | `TypeError` | Before any callback, once every argument is of its type |
| A market other than the one a batch names | `ValueError` | Before any callback, and before the engine is asked anything |
| A market that is not Spot | `ValueError` | Before any callback, in the engine's words |
| No candles | `ValueError` | Before any callback, in the engine's words |
| An amount beyond the engine's decimal | `OverflowError` | In the run, at the fill or the mark that could not be computed, or after the last callback, where the result's figures are worked out |
| A callback that raises | The callback's own exception | In the run, at that callback |
| A callback that returns what it may not | `TypeError` | In the run, at that callback |
<!-- end: run failures -->

The candles are read first, to their end, and then the other arguments
are held to their types. Once all four are of their types, the market of
the run is settled: one has to be named, and two that differ are refused.
Then the engine looks at the market before it looks at the candles.

**An amount is beyond the engine's decimal when its whole part is.** The
largest decimal is 79228162514264337593543950335, of 29 digits, and at a
price of `1E-28`, 1000 buys a quantity of 32. Decimal places that do not
fit are no error: the engine rounds a quantity or a fee to the digits a
decimal holds, the same way on every run.

<!-- begin: run refusals -->
| Run | Raises |
| --- | --- |
| `qf.run_backtest(5, strategy, config, market=market)` | `TypeError: candles must be a CandleBatch or an iterable of Candle, not int` |
| `qf.run_backtest("candles.csv", strategy, config, market=market)` | `TypeError: candles must be a CandleBatch or an iterable of Candle, not str` |
| `qf.run_backtest([candle, 100], strategy, config, market=market)` | `TypeError: candles[1] must be a Candle, not int` |
| `qf.run_backtest(candles, EnterThenExit, config, market=market)` | `TypeError: strategy must be a Strategy, not the class EnterThenExit: build one from it` |
| `qf.run_backtest(candles, strategy, {"initial_cash": 1000}, market=market)` | `TypeError: config must be a BacktestConfig, not dict` |
| `qf.run_backtest(candles, strategy, config, market="binance_spot")` | `TypeError: market must be a MarketId, not str` |
| `qf.run_backtest(candles, strategy, config)` | `TypeError: market is required: the candles name no market` |
| `qf.run_backtest(named, strategy, config, market=qf.MarketId("bybit_spot", "BTCUSDT", "1m"))` | `ValueError: the candles are of binance_spot BTCUSDT 1m, and market names bybit_spot BTCUSDT 1m` |
| `qf.run_backtest(qf.CandleBatch.from_candles(candles, market=qf.MarketId("bybit_linear", "BTCUSDT", "1m")), strategy, config)` | `ValueError: candle strategies run on spot markets only, and bybit_linear trades linear futures` |
| `qf.run_backtest(candles, strategy, config, market=qf.MarketId("binance_inverse", "BTCUSD", "1m"))` | `ValueError: candle strategies run on spot markets only, and binance_inverse trades inverse futures` |
| `qf.run_backtest([], strategy, config, market=market)` | `ValueError: no candles provided` |
| `qf.run_backtest([minute(0, 1, 1), minute(1, Decimal("1E-28"), 1)], EnterThenExit(), config, market=market)` | `OverflowError: decimal overflow computing the entry` |
<!-- end: run refusals -->

`candle`, `candles`, `named`, `strategy`, `config` and `market` are those
of the examples above.

**A callback that fails ends the run with its own exception,** as
[Strategies](#strategies) says: the object it raised, with its traceback
and one note that says where the strategy failed. A callback that returns
what it may not has failed too, with the `TypeError` that names what it
returned.

**The run is on the thread that asked for it.** The callbacks run where
the caller is, one after another, and the call returns when the run is
over. `KeyboardInterrupt` interrupts it at the next callback, as it
interrupts any Python code.

The package's tests run a strategy that decides from its bars over four
hours of fixture candles, and hold it to the engine's run of the same rule
written in Rust: the decision on every bar, every callback, and the
result, digit for digit (`quantforge-py/tests/test_end_to_end.py`).

**And they hold an SMA cross written in Python to the engine's own.**
[The goldens](golden-parity.md) are one frozen day of candles and what the
built-in Rust SMA cross made of them. The same strategy written in Python,
run through `run_backtest`, has to make the same trades, the same final
equity and the same drawdown, each compared as the text of its decimal
(`quantforge-py/tests/test_golden_parity.py`).

## Decimal and float

The engine computes in exact decimals, and Python's numeric libraries in
floats. The two meet in the engine's own two conversions, which follow the
[decimal and float64 policy](decimal-float-policy.md). The package
publishes them as two functions, applies them to a batch's columns as a
whole in [`to_numpy` and `from_numpy`](python-numpy.md), and to the amounts
of a result where it is given as a
[pandas DataFrame](python-pandas.md). Nothing else in the package turns one
into the other.

```python
>>> qf.decimal_to_f64(Decimal("81940.01000000"))
81940.01
>>> qf.decimal_from_f64(0.1)
Decimal('0.1')
>>> qf.decimal_from_f64(float("nan"))
Traceback (most recent call last):
    ...
ValueError: cannot convert NaN to a decimal: not finite
```

- **`decimal_to_f64`** gives the float nearest to a decimal: Python's
  `float(str(value))`, but for a negative zero, which gives `0.0`.
- **`decimal_from_f64`** gives the decimal of a float's shortest text,
  `Decimal(repr(value))`, with a negative zero made zero. Where two texts
  of that length are equally near the float, it is the one `repr` writes,
  whose last digit is even. It refuses NaN, the infinities, and a float
  whose shortest text does not fit the engine's decimal.

The package's tests hold both to the cases the engine's own tests hold
them to, `tests/fixtures/decimal_float_cases.csv`.

## Fields

Everything a value gives, by the name it is read with:

<!-- begin: fields -->
| Type | Field | Gives | What it is |
| --- | --- | --- | --- |
| `Interval` | `step_ms` | `int` | The length of one candle, in milliseconds |
| `Category` | `is_futures` | `bool` | Whether the family trades futures contracts rather than inventory |
| `ExchangeId` | `category` | `Category` | The line's product family |
| `MarketId` | `exchange` | `ExchangeId` | The product line the market trades on |
| `MarketId` | `symbol` | `Symbol` | The market's symbol |
| `MarketId` | `interval` | `Interval` | The interval of the market's candles |
| `MarketId` | `category` | `Category` | The market's product family: its product line's |
| `Candle` | `open_time_ms` | `int` | When the bar opened |
| `Candle` | `close_time_ms` | `int` | When the bar closed |
| `Candle` | `open` | `Decimal` | The first price of the bar |
| `Candle` | `high` | `Decimal` | The highest price of the bar |
| `Candle` | `low` | `Decimal` | The lowest price of the bar |
| `Candle` | `close` | `Decimal` | The last price of the bar |
| `Candle` | `volume` | `Decimal` | The volume traded in the bar, in the base asset |
| `Candle` | `trades` | `int or None` | The exchange's trade count, `None` where it reported none |
| `CandleBatch` | `open_time_ms` | `tuple of int` | Every candle's open time |
| `CandleBatch` | `close_time_ms` | `tuple of int` | Every candle's close time |
| `CandleBatch` | `open` | `tuple of Decimal` | Every candle's first price |
| `CandleBatch` | `high` | `tuple of Decimal` | Every candle's highest price |
| `CandleBatch` | `low` | `tuple of Decimal` | Every candle's lowest price |
| `CandleBatch` | `close` | `tuple of Decimal` | Every candle's last price |
| `CandleBatch` | `volume` | `tuple of Decimal` | Every candle's volume |
| `CandleBatch` | `trades` | `tuple of int or None` | Every candle's trade count |
| `CandleBatch` | `market` | `MarketId or None` | The market the candles are of, `None` where the batch names none |
| `BacktestConfig` | `initial_cash` | `Decimal` | Quote currency to start with, above zero |
| `BacktestConfig` | `fee_bps` | `Decimal` | The fee on every fill, in basis points of its notional: 10 is 0.1% |
| `BacktestConfig` | `close_out_at_end` | `bool` | Whether a position still open after the last bar is sold at that bar's close; otherwise it is only valued there |
| `ClosedTrade` | `symbol` | `Symbol` | The symbol traded |
| `ClosedTrade` | `entry_time_ms` | `int` | When the entry filled |
| `ClosedTrade` | `exit_time_ms` | `int` | When the exit filled |
| `ClosedTrade` | `entry_price` | `Decimal` | The price the entry filled at |
| `ClosedTrade` | `exit_price` | `Decimal` | The price the exit filled at |
| `ClosedTrade` | `qty` | `Decimal` | The quantity traded, in the base asset |
| `ClosedTrade` | `gross_quote_pnl` | `Decimal` | The trade's PnL in the quote asset: in a backtest, net of both fees |
| `ClosedTrade` | `entry_order_id` | `str or None` | The venue's ID of the entry order, `None` where none reached a venue |
| `ClosedTrade` | `exit_order_id` | `str or None` | The venue's ID of the exit order, `None` where none reached a venue |
| `EquityPoint` | `open_time_ms` | `int` | The open time of the bar, which names it |
| `EquityPoint` | `equity` | `Decimal` | Cash plus the position valued at the bar's close, after the bar's fill |
| `BacktestResult` | `initial_cash` | `Decimal` | The starting cash |
| `BacktestResult` | `final_equity` | `Decimal` | Cash plus any open position valued at the last close, after close-out |
| `BacktestResult` | `total_return_pct` | `Decimal` | The gain over the starting cash, in percent |
| `BacktestResult` | `trade_count` | `int` | The number of closed trades |
| `BacktestResult` | `max_drawdown_pct` | `Decimal` | The largest fall of the close-marked equity from an earlier peak, in percent |
| `BacktestResult` | `trades` | `tuple of ClosedTrade` | Every round trip, in order |
| `BacktestResult` | `equity_curve` | `tuple of EquityPoint` | The close-marked equity, one point per bar: it leaves out the close-out fee, which `final_equity` includes |
| `StrategyContext` | `market` | `MarketId` | The market the run is on |
| `StrategyContext` | `now_ms` | `int` | The time of the callback: in a backtest, the open time of the bar |
| `StrategyContext` | `cash` | `Decimal` | The quote currency held |
| `StrategyContext` | `position_qty` | `Decimal` | The base asset held |
<!-- end: fields -->

A record's fields are the names it is built with, in that order. Every
time is milliseconds since the Unix epoch, UTC, and every amount of a
backtest is in the quote currency. A `TargetPosition` has no field, and a
`Strategy` none but those its class gives it.

## Equality, keys and repr

<!-- begin: keys -->
| Type | Equal when | A key |
| --- | --- | --- |
| `Symbol` | Its text is | yes |
| `Interval` | It is the same interval | yes |
| `Category` | It is the same family | yes |
| `ExchangeId` | It is the same product line | yes |
| `MarketId` | All three parts are | yes |
| `Candle` | Every field is | no |
| `CandleBatch` | Every candle is, in order, and both name the same market or none | no |
| `BacktestConfig` | All three settings are | no |
| `ClosedTrade` | Every field is | no |
| `EquityPoint` | Both fields are | no |
| `BacktestResult` | Every field is, trades and curve in order | no |
| `TargetPosition` | It is the same target | yes |
| `StrategyContext` | Every field is | no |
<!-- end: keys -->

**Decimals are compared by value:** a candle that opened at
`81940.01000000` equals one that opened at `81940.01`. Read a field to
tell the two apart.

**A value is equal only to a value of its own type.** A `Symbol` is not
equal to its text, and a `MarketId` is not equal to a tuple of its parts.

**The identity types are keys,** of a dict or a set, and so is a target.
The records are not: `hash()` of a candle is a `TypeError`. The engine's
records have no hash either.

**`repr()` is an expression that builds the value again,** digit for
digit, wherever that is short enough to read. The two that hold
sequences count them instead, and their `repr` cannot be evaluated:
`CandleBatch(<1440 candles>)`, and a result's `trades=<3 trades>,
equity_curve=<1440 points>`. A batch that names its market writes it after
the count, as it is given: `CandleBatch(<1440 candles>, market=MarketId(...))`.
A target's `repr` names its constant, `TargetPosition.FLAT`, which is the
target itself.

**A value cannot be pickled or deep-copied yet.** No story has asked for
it; one that needs to send a value between processes adds it.

**A strategy is not a value.** It is an object like any other of
Python's, equal to itself alone and a key by what it is, unless its class
says otherwise, and its `repr` is its class's own.

## Refusals

One refusal of each kind, as it is written and as it is refused:

<!-- begin: refusals -->
| Written | Refused with |
| --- | --- |
| `qf.Symbol("  ")` | `ValueError: invalid symbol: empty` |
| `qf.Interval("7m")` | `ValueError: invalid interval: 7m` |
| `qf.Interval("8H")` | `ValueError: invalid interval: 8H` |
| `qf.Category("futures")` | `ValueError: invalid category "futures": expected one of spot, linear, inverse` |
| `qf.ExchangeId("binance")` | `ValueError: invalid exchange "binance": expected one of binance_spot, binance_linear, binance_inverse, bybit_spot, bybit_linear, bybit_inverse` |
| `qf.MarketId("binance_spot", "BTCUSDT", "7m")` | `ValueError: invalid interval: 7m` |
| `qf.MarketId("binance_spot", "BTCUSDT", 60)` | `TypeError: interval must be an Interval or a str, not int` |
| `qf.EquityPoint(open_time_ms=0, equity=0.5)` | `TypeError: equity must be a decimal.Decimal or an int, not float: a float is never read as a decimal; convert it with decimal_from_f64` |
| `qf.EquityPoint(open_time_ms=0, equity="0.5")` | `TypeError: equity must be a decimal.Decimal or an int, not str` |
| `qf.EquityPoint(open_time_ms=0, equity=Decimal("NaN"))` | `ValueError: invalid equity NaN: must be a finite number` |
| `qf.EquityPoint(open_time_ms=0, equity=Decimal("1E-29"))` | `ValueError: invalid equity 1E-29: must fit a 96-bit decimal with at most 28 decimal places` |
| `qf.EquityPoint(open_time_ms=0, equity=2**96)` | `ValueError: invalid equity 79228162514264337593543950336: must fit a 96-bit decimal with at most 28 decimal places` |
| `qf.EquityPoint(open_time_ms=0.0, equity=1)` | `TypeError: open_time_ms must be an int, not float` |
| `qf.EquityPoint(open_time_ms=2**63, equity=1)` | `ValueError: invalid open_time_ms 9223372036854775808: must be between -9223372036854775808 and 9223372036854775807` |
| `qf.Candle(open_time_ms=0, close_time_ms=1, open=1, high=1, low=1, close=1, volume=1, trades=-1)` | `ValueError: invalid trades -1: must be between 0 and 18446744073709551615` |
| `qf.CandleBatch.from_columns(open_time_ms=[0, 1], close_time_ms=[1, 2], open=[1, 2], high=[1, 2], low=[1], close=[1, 2], volume=[1, 2], trades=[None, 3])` | ``ValueError: candle column `low` has 1 rows, expected 2`` |
| `qf.CandleBatch.from_columns(open_time_ms=[0, 1], close_time_ms=[1, 2], open=[1, 2.5], high=[1, 2], low=[1, 2], close=[1, 2], volume=[1, 2], trades=[None, 3])` | `TypeError: open[1] must be a decimal.Decimal or an int, not float: a float is never read as a decimal; convert it with decimal_from_f64` |
| `qf.CandleBatch.from_candles([1])` | `TypeError: candles[0] must be a Candle, not int` |
| `qf.CandleBatch.from_candles("BTCUSDT")` | `TypeError: candles must be an iterable of Candle, not str` |
| `qf.CandleBatch.from_candles([], market="binance_spot")` | `TypeError: market must be a MarketId or None, not str` |
| `qf.BacktestConfig(initial_cash=0, fee_bps=10, close_out_at_end=True)` | `ValueError: invalid configuration: initial_cash must be greater than 0, got 0` |
| `qf.BacktestConfig(initial_cash=1, fee_bps=-1, close_out_at_end=True)` | `ValueError: invalid configuration: fee_bps must be zero or greater, got -1` |
| `qf.BacktestConfig(initial_cash=1, fee_bps=0, close_out_at_end=1)` | `TypeError: close_out_at_end must be a bool, not int` |
| `qf.ClosedTrade(symbol="BTCUSDT", entry_time_ms=0, exit_time_ms=1, entry_price=1, exit_price=1, qty=1, gross_quote_pnl=0, entry_order_id="has space")` | `ValueError: invalid venue order id "has space": 1 to 128 characters without spaces` |
| `qf.StrategyContext(market="binance_spot", now_ms=0, cash=1, position_qty=0)` | `TypeError: market must be a MarketId, not str` |
| `qf.StrategyContext(market=qf.MarketId("binance_spot", "BTCUSDT", "1m"), now_ms=0, cash=0.5, position_qty=0)` | `TypeError: cash must be a decimal.Decimal or an int, not float: a float is never read as a decimal; convert it with decimal_from_f64` |
| `qf.TargetPosition()` | `TypeError: cannot create 'quantforge_py.TargetPosition' instances` |
| `qf.Strategy()` | `TypeError: Strategy cannot be built: derive a class from it that defines on_bar` |
| `type("Undecided", (qf.Strategy,), {})()` | `TypeError: Undecided cannot be built: it does not define on_bar` |
| `qf.decimal_from_f64(float("inf"))` | `ValueError: cannot convert inf to a decimal: not finite` |
| `qf.decimal_from_f64(1)` | `TypeError: value must be a float, not int` |
| `qf.decimal_to_f64(0.5)` | `TypeError: value must be a decimal.Decimal or an int, not float` |
<!-- end: refusals -->

A value that is refused is quoted as Python writes it, and in part where
it is very long.

## What is not bound

**No futures execution type is public in Python.** ADR 0002 §14 froze it:
nothing futures-specific crosses the candle-strategy boundary, and there
is no Python webhook API. The types of these modules are the engine's
own, and stay in Rust:

<!-- begin: not bound -->
| Module | What it holds |
| --- | --- |
| `quantforge::model::futures` | The futures contract: instruments, accounts and positions, orders and stops, intents and position cycles, a profile's policy, risk sizing, external signals, and `RealizedPnl` |
| `quantforge::model::execution` | The execution store's records: the signal inbox, the ledger, incidents, the outbox, leases |
| `quantforge::ports::futures` | The futures connector ports |
| `quantforge::automation` | The policy, signal and ledger services |
<!-- end: not bound -->

They are public in Rust, where an adapter and the automation are written
against them, and internal to the engine as far as Python goes: a
strategy written in Python is a candle strategy on Spot, and execution on
external signals is the engine's own.

**What the futures contract changed in the shared model reaches Python in
three places,** and nowhere else
([shared-model.md](shared-model.md)):

- **`ExchangeId` names every product line,** a venue's linear and inverse
  markets beside its Spot market.
- **`Category` is a market's family,** read from its product line and
  never assumed.
- **A venue's order ID is a `str`,** in a `ClosedTrade`.

**The Spot result is the one result there is.** `BacktestResult` and
`ClosedTrade` are the candle strategy's, with the fields and the meanings
they have always had, and are not versioned. No authoritative execution
result is bound: when one is, it is a type of its own with a format
number of its own, as the engine's stored records have, and it does not
change these two. The frames of a result are views of these two as well,
under their fields' own names: none is a frame of futures execution
([a result's trades](python-pandas.md#a-results-trades)).

**No credential crosses.** These are values: none holds an exchange key, a
database password or a connection, and building one opens nothing. A
`CandleReader`, which is no value, is the one object that opens a store, and
what it reads carries nothing of it
([credentials](python-reader.md#credentials)).
