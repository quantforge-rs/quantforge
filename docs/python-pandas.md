# Candles and results as pandas DataFrames

`candles_to_dataframe` gives a batch of candles as a pandas DataFrame, and
`dataframe_to_candles` builds a batch from one. `trades_to_dataframe` and
`equity_to_dataframe` give the closed trades and the equity curve of a
`BacktestResult`. This page is the contract of the four: what the index and
the columns of each frame are, what crosses exactly and what crosses as a
float, what is refused, and where a market stays.

pandas is one of the package's two extras, and the four are all that need
it: `pip install "quantforge-py[pandas]"` installs the package with it, and
with the NumPy a frame is made of ([pandas itself](#pandas-itself)).

It applies three pages and restates none. What a batch and a result are,
and every other type, is [the Python types](python-types.md). A frame of
candles is made of a batch's arrays and read back through them, which
[candles as NumPy arrays](python-numpy.md) is the contract of. How a decimal
becomes a float and a float a decimal is the
[decimal and float64 policy](decimal-float-policy.md).

```python
>>> from decimal import Decimal
>>> import pandas as pd
>>> import quantforge_py as qf
```

`qf`, `pd` and `Decimal` are what the examples use.

## Contents

- [The four functions](#the-four-functions)
- [A frame of candles](#a-frame-of-candles)
- [Times](#times)
- [A count the exchange did not report](#a-count-the-exchange-did-not-report)
- [A batch from a frame](#a-batch-from-a-frame)
- [What crosses exactly, and what as a float](#what-crosses-exactly-and-what-as-a-float)
- [The market stays with the batch](#the-market-stays-with-the-batch)
- [A result's trades](#a-results-trades)
- [A result's equity curve](#a-results-equity-curve)
- [A frame is for computing with](#a-frame-is-for-computing-with)
- [Refusals](#refusals)
- [pandas itself](#pandas-itself)
- [How long it takes](#how-long-it-takes)

## The four functions

As Python's `inspect.signature` gives them:

<!-- begin: signatures -->
```text
candles_to_dataframe(candles, /)
dataframe_to_candles(frame, /, *, market=None)
trades_to_dataframe(result, /)
equity_to_dataframe(result, /)
```
<!-- end: signatures -->

<!-- begin: functions -->
| Function | Takes | Gives |
| --- | --- | --- |
| `candles_to_dataframe` | A `CandleBatch` | A frame with one row for each candle, indexed by the time each bar opened |
| `dataframe_to_candles` | Such a frame, and by name alone the `MarketId` its candles are of, or nothing | A `CandleBatch` of the frame's rows |
| `trades_to_dataframe` | A `BacktestResult` | A frame with one row for each closed trade, numbered from 0 |
| `equity_to_dataframe` | A `BacktestResult` | A frame with one row for each point of the equity curve, indexed by the time each bar opened |
<!-- end: functions -->

- **A frame is new each time, and the caller's own.** Changing one changes
  nothing in the batch or the result it was made of, and the next call
  gives a frame of its own.
- **What a frame's index and columns are is stated in each function's
  docstring** as well, by name and by type, so `help(qf.candles_to_dataframe)`
  says what this page says.
- **The frames are the same on every pandas the extra takes.** Each column
  is made of the type stated here, and the names of the columns are text of
  Python's own, of the type `object`. Neither is what a version of pandas
  would choose, or what an option of pandas' says of text
  ([beside a frame pandas made](#a-frame-is-for-computing-with)).

## A frame of candles

**`candles_to_dataframe` gives one row for each candle,** in the batch's
order. Its index is the time each bar opened, and its seven columns are the
batch's other columns, in their order:

<!-- begin: candle columns -->
| Column | Type | What it is | To pandas | From pandas |
| --- | --- | --- | --- | --- |
| `open_time`, the index | `datetime64[ms, UTC]` | When the bar opened | Exact | Exact |
| `close_time` | `datetime64[ms, UTC]` | When the bar closed | Exact | Exact |
| `open` | `float64` | The first price of the bar, in the quote asset for one of the base asset | The float nearest to the decimal | The decimal of the float's shortest text |
| `high` | `float64` | The highest price of the bar | The float nearest to the decimal | The decimal of the float's shortest text |
| `low` | `float64` | The lowest price of the bar | The float nearest to the decimal | The decimal of the float's shortest text |
| `close` | `float64` | The last price of the bar | The float nearest to the decimal | The decimal of the float's shortest text |
| `volume` | `float64` | The volume traded in the bar, in the base asset | The float nearest to the decimal | The decimal of the float's shortest text |
| `trades` | `UInt64` | The exchange's trade count | Exact, and `<NA>` where the count is unknown | Exact, and unknown where it is `<NA>` |
<!-- end: candle columns -->

```python
>>> def minute(index, close, trades=None):
...     opened = 1_767_268_800_000 + index * 60_000
...     return qf.Candle(
...         open_time_ms=opened,
...         close_time_ms=opened + 59_999,
...         open=close,
...         high=close,
...         low=close,
...         close=close,
...         volume=Decimal("0.17235000"),
...         trades=trades,
...     )
...
>>> market = qf.MarketId("binance_spot", "BTCUSDT", "1m")
>>> candles = [minute(0, Decimal("81940.01000000"), 412), minute(1, Decimal("81955.25")), minute(2, 81900, 0)]
>>> batch = qf.CandleBatch.from_candles(candles, market=market)
>>> frame = qf.candles_to_dataframe(batch)
>>> frame.dtypes
close_time    datetime64[ms, UTC]
open                      float64
high                      float64
low                       float64
close                     float64
volume                    float64
trades                     UInt64
dtype: object
>>> frame[["close", "volume", "trades"]]
                              close   volume  trades
open_time
2026-01-01 12:00:00+00:00  81940.01  0.17235     412
2026-01-01 12:01:00+00:00  81955.25  0.17235    <NA>
2026-01-01 12:02:00+00:00  81900.00  0.17235       0
```

- **The index is a `DatetimeIndex` named `open_time`,** and the frame is
  a plain `pandas.DataFrame`.
- **A price and a volume are float64:** each decimal as the float nearest
  to it ([below](#what-crosses-exactly-and-what-as-a-float)).
- **A trade count is `UInt64`,** pandas' whole number that may be missing,
  and missing where the exchange reported none
  ([below](#a-count-the-exchange-did-not-report)).
- **Nothing is sorted and nothing is checked,** as nothing is in a batch: a
  batch out of order is a frame out of order, and two bars of one time are
  two rows under one time.
- **The frame holds no market.** It
  [stays with the batch](#the-market-stays-with-the-batch).

`candles_to_dataframe` takes a `CandleBatch` and nothing else: a list of
candles becomes a batch through `CandleBatch.from_candles`. An empty batch
gives a frame of no rows, with the same index and the same columns, each of
its type.

## Times

**A time is the engine's own integer, shown as a UTC time.** The engine
keeps a time as milliseconds since the Unix epoch, UTC, in 64 bits. In a
frame it is a `datetime64[ms, UTC]`: the same integer, which pandas shows
as a date and a time of day in UTC.

```python
>>> frame.index.dtype
datetime64[ms, UTC]
>>> frame.index[0]
Timestamp('2026-01-01 12:00:00+0000', tz='UTC')
>>> frame["close_time"].iloc[0] - frame.index[0]
Timedelta('0 days 00:00:59.999000')
>>> frame.index.astype("int64").tolist() == list(batch.open_time_ms)
True
```

- **Nothing is converted on the way.** The integers themselves are read as
  milliseconds, so no time is rounded, and none passes through a float.
- **The resolution is milliseconds on every pandas.** Asked to read the
  same integers itself, as `pandas.to_datetime(times, unit="ms")`, pandas
  gives nanoseconds before 3.0 and another unit from it on. A frame's times
  do not depend on that.
- **Every time the engine holds is a time in a frame, but one.** pandas
  keeps a time that is missing, `NaT`, as the lowest integer of 64 bits, so
  that integer is no time to it. A batch that holds it is refused by its
  column and its row, and is never given a missing time in its place.
- **`astype("int64")` gives the integers back,** as above: for a frame
  that `candles_to_dataframe` gave, they are the engine's milliseconds.

## A count the exchange did not report

**A trade count that is unknown is missing, `<NA>`.** `trades` is of
pandas' `UInt64` type, a whole number of 64 bits with a flag for every row
that says whether it is there.

```python
>>> frame["trades"].tolist()
[412, <NA>, 0]
>>> frame["trades"].isna().tolist()
[False, True, False]
>>> int(frame["trades"].sum()), int(frame["trades"].count())
(412, 2)
```

- **No number stands for it.** The policy says an unknown count is never
  written as NaN, 0 or -1, and it is not: a sum leaves it out, and what
  `fillna` puts in its place is the reader's own choice.
- **Zero is a count.** The third candle above traded nothing, and its count
  is 0 and not missing.
- **A count is never a float.** Every count up to 18446744073709551615 is
  the whole number it is. Cast to float64, a count above 2^53 is another
  number and a missing one is NaN, which `dataframe_to_candles` does not
  take back.

## A batch from a frame

**`dataframe_to_candles` builds the batch of a frame's rows,** each as
`candles_to_dataframe` gives it. Given its market again, a batch's own
frame builds the batch again where a float holds its decimals, as it holds
these:

```python
>>> qf.dataframe_to_candles(frame, market=market) == batch
True
```

A frame made elsewhere builds a batch the same way. Here the times are read
from text, in whatever unit pandas reads them, and no candle has a trade
count:

```python
>>> opened = pd.to_datetime(["2026-01-01 12:00", "2026-01-01 12:01"], utc=True)
>>> made = pd.DataFrame(
...     {
...         "close_time": opened + pd.Timedelta("59.999s"),
...         "open": [100.0, 100.5],
...         "high": [101.5, 102.0],
...         "low": [99.0, 100.0],
...         "close": [100.5, 101.0],
...         "volume": [12.5, 8.25],
...         "trades": pd.array([None, None], dtype="UInt64"),
...     },
...     index=opened.rename("open_time"),
... )
>>> elsewhere = qf.dataframe_to_candles(made)
>>> elsewhere.open_time_ms
(1767268800000, 1767268860000)
>>> elsewhere.close
(Decimal('100.5'), Decimal('101'))
>>> elsewhere.trades
(None, None)
```

What a frame has to be:

- **It is a `pandas.DataFrame` itself.** A frame of a type derived from it
  is refused: such a type may mean something its cells do not say, a unit
  or a geometry, and is read through its own code. `pandas.DataFrame(frame)`
  gives the plain frame of one, and whoever calls it has decided what its
  cells mean.
- **Its index is the time each bar opened:** a `DatetimeIndex` itself,
  named `open_time`. A frame whose times are a column beside another index
  is refused, and so is an index of any other name: the index is found by
  its name, as the columns are, and is never taken for the opening times
  because it holds times. An index of a type derived from `DatetimeIndex`
  is refused as a frame of a derived type is.
- **Its columns are found by name, in any order.** The seven of the table
  above have to be there, each once, each under a name that is text.
- **Any other column is left unread.** A frame that gained columns of its
  reader's own, an average or a signal, still gives its candles back.
- **Each column is of its type.** Nothing is cast: an int64 column is no
  column of prices, a float32 column is not widened, and pandas' own
  `Float64` is not float64.
- **`trades` is `UInt64`, or `uint64`.** A plain column of uint64 says that
  every count is known, and a `UInt64` one that those which are `<NA>` are
  not.
- **A time is in any time zone, and in any of pandas' units.** A time in
  another zone is the same instant, and is read as it; the batch that comes
  back holds the instant, and no zone. Seconds, milliseconds, microseconds
  and nanoseconds are all read, exactly.
- **A time without a zone is refused.** It is a date and a time of day, and
  no instant until it is said where: nothing takes it for UTC.
  `tz_localize("UTC")` says that it is.
- **A time that is no whole millisecond is refused, never rounded,** and so
  is one that is missing, `NaT`. In seconds, pandas holds times further
  off than 64 bits of milliseconds reach: such a time is refused too.

```python
>>> local = frame.tz_convert("Europe/Berlin").assign(average=frame["close"].rolling(2).mean().to_numpy())
>>> local.index[0]
Timestamp('2026-01-01 13:00:00+0100', tz='Europe/Berlin')
>>> qf.dataframe_to_candles(local, market=market) == batch
True
```

**The frame is read, and not kept.** The batch holds values of its own, so
changing the frame afterwards changes nothing in it.

**Nothing checks the candles,** as nothing checks a batch's: their order,
their spacing, and whether their prices make a bar are as the frame has
them.

**Everything is held to its type before a value is read:** the frame, its
index, each column in the order of the table, and `market` last. Then the
times are read, those of the index first, and then the floats and the
counts, column by column and row by row, as
[`from_numpy`](python-numpy.md#building-a-batch-from-arrays) reads them. So
the first value that is refused is the one a refusal names, by its column
and its row, `close[1]`, whatever it is refused for: of a time that is no
whole millisecond and a missing one after it, the first is named. A row is
counted from 0, whatever the frame's index says.

## What crosses exactly, and what as a float

**Times, trade counts, which counts are unknown, and the order of the
candles cross exactly,** both ways.

**A price and a volume cross as a float64, by the engine's own two
conversions** and by nothing else: a frame of candles is made of the arrays
`CandleBatch.to_numpy` gives, and read back through
`CandleBatch.from_numpy`. So everything
[candles as NumPy arrays](python-numpy.md#what-crosses-exactly-and-what-as-a-float)
says of a float holds of a frame's, cell for cell:

- **To pandas, a decimal becomes the float nearest to it.** Every decimal
  the engine holds converts, no float is NaN or infinite, and a zero is
  `+0.0`.
- **From pandas, a float becomes the decimal of its shortest text.** A
  float that has no such decimal is refused, never rounded to fit: NaN, the
  infinities, and a float beyond the engine's decimal.

```python
>>> def there_and_back(value):
...     """A price through a frame, and back."""
...     one = qf.CandleBatch.from_columns(
...         open_time_ms=[0], close_time_ms=[59_999], open=[1], high=[1], low=[1],
...         close=[value], volume=[1], trades=[None],
...     )
...     return qf.dataframe_to_candles(qf.candles_to_dataframe(one)).close[0]
...
>>> there_and_back(Decimal("81940.01000000"))
Decimal('81940.01')
```

<!-- begin: there and back -->
| A decimal | Comes back as | Because |
| --- | --- | --- |
| `Decimal("81940.01")` | `Decimal("81940.01")` | A decimal of at most 15 significant digits comes back the same value |
| `Decimal("81940.01000000")` | `Decimal("81940.01")` | The same value. The decimal places an exchange wrote are no part of a float, and are not restored |
| `Decimal("0.00000000")` | `Decimal("0")` | Zero is zero, whatever places it was written with |
| `Decimal("100")` | `Decimal("100")` | A whole number comes back with no decimal place |
| `Decimal("66633140290.84541427")` | `Decimal("66633140290.84541")` | 19 digits: the shortest text of the float nearest to them |
| `Decimal("79228162514264337593543950335")` | `ValueError: close[0]: cannot convert 7.922816251426434e28 to a decimal: beyond the decimal range` | The largest decimal is nearest to the float 2^96, which is beyond a decimal |
<!-- end: there and back -->

- **A batch that went to a frame comes back the same batch by value** where
  its decimals have at most 15 significant digits. A price or a volume with
  8 decimal places has that many when it has at most 7 digits before the
  point.
- **Its decimal places do not come back.** `81940.01000000` returns as
  `81940.01`, the same price. Compare decimals by value, never as text.
- **A NaN is no price.** pandas writes a value that is missing in a float
  column as NaN, as after a join that found no row. A frame that holds one
  among its prices is refused by its column and its row.

## The market stays with the batch

**A frame holds values, and the batch holds what they are of.** A batch
[names the market its candles are of](python-types.md#candles), or none,
and a frame names none: it has no column of a market, and nothing is kept
in its `attrs`.

```python
>>> batch.market
MarketId(ExchangeId('binance_spot'), Symbol('BTCUSDT'), Interval('1m'))
>>> frame.attrs
{}
>>> unnamed = qf.dataframe_to_candles(frame)
>>> unnamed.market is None
True
>>> unnamed == batch
False
```

- **A batch of a frame is of the market it is told,** by `market=`, and of
  none where it is told none. Nothing is read from the frame, and nothing
  is assumed for it.
- **Whoever built the frame says what it is of.** A frame worked out from
  a batch's is not always of the batch's market: five-minute bars made
  from one-minute ones are another series, with another `MarketId`.
- **That is why a frame's `attrs` do not hold it.** pandas copies a
  frame's `attrs` to the frames worked out from it, so a market kept there
  would name the one-minute series on the five-minute bars. The frame
  below is of the market its maker names, and of no other:

```python
>>> five = frame.resample("5min").agg(
...     {
...         "close_time": "last",
...         "open": "first",
...         "high": "max",
...         "low": "min",
...         "close": "last",
...         "volume": "sum",
...         "trades": lambda counts: counts.sum(skipna=False),
...     }
... )
>>> bars = qf.dataframe_to_candles(five, market=qf.MarketId("binance_spot", "BTCUSDT", "5m"))
>>> len(bars), bars.market.interval, bars.trades
(1, Interval('5m'), (None,))
```

The count of those five minutes is unknown, since one minute's is: pandas'
`sum` alone leaves a missing count out and would call the rest, 412, the
count of the bar. What a frame's reader works out of it is the reader's
own, and `skipna=False` is how pandas is told that a sum with a part
missing is missing.

- **The same values under two product lines are two batches, and one
  frame.** `BTCUSDT` on `binance_spot` and on `binance_linear` give equal
  frames when their candles are the same, and their batches are not equal.
- **A batch is never run on a market other than the one it names.**
  [`run_backtest`](python-types.md#running-a-backtest) takes a batch's own
  market where none is given beside it, refuses one that differs, and
  refuses to run candles that name none without one.

## A result's trades

**`trades_to_dataframe` gives one row for each closed trade of a
`BacktestResult`,** in the result's order, with every field of a
[`ClosedTrade`](python-types.md#fields) as a column: under the field's own
name, but for the two times, which are times in a frame and no integers.

<!-- begin: trade columns -->
| Column | Type | What it is |
| --- | --- | --- |
| `symbol` | `object` | The symbol traded, as text |
| `entry_time` | `datetime64[ms, UTC]` | When the entry filled: the field `entry_time_ms` |
| `exit_time` | `datetime64[ms, UTC]` | When the exit filled: the field `exit_time_ms` |
| `entry_price` | `float64` | The price the entry filled at, in the quote asset for one of the base asset |
| `exit_price` | `float64` | The price the exit filled at |
| `qty` | `float64` | The quantity traded, in the base asset |
| `gross_quote_pnl` | `float64` | The trade's PnL in the quote asset: in a backtest, net of both fees |
| `entry_order_id` | `object` | The venue's ID of the entry order, as text, and `None` where none reached a venue |
| `exit_order_id` | `object` | The venue's ID of the exit order, as text, and `None` where none reached a venue |
<!-- end: trade columns -->

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
>>> def bar(index, open, close):
...     opened = 1_767_268_800_000 + index * 60_000
...     return qf.Candle(
...         open_time_ms=opened,
...         close_time_ms=opened + 59_999,
...         open=open,
...         high=max(open, close),
...         low=min(open, close),
...         close=close,
...         volume=1,
...     )
...
>>> run = qf.CandleBatch.from_candles([bar(0, 100, 100), bar(1, 100, 110), bar(2, 110, 120), bar(3, 125, 125)], market=market)
>>> config = qf.BacktestConfig(initial_cash=1000, fee_bps=0, close_out_at_end=True)
>>> result = qf.run_backtest(run, EnterThenExit(), config)
>>> trades = qf.trades_to_dataframe(result)
>>> trades.dtypes
symbol                          object
entry_time         datetime64[ms, UTC]
exit_time          datetime64[ms, UTC]
entry_price                    float64
exit_price                     float64
qty                            float64
gross_quote_pnl                float64
entry_order_id                  object
exit_order_id                   object
dtype: object
>>> trades[["entry_time", "entry_price", "exit_price", "gross_quote_pnl"]]
                 entry_time  entry_price  exit_price  gross_quote_pnl
0 2026-01-01 12:01:00+00:00        100.0       125.0            250.0
>>> trades["entry_order_id"].tolist()
[None]
```

- **The index numbers the trades from 0,** a `RangeIndex` with no name: row
  `i` is `result.trades[i]`. A trade has two times and is of neither, so
  neither is its index; `trades.set_index("exit_time")` makes one of them
  so.
- **A time is exact, and an amount a float64,** as in a frame of candles:
  the engine's own milliseconds, and the float nearest to each decimal.
- **Text is Python's own on every pandas.** `symbol` and the two order IDs
  are of the type `object`, holding a `str`, or `None` for an order ID
  where no order reached a venue, as in every backtest. What a column of
  text is by default changed with pandas 3, and a frame's does not follow
  it: a missing order ID is `None` on every pandas, and never NaN.
- **A trade names its symbol, and no product line.** These are a candle
  strategy's trades, which are Spot's
  ([running a backtest](python-types.md#running-a-backtest)).

**`gross_quote_pnl` is Spot's PnL field, under its own name, and keeps the
two meanings it has in the engine.** In a backtest it is the change in cash
over the trade, so both fees are already taken off; in live trading it is
the difference of the two prices times the quantity, before fees
([Backtests](python-types.md#backtests)). The frame renames nothing and
adds nothing to it: no column is a net PnL, a fee or a return, and none is
a futures PnL.

**There is no frame of futures execution.** The engine keeps what it did
on futures in its own ledger, with fees, funding and a realized PnL that is
net, and none of that is bound to Python
([what is not bound](python-types.md#what-is-not-bound)). When a result of
it is, its frame is a view of its own with a format number of its own, and
it does not change this one.

## A result's equity curve

**`equity_to_dataframe` gives one row for each point of a result's equity
curve,** which is one for every bar of the run, in order.

<!-- begin: equity columns -->
| Column | Type | What it is |
| --- | --- | --- |
| `open_time`, the index | `datetime64[ms, UTC]` | The open time of the bar, which names it |
| `equity` | `float64` | Cash plus the position valued at the bar's close, after the bar's fill, in the quote currency |
<!-- end: equity columns -->

```python
>>> equity = qf.equity_to_dataframe(result)
>>> equity
                           equity
open_time
2026-01-01 12:00:00+00:00  1000.0
2026-01-01 12:01:00+00:00  1100.0
2026-01-01 12:02:00+00:00  1200.0
2026-01-01 12:03:00+00:00  1250.0
>>> equity.index.equals(qf.candles_to_dataframe(run).index)
True
```

- **Its index is the index of the candles' frame.** A point is named by
  the open time of its bar, so the curve of a run and the frame of the
  candles it ran on have one index, and `candles.join(equity)` puts each
  bar beside the equity at its close.
- **The curve leaves out the close-out fee,** which the result's
  `final_equity` includes ([Fields](python-types.md#fields)): the frame is
  the curve as the result holds it.

## A frame is for computing with

**No frame is what is stored, hashed or compared for parity.** A float has
lost the decimal places an exchange wrote, and holds a decimal of more than
15 digits only nearly
([what is stored is never a float](python-numpy.md#what-is-stored-is-never-a-float)).

- **The exact values stay where they were:** a batch's decimals in the
  batch, and a result's in the result, each a `decimal.Decimal` with every
  digit.
- **A result is compared as the text of its decimals,** never as a frame:
  the goldens hold a run to its trades, its final equity and its drawdown
  digit for digit ([golden-parity.md](golden-parity.md)), and a frame of
  the same run holds the floats nearest to them.
- **A sum over a frame is a sum of floats.**
  `trades["gross_quote_pnl"].sum()` is near the exact total, and need not
  be it. `sum(trade.gross_quote_pnl for trade in result.trades)` adds the
  decimals, and is.

The package's tests take the goldens' frozen day of candles to a frame and
back, where it is the same batch by value and its backtest the frozen
result by value, and hold the frames of that result to Python's own reading
of the frozen file's text, cell for cell.

**A frame equals the one pandas makes of the same cells,** name for name
and cell for cell. From pandas 3 on the two still differ in one thing a
comparison may look at: the type of the names of their columns, which
pandas makes text of its own there, and which is `object` in the package's
frames on every pandas.

```python
>>> own = pd.DataFrame({name: frame[name] for name in frame.columns})
>>> frame.equals(own)
True
>>> pd.testing.assert_frame_equal(frame, own, check_column_type=False)
```

- **`equals` compares the names by what they say,** and a column is found
  by its name whichever type the names are of.
- **`pandas.testing.assert_frame_equal` compares their type too,** and
  from pandas 3 on takes the two for equal only where it is told not to,
  with `check_column_type=False`.
- **A step of pandas' that makes the names anew gives them back as pandas'
  own,** `rename` among them: the names of a frame worked out from one of
  the package's are what pandas makes of them.

## Refusals

One refusal of each kind, as it is written and as it is refused. `candles`,
`batch`, `frame`, `market` and `result` are those of the examples above:

<!-- begin: refusals -->
| Written | Refused with |
| --- | --- |
| `qf.candles_to_dataframe(candles)` | `TypeError: candles must be a CandleBatch, not list` |
| `qf.candles_to_dataframe(qf.CandleBatch.from_columns(open_time_ms=[-2**63], close_time_ms=[0], open=[1], high=[1], low=[1], close=[1], volume=[1], trades=[None]))` | `ValueError: invalid open_time_ms[0] -9223372036854775808: pandas reads it as no time at all (NaT)` |
| `qf.dataframe_to_candles(batch)` | `TypeError: frame must be a pandas.DataFrame, not CandleBatch` |
| `qf.dataframe_to_candles(frame.reset_index())` | `TypeError: the index must be a pandas.DatetimeIndex named open_time, not RangeIndex` |
| `qf.dataframe_to_candles(frame.rename_axis("time"))` | `ValueError: the index must be named open_time, not 'time'` |
| `qf.dataframe_to_candles(frame.tz_localize(None))` | `TypeError: open_time must be an index of datetime64 with a time zone, not of datetime64[ms]: a time without a zone is never read as an instant; name its zone with tz_localize` |
| `qf.dataframe_to_candles(frame.drop(columns="volume"))` | `ValueError: frame has no column named volume` |
| `qf.dataframe_to_candles(pd.concat([frame, frame[["close"]]], axis=1))` | `ValueError: frame has 2 columns named close` |
| `qf.dataframe_to_candles(frame.assign(close_time=list(batch.close_time_ms)))` | `TypeError: close_time must be a column of datetime64 with a time zone, not of int64` |
| `qf.dataframe_to_candles(frame.astype({"close": "float32"}))` | `TypeError: close must be a column of float64, not of float32` |
| `qf.dataframe_to_candles(frame.assign(close=[81940, 81955, 81900]))` | `TypeError: close must be a column of float64, not of int64` |
| `qf.dataframe_to_candles(frame.astype({"trades": "float64"}))` | `TypeError: trades must be a column of UInt64 or of uint64, not of float64` |
| `qf.dataframe_to_candles(frame, market="binance_spot")` | `TypeError: market must be a MarketId or None, not str` |
| `qf.dataframe_to_candles(frame.set_axis((frame.index.as_unit("us") + pd.Timedelta(1, "us")).as_unit("us"), axis=0))` | `ValueError: invalid open_time[0] 2026-01-01T12:00:00.000001 UTC: must be a whole number of milliseconds` |
| `qf.dataframe_to_candles(frame.assign(close_time=frame["close_time"].where(frame["close"] > 81900)))` | `ValueError: invalid close_time[2] NaT: a candle's time is never missing` |
| `qf.dataframe_to_candles(frame.set_axis(pd.DatetimeIndex(pd.Series([2**62] * 3).to_numpy().view("datetime64[s]"), name="open_time").tz_localize("UTC"), axis=0))` | `ValueError: invalid open_time[0] 146138514283-06-19T07:45:04 UTC: its milliseconds must fit 64 bits` |
| `qf.dataframe_to_candles(frame.assign(close=[81940.01, float("nan"), 81900.0]))` | `ValueError: close[1]: cannot convert NaN to a decimal: not finite` |
| `qf.dataframe_to_candles(frame.assign(volume=[1e29, 1.0, 1.0]))` | `ValueError: volume[0]: cannot convert 1e29 to a decimal: beyond the decimal range` |
| `qf.trades_to_dataframe(result.trades)` | `TypeError: result must be a BacktestResult, not tuple` |
| `qf.equity_to_dataframe(batch)` | `TypeError: result must be a BacktestResult, not CandleBatch` |
<!-- end: refusals -->

**A frame is refused with one of two exceptions,** as an argument is
everywhere in the package: a `TypeError` for a frame, an index or a column
of a type that is not taken, and a `ValueError` for one of the right type
that is not taken, in the engine's own words where it has a rule for it. A
value is named by its column and its row, `close[1]`, and a time of a frame
is written in UTC. Whatever else a function raises is of no frame: an
`ImportError` where there is no pandas or no NumPy for it
([pandas itself](#pandas-itself)), and a `MemoryError`.

## pandas itself

**pandas is an extra of the package, and no requirement of it.** The
package installs, imports and runs a backtest without pandas, and the four
functions of this page are all that need one. `pip` installs it where the
package is asked for with the extra, and the NumPy the arrays need with it:

<!-- begin: pandas -->
| What | Which |
| --- | --- |
| The package with its extra, as `pip` is asked for it | `quantforge-py[pandas]` |
| What the extra requires | `numpy>=2.0,<3`, `pandas>=2.2.2,<4` |
| The oldest pandas the tests run on | `2.2.2` |
<!-- end: pandas -->

- **pandas is imported when one of the four is first called,** and not
  when the package is: `import quantforge_py`, building a batch, a batch's
  arrays and a backtest import none of it.
- **Where Python finds no pandas, each of the four says what to install,**
  and the rest of the package works as it does with one. The error is a
  `ModuleNotFoundError`, with Python's own as its cause.
- **A pandas the extra does not take is refused as well,** with an
  `ImportError`, before anything else is asked of it: one older than 2.2.2,
  and one from 4 on. Nothing but `pip` holds an environment to the extra's
  bounds, and only where it was asked for the extra. A version is read as
  `pip` reads the bounds, as a NumPy's is
  ([NumPy itself](python-numpy.md#numpy-itself)).
- **2.2.2 is the first pandas that runs beside NumPy 2,** which the arrays
  a frame is made of need.
- **The extra stops before pandas 4 because a frame's columns are stated
  type by type.** pandas 3 changed what text is by default, in a column
  and in the names of a frame's columns, which unit it reads a time in, and
  how a frame shares its memory. The frames here are the same before and
  after it, each of those being asked for by name, and the tests hold them
  to that on both. A pandas 4 may change as much again, and there is none
  to test on: the bound is raised once there is one and the tests run on
  it.
- **A frame needs NumPy too, and asks for it as the arrays do.** A NumPy
  the `numpy` extra does not take is refused in the arrays' own words, and
  a pandas that is taken beside it changes nothing in that: pandas 2.2 runs
  on NumPy 1.26, and the arrays do not.

<!-- begin: without pandas -->
| Where | Each of the four raises |
| --- | --- |
| Python finds no pandas to import | `ModuleNotFoundError: a DataFrame needs pandas 2.2.2 or later and before 4, and Python finds none to import: pip install "quantforge-py[pandas]"` |
| The pandas it finds is older than the extra takes, here 2.1.4 | `ImportError: a DataFrame needs pandas 2.2.2 or later and before 4, and this is pandas 2.1.4: pip install "quantforge-py[pandas]"` |
| The pandas it finds is newer than the extra takes, here 4.0.0 | `ImportError: a DataFrame needs pandas 2.2.2 or later and before 4, and this is pandas 4.0.0: pip install "quantforge-py[pandas]"` |
| The pandas is taken, and the NumPy beside it is older than the arrays take, here 1.26.4 | `ImportError: to_numpy and from_numpy need NumPy 2.0 or later and before 3, and this is NumPy 1.26.4: pip install "quantforge-py[numpy]"` |
<!-- end: without pandas -->

- **pandas is asked for first, then NumPy, and only then is an argument
  looked at.** So a call that could not give a frame says what is missing
  before it says anything of what it was handed.
- **A pandas that fails as it is imported raises its own error,** which
  says what is wrong with it. So does a module pandas looks for and does
  not find: only `pandas` itself, not found, is pandas missing.
- **What imports as `pandas` and states no version is refused as well,**
  with an `ImportError` that shows where it was found: where pandas is not
  installed, a directory of that name on Python's path imports as a module
  with nothing in it.
- **The tests run on the oldest pandas the extra takes and on the newest,**
  each on a Python it has a wheel for and beside the NumPy of the same age;
  **once with NumPy and no pandas,** as the `numpy` extra alone leaves an
  environment; **and once with neither,** on the package installed without
  its extras ([python-package.md](python-package.md#testing-it)).
- **What a strategy computes with a frame depends on the pandas and the
  NumPy it runs on.** The frames do not, and are the same in every process.
  Float arithmetic can differ between two versions of either, which is why
  the policy has a remote worker pin what a strategy was run with locally
  ([parity](decimal-float-policy.md#parity)).

## How long it takes

**A frame of candles costs what its arrays cost, and little beside.**
`candles_to_dataframe` is `to_numpy` and the frame pandas makes of the
eight arrays, and `dataframe_to_candles` is `from_numpy` over the frame's
own columns ([how long the arrays take](python-numpy.md#how-long-it-takes)).

**A result's columns are made in the extension,** each amount converted
there and written straight into its array, and not row by row in Python:
a curve has a point for every bar of a run.

No test holds a run to a figure. The pull request that added the four
functions records what they took on the machine that built it.
