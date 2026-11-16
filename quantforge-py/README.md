# quantforge-py

[QuantForge](https://github.com/quantforge-rs/quantforge) for Python:
strategies written in Python, run on the same deterministic Rust engine as
the `quantforge` command line.

**Not released yet.** The package is being built in the open. So far it
says what it is, holds the engine's types, each built by the engine's own
rule, has the class a strategy derives from, and runs a backtest:

```python
from decimal import Decimal

import quantforge_py as qf

qf.__version__       # the package's own version
qf.ENGINE_VERSION    # the engine it was built with
qf.STORAGE_BACKENDS  # ("sqlite", "postgres")

market = qf.MarketId("binance_spot", "btcusdt", "1m")
market.symbol        # Symbol('BTCUSDT')
market.category      # Category('spot')

candle = qf.Candle(
    open_time_ms=1_700_000_000_000,
    close_time_ms=1_700_000_059_999,
    open=Decimal("81940.01000000"),
    high=Decimal("81990.5"),
    low=Decimal("81900"),
    close=Decimal("81955.25"),
    volume=Decimal("0.17235000"),
)
candle.open          # Decimal('81940.01000000'): exact, never a float

qf.BacktestConfig(initial_cash=Decimal("10000"), fee_bps=10, close_out_at_end=True)
```

A price is a `decimal.Decimal` on both sides, and a value the engine would
refuse is refused as it is built, in the engine's words:
`qf.Interval("7m")` raises `ValueError: invalid interval: 7m`.

A strategy is a class derived from `qf.Strategy` that defines `on_bar`,
which the engine calls once for every closed candle:

```python
class EnterThenExit(qf.Strategy):
    """Goes long on the first bar and flat on the third."""

    def __init__(self):
        self.bars = 0

    def on_bar(self, ctx, bar):
        self.bars += 1
        if self.bars == 1:
            return qf.TargetPosition.LONG_ALL_IN
        if self.bars == 3:
            return qf.TargetPosition.FLAT
        return None  # leave the position as it is
```

`ctx` is the engine's state at the bar: the market, the time, the cash and
the position. `run_backtest` runs a strategy over candles in the engine,
and gives the result:

```python
def minute(index, open, close):
    return qf.Candle(
        open_time_ms=index * 60_000,
        close_time_ms=index * 60_000 + 59_999,
        open=open,
        high=max(open, close),
        low=min(open, close),
        close=close,
        volume=1,
    )


candles = [minute(0, 100, 100), minute(1, 100, 110), minute(2, 110, 120), minute(3, 125, 125)]
config = qf.BacktestConfig(initial_cash=1000, fee_bps=0, close_out_at_end=True)

result = qf.run_backtest(candles, EnterThenExit(), config, market=market)
result.final_equity           # Decimal('1250')
result.total_return_pct       # Decimal('25.00')
result.trades[0].entry_price  # Decimal('100'): the open of the bar after the decision
result.trades[0].exit_price   # Decimal('125')
```

A decision made at a bar's close is filled at the next bar's open, a price
the strategy had not seen when it decided. The same candles and config give
the same result on every run, for a strategy that starts each run in the
same state and decides from its bars and that state alone, with no clock
and no randomness. A strategy keeps its state from one run to the next, so
each run is given one built anew, the same way. `market` says whose candles
they are, and is a Spot market: a candle strategy does not run on a futures
series. An exception a strategy raises ends the run as itself, with its
traceback.

The result is exact, and held to be: an SMA cross written in Python makes
the trades, the final equity and the drawdown of the engine's own Rust one
over a frozen day of candles, digit for digit, on every pull request
([the goldens](https://github.com/quantforge-rs/quantforge/blob/main/docs/golden-parity.md)).

The package installs as `quantforge-py` and imports as `quantforge_py`.
`quantforge` on PyPI is another project.

One wheel per platform, built against Python's stable ABI, loads on CPython
3.11 and every later version.

[The Python package](https://github.com/quantforge-rs/quantforge/blob/main/docs/python-package.md)
says how it is named, built and tested, and
[the Python types](https://github.com/quantforge-rs/quantforge/blob/main/docs/python-types.md)
what each type takes and gives.
