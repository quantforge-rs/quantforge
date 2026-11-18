# Candles as NumPy arrays

`CandleBatch.to_numpy()` gives a batch's columns as NumPy arrays, and
`CandleBatch.from_numpy(...)` builds a batch from arrays. This page is the
contract of the two: what each array is, what crosses exactly and what
crosses as a float, what is refused, and where a batch's market stays.

NumPy is the package's one extra, and the two are all that need it:
`pip install "quantforge-py[numpy]"` installs the package with it
([NumPy itself](#numpy-itself)).

It applies two pages and restates neither. What a batch is, and every other
type, is [the Python types](python-types.md). How a decimal becomes a float
and a float a decimal is the
[decimal and float64 policy](decimal-float-policy.md).

```python
>>> from decimal import Decimal
>>> import numpy as np
>>> import quantforge_py as qf
```

`qf`, `np` and `Decimal` are what the examples use.

## Contents

- [The arrays](#the-arrays)
- [A count the exchange did not report](#a-count-the-exchange-did-not-report)
- [Building a batch from arrays](#building-a-batch-from-arrays)
- [What crosses exactly, and what as a float](#what-crosses-exactly-and-what-as-a-float)
- [What is stored is never a float](#what-is-stored-is-never-a-float)
- [The market stays with the batch](#the-market-stays-with-the-batch)
- [Refusals](#refusals)
- [NumPy itself](#numpy-itself)
- [How long it takes](#how-long-it-takes)

## The arrays

**`to_numpy()` gives a dict of eight arrays,** one for each column, by the
column's name and in the columns' order. **Every array has one dimension
and one cell for each candle,** so its shape is `(len(batch),)`, and row
`i` of every array belongs to the same candle.

<!-- begin: columns -->
| Column | dtype | Array | To NumPy | From NumPy |
| --- | --- | --- | --- | --- |
| `open_time_ms` | `int64` | `numpy.ndarray` | Exact | Exact |
| `close_time_ms` | `int64` | `numpy.ndarray` | Exact | Exact |
| `open` | `float64` | `numpy.ndarray` | The float nearest to the decimal | The decimal of the float's shortest text |
| `high` | `float64` | `numpy.ndarray` | The float nearest to the decimal | The decimal of the float's shortest text |
| `low` | `float64` | `numpy.ndarray` | The float nearest to the decimal | The decimal of the float's shortest text |
| `close` | `float64` | `numpy.ndarray` | The float nearest to the decimal | The decimal of the float's shortest text |
| `volume` | `float64` | `numpy.ndarray` | The float nearest to the decimal | The decimal of the float's shortest text |
| `trades` | `uint64` | `numpy.ma.MaskedArray` | Exact, and masked where the count is unknown | Exact, and unknown where it is masked |
<!-- end: columns -->

```python
>>> def minute(index, close, trades=None):
...     return qf.Candle(
...         open_time_ms=index * 60_000,
...         close_time_ms=index * 60_000 + 59_999,
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
>>> arrays = batch.to_numpy()
>>> list(arrays)
['open_time_ms', 'close_time_ms', 'open', 'high', 'low', 'close', 'volume', 'trades']
>>> arrays["close"].dtype, arrays["close"].shape
(dtype('float64'), (3,))
>>> arrays["close"].tolist()
[81940.01, 81955.25, 81900.0]
>>> arrays["open_time_ms"].tolist()
[0, 60000, 120000]
>>> arrays["trades"].tolist()
[412, None, 0]
```

- **A time is the engine's own integer,** milliseconds since the Unix
  epoch, UTC, in 64 bits. It is not NumPy's `datetime64`.
- **A price and a volume are float64:** each decimal as the float nearest
  to it ([below](#what-crosses-exactly-and-what-as-a-float)).
- **A trade count is uint64, in a masked array,** masked where the exchange
  reported none
  ([below](#a-count-the-exchange-did-not-report)).
- **Each array is new, and the caller's own.** NumPy allocates it. Its
  cells lie one after another, in this machine's byte order, and can be
  written. Changing one changes nothing in the batch, and the next call
  gives arrays of its own.
- **The arrays hold no market.** It
  [stays with the batch](#the-market-stays-with-the-batch).

`to_numpy()` takes no argument, and an empty batch gives eight arrays of
no cells, each of its type.

## A count the exchange did not report

**A trade count that is unknown is a masked entry.** `trades` is a
`numpy.ma.MaskedArray`, NumPy's own way of saying that a value is missing.
Its mask has one flag for every row, `True` where the exchange reported no
count, and it has them where every count is known as well.

```python
>>> trades = arrays["trades"]
>>> trades.mask.tolist()
[False, True, False]
>>> trades[1] is np.ma.masked
True
>>> int(trades.sum()), int(trades.count())
(412, 2)
```

- **No number stands for it.** The policy says an unknown count is never
  written as NaN, 0 or -1, and it is not: `tolist()` gives `None` for it,
  a sum leaves it out, and what `filled()` puts in its place is the
  reader's own choice.
- **Zero is a count.** The third candle above traded nothing, and its count
  is 0 and not masked.
- **The cell under a mask holds zero, and is no count.** A cell has to hold
  something, and it is the same on every run. Code that reads
  `trades.data`, or calls `numpy.asarray(trades)`, reads the cells without
  their mask and takes that zero for a count: read the mask with them.

## Building a batch from arrays

**`from_numpy` takes the eight arrays by name,** each as `to_numpy` gives
it. Given its market again, a batch's own arrays build the batch again
where a float holds its decimals, as it holds these:

```python
>>> qf.CandleBatch.from_numpy(**arrays, market=market) == batch
True
```

Arrays made elsewhere build a batch the same way. Here the prices are the
columns of one table, and no candle has a trade count:

```python
>>> table = np.array([[100.0, 101.5, 99.0, 100.5], [100.5, 102.0, 100.0, 101.0]])
>>> opened = np.array([0, 60_000], dtype=np.int64)
>>> made = qf.CandleBatch.from_numpy(
...     open_time_ms=opened,
...     close_time_ms=opened + 59_999,
...     open=table[:, 0],
...     high=table[:, 1],
...     low=table[:, 2],
...     close=table[:, 3],
...     volume=np.array([12.5, 8.25]),
...     trades=np.ma.masked_all(2, dtype=np.uint64),
... )
>>> made.close
(Decimal('100.5'), Decimal('101'))
>>> made.trades
(None, None)
```

What an array has to be:

- **It is a `numpy.ndarray` itself,** and `trades` that or a
  `numpy.ma.MaskedArray` itself, made over a plain array. An array of a
  type derived from either is refused, and so is a masked array made over
  one: its type may mean something its cells do not say, a mask or a unit,
  which reading the cells would drop without a word. That is why a masked
  array is no column of prices. `numpy.asarray` gives the plain array of
  one, and whoever calls it has decided what its cells mean.
- **Its cells are of the column's type,** in this machine's byte order.
  Nothing is cast: an int64 array is no column of prices, a float32 array
  is not widened, and NumPy's `datetime64` is no time.
- **It has one dimension.** A column of a table is such an array, and the
  table is not.
- **Its cells may lie anywhere in memory:** a column of a table, every
  other cell, backwards, a field of a record, memory that cannot be
  written. Where they lie one after another on their alignment they are
  read where they are, and no copy is made. Otherwise NumPy copies the
  array first, and the copy is read.
- **All eight are as long as `open_time_ms`.** The first that is not is
  refused in the engine's words.
- **A plain array of counts says that every count is known,** and a masked
  one that the masked ones are not. Whatever lies under a mask is never
  read as a count.

**The arrays are read, and not kept.** The batch holds values of its own,
so changing an array afterwards changes nothing in it, and it keeps no
array alive.

**An array is held to its type again as its cells are read.** NumPy lets
an array's type and shape be changed where it stands, and other code can
run between the moment an array is taken and the moment it is read,
another thread's among it. An array that is no longer what it was taken
for is refused for what it is, and its cells are never read past their
end. So it is with the mask of masked counts, which is read through
nothing that was made of it earlier: one resized where it stands is
refused for its length.

**Nothing of Python's runs while an array is being read.** It is borrowed
last, once whatever reading it takes has run, and the wording of a refusal
waits until it has been given back. So nothing changes what an array is
under the read, its type, its shape or where its memory lies, and one
that is changed again while its refusal is worded is refused all the
same. Its cells are NumPy's memory still: code that has let the
interpreter go, as NumPy's own loops do in another thread, can write them
while they are read, as it can under any reader.

That is as far as anyone who reads NumPy's memory can hold it. An array
that is a view of another reads the other's memory, as every masked array
does, and NumPy itself reads that memory where it was after the other is
resized by whoever asks NumPy not to look for its views
(`resize(refcheck=False)`).

**Every argument is held to its type before anything else is refused,**
each in the order written, `market` last: what it is, what its cells are,
and that it has one dimension. Then the lengths are compared. Only then is
a value read, column by column in the columns' order and row by row, so the
first float that has no decimal is the one a refusal names.

## What crosses exactly, and what as a float

**Times, trade counts, which counts are unknown, and the order of the
candles cross exactly,** both ways. An integer is never a float on the way.

**A price and a volume cross as a float64, by the engine's own two
conversions** and by nothing else, which the
[policy](decimal-float-policy.md) defines and the package publishes as
[`decimal_to_f64` and `decimal_from_f64`](python-types.md#decimal-and-float):

- **To NumPy, a decimal becomes the float nearest to it.** Every decimal
  the engine holds converts, no float is NaN or infinite, and a zero is
  `+0.0`. It is Python's `float(str(d))`.
- **From NumPy, a float becomes the decimal of its shortest text,** the
  text that reads back as the same float, with the even last digit where
  two such texts are equally near. It is the number Python's
  `Decimal(repr(x))` is, with no decimal place where the float is whole:
  `100.0` becomes `100`. A negative zero becomes zero.
- **A float that has no such decimal is refused, never rounded to fit:**
  NaN, the infinities, a float beyond ±79228162514264337593543950335, and
  one whose shortest text needs more than 28 decimal places.

So a batch that crosses to NumPy and back is the same batch where a float
holds its decimals, and another where it does not:

```python
>>> def there_and_back(value):
...     """A price through a float64 array, and back."""
...     one = qf.CandleBatch.from_columns(
...         open_time_ms=[0], close_time_ms=[59_999], open=[1], high=[1], low=[1],
...         close=[value], volume=[1], trades=[None],
...     )
...     return qf.CandleBatch.from_numpy(**one.to_numpy()).close[0]
...
>>> there_and_back(Decimal("81940.01000000"))
Decimal('81940.01')
```

<!-- begin: there and back -->
| A decimal | Comes back as | Because |
| --- | --- | --- |
| `Decimal("81940.01")` | `Decimal("81940.01")` | A decimal of at most 15 significant digits comes back the same value |
| `Decimal("81940.01000000")` | `Decimal("81940.01")` | The same value. The decimal places an exchange wrote are no part of a float, and are not restored |
| `Decimal("0.17235000")` | `Decimal("0.17235")` | The same again: 8 decimal places and at most 7 digits before them are 15 digits |
| `Decimal("0.00000000")` | `Decimal("0")` | Zero is zero, whatever places it was written with |
| `Decimal("100")` | `Decimal("100")` | A whole number comes back with no decimal place |
| `Decimal("999999999999999.9")` | `Decimal("999999999999999.9")` | Many decimals of 16 digits come back too, and none is promised to |
| `Decimal("9007199254740993")` | `Decimal("9007199254740992")` | 2^53 + 1 has 16 digits, and no float is that number |
| `Decimal("66633140290.84541427")` | `Decimal("66633140290.84541")` | 19 digits: the shortest text of the float nearest to them |
| `Decimal("0.0000000000000000000000000001")` | `Decimal("0.0000000000000000000000000001")` | The smallest decimal there is has one digit |
| `Decimal("79228162514264333195497439231")` | `Decimal("79228162514264330000000000000")` | The largest decimal whose nearest float is below 2^96: it comes back, as that float's shortest text |
| `Decimal("79228162514264337593543950335")` | `ValueError: close[0]: cannot convert 7.922816251426434e28 to a decimal: beyond the decimal range` | The largest decimal is nearest to the float 2^96, which is beyond a decimal |
<!-- end: there and back -->

- **A decimal of at most 15 significant digits comes back the same
  value.** A price or a volume with 8 decimal places has that many when it
  has at most 7 digits before the point.
- **Its decimal places do not come back.** A float has none of its own:
  `81940.01000000` returns as `81940.01`, the same price. Compare decimals
  by value, never as text.
- **A decimal of more digits comes back as its nearest float's shortest
  text,** which is another decimal, close to it and not it.
- **A float comes back the same float, bit for bit,** wherever it has a
  decimal at all, so one more crossing changes nothing. A negative zero
  comes back `0.0`.

## What is stored is never a float

**A batch that came back from NumPy is not the batch a store holds.** Its
prices are the same numbers, where a float holds them, and are no longer
written as the exchange wrote them.

- **A store keeps every decimal as its exact text, with its places**
  ([storage-schema.md](storage-schema.md)), and gives it back unchanged.
- **A segment's hash is taken over that text**
  ([segment-spec.md](segment-spec.md)): `81940.01000000` and `81940.01`
  are one price and two segments.
- **The goldens are compared as the text of their decimals**
  ([golden-parity.md](golden-parity.md)).

So floats are for computing with. A float array is never what is stored,
hashed or compared for parity, and nothing in the package writes a candle
to a store. The package's tests take the goldens' frozen day of candles to
NumPy and back: it is the same batch by value, a backtest of it is the
frozen result by value, and a price a trade copied from a candle is written
`82659.21` where the frozen file has `82659.21000000`.

## The market stays with the batch

**The arrays hold values, and the batch holds what they are of.** A batch
[names the market its candles are of](python-types.md#candles), or none,
and `to_numpy()` gives no market: a NumPy array has nowhere to keep one.

```python
>>> batch.market
MarketId(ExchangeId('binance_spot'), Symbol('BTCUSDT'), Interval('1m'))
>>> "market" in arrays
False
>>> unnamed = qf.CandleBatch.from_numpy(**arrays)
>>> unnamed.market is None
True
>>> unnamed == batch
False
```

- **A batch of arrays is of the market it is told,** by `market=`, and of
  none where it is told none. Nothing is read from the arrays, and nothing
  is assumed for them.
- **Whoever built the arrays says what they are of.** Arrays worked out
  from a batch's are not always of the batch's market: five-minute bars
  made from one-minute ones are another series, with another `MarketId`.
- **The same values under two product lines are two batches.** `BTCUSDT`
  on `binance_spot` and on `binance_linear` give the same arrays cell for
  cell when their candles are the same, and their batches are not equal.
- **A batch is never run on a market other than the one it names.**
  [`run_backtest`](python-types.md#running-a-backtest) takes a batch's own
  market where none is given beside it, and refuses one that differs, so a
  linear series is not run as its Spot namesake.

## Refusals

One refusal of each kind, as it is written and as it is refused. `arrays`
is the batch's own, from the examples above:

<!-- begin: refusals -->
| Written | Refused with |
| --- | --- |
| `qf.CandleBatch.from_numpy(**{**arrays, "close": [81940.01, 81955.25, 81900.0]})` | `TypeError: close must be a numpy.ndarray of float64, not list` |
| `qf.CandleBatch.from_numpy(**{**arrays, "close": arrays["close"].astype("float32")})` | `TypeError: close must be a numpy.ndarray of float64, not of float32` |
| `qf.CandleBatch.from_numpy(**{**arrays, "close": arrays["open_time_ms"]})` | `TypeError: close must be a numpy.ndarray of float64, not of int64` |
| `qf.CandleBatch.from_numpy(**{**arrays, "close": arrays["close"].astype(">f8")})` | `TypeError: close must be a numpy.ndarray of float64, not of >f8` |
| `qf.CandleBatch.from_numpy(**{**arrays, "open_time_ms": arrays["open_time_ms"].astype("datetime64[ms]")})` | `TypeError: open_time_ms must be a numpy.ndarray of int64, not of datetime64[ms]` |
| `qf.CandleBatch.from_numpy(**{**arrays, "close": np.ma.masked_invalid(arrays["close"])})` | `TypeError: close must be a numpy.ndarray of float64, not MaskedArray` |
| `qf.CandleBatch.from_numpy(**{**arrays, "close": arrays["close"].reshape(3, 1)})` | `ValueError: close must have one dimension, not 2` |
| `qf.CandleBatch.from_numpy(**{**arrays, "close": arrays["close"][:2]})` | ``ValueError: candle column `close` has 2 rows, expected 3`` |
| `qf.CandleBatch.from_numpy(**{**arrays, "close": np.array([81940.01, float("nan"), 81900.0])})` | `ValueError: close[1]: cannot convert NaN to a decimal: not finite` |
| `qf.CandleBatch.from_numpy(**{**arrays, "volume": np.array([1e29, 1.0, 1.0])})` | `ValueError: volume[0]: cannot convert 1e29 to a decimal: beyond the decimal range` |
| `qf.CandleBatch.from_numpy(**{**arrays, "volume": np.array([1.0, 1.0, 1e-29])})` | `ValueError: volume[2]: cannot convert 1e-29 to a decimal: more than 28 decimal places` |
| `qf.CandleBatch.from_numpy(**{**arrays, "trades": [412, None, 0]})` | `TypeError: trades must be a numpy.ndarray or a numpy.ma.MaskedArray of uint64, not list` |
| `qf.CandleBatch.from_numpy(**{**arrays, "trades": arrays["trades"].astype("int64")})` | `TypeError: trades must be a numpy.ndarray or a numpy.ma.MaskedArray of uint64, not of int64` |
| `qf.CandleBatch.from_numpy(**{**arrays, "trades": arrays["trades"].astype("float64").filled(np.nan)})` | `TypeError: trades must be a numpy.ndarray or a numpy.ma.MaskedArray of uint64, not of float64` |
| `qf.CandleBatch.from_numpy(**{**arrays, "trades": np.ma.MaskedArray(np.zeros(3, dtype=np.uint64).view(np.recarray))})` | `TypeError: trades must be a numpy.ndarray or a numpy.ma.MaskedArray of uint64, not a MaskedArray over recarray` |
| `qf.CandleBatch.from_numpy(**arrays, market="binance_spot")` | `TypeError: market must be a MarketId or None, not str` |
<!-- end: refusals -->

**An array is refused with one of two exceptions,** as an argument is
everywhere in the package: a `TypeError` for an array of a type the column
does not take, and a `ValueError` for one of the right type that the engine
does not take, in the engine's own words where it has a rule for it. A
float is named by its column and its row, `close[1]`. Whatever else a
conversion raises is of no array: an `ImportError` where there is no NumPy
for it, or NumPy's own error where it fails as it is imported
([NumPy itself](#numpy-itself)), and a `MemoryError`.

**Memory that is not there is a `MemoryError`** in the two conversions:
NumPy's own where it cannot allocate an array, one that `to_numpy` gives or
the copy `from_numpy` has it make of a column that lies apart, and
`no memory for 2000000 rows of open_time_ms` where `from_numpy` cannot hold
a column of the batch. That is so of the two conversions, and not yet of
what takes a batch next: a run, `to_candles()` and a column read as a
tuple build what they give in the engine's own memory, and end the process
where there is none, as they did before a batch could be built from
arrays.

## NumPy itself

**NumPy is the package's one extra, and no requirement of it.** The
package installs, imports and runs a backtest without NumPy, and
`to_numpy` and `from_numpy` are all that need one. `pip` installs it where
the package is asked for with its extra:

<!-- begin: numpy -->
| What | Which |
| --- | --- |
| The package with its extra, as `pip` is asked for it | `quantforge-py[numpy]` |
| What the extra requires | `numpy>=2.0,<3` |
| The oldest NumPy the tests run on | `2.0.0` |
<!-- end: numpy -->

- **NumPy is imported when a conversion is first asked for,** and not when
  the package is: `import quantforge_py`, building a batch and running a
  backtest import none of it.
- **Where Python finds no NumPy, a conversion says what to install,** and
  the rest of the package works as it does with one. The error is a
  `ModuleNotFoundError`, with Python's own as its cause.
- **A NumPy the extra does not take is refused as well,** with an
  `ImportError`, before anything else is asked of it: one older than 2.0,
  and one from 3 on. Nothing but `pip` holds an environment to the extra's
  bounds, and only where it was asked for the extra: an environment that
  had another NumPy already is told so, and is never handed arrays of a
  NumPy the tests have not seen the like of. A version NumPy has stated is
  read as `pip` reads the bounds, so a pre-release of 2.0 itself is older
  than it, and a pre-release of 3 is not before it. Other text is read by
  the numbers it begins with.
- **The extra stops before NumPy 3 because the array bindings know NumPy
  2.** They know the module its arrays are kept in, the place of each of
  its functions in the table it hands out, and how it lays a type out, and
  they ask a NumPy only whether it is 2.0 or later. One that changed any
  of that would be read as a NumPy 2, and nothing would refuse it. There
  is no NumPy 3 to test on yet: the bound is raised once there is one and
  the tests run on it.

<!-- begin: without numpy -->
| Where | A conversion raises |
| --- | --- |
| Python finds no NumPy to import | `ModuleNotFoundError: to_numpy and from_numpy need NumPy 2.0 or later and before 3, and Python finds none to import: pip install "quantforge-py[numpy]"` |
| The NumPy it finds is older than the extra takes, here 1.26.4 | `ImportError: to_numpy and from_numpy need NumPy 2.0 or later and before 3, and this is NumPy 1.26.4: pip install "quantforge-py[numpy]"` |
| The NumPy it finds is newer than the extra takes, here 3.0.0 | `ImportError: to_numpy and from_numpy need NumPy 2.0 or later and before 3, and this is NumPy 3.0.0: pip install "quantforge-py[numpy]"` |
<!-- end: without numpy -->

- **A NumPy that fails as it is imported raises its own error,** which
  says what is wrong with it. So does a module NumPy looks for and does not
  find: only `numpy` itself, not found, is NumPy missing.
- **What imports as `numpy` and is not NumPy is refused as well,** with an
  `ImportError` that shows where it was found: a module that states no
  version, and one that states a version and does not hold NumPy's arrays.
  Where NumPy is not installed, a directory of that name on Python's path
  imports as a module with nothing in it, and a test may put anything in
  NumPy's place. That is so where NumPy itself was not used in the process
  before: once it was, its arrays are still found where they are, whatever
  stands under its name.
- **The tests run on the oldest NumPy the extra takes and on the newest,**
  each on a Python it has a wheel for, **and once with no NumPy at all,**
  on the package installed without its extra
  ([python-package.md](python-package.md#testing-it)). One extension serves
  every NumPy the tests run on, from 2.0.0 to the newest: it finds NumPy
  as it runs, and is not built against one. A NumPy 2 newer than the tests
  is taken as it comes, and a NumPy 3 is not.
- **What a strategy computes with its arrays depends on the NumPy it runs
  on.** The conversions do not, and are the same in every process. Float
  arithmetic can differ between two versions of NumPy, which is why the
  policy has a remote worker pin the NumPy a strategy was run with locally
  ([parity](decimal-float-policy.md#parity)).

## How long it takes

A conversion costs what its decimals cost: five floats for every candle one
way, and five decimals from their shortest text the other. Copies are
avoided where they can be.

- **`to_numpy`** writes every float and every count straight into the array
  NumPy allocated for it, and copies the two columns of times once.
- **`from_numpy`** reads an array where it lies when its cells lie one
  after another on their alignment, and copies the two columns of times
  once, into the batch.

`quantforge-py/benches/candle_arrays.py` times both on a million candles,
and beside them the same columns read as tuples of `Decimal` in Python:

```bash
cd quantforge-py
maturin develop --release --extras numpy
python benches/candle_arrays.py
```

Its figures are information, and no test holds a run to them. The pull
request that added the conversions records what they were on the machine
that built it.
