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
- ``Candle`` and ``CandleBatch``: one bar, and bars held column by column
  with the market they are of. ``CandleBatch.to_numpy`` gives the columns
  as NumPy arrays, and ``CandleBatch.from_numpy`` takes arrays back.
- ``BacktestConfig``, ``ClosedTrade``, ``EquityPoint`` and
  ``BacktestResult``: a backtest's settings and what one produces.
- ``Strategy``, ``StrategyContext`` and ``TargetPosition``: the strategy a
  class implements, what its callbacks are handed, and the position it asks
  for.
- ``run_backtest``: runs a strategy over candles in the engine, and gives
  the result.
- ``decimal_to_f64`` and ``decimal_from_f64``: the one crossing between an
  exact decimal and a float.
- ``candles_to_dataframe`` and ``dataframe_to_candles``: a batch of candles
  as a pandas DataFrame, indexed by the UTC time each bar opened, and such
  a frame as a batch.
- ``trades_to_dataframe`` and ``equity_to_dataframe``: the closed trades
  and the equity curve of a ``BacktestResult`` as pandas DataFrames.

A price, a quantity or an amount is a ``decimal.Decimal`` on both sides of
every type, and never a float. It becomes one only where it is asked to, by
the engine's own conversion: ``decimal_to_f64`` for one value,
``CandleBatch.to_numpy`` for the columns of a batch, and the four functions
that give or take a DataFrame, which are for computing with.

The package requires nothing but Python. NumPy and pandas are its two
extras: ``pip install "quantforge-py[numpy]"`` for the arrays, and
``pip install "quantforge-py[pandas]"`` for the DataFrames, which brings
NumPy too. Neither is imported before it is needed, and where one is missing
what needs it says what to install.

The types come from the compiled extension module, ``quantforge_py._native``,
and the DataFrame functions from ``quantforge_py._frames``. Both are private:
import the names from here.
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
from ._frames import (
    candles_to_dataframe,
    dataframe_to_candles,
    equity_to_dataframe,
    trades_to_dataframe,
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
    "candles_to_dataframe",
    "dataframe_to_candles",
    "trades_to_dataframe",
    "equity_to_dataframe",
]
