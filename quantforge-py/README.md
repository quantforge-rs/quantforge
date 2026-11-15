# quantforge-py

[QuantForge](https://github.com/quantforge-rs/quantforge) for Python:
strategies written in Python, run on the same deterministic Rust engine as
the `quantforge` command line.

**Not released yet.** The package is being built in the open. So far it
says what it is, and holds the engine's types, each built by the engine's
own rule:

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

The package installs as `quantforge-py` and imports as `quantforge_py`.
`quantforge` on PyPI is another project.

One wheel per platform, built against Python's stable ABI, loads on CPython
3.11 and every later version.

[The Python package](https://github.com/quantforge-rs/quantforge/blob/main/docs/python-package.md)
says how it is named, built and tested, and
[the Python types](https://github.com/quantforge-rs/quantforge/blob/main/docs/python-types.md)
what each type takes and gives.
