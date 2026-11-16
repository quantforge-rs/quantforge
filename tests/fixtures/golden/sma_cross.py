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
