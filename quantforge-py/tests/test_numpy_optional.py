"""NumPy as an extra of the package: when the package imports it, and what
`CandleBatch.to_numpy` and `CandleBatch.from_numpy` say where Python finds
none to import, or one the extra does not take: one older than its lower
bound, or one from its upper bound on.

These tests run with NumPy and without it. Each asks a fresh interpreter,
which is kept from importing NumPy, or handed a module of the test's own in
its place: so nothing here needs a NumPy, and none is imported by this
module. Where the package is installed without its extra the run is told so
(`conftest.py`), and the interpreter is then one that has no NumPy at all.

What the distribution says of the extra is held in `test_distribution.py`,
and what the arrays are in `test_numpy.py`.

Whether a version is one the extra takes is never asked of the package
itself: `packaging`, which is how `pip` reads a requirement, says so."""

import re
import subprocess
import sys
import textwrap

import pytest
from packaging.requirements import Requirement
from packaging.version import InvalidVersion, Version

PAGE = "docs/python-numpy.md"

# What every interpreter below starts with: a batch, eight columns that are
# no arrays, and how what a call raised is written.
BATCH = """
    import sys
    import quantforge_py as qf

    candle = qf.Candle(open_time_ms=0, close_time_ms=59_999, open=1, high=1, low=1, close=1, volume=1)
    batch = qf.CandleBatch.from_candles([candle])
    eight = dict.fromkeys(
        ["open_time_ms", "close_time_ms", "open", "high", "low", "close", "volume", "trades"], [1]
    )

    def said(call):
        try:
            call()
        except BaseException as error:
            return f"{type(error).__name__}: {error}"
        return "nothing was raised"
"""

# As Python has it where a module is not to be imported.
KEPT_FROM_NUMPY = """
    import sys

    sys.modules["numpy"] = None
"""

# Versions as NumPy writes them, and as `pip` orders them: releases, their
# pre-releases, development builds with a build's own label, and versions
# written with fewer numbers or far larger ones. Then the other ways `pip`
# takes a pre-release and a later release to be spelled.
VERSIONS = [
    "0.9",
    "1.26.4",
    "1.99.0",
    "2",
    "2rc1",
    "2.0",
    "2.0.0",
    "2.0.0a1",
    "2.0.0b1",
    "2.0.0rc1",
    "2.0.0rc2",
    "2.0.0.dev0+git20240113.d5f5bcf",
    "2.0.0+local",
    "2.0.0+rc1",
    "2.0.0.post1",
    "2.0.0.post1.dev0",
    "2.0.0.0",
    "2.0.1rc1",
    "2.0.1",
    "2.0.2",
    "2.1.0.dev0+git20240601.3ec8ebe",
    "2.1.0rc1",
    "2.4.6",
    "2.5.5",
    "2.99.0rc1",
    "2.99.0",
    "2.99.0.post1",
    "3",
    "3.0",
    "3.0.0.dev0+git20260601.3ec8ebe",
    "3.0.0a1",
    "3.0.0rc1",
    "3.0.0",
    "3.0.0+local",
    "3.0.0.post1",
    "3.0.1",
    "3.1.4",
    "10.0",
    "99999999999999999999999.0",
    # A number too large for the package to read is still beyond every
    # number of a bound, and not zero: this is no pre-release of 2.0.
    "2.99999999999999999999999",
    "2.99999999999999999999999rc1",
    "2.0.99999999999999999999999rc1",
    "2.0.0alpha1",
    "2.0.0beta2",
    "2.0.0c1",
    "2.0.0pre1",
    "2.0.0preview1",
    "2.0.0.rc1",
    "2.0.0-rc1",
    "2.0.0_rc1",
    "2.0.0RC1",
    "2.0.0dev3",
    "2.0.0-1",
    "2.0.0post1",
    "2.0.0rev1",
    "2.0.0r1",
]

# Text that is no version at all.
NO_VERSIONS = ["", "unknown", ".5", "2.0.", "2..0", "two"]

# Text `pip` would not take for a version, which NumPy has never stated and
# a NumPy someone built for themselves may. It is read by the numbers it
# begins with, and what follows them is looked at only for the word that
# marks a pre-release: a name that merely begins like one marks none.
BY_ITS_NUMBERS = {
    "2.0.0-custom": True,
    "2.0.0-acme": True,
    "2.0.0-build5": True,
    "2.0.0.cafe": True,
    "2.0.0.dirty": True,
    "2.0.0devel": True,
    "2.0.0 (vendor)": True,
    "2.1.0-custom": True,
    "2.5garbage": True,
    "2.99.0-custom": True,
    "3x": False,
    "3.0.0-custom": False,
    "3.0.0.dirty": False,
    "4.1 (vendor)": False,
    "1.26.4-custom": False,
    "1.99.9.dirty": False,
    # And what `pip` takes for a version and NumPy never writes: a `v`
    # before it, a space, an epoch. None begins with a number that is its
    # version's first.
    "v2.0.0": False,
    " 2.0.0": False,
    "1!1.0": False,
    "0!2.0": False,
}


def in_a_fresh_interpreter(*parts: str) -> subprocess.CompletedProcess:
    """Runs the parts, one after another, in an interpreter of its own."""
    code = "\n".join(textwrap.dedent(part) for part in parts)
    return subprocess.run([sys.executable, "-c", code], capture_output=True, text=True, timeout=120)


@pytest.fixture(scope="module")
def extra(pyproject) -> tuple[str, Requirement]:
    """The package's `numpy` extra: its name, and the one requirement it
    has."""
    name = "numpy"
    (required,) = pyproject["project"]["optional-dependencies"][name]
    return name, Requirement(required)


@pytest.fixture(scope="module")
def needed(extra) -> str:
    """What every refusal opens with: the oldest NumPy the extra takes, and
    the first it does not."""
    _, required = extra
    oldest, following = sorted(required.specifier, key=lambda bound: Version(bound.version))
    assert (required.name, oldest.operator, following.operator) == ("numpy", ">=", "<")
    return (
        f"to_numpy and from_numpy need NumPy {oldest.version} or later"
        f" and before {following.version}"
    )


@pytest.fixture(scope="module")
def install(pyproject, extra) -> str:
    """What every refusal ends with: the package with its extra, as `pip` is
    asked for it."""
    name, _ = extra
    return f'pip install "{pyproject["project"]["name"]}[{name}]"'


@pytest.fixture(scope="module")
def not_found(needed, install) -> str:
    return f"ModuleNotFoundError: {needed}, and Python finds none to import: {install}"


@pytest.fixture
def stood_in(tmp_path):
    """Puts a module of the test's own where an interpreter looks for NumPy
    first: `stood_in(source)` writes it, with each further file given as a
    part of it by its path within it, and gives the part that makes a fresh
    interpreter find it before any other."""

    def stood_in(source: str, parts: dict[str, str] | None = None) -> str:
        package = tmp_path / "numpy"
        package.mkdir(exist_ok=True)
        (package / "__init__.py").write_text(textwrap.dedent(source), encoding="utf-8")
        for name, part in (parts or {}).items():
            file = package / name
            file.parent.mkdir(parents=True, exist_ok=True)
            file.write_text(textwrap.dedent(part), encoding="utf-8")
        return f"import sys\n\nsys.path.insert(0, {str(tmp_path)!r})\n"

    return stood_in


# A module that states a version and nothing else: whatever else is asked of
# it says so.
STATES_A_VERSION = """
    __version__ = "0"


    def __getattr__(name):
        raise AssertionError(f"numpy.{name} was asked for")
"""


@pytest.fixture
def no_arrays(needed, tmp_path) -> str:
    """What a conversion says of the module `stood_in` writes where its
    version is one the extra takes: it is looked in for NumPy's arrays, as
    the array bindings look for them, and has none."""
    found = f"<module 'numpy' from {str(tmp_path / 'numpy' / '__init__.py')!r}>"
    return f"ImportError: {needed}, and what imports as numpy here does not hold NumPy's arrays: {found}"


def test_numpy_is_imported_when_a_conversion_is_first_asked_for_and_not_before(
    without_numpy, not_found
):
    ran = in_a_fresh_interpreter(
        BATCH,
        """
        market = qf.MarketId("binance_spot", "BTCUSDT", "1m")
        config = qf.BacktestConfig(initial_cash=1000, fee_bps=0, close_out_at_end=True)

        class Holds(qf.Strategy):
            def on_bar(self, ctx, bar):
                return qf.TargetPosition.LONG_ALL_IN

        qf.run_backtest(batch, Holds(), config, market=market)
        repr(batch), batch.open, batch.to_candles(), batch == batch
        print("before:", "numpy" in sys.modules)
        print(said(batch.to_numpy))
        print("after:", "numpy" in sys.modules)
        try:
            batch.to_numpy()
        except ImportError as error:
            print(f"{type(error.__cause__).__name__}: {error.__cause__}")
        """,
    )
    assert ran.returncode == 0, ran.stderr
    if without_numpy:
        # The package is installed without its extra: everything before the
        # conversion worked, and the conversion says what to install.
        assert ran.stdout.splitlines() == [
            "before: False",
            not_found,
            "after: False",
            "ModuleNotFoundError: No module named 'numpy'",
        ]
    else:
        assert ran.stdout.splitlines() == ["before: False", "nothing was raised", "after: True"]


def test_where_python_finds_no_numpy_the_two_conversions_say_what_to_install(not_found):
    ran = in_a_fresh_interpreter(
        KEPT_FROM_NUMPY,
        BATCH,
        """
        print(said(batch.to_numpy))
        print(said(lambda: qf.CandleBatch.from_numpy(**eight)))
        try:
            batch.to_numpy()
        except ModuleNotFoundError as error:
            cause = error.__cause__
            print(error.name, "|", f"{type(cause).__name__}: {cause}", "|", error.__suppress_context__)
        # Everything else of the package works without it.
        print(batch.close, len(batch.to_candles()), qf.decimal_to_f64(batch.close[0]))
        print(qf.CandleBatch.from_columns(**eight) == qf.CandleBatch.from_columns(**eight))
        """,
    )
    assert ran.returncode == 0, ran.stderr
    assert ran.stdout.splitlines() == [
        # Asked before any argument is looked at: these eight are no arrays.
        not_found,
        not_found,
        # Python's own error is its cause, and it names the module as that does.
        "numpy | ModuleNotFoundError: import of numpy halted; None in sys.modules | True",
        "(Decimal('1'),) 1 1.0",
        "True",
    ]
    # An error of Python's, and no panic of the extension's.
    assert "panic" not in ran.stderr.lower()


def test_a_numpy_is_taken_or_refused_by_its_version_as_pip_reads_the_extras_bound(
    stood_in, extra, needed, install, no_arrays
):
    _, required = extra
    stated = VERSIONS + NO_VERSIONS + list(BY_ITS_NUMBERS)
    ran = in_a_fresh_interpreter(
        stood_in(STATES_A_VERSION),
        BATCH,
        f"""
        import numpy

        for version in {stated!r}:
            numpy.__version__ = version
            print(said(batch.to_numpy))
        """,
    )
    assert ran.returncode == 0, ran.stderr
    said = dict(zip(stated, ran.stdout.splitlines(), strict=True))

    def refused(version):
        return f"ImportError: {needed}, and this is NumPy {version}: {install}"

    # A module whose version is taken is asked for NumPy's arrays next, and
    # this one has none: so that refusal is what shows a version was taken.
    taken = []
    for version in VERSIONS:
        # `pip` holds a pre-release to a bound as it holds a release, once it
        # is asked to look at pre-releases at all.
        if required.specifier.contains(Version(version), prereleases=True):
            assert said[version] == no_arrays, version
            taken.append(version)
        else:
            assert said[version] == refused(version), version
    # Both kinds are among them, and the pre-releases of each bound itself:
    # one of the lower bound is older than it, and one of the upper bound is
    # not before it.
    assert 16 < len(taken) < len(VERSIONS) - 16
    assert "2.0.0" in taken and "2.0.0rc1" not in taken and "2.0.1rc1" in taken
    assert "2.99.0" in taken and "3.0.0rc1" not in taken and "3" not in taken
    assert "2.99999999999999999999999rc1" in taken and "99999999999999999999999.0" not in taken
    below = [version for version in VERSIONS if Version(version) < Version("2.0")]
    above = [version for version in VERSIONS if Version(version) >= Version("3.dev0")]
    assert len(below) > 8 and len(above) > 8
    assert sorted(taken + below + above) == sorted(VERSIONS)
    for text in NO_VERSIONS:
        with pytest.raises(InvalidVersion):
            Version(text)
        assert said[text] == refused(text), text
    # What is no version of `pip`'s is read by its numbers, and never by a
    # letter its own name happens to begin with.
    for text, is_taken in BY_ITS_NUMBERS.items():
        assert said[text] == (no_arrays if is_taken else refused(text)), text
    assert "panic" not in ran.stderr.lower()


@pytest.mark.parametrize("version", ["1.26.4", "3.0.0"], ids=["older", "newer"])
def test_a_numpy_the_extra_does_not_take_is_refused_before_anything_else_is_asked_of_it(
    version, stood_in, needed, install
):
    ran = in_a_fresh_interpreter(
        stood_in(STATES_A_VERSION.replace('"0"', repr(version))),
        BATCH,
        """
        print(said(batch.to_numpy))
        print(said(lambda: qf.CandleBatch.from_numpy(**eight)))
        try:
            batch.to_numpy()
        except ImportError as error:
            print(type(error) is ImportError, error.name, error.__cause__)
        # Everything else of the package works beside it.
        print(batch.close, len(batch.to_candles()))
        """,
    )
    assert ran.returncode == 0, ran.stderr
    refused = f"ImportError: {needed}, and this is NumPy {version}: {install}"
    assert ran.stdout.splitlines() == [refused, refused, "True numpy None", "(Decimal('1'),) 1"]
    assert "panic" not in ran.stderr.lower()


@pytest.mark.parametrize(
    "module, cause",
    [
        ("", "AttributeError: module 'numpy' has no attribute '__version__'"),
        ("__version__ = (2, 0)", "NoneType: None"),
        ("__version__ = None", "NoneType: None"),
    ],
    ids=["no version", "a version that is no text", "a version of None"],
)
def test_what_imports_as_numpy_and_states_no_version_is_refused(
    module, cause, stood_in, needed, tmp_path
):
    # A directory named numpy that Python finds before NumPy, or in place of
    # it, imports as a module with nothing in it. The array bindings would
    # look for NumPy's arrays in it, and stop the call with a panic.
    ran = in_a_fresh_interpreter(
        stood_in(module),
        BATCH,
        """
        print(said(batch.to_numpy))
        print(said(lambda: qf.CandleBatch.from_numpy(**eight)))
        try:
            batch.to_numpy()
        except ImportError as error:
            print(f"{type(error.__cause__).__name__}: {error.__cause__}")
        """,
    )
    assert ran.returncode == 0, ran.stderr
    found = f"<module 'numpy' from {str(tmp_path / 'numpy' / '__init__.py')!r}>"
    stated = f"ImportError: {needed}, and what imports as numpy here states no version: {found}"
    # Where the module has no version to state, Python's own error for that
    # is what the refusal was raised from.
    assert ran.stdout.splitlines() == [stated, stated, cause]
    assert "panic" not in ran.stderr.lower()


# The parts of NumPy the array bindings go through on their way to its
# arrays, by name and with nothing of NumPy in them: how it reads its own
# version, and the module its arrays are kept in.
NAMED_AS_NUMPYS_PARTS = {
    "lib.py": """
        class NumpyVersion:
            def __init__(self, version):
                self.major = int(version.split(".")[0])
    """,
    "_core/__init__.py": "",
}

# What states a version the extra takes, and is not NumPy: a file or a
# package of that name with a version in it, what a test puts where NumPy
# is, and a package with every part of NumPy's the array bindings look for
# but the table of NumPy's own functions. The bindings panic on each.
ONLY_NAMED_NUMPY = {
    "a version and nothing else": ('__version__ = "2.4.6"', None, None),
    "a version and a zeros of its own": (
        '__version__ = "2.4.6"\n\n\ndef zeros(rows, dtype):\n    return [0] * rows',
        None,
        None,
    ),
    "a namespace with a version": (
        None,
        None,
        "import sys, types\n\nsys.modules['numpy'] = types.SimpleNamespace(__version__='2.4.6')\n",
    ),
    "a mock with a version": (
        None,
        None,
        "import sys, unittest.mock\n\nsys.modules['numpy'] = unittest.mock.MagicMock(__version__='2.4.6')\n",
    ),
    "its parts by name and no table of its functions": (
        '__version__ = "2.4.6"',
        {**NAMED_AS_NUMPYS_PARTS, "_core/multiarray.py": ""},
        None,
    ),
    "a table of its functions that is no capsule": (
        '__version__ = "2.4.6"',
        {**NAMED_AS_NUMPYS_PARTS, "_core/multiarray.py": "_ARRAY_API = object()"},
        None,
    ),
    "the capsule of another module": (
        '__version__ = "2.4.6"',
        {
            **NAMED_AS_NUMPYS_PARTS,
            "_core/multiarray.py": "import datetime\n\n_ARRAY_API = datetime.datetime_CAPI",
        },
        None,
    ),
}


@pytest.mark.parametrize("kind", ONLY_NAMED_NUMPY)
def test_what_states_a_version_and_does_not_hold_numpys_arrays_is_refused(kind, stood_in, needed):
    module, parts, put_in_place = ONLY_NAMED_NUMPY[kind]
    first = stood_in(module, parts) if module is not None else put_in_place
    ran = in_a_fresh_interpreter(
        first,
        BATCH,
        """
        print(said(batch.to_numpy))
        print(said(lambda: qf.CandleBatch.from_numpy(**eight)))
        try:
            batch.to_numpy()
        except ImportError as error:
            print(type(error) is ImportError, error.name, isinstance(error.__cause__, Exception))
        # Everything else of the package works beside it.
        print(batch.close, len(batch.to_candles()))
        """,
    )
    assert ran.returncode == 0, ran.stderr
    given, taken, raised, rest = ran.stdout.splitlines()
    stated = f"ImportError: {needed}, and what imports as numpy here does not hold NumPy's arrays: "
    # An error that says so, with what was looked for and not found as its
    # cause: not the panic of the array bindings, which is no `Exception`.
    assert given.startswith(stated), given
    assert taken == given
    assert raised == "True numpy True"
    assert rest == "(Decimal('1'),) 1"
    assert "panic" not in ran.stderr.lower()


def test_a_directory_named_numpy_is_no_numpy(tmp_path, without_numpy, needed):
    # Python imports a directory that has no `__init__.py` as a module with
    # nothing in it, and only where it finds no package of that name: so a
    # stray directory is taken for NumPy only where NumPy is not installed.
    (tmp_path / "numpy").mkdir()
    ran = in_a_fresh_interpreter(
        f"import sys\n\nsys.path.insert(0, {str(tmp_path)!r})\n",
        BATCH,
        """
        print(said(batch.to_numpy))
        print(said(lambda: qf.CandleBatch.from_numpy(**eight)))
        import numpy
        print(hasattr(numpy, "__version__"))
        """,
    )
    assert ran.returncode == 0, ran.stderr
    given, taken, is_numpy = ran.stdout.splitlines()
    if without_numpy:
        # It is shown as Python shows a module that is only a directory,
        # which is not the same in every version of Python.
        stated = f"ImportError: {needed}, and what imports as numpy here states no version: "
        assert given.startswith(stated + "<module 'numpy' ("), given
        assert "namespace" in given.lower()
        assert taken == given
        assert is_numpy == "False"
    else:
        # NumPy itself is found past it, and the eight that are no arrays
        # are refused as such.
        assert given == "nothing was raised"
        assert taken == "TypeError: open_time_ms must be a numpy.ndarray of int64, not list"
        assert is_numpy == "True"
    assert "panic" not in ran.stderr.lower()


@pytest.mark.parametrize(
    "module, raised",
    [
        (
            'raise ImportError("numpy is broken in a way of its own")',
            "ImportError: numpy is broken in a way of its own",
        ),
        (
            'raise RuntimeError("numpy is broken in another way")',
            "RuntimeError: numpy is broken in another way",
        ),
        # It was found, and failed under its own name: that is not NumPy
        # missing either.
        (
            "raise ImportError(\"numpy was found, and could not be loaded\", name='numpy')",
            "ImportError: numpy was found, and could not be loaded",
        ),
        # A module NumPy looked for and did not find is not NumPy missing.
        (
            "import a_module_no_one_has_installed",
            "ModuleNotFoundError: No module named 'a_module_no_one_has_installed'",
        ),
        (
            "raise ModuleNotFoundError(\"No module named 'numpy._core'\", name='numpy._core')",
            "ModuleNotFoundError: No module named 'numpy._core'",
        ),
        # Nor is an error of the module's own, raised as its version is asked for.
        (
            'def __getattr__(name):\n    raise RuntimeError(f"no {name} today")',
            "RuntimeError: no __version__ today",
        ),
    ],
    ids=[
        "an import error",
        "another error",
        "an import error that names it",
        "a module it lacks",
        "a part of it that is missing",
        "an error as its version is read",
    ],
)
def test_a_numpy_that_fails_as_it_is_imported_raises_its_own_error(module, raised, stood_in):
    ran = in_a_fresh_interpreter(
        stood_in(module),
        BATCH,
        """
        print(said(batch.to_numpy))
        print(said(lambda: qf.CandleBatch.from_numpy(**eight)))
        try:
            batch.to_numpy()
        except BaseException as error:
            print(error.__cause__)
        """,
    )
    assert ran.returncode == 0, ran.stderr
    # As it was raised, and not as a NumPy that is missing: what is wrong
    # with it is its own to say.
    assert ran.stdout.splitlines() == [raised, raised, "None"]


def test_an_interrupt_as_numpys_arrays_are_looked_for_is_raised_as_it_is(stood_in):
    # NumPy's arrays are looked for in a module that states a version the
    # extra takes. What stops that which is no failure to find them, as an
    # interrupt is none, is not taken for a module without them.
    ran = in_a_fresh_interpreter(
        stood_in('__version__ = "2.4.6"', {"lib.py": 'raise KeyboardInterrupt("stopped by hand")'}),
        BATCH,
        """
        print(said(batch.to_numpy))
        print(said(lambda: qf.CandleBatch.from_numpy(**eight)))
        """,
    )
    assert ran.returncode == 0, ran.stderr
    interrupted = "KeyboardInterrupt: stopped by hand"
    assert ran.stdout.splitlines() == [interrupted, interrupted]
    assert "panic" not in ran.stderr.lower()


def test_the_page_shows_what_a_conversion_says_without_a_numpy_it_takes(
    page, quoted, stood_in, extra, not_found, needed, install
):
    rows = page(PAGE).rows("without numpy")
    (none, older, newer) = [quoted(raised) for _, raised in rows]
    # Where Python finds none.
    assert none == not_found
    # And where the one it finds is one the extra does not take, by the
    # version the page names in each row: one older, and one newer.
    stated = {}
    for (where, _), raised in zip(rows[1:], (older, newer), strict=True):
        (version,) = re.findall(r" and this is NumPy (\S+): ", raised)
        assert raised == f"ImportError: {needed}, and this is NumPy {version}: {install}"
        assert f"here {version}" in where
        stated[version] = raised
    _, required = extra
    oldest, following = sorted(Version(bound.version) for bound in required.specifier)
    below, above = map(Version, stated)
    assert below < oldest and above >= following
    assert "older" in rows[1][0] and "newer" in rows[2][0]
    ran = in_a_fresh_interpreter(
        stood_in(STATES_A_VERSION),
        BATCH,
        f"""
        import numpy

        for version in {list(stated)!r}:
            numpy.__version__ = version
            print(said(batch.to_numpy))
        """,
    )
    assert ran.returncode == 0, ran.stderr
    assert ran.stdout.splitlines() == list(stated.values())
    ran = in_a_fresh_interpreter(KEPT_FROM_NUMPY, BATCH, "print(said(batch.to_numpy))")
    assert ran.returncode == 0, ran.stderr
    assert ran.stdout.splitlines() == [none]
    # The page gives the command itself, as the refusals do.
    assert f"`{install}`" in page(PAGE).text
