"""pandas as an extra of the package: when the package imports it, and what
the four functions that give or take a DataFrame say where Python finds none
to import, or one the extra does not take: one older than its lower bound,
or one from its upper bound on. And the NumPy a frame needs beside it, which
is asked for after pandas and before any argument is looked at.

These tests run with pandas and without it. Each asks a fresh interpreter,
which is kept from importing pandas, or handed a module of the test's own in
its place: so nothing here needs a pandas, and none is imported by this
module. Where the package is installed without its extras the run is told so
(`conftest.py`), and the interpreter is then one that has no pandas at all.

How an extra is found is one piece of the extension for both extras, and
`test_numpy_optional.py` holds it to every way a version is spelled. What is
held here is that pandas is found through it, with bounds of its own.

Whether a version is one the extra takes is never asked of the package
itself: `packaging`, which is how `pip` reads a requirement, says so."""

import re
import subprocess
import sys
import textwrap

import pytest
from packaging.requirements import Requirement
from packaging.version import InvalidVersion, Version

PAGE = "docs/python-pandas.md"

# What every interpreter below starts with: a batch, a result, the four
# functions each with what it takes, and how what a call raised is written.
START = """
    import sys
    import quantforge_py as qf
    from quantforge_py import _native

    candle = qf.Candle(open_time_ms=0, close_time_ms=59_999, open=1, high=1, low=1, close=1, volume=1)
    batch = qf.CandleBatch.from_candles([candle])
    result = qf.BacktestResult(
        initial_cash=1, final_equity=1, total_return_pct=0, trade_count=0, max_drawdown_pct=0,
        trades=[], equity_curve=[],
    )
    four = {
        "candles_to_dataframe": lambda: qf.candles_to_dataframe(batch),
        "dataframe_to_candles": lambda: qf.dataframe_to_candles(None),
        "trades_to_dataframe": lambda: qf.trades_to_dataframe(result),
        "equity_to_dataframe": lambda: qf.equity_to_dataframe(result),
    }
    # None of these four is of a type its function takes.
    of_no_type = {
        "candles_to_dataframe": lambda: qf.candles_to_dataframe(None),
        "dataframe_to_candles": lambda: qf.dataframe_to_candles(None),
        "trades_to_dataframe": lambda: qf.trades_to_dataframe(None),
        "equity_to_dataframe": lambda: qf.equity_to_dataframe(None),
    }

    def said(call):
        try:
            call()
        except BaseException as error:
            return f"{type(error).__name__}: {error}"
        return "nothing was raised"
"""

# As Python has it where a module is not to be imported.
KEPT_FROM_PANDAS = """
    import sys

    sys.modules["pandas"] = None
"""

FOUR = [
    "candles_to_dataframe",
    "dataframe_to_candles",
    "trades_to_dataframe",
    "equity_to_dataframe",
]

# Versions as pandas writes them, and as `pip` orders them, about each of the
# extra's two bounds: releases, their pre-releases, development builds with a
# build's own label, and versions written with fewer numbers or more.
VERSIONS = [
    "0.25.3",
    "1.5.3",
    "2",
    "2.1.4",
    "2.2",
    "2.2.0",
    "2.2.1",
    "2.2.2a1",
    "2.2.2rc0",
    "2.2.2.dev0+123.g0123abc",
    "2.2.2",
    "2.2.2+local",
    "2.2.2.post1",
    "2.2.2.0",
    "2.2.3rc0",
    "2.2.3",
    "2.2.10",
    "2.3",
    "2.3.0.dev0+12.g0123abc",
    "2.3.3",
    "3",
    "3.0",
    "3.0.0rc0",
    "3.0.0",
    "3.1.0rc0",
    "3.1.1",
    "3.99.0rc1",
    "3.99.0",
    "3.99.0.post1",
    "4",
    "4.0",
    "4.0.0.dev0+1.g0123abc",
    "4.0.0rc0",
    "4.0.0",
    "4.0.1",
    "4.1.0",
    "10.0",
    "99999999999999999999999.0",
    "2.2.99999999999999999999999",
    "3.99999999999999999999999rc1",
]

# Text that is no version at all.
NO_VERSIONS = ["", "unknown", ".5", "2.2.", "2..2", "two"]


def in_a_fresh_interpreter(*parts: str) -> subprocess.CompletedProcess:
    """Runs the parts, one after another, in an interpreter of its own."""
    code = "\n".join(textwrap.dedent(part) for part in parts)
    return subprocess.run([sys.executable, "-c", code], capture_output=True, text=True, timeout=120)


@pytest.fixture(scope="module")
def required(pyproject) -> Requirement:
    """What the package's `pandas` extra requires of pandas."""
    numpy, pandas = map(Requirement, pyproject["project"]["optional-dependencies"]["pandas"])
    assert (numpy.name, pandas.name) == ("numpy", "pandas")
    return pandas


@pytest.fixture(scope="module")
def needed(required) -> str:
    """What every refusal opens with: the oldest pandas the extra takes, and
    the first it does not."""
    oldest, following = sorted(required.specifier, key=lambda bound: Version(bound.version))
    assert (oldest.operator, following.operator) == (">=", "<")
    return f"a DataFrame needs pandas {oldest.version} or later and before {following.version}"


@pytest.fixture(scope="module")
def install(pyproject) -> str:
    """What every refusal ends with: the package with its extra, as `pip` is
    asked for it."""
    return f'pip install "{pyproject["project"]["name"]}[pandas]"'


@pytest.fixture(scope="module")
def not_found(needed, install) -> str:
    return f"ModuleNotFoundError: {needed}, and Python finds none to import: {install}"


@pytest.fixture(scope="module")
def no_numpy(pyproject) -> str:
    """What the arrays say where Python finds no NumPy, in their own words."""
    (numpy,) = pyproject["project"]["optional-dependencies"]["numpy"]
    assert numpy == "numpy>=2.0,<3"
    return (
        "ModuleNotFoundError: to_numpy and from_numpy need NumPy 2.0 or later and before 3, "
        f'and Python finds none to import: pip install "{pyproject["project"]["name"]}[numpy]"'
    )


@pytest.fixture
def stood_in(tmp_path):
    """Puts a module of the test's own where an interpreter looks for a
    library first: `stood_in(source, library)` writes it, and gives the part
    that makes a fresh interpreter find it before any other."""

    def stood_in(source: str, library: str = "pandas") -> str:
        package = tmp_path / library
        package.mkdir(exist_ok=True)
        (package / "__init__.py").write_text(textwrap.dedent(source), encoding="utf-8")
        return f"import sys\n\nsys.path.insert(0, {str(tmp_path)!r})\n"

    return stood_in


# A module that states a version and nothing else: whatever else is asked of
# it says so.
STATES_A_VERSION = """
    __version__ = "0"


    def __getattr__(name):
        raise AssertionError(f"{__name__}.{name} was asked for")
"""


def test_pandas_is_imported_when_a_frame_is_first_asked_for_and_not_before(
    without_pandas, without_numpy, not_found
):
    ran = in_a_fresh_interpreter(
        START,
        """
        market = qf.MarketId("binance_spot", "BTCUSDT", "1m")
        config = qf.BacktestConfig(initial_cash=1000, fee_bps=0, close_out_at_end=True)

        class Holds(qf.Strategy):
            def on_bar(self, ctx, bar):
                return qf.TargetPosition.LONG_ALL_IN

        made = qf.run_backtest(batch, Holds(), config, market=market)
        repr(batch), batch.open, batch.to_candles(), batch == batch, made.trades, made.equity_curve
        # A batch's own arrays are NumPy's, and need no pandas.
        said(batch.to_numpy)
        print("before:", "pandas" in sys.modules)
        for name, call in four.items():
            print(name, "|", said(call))
            print("after:", "pandas" in sys.modules)
            break
        try:
            qf.candles_to_dataframe(batch)
        except ImportError as error:
            print(f"{type(error.__cause__).__name__}: {error.__cause__}")
        """,
    )
    assert ran.returncode == 0, ran.stderr
    if without_pandas:
        # The package is installed without its extras: everything before
        # the frame worked, and the frame says what to install.
        assert ran.stdout.splitlines() == [
            "before: False",
            f"candles_to_dataframe | {not_found}",
            "after: False",
            "ModuleNotFoundError: No module named 'pandas'",
        ]
    else:
        assert not without_numpy
        assert ran.stdout.splitlines() == [
            "before: False",
            "candles_to_dataframe | nothing was raised",
            "after: True",
        ]


def test_where_python_finds_no_pandas_each_of_the_four_says_what_to_install(not_found):
    ran = in_a_fresh_interpreter(
        KEPT_FROM_PANDAS,
        START,
        """
        for name, call in four.items():
            print(name, "|", said(call))
        # Asked before any argument is looked at: none of these is of a type
        # its function takes.
        for name, call in of_no_type.items():
            print(name, "|", said(call))
        try:
            qf.candles_to_dataframe(batch)
        except ModuleNotFoundError as error:
            cause = error.__cause__
            print(error.name, "|", f"{type(cause).__name__}: {cause}", "|", error.__suppress_context__)
        # Everything else of the package works without it.
        print(batch.close, len(batch.to_candles()), qf.decimal_to_f64(batch.close[0]))
        print(result.trades, result.equity_curve, "pandas" in dir(qf))
        """,
    )
    assert ran.returncode == 0, ran.stderr
    assert ran.stdout.splitlines() == [
        *(f"{name} | {not_found}" for name in FOUR),
        *(f"{name} | {not_found}" for name in FOUR),
        # Python's own error is its cause, and it names the module as that does.
        "pandas | ModuleNotFoundError: import of pandas halted; None in sys.modules | True",
        "(Decimal('1'),) 1 1.0",
        "() () False",
    ]


def test_a_batchs_arrays_need_no_pandas_and_say_nothing_of_it(without_numpy):
    ran = in_a_fresh_interpreter(
        KEPT_FROM_PANDAS,
        START,
        """
        print(said(batch.to_numpy))
        print(said(lambda: qf.CandleBatch.from_numpy(**batch.to_numpy()) == batch))
        """,
    )
    assert ran.returncode == 0, ran.stderr
    given, taken = ran.stdout.splitlines()
    if without_numpy:
        # Where there is no NumPy either, the arrays say so, of NumPy.
        assert given == taken and "pandas" not in given and "need NumPy" in given
    else:
        assert (given, taken) == ("nothing was raised", "nothing was raised")


def test_a_pandas_is_taken_or_refused_by_its_version_as_pip_reads_the_extras_bounds(
    stood_in, required, needed, install
):
    stated = VERSIONS + NO_VERSIONS
    ran = in_a_fresh_interpreter(
        stood_in(STATES_A_VERSION),
        START,
        f"""
        import pandas

        for version in {stated!r}:
            pandas.__version__ = version
            try:
                found = _native._pandas()
            except ImportError as error:
                print(f"{{type(error).__name__}}: {{error}}")
            else:
                # What is taken is handed back as it was imported.
                print("taken" if found is pandas else "another module")
        """,
    )
    assert ran.returncode == 0, ran.stderr
    said = dict(zip(stated, ran.stdout.splitlines(), strict=True))

    def refused(version):
        return f"ImportError: {needed}, and this is pandas {version}: {install}"

    taken = []
    for version in VERSIONS:
        # `pip` holds a pre-release to a bound as it holds a release, once it
        # is asked to look at pre-releases at all.
        if required.specifier.contains(Version(version), prereleases=True):
            assert said[version] == "taken", version
            taken.append(version)
        else:
            assert said[version] == refused(version), version
    # Both kinds are among them, and the pre-releases of each bound itself:
    # one of the lower bound is older than it, and one of the upper bound is
    # not before it. The lower bound has three numbers, and each counts.
    assert 12 < len(taken) < len(VERSIONS) - 12
    assert "2.2.2" in taken and "2.2.2rc0" not in taken and "2.2.3rc0" in taken
    assert "2.2.1" not in taken and "2.2" not in taken and "2.3" in taken and "2.2.2.0" in taken
    assert "3.99.0" in taken and "4.0.0rc0" not in taken and "4" not in taken
    assert "3.99999999999999999999999rc1" in taken and "99999999999999999999999.0" not in taken
    below = [version for version in VERSIONS if Version(version) < Version("2.2.2")]
    above = [version for version in VERSIONS if Version(version) >= Version("4.dev0")]
    assert len(below) > 8 and len(above) > 8
    assert sorted(taken + below + above) == sorted(VERSIONS)
    for text in NO_VERSIONS:
        with pytest.raises(InvalidVersion):
            Version(text)
        assert said[text] == refused(text), text
    assert "panic" not in ran.stderr.lower()


@pytest.mark.parametrize("version", ["2.1.4", "4.0.0"], ids=["older", "newer"])
def test_a_pandas_the_extra_does_not_take_is_refused_before_anything_else_is_asked_of_it(
    version, stood_in, needed, install
):
    ran = in_a_fresh_interpreter(
        stood_in(STATES_A_VERSION.replace('"0"', repr(version))),
        START,
        """
        for name, call in four.items():
            print(name, "|", said(call))
        try:
            qf.candles_to_dataframe(batch)
        except ImportError as error:
            print(type(error) is ImportError, error.name, error.__cause__)
        # Everything else of the package works beside it.
        print(batch.close, len(batch.to_candles()))
        """,
    )
    assert ran.returncode == 0, ran.stderr
    refused = f"ImportError: {needed}, and this is pandas {version}: {install}"
    assert ran.stdout.splitlines() == [
        *(f"{name} | {refused}" for name in FOUR),
        "True pandas None",
        "(Decimal('1'),) 1",
    ]


@pytest.mark.parametrize(
    "module, cause",
    [
        ("", "AttributeError: module 'pandas' has no attribute '__version__'"),
        ("__version__ = (2, 2, 2)", "NoneType: None"),
        ("__version__ = None", "NoneType: None"),
    ],
    ids=["no version", "a version that is no text", "a version of None"],
)
def test_what_imports_as_pandas_and_states_no_version_is_refused(
    module, cause, stood_in, needed, tmp_path
):
    ran = in_a_fresh_interpreter(
        stood_in(module),
        START,
        """
        for name, call in four.items():
            print(said(call))
        try:
            qf.candles_to_dataframe(batch)
        except ImportError as error:
            print(f"{type(error.__cause__).__name__}: {error.__cause__}")
        """,
    )
    assert ran.returncode == 0, ran.stderr
    found = f"<module 'pandas' from {str(tmp_path / 'pandas' / '__init__.py')!r}>"
    stated = f"ImportError: {needed}, and what imports as pandas here states no version: {found}"
    # Where the module has no version to state, Python's own error for that
    # is what the refusal was raised from.
    assert ran.stdout.splitlines() == [stated] * 4 + [cause]


def test_a_directory_named_pandas_is_no_pandas(tmp_path, without_pandas, needed):
    # Python imports a directory that has no `__init__.py` as a module with
    # nothing in it, and only where it finds no package of that name: so a
    # stray directory is taken for pandas only where pandas is not installed.
    (tmp_path / "pandas").mkdir()
    ran = in_a_fresh_interpreter(
        f"import sys\n\nsys.path.insert(0, {str(tmp_path)!r})\n",
        START,
        """
        print(said(four["candles_to_dataframe"]))
        import pandas
        print(hasattr(pandas, "__version__"))
        """,
    )
    assert ran.returncode == 0, ran.stderr
    given, is_pandas = ran.stdout.splitlines()
    if without_pandas:
        # It is shown as Python shows a module that is only a directory,
        # which is not the same in every version of Python.
        stated = f"ImportError: {needed}, and what imports as pandas here states no version: "
        assert given.startswith(stated + "<module 'pandas' ("), given
        assert "namespace" in given.lower()
        assert is_pandas == "False"
    else:
        # pandas itself is found past it.
        assert (given, is_pandas) == ("nothing was raised", "True")


@pytest.mark.parametrize(
    "module, raised",
    [
        (
            'raise ImportError("pandas is broken in a way of its own")',
            "ImportError: pandas is broken in a way of its own",
        ),
        (
            'raise RuntimeError("pandas is broken in another way")',
            "RuntimeError: pandas is broken in another way",
        ),
        # It was found, and failed under its own name: that is not pandas
        # missing either.
        (
            "raise ImportError(\"pandas was found, and could not be loaded\", name='pandas')",
            "ImportError: pandas was found, and could not be loaded",
        ),
        # A module pandas looked for and did not find is not pandas missing:
        # its own way of saying that it has no NumPy among them.
        (
            "import a_module_no_one_has_installed",
            "ModuleNotFoundError: No module named 'a_module_no_one_has_installed'",
        ),
        (
            "raise ModuleNotFoundError(\"No module named 'pandas._libs'\", name='pandas._libs')",
            "ModuleNotFoundError: No module named 'pandas._libs'",
        ),
        # Nor is an error of the module's own, raised as its version is asked for.
        (
            'def __getattr__(name):\n    raise RuntimeError(f"no {name} today")',
            "RuntimeError: no __version__ today",
        ),
        ('raise KeyboardInterrupt("stopped by hand")', "KeyboardInterrupt: stopped by hand"),
    ],
    ids=[
        "an import error",
        "another error",
        "an import error that names it",
        "a module it lacks",
        "a part of it that is missing",
        "an error as its version is read",
        "an interrupt",
    ],
)
def test_a_pandas_that_fails_as_it_is_imported_raises_its_own_error(module, raised, stood_in):
    ran = in_a_fresh_interpreter(
        stood_in(module),
        START,
        """
        for name, call in four.items():
            print(said(call))
        try:
            qf.candles_to_dataframe(batch)
        except BaseException as error:
            print(error.__cause__)
        """,
    )
    assert ran.returncode == 0, ran.stderr
    # As it was raised, and not as a pandas that is missing: what is wrong
    # with it is its own to say.
    assert ran.stdout.splitlines() == [raised] * 4 + ["None"]


def test_numpy_is_asked_for_after_pandas_and_before_any_argument_is_looked_at(
    stood_in, no_numpy, not_found
):
    # A pandas that is taken, and no NumPy: each of the four says so in the
    # arrays' own words, whatever it was handed.
    ran = in_a_fresh_interpreter(
        stood_in(STATES_A_VERSION.replace('"0"', '"2.2.3"')),
        'import sys\n\nsys.modules["numpy"] = None\n',
        START,
        """
        for name, call in four.items():
            print(name, "|", said(call))
        for name, call in of_no_type.items():
            print(name, "|", said(call))
        """,
    )
    assert ran.returncode == 0, ran.stderr
    assert ran.stdout.splitlines() == [f"{name} | {no_numpy}" for name in FOUR] * 2
    # Neither pandas nor NumPy: pandas is the one that is named.
    ran = in_a_fresh_interpreter(
        KEPT_FROM_PANDAS,
        'sys.modules["numpy"] = None\n',
        START,
        """
        for name, call in of_no_type.items():
            print(name, "|", said(call))
        """,
    )
    assert ran.returncode == 0, ran.stderr
    assert ran.stdout.splitlines() == [f"{name} | {not_found}" for name in FOUR]


def test_the_extension_hands_back_pandas_and_numpy_themselves(
    without_pandas, without_numpy, not_found, no_numpy
):
    ran = in_a_fresh_interpreter(
        START,
        """
        def handed(found, library):
            try:
                module = found()
            except ImportError as error:
                return f"{type(error).__name__}: {error}"
            return module is __import__(library)

        print(handed(_native._pandas, "pandas"))
        print(handed(_native._numpy, "numpy"))
        print(said(lambda: _native._pandas(1)), "|", said(lambda: _native._numpy(1)))
        """,
    )
    assert ran.returncode == 0, ran.stderr
    pandas, numpy, takes_nothing = ran.stdout.splitlines()
    # Each is the module Python imports under that name, where there is one,
    # and the refusal of its extra where there is none.
    assert pandas == (not_found if without_pandas else "True")
    assert numpy == (no_numpy if without_numpy else "True")
    assert takes_nothing.count("TypeError: ") == 2


def test_the_page_shows_what_each_of_the_four_says_without_a_pandas_it_takes(
    page, quoted, stood_in, required, not_found, needed, install
):
    rows = page(PAGE).rows("without pandas")
    none, older, newer, numpy = [quoted(raised) for _, raised in rows]
    # Where Python finds none.
    assert none == not_found
    # And where the one it finds is one the extra does not take, by the
    # version the page names in each row: one older, and one newer.
    stated = {}
    for (where, _), raised in zip(rows[1:3], (older, newer), strict=True):
        (version,) = re.findall(r" and this is pandas (\S+): ", raised)
        assert raised == f"ImportError: {needed}, and this is pandas {version}: {install}"
        assert f"here {version}" in where
        stated[version] = raised
    oldest, following = sorted(Version(bound.version) for bound in required.specifier)
    below, above = map(Version, stated)
    assert below < oldest and above >= following
    assert "older" in rows[1][0] and "newer" in rows[2][0]
    ran = in_a_fresh_interpreter(
        stood_in(STATES_A_VERSION),
        START,
        f"""
        import pandas

        for version in {list(stated)!r}:
            pandas.__version__ = version
            print(said(four["trades_to_dataframe"]))
        """,
    )
    assert ran.returncode == 0, ran.stderr
    assert ran.stdout.splitlines() == list(stated.values())
    ran = in_a_fresh_interpreter(
        KEPT_FROM_PANDAS, START, 'print(said(four["equity_to_dataframe"]))'
    )
    assert ran.returncode == 0, ran.stderr
    assert ran.stdout.splitlines() == [none]
    # The last row: a pandas that is taken, beside a NumPy the arrays do not
    # take, each by the version the page names.
    (version,) = re.findall(r" and this is NumPy (\S+): ", numpy)
    assert f"here {version}" in rows[3][0] and "taken" in rows[3][0]
    assert Version(version) < Version("2.0")
    ran = in_a_fresh_interpreter(
        stood_in(STATES_A_VERSION.replace('"0"', '"2.2.3"')),
        stood_in(STATES_A_VERSION.replace('"0"', repr(version)), "numpy"),
        START,
        """
        for name, call in four.items():
            print(said(call))
        """,
    )
    assert ran.returncode == 0, ran.stderr
    assert ran.stdout.splitlines() == [numpy] * 4
    assert numpy.startswith("ImportError: to_numpy and from_numpy need NumPy 2.0 or later")
    # The page gives the command itself, as the refusals do.
    assert f"`{install}`" in page(PAGE).text
