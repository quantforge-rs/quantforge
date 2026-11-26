"""The package as Python sees it: what it is called, what it publishes, and
what it says about the build it is."""

import inspect
import pathlib
import re
import sys

import pytest

import quantforge_py
from quantforge_py import (
    _answer,
    _bundles,
    _client,
    _errors,
    _frames,
    _jobs,
    _native,
    _source,
    _submit,
    _wire,
)

# MAJOR.MINOR.PATCH without leading zeros: an engine version as a strategy
# bundle's `engine` requirement names one (docs/bundle-spec.md).
ENGINE_VERSION = re.compile(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)")

# What the package publishes, in the order `__all__` gives it: what it says
# about itself, the engine's types, the function that runs a backtest, the
# two conversions, the reader of a store's candles, the functions that give
# or take a pandas DataFrame, the client of a server with what it gives,
# what a bundle is refused with before it is sent, and what a request to a
# server fails with.
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
FUNCTIONS = ["run_backtest", "decimal_to_f64", "decimal_from_f64"]
# A class as the types are, and no value: it holds a store that is open.
READER = ["CandleReader"]
# Written in Python, in the package's private `_frames` module: pandas is at
# home there.
FRAMES = [
    "candles_to_dataframe",
    "dataframe_to_candles",
    "trades_to_dataframe",
    "equity_to_dataframe",
]

# Classes as the types are, written in Python with the standard library
# alone: the client of a QuantForge server, the handle it gives on a job,
# and where a job stood when the server was last asked.
CLIENT = ["Client", "Job", "JobStatus"]
# An exception of the client's that is no failure of a request: a strategy
# bundle refused before anything of it is sent, which is a `ValueError`.
BUNDLES = ["BundleError"]
# The exceptions of a request, which are derived from as any exception is:
# the first is the one every other is derived from.
ERRORS = [
    "RemoteError",
    "AuthError",
    "ValidationError",
    "NotFound",
    "Conflict",
    "ServerUnavailable",
    "VersionMismatch",
]

# The private module each name written in Python is written in. Every other
# name is the extension module's.
WRITTEN_IN = {
    **dict.fromkeys(FRAMES, _frames),
    "Client": _client,
    "Job": _jobs,
    "JobStatus": _jobs,
    "BundleError": _bundles,
    **dict.fromkeys(ERRORS, _errors),
}

# The one class that exists to be derived from.
DERIVED_FROM = "Strategy"


def public(module) -> list[str]:
    return sorted(name for name in vars(module) if not name.startswith("_"))


def test_the_package_publishes_its_names_and_holds_no_other_public_one():
    assert quantforge_py.__all__ == (
        ABOUT + TYPES + FUNCTIONS + READER + FRAMES + CLIENT + BUNDLES + ERRORS
    )
    assert len(quantforge_py.__all__) == len(set(quantforge_py.__all__)) == 36
    published = sorted(name for name in quantforge_py.__all__ if not name.startswith("_"))
    assert public(quantforge_py) == published
    assert sorted(WRITTEN_IN) == sorted(FRAMES + CLIENT + BUNDLES + ERRORS)
    # The extension module holds the same names and no other, but for what
    # is written in Python.
    assert public(_native) == sorted(set(published) - set(WRITTEN_IN))


def test_every_published_name_is_the_extension_modules_own_or_a_private_modules():
    for name in quantforge_py.__all__:
        home = WRITTEN_IN.get(name, _native)
        assert getattr(quantforge_py, name) is getattr(home, name), name
    # A function written in Python says which module it is written in, which
    # is private as the extension module is.
    for name in FRAMES:
        assert getattr(quantforge_py, name).__module__ == _frames.__name__, name
    # Every module of the package is private, and lies beside the others:
    # the eight of the client among them, four of which publish nothing:
    # how a request reaches a server, how its answer is read, how a class's
    # source is found, and the order a submission is made in.
    package = pathlib.Path(quantforge_py.__file__).parent
    modules = [_answer, _bundles, _client, _errors, _frames, _jobs, _source, _submit, _wire]
    assert set(WRITTEN_IN.values()) == set(modules) - {_answer, _source, _submit, _wire}
    for module in modules:
        assert module.__name__.startswith("quantforge_py._"), module
        assert pathlib.Path(module.__file__).parent == package
        # What a module holds under a public name is a name the package
        # publishes, or a module of the standard library it imports.
        held = {name for name in public(module) if not inspect.ismodule(getattr(module, name))}
        assert held <= set(quantforge_py.__all__), module


def test_every_type_is_a_class_of_the_package_and_a_strategy_alone_is_derived_from():
    for name in TYPES + READER + CLIENT:
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
    for name in FUNCTIONS + FRAMES:
        bound = getattr(quantforge_py, name)
        assert callable(bound) and not isinstance(bound, type), name
        assert bound.__name__ == name
        assert bound.__doc__, name
    # An exception is a class of the package too, and is derived from as any
    # exception is. Each of a request's is a `RemoteError`; a bundle's is
    # none, and is a `ValueError`.
    for name in BUNDLES + ERRORS:
        bound = getattr(quantforge_py, name)
        assert isinstance(bound, type) and issubclass(bound, Exception), name
        assert (bound.__module__, bound.__qualname__) == ("quantforge_py", name)
        assert repr(bound) == f"<class 'quantforge_py.{name}'>"
        assert bound.__doc__, name
        assert issubclass(bound, quantforge_py.RemoteError) == (name in ERRORS), name
        assert issubclass(bound, ValueError) == (name in BUNDLES), name
        assert issubclass(type("Derived", (bound,), {}), bound)


def test_the_extension_module_is_private_and_built_for_the_stable_abi():
    assert _native.__name__ == "quantforge_py._native"
    extension = pathlib.Path(_native.__file__)
    # Windows has no stable ABI tag for an extension's file name.
    assert extension.name == ("_native.pyd" if sys.platform == "win32" else "_native.abi3.so")
    assert extension.parent == pathlib.Path(quantforge_py.__file__).parent


def test_the_extension_module_holds_eleven_private_entries_which_the_package_does_not_publish():
    private = [name for name in vars(_native) if name.startswith("_") and not name.startswith("__")]
    assert sorted(private) == [
        # The window of a job a client submits, read as a reader reads a
        # range.
        "_bounds",
        # What the client builds a bundle with: a bundle collected, checked
        # and packed by the engine's own code.
        "_bundle",
        # A backtest as a host that embeds Python runs one, for the package's
        # own tests: the run Python hosts is `run_backtest`, which is
        # published.
        "_embedded_backtest",
        # What the functions that give or take a DataFrame are built on: a
        # result's equity curve and its trades column by column, and NumPy
        # and pandas as they are found.
        "_equity_columns",
        # The manifest a client writes for a strategy without one, by the
        # writer the command line writes with.
        "_manifest",
        "_numpy",
        "_pandas",
        # A time as the engine writes one.
        "_rfc3339",
        # The worker a server runs Python bundles in, which
        # `quantforge_py._worker` starts.
        "_run_worker",
        # What a server's workers run bundles on, read as the protocol reads
        # it.
        "_server_runtime",
        "_trade_columns",
    ]
    package = pathlib.Path(quantforge_py.__file__).parent
    for name in private:
        assert not hasattr(quantforge_py, name), name
        assert name not in quantforge_py.__all__
        assert getattr(_native, name).__doc__, name
        # Nor has one the name of a module of the package, which the
        # package holds under that name once the module is imported.
        assert not (package / f"{name}.py").exists(), name
    signatures = {name: str(inspect.signature(getattr(_native, name))) for name in private}
    del signatures["_embedded_backtest"]
    assert signatures == {
        "_bounds": "(start, end)",
        "_bundle": "(root, files, limits=None, runtime=None)",
        "_equity_columns": "(result, /)",
        "_manifest": "(entry, python, params, deps)",
        "_numpy": "()",
        "_pandas": "()",
        "_rfc3339": "(ms, /)",
        "_run_worker": "(frames)",
        "_server_runtime": "(text)",
        "_trade_columns": "(result, /)",
    }
    # It takes what the published function takes.
    taken = inspect.signature(_native._embedded_backtest)
    assert str(taken) == "(candles, strategy, config, *, market=None)"
    assert taken == inspect.signature(quantforge_py.run_backtest)
    assert _native._embedded_backtest.__doc__


def test_the_version_is_the_crates_own(crate):
    assert quantforge_py.__version__ == crate["package"]["version"]


def test_the_engine_version_is_the_quantforge_crates(core_crate):
    assert quantforge_py.ENGINE_VERSION == core_crate["package"]["version"]
    assert ENGINE_VERSION.fullmatch(quantforge_py.ENGINE_VERSION)


def test_the_storage_backends_are_the_ones_the_build_holds(storage_backends):
    assert type(quantforge_py.STORAGE_BACKENDS) is tuple
    assert quantforge_py.STORAGE_BACKENDS == storage_backends
