# The goldens

**One frozen day of candles, and what QuantForge's Rust SMA cross made of
it, digit for digit.** A strategy written in Python and run through the
same engine has to make the same trades, the same final equity and the same
drawdown, each compared as the text of its decimal: with no tolerance, and
never as a float. That is what the Python package promises, and these
files are what it is held to.

They are in [`tests/fixtures/golden/`](../tests/fixtures/golden/):

<!-- begin: files -->
| File | What it holds |
| --- | --- |
| `candles.csv` | The dataset: one day of one-minute candles |
| `sma_cross.json` | What the Rust `SmaCrossStrategy` made of them: its decision on every bar, every trade, the equity at every bar, the final equity, the return and the drawdown |
| `sma_cross.py` | The same strategy written in Python, which is held to those results |
<!-- end: files -->

**The two data files are frozen.** They were written once, from the Rust
strategy, and are never edited or written again:
[the goldens are frozen](#the-goldens-are-frozen).

## Contents

- [The run](#the-run)
- [The result](#the-result)
- [The dataset](#the-dataset)
- [What the dataset is made to show](#what-the-dataset-is-made-to-show)
- [The strategy in Python](#the-strategy-in-python)
- [Decimals in the strategy](#decimals-in-the-strategy)
- [The two files](#the-two-files)
- [Where the results come from](#where-the-results-come-from)
- [The goldens are frozen](#the-goldens-are-frozen)
- [What holds them](#what-holds-them)
- [Running it yourself](#running-it-yourself)
- [What comes next](#what-comes-next)

## The run

One backtest, at the defaults of the `quantforge backtest` command:

<!-- begin: run -->
| What | Value |
| --- | --- |
| Market | `binance_spot` `BTCUSDT` `1m` |
| Strategy | `sma_cross` |
| `fast` | `20` |
| `slow` | `50` |
| `initial_cash` | `10000` |
| `fee_bps` | `10` |
| `close_out_at_end` | `true` |
<!-- end: run -->

The strategy is long while the average of the last 20 closes is above that
of the last 50, and flat while it is below. The run is the engine's own,
`BacktestEngine::run`, under the execution model
[`src/engine/backtest.rs`](../src/engine/backtest.rs) states: a decision
made at a bar's close fills at the next bar's open, every fill pays the
fee, and a position still open after the last bar is sold at that bar's
close.

## The result

<!-- begin: result -->
| Figure | Value |
| --- | --- |
| `initial_cash` | `10000` |
| `final_equity` | `9921.202457428483046329087897` |
| `total_return_pct` | `-0.7879754257151695367091210300` |
| `trade_count` | `15` |
| `max_drawdown_pct` | `1.8091109103945370264444945700` |
<!-- end: result -->

Every trade, in order. A bar is named by its place among the candles,
counted from zero:

<!-- begin: trades -->
| Trade | Entry bar | Exit bar | Entry price | Exit price | Quantity | PnL |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | 50 | 55 | `82659.21000000` | `82565.63000000` | `0.1208577966086294559310451913` | `-31.278582714048888961521202` |
| 2 | 128 | 155 | `81969.80000000` | `81792.07000000` | `0.1214930700652108433989985527` | `-41.488895688609872856601173` |
| 3 | 194 | 270 | `81874.38000000` | `82341.45000000` | `0.1211284312185441927974780834` | `36.684250500094290118000202` |
| 4 | 346 | 453 | `81846.53000000` | `82154.47000000` | `0.1216174077176900154584312721` | `17.505488049476583135071139` |
| 5 | 530 | 569 | `81656.50000000` | `81525.58000000` | `0.1221145996869517373007421444` | `-35.914157766299854799761850` |
| 6 | 641 | 711 | `81137.69000000` | `81215.63000000` | `0.1224532338775082393292198369` | `-10.336684016336943809134019` |
| 7 | 746 | 810 | `81266.49000000` | `81487.75000000` | `0.1221320887882826393707474833` | `7.145430674945954911191503` |
| 8 | 815 | 818 | `81487.76000000` | `81487.75000000` | `0.1218880536730258215603929777` | `-19.865986590805486770189645` |
| 9 | 820 | 850 | `81487.76000000` | `81487.75000000` | `0.1216445061703044909993547175` | `-19.826291876865224320820238` |
| 10 | 855 | 856 | `81487.76000000` | `81487.63000000` | `0.1214014453058081083454414183` | `-19.801230083167500776845476` |
| 11 | 926 | 960 | `81355.01000000` | `81092.88000000` | `0.1213563908368800491954741441` | `-51.525240359537865470510609` |
| 12 | 992 | 1077 | `81393.47000000` | `82190.83000000` | `0.1206666409910443657279672473` | `76.475584860747836620298452` |
| 13 | 1164 | 1237 | `82195.57000000` | `82379.07000000` | `0.1204186041800113146536336729` | `2.278965434804218933893293` |
| 14 | 1299 | 1348 | `82245.42000000` | `82263.44000000` | `0.1203732985730569650586974021` | `-17.633347282406741526508415` |
| 15 | 1421 | 1439, at its close | `81851.46000000` | `82253.96000000` | `0.1207374525313979114060644115` | `28.783154286492541902525935` |
<!-- end: trades -->

A fill is at its bar's open, but for the last one: the fifteenth position
was still open after the last bar, and the run closed it out at that bar's
close. A trade's PnL is the change in cash over the trade, both fees taken
off.

**The run ends four fifths of a percent down.** Its trades made about 219
on the price, and thirty fills at ten basis points cost about 298. The
files pin what the engine computes, and say nothing for the strategy.

`sma_cross.json` holds the same figures, and with them the equity at each
of the 1,440 bars and the strategy's decision on each.

## The dataset

<!-- begin: dataset -->
| What | Value |
| --- | --- |
| Candles | `1440` |
| First open | `2026-01-01T12:00:00Z`, which is `1767268800000` |
| Last open | `2026-01-02T11:59:00Z`, which is `1767355140000` |
| On the first of the two days, UTC | `720` |
| Still hours | Bars `392` to `451`, and `1098` to `1157` |
| Quiet stretch | Bars `766` to `855` |
<!-- end: dataset -->

**No exchange's data is in it.** Every number comes from a generator
written out in [`tests/golden/dataset.rs`](../tests/golden/dataset.rs), so
the candles can be made again from that file alone, with no clock and no
random source. The crate's tests make them again on every run and hold the
file to them.

**The candles are shaped as Binance writes a Spot kline,** since that is
what a store holds after `data sync`:

- a price is a whole number of cents, the market's tick, written with
  eight decimal places: `82499.72000000`;
- a volume has eight decimal places, and a trade count is on every bar;
- a bar in which nothing traded stands at the close before it, with a
  volume of `0.00000000` and a trade count of `0`.

**The decimal places are part of the data.** A price read back as
`82499.72` is the same number written otherwise, and the engine hands a
price on as it is written: in the bar a strategy is handed, and as a
trade's entry or exit price. A store or a reader that dropped the trailing
zeros would show in the thirty prices the goldens' trades copy. It would
not show in the figures the engine computes, which come out the same on
this dataset, digit for digit.

**The price rises and falls by turns under noise of its own,** between
80,800 and 82,900. A turn lasts 20 to 79 bars and moves the price by 6.00
to 17.99 a bar; a bar opens within 1.00 of the close before it, closes its
turn's slope away from its open give or take 30.00, and has wicks of less
than 15.00.

**Three times the market does something else,** each for a reason
[the next section](#what-the-dataset-is-made-to-show) gives:

- **Twice it stands still for an hour.** Nothing trades, and every bar
  stands at the close before the hour.
- **Once it is quiet for an hour and a half.** Every bar opens where the
  one before it closed, and closes at one of two prices a tick apart, on
  little volume.

**It runs from noon to noon, across two days,** and that is deliberate. A
store keeps one-minute candles in windows of a UTC day
([segment-spec.md](segment-spec.md#windows)), so the first 720 candles are
a window a store packs into a segment, and the last 720 the bars it has
not packed yet. Whoever loads the dataset through a store reads both.

## What the dataset is made to show

The dataset is chosen so that a strategy which differs from the Rust one in
a small way does not reproduce it:

- **Nothing is decided before the slow average is full.** The price rises
  at first, and on bar 49, the first the slow average is full at, the fast
  one is still above it: the first decision is to go long there, and the
  first fill is at bar 50. A strategy that averaged the closes it had so
  far would decide to go long on bar 20.
- **The bar a decision is made on counts.** A strategy that decided from
  the bars before the one it is handed is one bar late throughout.
- **Equal averages decide nothing.** From the 49th bar of a still hour the
  slow average holds that one price alone, the two averages are equal, and
  the strategy keeps the position it has, for 12 bars each time: long
  through the first hour, and flat through the second. A strategy that took
  equal averages for a reason to be long, or to be flat, makes a sixteenth
  trade.
- **Averages a hair apart do decide.** From the 49th bar of the quiet
  stretch both averages hold its two prices alone, and for 42 bars they are
  never half a cent apart. The strategy is long or flat there by a few
  ten-thousandths, the least of them a hundredth of a tick, and makes three
  trades on them. A strategy that rounded its averages to the cent, the
  market's tick, finds them equal where they are not, and makes two trades
  fewer.
- **Averages can be equal where their bars are not.** On 8 of those bars
  the two averages are equal, and not because every bar is: each window
  holds both prices, in the same proportion. A float does not always find
  them equal. A strategy that averages floats trades differently if it
  keeps running sums, if it adds its closes up anew with Python's `sum`,
  and if it sums them exactly with `math.fsum`.
- **The decision on every bar is recorded, and compared.** One float
  strategy makes the goldens' trades all the same: taking each sum as the
  difference of two running totals, it decides to be long on 18 bars where
  the averages are equal, and is long already on every one of them. Its
  result is the goldens', and its decisions are not.
- **The run ends in a position,** so the close-out, its fee and the last
  point of the equity curve are part of the result.

The Python package's tests run each of those other strategies and show
that it does not reproduce the goldens. Python's own `sum` of floats is
caught on every version, at another bar: up to 3.11 it adds one float after
another, and already finds the averages of a still hour apart; from 3.12 on
it corrects its sum, and is caught in the quiet stretch.

## The strategy in Python

[`tests/fixtures/golden/sma_cross.py`](../tests/fixtures/golden/sma_cross.py),
in full:

<!-- begin: strategy -->
```python
"""The SMA crossover, written in Python: the strategy QuantForge's goldens
hold (docs/golden-parity.md).

It is QuantForge's built-in Rust `SmaCrossStrategy`, rule for rule. The Python
package's tests run it through `run_backtest` over the frozen candles beside
this file, `candles.csv`, and hold it to what the Rust strategy made of them,
`sma_cross.json`: every decision, every trade, the final equity and the
drawdown, digit for digit.

One file with one class, as a strategy bundle carries one
(`entry = "sma_cross.py:SmaCross"`); its two windows are the bundle's
parameters.
"""

from collections import deque

import quantforge_py as qf


class SmaCross(qf.Strategy):
    """Long while the fast average of closes is above the slow one, and flat
    while it is below.

    It compares the two averages on every bar, not their crossing: while the
    fast one stays above the slow one it asks for the long position again,
    which an engine takes as nothing to do. Equal averages decide nothing, so
    the position held is kept. Nothing is decided before `slow` bars have
    closed.

    The averages are sums and divisions of `Decimal`s, in the decimal context
    of the thread the run is on. At Python's default precision the sums of an
    exchange's prices are exact, and two averages of them that differ at all
    differ by far more than a division rounds away, so the comparison comes
    out as it does in the engine's own decimal.
    """

    name = "sma_cross"

    def __init__(self, fast: int, slow: int):
        if type(fast) is not int or type(slow) is not int:
            raise TypeError("fast and slow windows must be whole numbers of bars")
        if fast < 1 or slow < 1:
            raise ValueError("fast and slow windows must be greater than zero")
        if fast >= slow:
            raise ValueError("fast window must be smaller than slow window")
        self.fast, self.slow = fast, slow
        # The closes of the last `slow` bars, the newest last.
        self.closes = deque(maxlen=slow)

    def on_bar(self, ctx, bar):
        self.closes.append(bar.close)
        if len(self.closes) < self.slow:
            return None
        closes = list(self.closes)
        fast = sum(closes[-self.fast :]) / self.fast
        slow = sum(closes) / self.slow
        if fast > slow:
            return qf.TargetPosition.LONG_ALL_IN
        if fast < slow:
            return qf.TargetPosition.FLAT
        return None
```
<!-- end: strategy -->

It is a strategy as anyone writes one: a class derived from `Strategy`
that defines `on_bar` ([python-types.md](python-types.md#strategies)). It
is one file that needs nothing but the package, and takes its two windows
by name, which is how a [strategy bundle](bundle-spec.md) carries a
strategy and hands it its parameters.

## Decimals in the strategy

**The engine computes every figure of a result, whichever language the
strategy is written in.** A strategy only decides, so two strategies that
decide alike on every bar bring the same result, digit for digit. What has
to match between the Rust strategy and the Python one is the comparison of
two averages, on each of 1,440 bars.

**The two compute an average in different arithmetic:**

| Strategy | An average is | Rounded to |
| --- | --- | --- |
| Rust | A division in the engine's decimal, `rust_decimal` | The 28 or 29 significant digits a 96-bit decimal holds, half to even |
| Python | A division of `decimal.Decimal`s, in the thread's decimal context | 28 significant digits, half to even, unless the context was changed |

**The quotients can differ in their last digit, and the comparison
cannot.** An exchange's prices have few digits, so their sums are exact in
both arithmetics. Two averages of such prices that differ at all differ by
at least one tick over the product of the two windows: `0.01 / (20 × 50)`,
a hundred-thousandth of a unit, and the closest two of this dataset are a
ten-thousandth apart. At these prices a division at 28 digits is right to
one part in 10²³ of a unit. Equal averages are equal in both arithmetics,
since each rounds the same exact value.

With windows of 20 and 50 no average is rounded at all: both divide a sum
of cents evenly. The tests therefore run both strategies at windows that do
not, 7 and 30, and 3 and 13, and hold each to what exact arithmetic
decides on every bar.

**A run neither sets nor reads the thread's decimal context**
([python-types.md](python-types.md#strategies)), so the context a strategy
computes in is one of its inputs:

- **What it has to hold is the sum of fifty prices,** which has nine
  digits. From nine digits up the Python strategy's result is the goldens',
  and with these windows nothing it computes is rounded at all. Python's
  own context has 28.
- **At eight digits every price is still held, and their sum is not.** The
  two sums are rounded apart, the strategy finds one average below the
  other where the two are equal, and it trades differently from the first
  still hour on. A strategy that needs a precision sets it itself.
- **How a context rounds has no say.** At the windows whose averages are
  rounded, 7 and 30, and 3 and 13, the comparison comes out as exact
  arithmetic's in each of Python's eight rounding modes, at 11 digits and
  at 28.
- **The engine's own figures follow no context.** They are computed in the
  engine's decimal and handed over as they are: the tests read the goldens
  back under a context of three digits that refuses to round at all.

## The two files

**`candles.csv`** is text with a line for the columns and then a line for
every candle, each ended by a line feed:

<!-- begin: columns -->
| Column | What it holds |
| --- | --- |
| `open_time_ms` | When the bar opened, in milliseconds since the Unix epoch, UTC |
| `close_time_ms` | When it closed: 59,999 milliseconds later |
| `open` | The first price of the bar |
| `high` | The highest |
| `low` | The lowest |
| `close` | The last |
| `volume` | The volume traded, in the base asset |
| `trades` | The trade count. Empty where an exchange reported none, which no line of this file is |
<!-- end: columns -->

These are the fields of the engine's `Candle`, under its names. A decimal
is written with every digit and decimal place it has, and no field is
quoted.

**`sma_cross.json`** is one JSON document, format 1:

<!-- begin: fields -->
| Field | What it holds |
| --- | --- |
| `format` | `1` |
| `about` | What the file is, for whoever opens it |
| `oracle.strategy` | The strategy that ran |
| `oracle.engine` | The engine it ran in, by its version |
| `oracle.commit` | The commit whose engine and strategy ran |
| `oracle.decimal` | The decimal arithmetic of that build |
| `oracle.written_on` | The day the file was written |
| `dataset.file` | The dataset's file, beside this one |
| `dataset.sha256` | The SHA-256 of that file: the results are of those candles and no others |
| `dataset.market` | The market the run was on: `exchange`, the product line; `symbol`; and `interval` |
| `dataset.candles` | How many candles |
| `dataset.first_open_ms` | The first candle's open time |
| `dataset.last_open_ms` | The last candle's open time |
| `dataset.first_open` | The first, as RFC 3339 |
| `dataset.last_open` | The last, as RFC 3339 |
| `strategy.name` | The strategy's name |
| `strategy.params` | Its parameters: `fast` and `slow` |
| `config.initial_cash` | The cash the run started with |
| `config.fee_bps` | The fee on every fill, in basis points |
| `config.close_out_at_end` | Whether a position left open was closed out |
| `result` | The engine's `BacktestResult`, field for field: `initial_cash`, `final_equity`, `total_return_pct`, `trade_count`, `max_drawdown_pct`, `trades` and `equity_curve` |
| `decisions` | What the strategy decided on each bar, in the engine's names: `"LongAllIn"`, `"Flat"`, or `null` where it decided nothing |
<!-- end: fields -->

**An amount is a string that holds the text of its decimal,** never a JSON
number: `"9921.202457428483046329087897"`. A time is a whole number of
milliseconds. `result` is written as
[`tests/fixtures/strategy_cases.json`](../tests/fixtures/strategy_cases.json)
writes one: a trade has the fields of the engine's `ClosedTrade`, its two
order IDs `null` since no order reached an exchange, and a point of the
curve has `open_time_ms` and `equity`. `decisions` and `equity_curve` have
one entry for every candle, in the candles' order.

## Where the results come from

<!-- begin: oracle -->
| What | Value |
| --- | --- |
| Strategy | `quantforge::SmaCrossStrategy` |
| Engine | `quantforge 0.2.0` |
| Commit | `1d38db875ce55953a361af55cd3db076aedcbeb5` |
| Decimal arithmetic | `rust_decimal 1.40.0` |
| Written on | `2026-11-16` |
<!-- end: oracle -->

**The Rust strategy is a one-time oracle.** QuantForge's built-in
strategies are removed before 0.3.0, when Python becomes the one way to
write a candle strategy (QUA-459). The results were written from the
built-in SMA cross while it exists, and stand on their own once it is
gone.

Until then [`tests/golden/oracle.rs`](../tests/golden/oracle.rs) shows on
every run that the file is that strategy's output and nothing else: the
document it makes from the strategy and the engine, as they are in the
build under test, is the file byte for byte. That module is deleted with
the built-in strategies, and the files are not.

## The goldens are frozen

<!-- begin: frozen -->
| File | SHA-256 |
| --- | --- |
| `candles.csv` | `67053dbf4618e1a8155bbecad1d2709a429af180bff1c6381d3d365e4c6f8b6c` |
| `sma_cross.json` | `ea2945bc520657a4a4444e47b573151b3e53eaa3afc346096f657f7002266978` |
<!-- end: frozen -->

**The two files are never edited and never written again.** Nothing in
the repository writes them: no command, no script and no variable,
`UPDATE_SNAPSHOTS` included. The tests pin the second hash, and the second
file names the first.

**A golden test that fails is a regression in the code.** The engine, what
crosses the strategy boundary, or the strategy written in Python now
computes something else, and that is what the files exist to catch. The
fix is in the code, never in the files.

**They are not regenerated for a change that should not move a result:**
another store, another venue, the market's identity, a new version of the
decimal library, a refactoring. [ADR 0002](adr/0002-automation-architecture.md)
holds its own changes to that, and
[shared-model.md](shared-model.md#not-changing) lists the goldens among
what does not change.

**Replacing them takes the maintainer's explicit decision,** as reducing
the [parity matrix](parity/README.md#keeping-the-matrix) does. It is
recorded on this page with its reason, and the pinned hash changes in the
same commit, where a review sees it. Once the built-in strategy is gone no
Rust strategy is left to write new results from, so the decision would
also have to say what the new ones are held to.

## What holds them

<!-- begin: held by -->
| Test | Runs in | What it holds |
| --- | --- | --- |
| `tests/golden/oracle.rs` | `checks`, on Linux, macOS and Windows | The Rust `SmaCrossStrategy` to `sma_cross.json`, byte for byte, and its averages to exact arithmetic. Deleted with the built-in strategies |
| `tests/golden/main.rs` | `checks` | The engine alone: the recorded decisions, played back, give the recorded result, on each Spot product line. And the pinned hash |
| `tests/golden/dataset.rs` | `checks` | The generator to `candles.csv`, and the dataset to what this page says of it |
| `tests/golden/docs.rs` | `checks` | This page to the files |
| `quantforge-py/tests/test_golden_parity.py` | `python` | The strategy written in Python, through `run_backtest` and as a host that embeds Python runs one: every decision, and the result digit for digit. The strategies that differ, the decimal contexts, and this page's example |
<!-- end: held by -->

Both checks are required of every pull request ([ci.md](ci.md#required-checks)).
The crate's tests need no Python, so the engine is held to the goldens on
every platform the crate is tested on; the Python package's run on Linux,
on the oldest Python it supports and the newest.

## Running it yourself

With the package installed ([python-package.md](python-package.md#building-it)),
from the repository's root:

```python
>>> import csv, json, sys
>>> from decimal import Decimal
>>> import quantforge_py as qf
>>> golden = "tests/fixtures/golden/"
>>> sys.path.insert(0, golden)
>>> from sma_cross import SmaCross
>>> with open(golden + "candles.csv", newline="") as file:
...     candles = [
...         qf.Candle(
...             open_time_ms=int(row["open_time_ms"]),
...             close_time_ms=int(row["close_time_ms"]),
...             trades=int(row["trades"]) if row["trades"] else None,
...             **{name: Decimal(row[name]) for name in ("open", "high", "low", "close", "volume")},
...         )
...         for row in csv.DictReader(file)
...     ]
...
>>> with open(golden + "sma_cross.json") as file:
...     expected = json.load(file)["result"]
...
>>> result = qf.run_backtest(
...     candles,
...     SmaCross(fast=20, slow=50),
...     qf.BacktestConfig(initial_cash=10000, fee_bps=10, close_out_at_end=True),
...     market=qf.MarketId("binance_spot", "BTCUSDT", "1m"),
... )
>>> def text(amount):
...     return format(amount, "f")
...
>>> text(result.final_equity)
'9921.202457428483046329087897'
>>> text(result.final_equity) == expected["final_equity"]
True
>>> text(result.max_drawdown_pct)
'1.8091109103945370264444945700'
>>> result.trade_count
15
>>> [text(trade.gross_quote_pnl) for trade in result.trades] == [
...     trade["gross_quote_pnl"] for trade in expected["trades"]
... ]
True
>>> [text(point.equity) for point in result.equity_curve] == [
...     point["equity"] for point in expected["equity_curve"]
... ]
True
```

`format(amount, "f")` writes a `Decimal` with every digit it has and no
exponent, which is the text the engine writes. `str()` is not that for every
amount: it writes a volume of `0.00000000` as `0E-8`.

## What comes next

The same files, held to more of QuantForge:

| What | Story |
| --- | --- |
| The dataset loaded through SQLite and through PostgreSQL, and the strategy run locally and as a server's job, each held to these results | QUA-458 |
| The strategy as a bundle, run by a server's worker | QUA-353 |
| The strategy run from the command line | QUA-461 |
| The Rust built-in strategies removed, and `tests/golden/oracle.rs` with them | QUA-459 |

The [parity matrix](parity/README.md) names these files, as ALG-383, among
the evidence for `strategy.backtest_local`: the frozen goldens reproduced
from candles stored for each venue in each store, from the command line
and from Python. This page's tests are the part of it that needs no store.
