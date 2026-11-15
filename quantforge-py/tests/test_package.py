"""The package as Python sees it: what it is called, what it publishes, and
what it says about the build it is."""

import inspect
import pathlib
import re
import sys

import pytest

import quantforge_py
from quantforge_py import _native

# MAJOR.MINOR.PATCH without leading zeros: an engine version as a strategy
# bundle's `engine` requirement names one (docs/bundle-spec.md).
ENGINE_VERSION = re.compile(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)")

# What the package publishes, in the order `__all__` gives it: what it says
# about itself, the engine's types, and the two conversions.
ABOUT = ["__version__", "ENGINE_VERSION", "STORAGE_BACKENDS"]
TYPES = [
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
]
FUNCTIONS = ["decimal_to_f64", "decimal_from_f64"]

# The one class that exists to be derived from.
DERIVED_FROM = "Strategy"


def public(module) -> list[str]:
    return sorted(name for name in vars(module) if not name.startswith("_"))


def test_the_package_publishes_its_names_and_holds_no_other_public_one():
    assert quantforge_py.__all__ == ABOUT + TYPES + FUNCTIONS
    published = sorted(name for name in quantforge_py.__all__ if not name.startswith("_"))
    assert public(quantforge_py) == published
    # The extension module holds the same names and no other.
    assert public(_native) == published


def test_every_published_name_is_the_extension_modules_own():
    for name in quantforge_py.__all__:
        assert getattr(quantforge_py, name) is getattr(_native, name), name


def test_every_type_is_a_class_of_the_package_and_a_strategy_alone_is_derived_from():
    for name in TYPES:
        bound = getattr(quantforge_py, name)
        assert isinstance(bound, type), name
        # A class says where it is imported from, not where it is compiled.
        assert (bound.__module__, bound.__qualname__) == ("quantforge_py", name)
        assert repr(bound) == f"<class 'quantforge_py.{name}'>"
        assert bound.__doc__, name
        if name == DERIVED_FROM:
            derived = type("Derived", (bound,), {"on_bar": lambda self, ctx, bar: None})
            assert isinstance(derived(), bound)
            continue
        with pytest.raises(TypeError, match="not an acceptable base type"):
            type("Derived", (bound,), {})
    for name in FUNCTIONS:
        bound = getattr(quantforge_py, name)
        assert callable(bound) and not isinstance(bound, type), name
        assert bound.__name__ == name
        assert bound.__doc__, name


def test_the_extension_module_is_private_and_built_for_the_stable_abi():
    assert _native.__name__ == "quantforge_py._native"
    extension = pathlib.Path(_native.__file__)
    # Windows has no stable ABI tag for an extension's file name.
    assert extension.name == ("_native.pyd" if sys.platform == "win32" else "_native.abi3.so")
    assert extension.parent == pathlib.Path(quantforge_py.__file__).parent


def test_the_extension_module_holds_one_private_entry_which_the_package_does_not_publish():
    private = [name for name in vars(_native) if name.startswith("_") and not name.startswith("__")]
    # The engine run on a strategy, for the package's own tests: the
    # function a user calls is bound by the story that follows.
    assert private == ["_backtest"]
    assert not hasattr(quantforge_py, "_backtest")
    assert "_backtest" not in quantforge_py.__all__
    assert str(inspect.signature(_native._backtest)) == (
        "(market, candles, strategy, config, *, embedded=False)"
    )
    assert _native._backtest.__doc__


def test_the_version_is_the_crates_own(crate):
    assert quantforge_py.__version__ == crate["package"]["version"]


def test_the_engine_version_is_the_quantforge_crates(core_crate):
    assert quantforge_py.ENGINE_VERSION == core_crate["package"]["version"]
    assert ENGINE_VERSION.fullmatch(quantforge_py.ENGINE_VERSION)


def test_the_storage_backends_are_the_ones_the_build_holds(storage_backends):
    assert type(quantforge_py.STORAGE_BACKENDS) is tuple
    assert quantforge_py.STORAGE_BACKENDS == storage_backends
