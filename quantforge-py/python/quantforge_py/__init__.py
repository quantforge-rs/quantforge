"""QuantForge for Python: strategies written in Python, run on the same
deterministic Rust engine as the ``quantforge`` command line.

The package says what it is:

- ``__version__``: the package's own version.
- ``ENGINE_VERSION``: the version of the engine it was built with, which a
  strategy bundle's ``engine`` requirement is held to.
- ``STORAGE_BACKENDS``: the stores this build can open, ``"sqlite"`` and,
  unless it is the SQLite-only build, ``"postgres"``.

And it holds the engine's types, each built by the engine's own rule and
refused in the engine's own words:

- ``Symbol``, ``Interval``, ``Category``, ``ExchangeId`` and ``MarketId``:
  what a market is.
- ``Candle`` and ``CandleBatch``: one bar, and bars held column by column.
- ``BacktestConfig``, ``ClosedTrade``, ``EquityPoint`` and
  ``BacktestResult``: a backtest's settings and what one produces.
- ``Strategy``, ``StrategyContext`` and ``TargetPosition``: the strategy a
  class implements, what its callbacks are handed, and the position it asks
  for.
- ``run_backtest``: runs a strategy over candles in the engine, and gives
  the result.
- ``decimal_to_f64`` and ``decimal_from_f64``: the one crossing between an
  exact decimal and a float.

A price, a quantity or an amount is a ``decimal.Decimal`` on both sides of
every one of them, and never a float.

The names come from the compiled extension module, ``quantforge_py._native``,
which is private: import them from here.
"""

from ._native import (
    ENGINE_VERSION,
    STORAGE_BACKENDS,
    BacktestConfig,
    BacktestResult,
    Candle,
    CandleBatch,
    Category,
    ClosedTrade,
    EquityPoint,
    ExchangeId,
    Interval,
    MarketId,
    Strategy,
    StrategyContext,
    Symbol,
    TargetPosition,
    __version__,
    decimal_from_f64,
    decimal_to_f64,
    run_backtest,
)

__all__ = [
    "__version__",
    "ENGINE_VERSION",
    "STORAGE_BACKENDS",
    "Symbol",
    "Interval",
    "Category",
    "ExchangeId",
    "MarketId",
    "Candle",
    "CandleBatch",
    "BacktestConfig",
    "ClosedTrade",
    "EquityPoint",
    "BacktestResult",
    "TargetPosition",
    "StrategyContext",
    "Strategy",
    "run_backtest",
    "decimal_to_f64",
    "decimal_from_f64",
]
