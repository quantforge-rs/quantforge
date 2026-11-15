# The Python types

`quantforge_py` holds the engine's types as Python classes: what a market
is, a candle and a batch of candles, and a backtest's settings and result.
Each class holds the engine's own value. It is built by the engine's own
rule and refused in the engine's own words, so that a value Python built
is a value the engine takes.

This page is what each type takes and gives. How the package is named,
built and tested is [the Python package](python-package.md).

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
| `decimal_to_f64` | `quantforge::decimal_to_f64` | The float nearest to a decimal |
| `decimal_from_f64` | `quantforge::decimal_from_f64` | The decimal of a float's shortest text |
<!-- end: bound types -->

A class cannot be derived from, and a value cannot be changed once it is
built: there is no setter, and nothing can be added to one.

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
- **A float is never read as a decimal.** It is a `TypeError` that names
  [`decimal_from_f64`](#decimal-and-float), the one way across.

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

## Signatures

Every way to build a value, and the one method there is, as Python's
`inspect.signature` gives it:

<!-- begin: signatures -->
```text
Symbol(value, /)
Interval(value, /)
Category(value, /)
ExchangeId(value, /)
MarketId(exchange, symbol, interval)
Candle(*, open_time_ms, close_time_ms, open, high, low, close, volume, trades=None)
CandleBatch.from_candles(candles, /)
CandleBatch.from_columns(*, open_time_ms, close_time_ms, open, high, low, close, volume, trades)
CandleBatch.to_candles()
BacktestConfig(*, initial_cash, fee_bps, close_out_at_end)
ClosedTrade(*, symbol, entry_time_ms, exit_time_ms, entry_price, exit_price, qty, gross_quote_pnl, entry_order_id=None, exit_order_id=None)
EquityPoint(*, open_time_ms, equity)
BacktestResult(*, initial_cash, final_equity, total_return_pct, trade_count, max_drawdown_pct, trades, equity_curve)
decimal_to_f64(value, /)
decimal_from_f64(value, /)
```
<!-- end: signatures -->

A type of one part takes it by position. A record takes its fields by
name, so that no price is taken for another, and every field is required
but the three that may be unknown.

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
candle strategy is a Spot strategy, and the story that binds the backtest
entry point refuses a linear or an inverse market there.

Every interval, family and product line is a constant of its class, and
`ALL` is a tuple of them in this order:

<!-- begin: constants -->
| Type | Constants | Their text |
| --- | --- | --- |
| `Interval` | `S1`, `M1`, `M3`, `M5`, `M15`, `M30`, `H1`, `H2`, `H4`, `H6`, `H8`, `H12`, `D1`, `D3`, `W1` | `1s`, `1m`, `3m`, `5m`, `15m`, `30m`, `1h`, `2h`, `4h`, `6h`, `8h`, `12h`, `1d`, `3d`, `1w` |
| `Category` | `SPOT`, `LINEAR`, `INVERSE` | `spot`, `linear`, `inverse` |
| `ExchangeId` | `BINANCE_SPOT`, `BINANCE_LINEAR`, `BINANCE_INVERSE`, `BYBIT_SPOT`, `BYBIT_LINEAR`, `BYBIT_INVERSE` | `binance_spot`, `binance_linear`, `binance_inverse`, `bybit_spot`, `bybit_linear`, `bybit_inverse` |
<!-- end: constants -->

An interval's constant is the engine's name for it, and the other two are
their text in capitals. The constants are listed from the engine when the
package is imported, so a product line the engine gains is a constant here
without a change to the package.

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
are exchanged in. It is built from candles or from eight columns, never
directly, and converts no value either way:

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

A batch holds candles and nothing about them: no market, and no check of
their order. NumPy arrays of its columns are bound by a later story.

## Backtests

These four are the candle strategy's backtest on Spot, as the engine has
it. The function that runs one is bound by a later story; until then a
result is built by hand, as the tests do.

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

## Decimal and float

The engine computes in exact decimals, and Python's numeric libraries in
floats. The two meet in two functions, the engine's own, which follow the
[decimal and float64 policy](decimal-float-policy.md). Nothing else in the
package turns one into the other.

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
  `Decimal(repr(value))`, with a negative zero made zero. It refuses NaN,
  the infinities, and a float whose shortest text does not fit the
  engine's decimal.

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
<!-- end: fields -->

A record's fields are the names it is built with, in that order. Every
time is milliseconds since the Unix epoch, UTC, and every amount of a
backtest is in the quote currency.

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
| `CandleBatch` | Every candle is, in order | no |
| `BacktestConfig` | All three settings are | no |
| `ClosedTrade` | Every field is | no |
| `EquityPoint` | Both fields are | no |
| `BacktestResult` | Every field is, trades and curve in order | no |
<!-- end: keys -->

**Decimals are compared by value:** a candle that opened at
`81940.01000000` equals one that opened at `81940.01`. Read a field to
tell the two apart.

**A value is equal only to a value of its own type.** A `Symbol` is not
equal to its text, and a `MarketId` is not equal to a tuple of its parts.

**The identity types are keys,** of a dict or a set, and the records are
not: `hash()` of a candle is a `TypeError`. The engine's records have no
hash either.

**`repr()` is an expression that builds the value again,** digit for
digit, wherever that is short enough to read. The two that hold
sequences count them instead: `CandleBatch(<1440 candles>)`, and a
result's `trades=<3 trades>, equity_curve=<1440 points>`.

**A value cannot be pickled or deep-copied yet.** No story has asked for
it; one that needs to send a value between processes adds it.

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
| `qf.BacktestConfig(initial_cash=0, fee_bps=10, close_out_at_end=True)` | `ValueError: invalid configuration: initial_cash must be greater than 0, got 0` |
| `qf.BacktestConfig(initial_cash=1, fee_bps=-1, close_out_at_end=True)` | `ValueError: invalid configuration: fee_bps must be zero or greater, got -1` |
| `qf.BacktestConfig(initial_cash=1, fee_bps=0, close_out_at_end=1)` | `TypeError: close_out_at_end must be a bool, not int` |
| `qf.ClosedTrade(symbol="BTCUSDT", entry_time_ms=0, exit_time_ms=1, entry_price=1, exit_price=1, qty=1, gross_quote_pnl=0, entry_order_id="has space")` | `ValueError: invalid venue order id "has space": 1 to 128 characters without spaces` |
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
change these two.

**No credential crosses.** These are values: none holds an exchange key, a
database password or a connection, and building one opens nothing.

Three more parts of the engine belong to the stories that follow:

| What | Story |
| --- | --- |
| `Strategy`, `StrategyContext` and `TargetPosition`: the strategy a Python class implements | QUA-464 |
| `run_backtest`, and its refusal of a market that is not Spot | QUA-470 |
| NumPy arrays of a batch's columns, and a batch's market kept with it | QUA-475 |
