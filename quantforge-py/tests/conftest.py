"""What the package's tests share: where the checkout is, its manifests, the
distribution under test, how a refusal and a `repr` are checked, how a
strategy is run in the engine, and how a result is compared with a fixture's.

The tests run against `quantforge_py` as it is installed, from a wheel or by
`maturin develop`, and hold it to the files of the checkout they are in.
"""

import decimal
import functools
import importlib.metadata
import importlib.util
import json
import os
import pathlib
import re
import tomllib
import warnings

import pytest

import quantforge_py
import stores
from quantforge_py import _native

# quantforge-py/, and the repository it is a member of.
PACKAGE_DIR = pathlib.Path(__file__).resolve().parents[1]
REPOSITORY = PACKAGE_DIR.parent

DISTRIBUTION = "quantforge-py"

# Says that the build under test is the SQLite-only one
# (`--no-default-features`). Unset, the tests expect the package as it is
# published, with both stores.
SQLITE_ONLY = "QUANTFORGE_PY_TEST_SQLITE_ONLY"

# Says that the run is on the oldest NumPy the package takes, the one
# `pyproject.toml` pins for it. Unset, the tests take any NumPy the package
# does.
OLDEST_NUMPY = "QUANTFORGE_PY_TEST_OLDEST_NUMPY"

# Says that the package under test is installed without its `numpy` extra,
# where there is no NumPy to import. Unset, the tests expect a NumPy, and
# the tests of the arrays run.
WITHOUT_NUMPY = "QUANTFORGE_PY_TEST_WITHOUT_NUMPY"

# Says that the run is on the oldest pandas the package takes, the one
# `pyproject.toml` pins for it. Unset, the tests take any pandas the package
# does.
OLDEST_PANDAS = "QUANTFORGE_PY_TEST_OLDEST_PANDAS"

# Says that the package under test is installed without its `pandas` extra,
# where there is no pandas to import. Unset, the tests expect a pandas, and
# the tests of the frames run. A run without NumPy is one without pandas
# too: pandas does not import without it.
WITHOUT_PANDAS = "QUANTFORGE_PY_TEST_WITHOUT_PANDAS"


def read(path: pathlib.Path) -> str:
    return path.read_text(encoding="utf-8")


def toml(path: pathlib.Path) -> dict:
    return tomllib.loads(read(path))


@pytest.fixture(scope="session")
def package_dir() -> pathlib.Path:
    return PACKAGE_DIR


@pytest.fixture(scope="session")
def repository() -> pathlib.Path:
    return REPOSITORY


@pytest.fixture(scope="session")
def crate() -> dict:
    """quantforge-py/Cargo.toml."""
    return toml(PACKAGE_DIR / "Cargo.toml")


@pytest.fixture(scope="session")
def core_crate() -> dict:
    """The repository's Cargo.toml: the `quantforge` crate and the workspace."""
    return toml(REPOSITORY / "Cargo.toml")


@pytest.fixture(scope="session")
def pyproject() -> dict:
    return toml(PACKAGE_DIR / "pyproject.toml")


@pytest.fixture(scope="session")
def distribution() -> importlib.metadata.Distribution:
    """The installed distribution the imported package belongs to."""
    return importlib.metadata.distribution(DISTRIBUTION)


@pytest.fixture(scope="session")
def editable(distribution) -> bool:
    """Whether `maturin develop` installed it, in place of a wheel."""
    record = distribution.read_text("direct_url.json")
    if record is None:
        return False
    return json.loads(record).get("dir_info", {}).get("editable", False)


class Page:
    """A page of the repository's documentation, as the tests read one: its
    tables and its lists sit between `<!-- begin: name -->` and `<!-- end:
    name -->` markers, and its examples in fences."""

    def __init__(self, path: pathlib.Path, name: str):
        self.name = name
        self.text = read(path)
        self.lines = self.text.splitlines()

    def section(self, name: str) -> list[str]:
        """The lines of a marked section, as written."""

        def marker(kind):
            text = f"<!-- {kind}: {name} -->"
            found = [at for at, line in enumerate(self.lines) if line.strip() == text]
            assert len(found) == 1, f"{self.name} has one {text}"
            return found[0]

        return self.lines[marker("begin") + 1 : marker("end")]

    def rows(self, name: str) -> list[list[str]]:
        """A marked table's rows after its header and separator, cell by
        cell. No cell of these tables holds a `|`."""
        return [
            [cell.strip() for cell in row.strip().strip("|").split("|")]
            for row in self.section(name)[2:]
        ]

    def fences(self, language: str) -> list[str]:
        """The text of every fence of `language`, in the page's order."""
        found, inside = [], None
        for line in self.lines:
            if inside is None and line.strip() == f"```{language}":
                inside = []
            elif inside is not None and line.strip() == "```":
                found.append("\n".join(inside) + "\n")
                inside = None
            elif inside is not None:
                inside.append(line)
        assert inside is None, f"{self.name} leaves a {language} fence open"
        return found


def code(cell: str) -> str:
    """What a table's cell quotes: the text of its one code span, which two
    backticks open when the text holds a backtick of its own."""
    ticks = "``" if cell.startswith("``") else "`"
    assert cell.startswith(ticks) and cell.endswith(ticks) and len(cell) > 2 * len(ticks), cell
    quoted = cell[len(ticks) : -len(ticks)]
    assert ticks not in quoted, cell
    return quoted


@pytest.fixture(scope="session")
def page():
    """Opens a page of the repository by its path from the root."""
    return lambda name: Page(REPOSITORY / name, name)


@pytest.fixture(scope="session")
def quoted():
    """Reads what a table's cell quotes."""
    return code


# Each extra of the package as a run is told of it: the module it brings,
# how the library is written, what of the tests needs it, and the variables
# that say a run has none of it or the oldest of it.
EXTRAS = [
    ("numpy", "NumPy", "arrays", WITHOUT_NUMPY, OLDEST_NUMPY),
    ("pandas", "pandas", "frames", WITHOUT_PANDAS, OLDEST_PANDAS),
]


def pytest_configure(config):
    """Stops the run on a value of a variable that says neither of the two
    things it can, and on a run that is not what it was said to be: one with
    a NumPy or a pandas that was said to have none, or one without that was
    not.

    The tests of the arrays are skipped where NumPy is not there, and those
    of the frames where pandas is not, so a run that lacked one by mistake
    would pass without them. It is stopped here instead."""
    for variable, means in [
        (SQLITE_ONLY, "for the SQLite-only build"),
        (OLDEST_NUMPY, "for a run on the oldest NumPy"),
        (WITHOUT_NUMPY, "for a run of the package installed without NumPy"),
        (OLDEST_PANDAS, "for a run on the oldest pandas"),
        (WITHOUT_PANDAS, "for a run of the package installed without pandas"),
    ]:
        said = os.environ.get(variable)
        if said not in (None, "1"):
            raise pytest.UsageError(
                f"{variable} is {said!r}: set it to 1 {means}, or leave it unset"
            )
    for module, library, tested, without, oldest in EXTRAS:
        none = os.environ.get(without) == "1"
        # Looked for, and not imported: the tests hold the package to
        # importing an extra only when something that needs it is asked for.
        found = importlib.util.find_spec(module) is not None
        if none and os.environ.get(oldest) == "1":
            raise pytest.UsageError(f"{without} and {oldest} are both 1: a run is one or the other")
        if none and found:
            raise pytest.UsageError(
                f"{without} is 1, and {library} is installed: unset it, or run where there is none"
            )
        if not none and not found:
            raise pytest.UsageError(
                f"{library} is not installed, and the tests of the {tested} need it: install the "
                f"package's {module} extra, or set {without}=1 to test the package without it"
            )
    if os.environ.get(WITHOUT_NUMPY) == "1" and os.environ.get(WITHOUT_PANDAS) != "1":
        raise pytest.UsageError(
            f"{WITHOUT_NUMPY} is 1 and {WITHOUT_PANDAS} is not: pandas does not import without "
            "NumPy, so a run without NumPy is one without pandas, and is told both"
        )


@pytest.fixture(scope="session")
def storage_backends() -> tuple[str, ...]:
    """The stores the build under test is expected to open."""
    if os.environ.get(SQLITE_ONLY) == "1":
        return ("sqlite",)
    return ("sqlite", "postgres")


@pytest.fixture(scope="session")
def oldest_numpy() -> bool:
    """Whether the run was told that it is on the oldest NumPy the package
    takes."""
    return os.environ.get(OLDEST_NUMPY) == "1"


@pytest.fixture(scope="session")
def without_numpy() -> bool:
    """Whether the run is of the package installed without its `numpy`
    extra, with no NumPy to import."""
    return os.environ.get(WITHOUT_NUMPY) == "1"


@pytest.fixture(scope="session")
def oldest_pandas() -> bool:
    """Whether the run was told that it is on the oldest pandas the package
    takes."""
    return os.environ.get(OLDEST_PANDAS) == "1"


@pytest.fixture(scope="session")
def without_pandas() -> bool:
    """Whether the run is of the package installed without its `pandas`
    extra, with no pandas to import."""
    return os.environ.get(WITHOUT_PANDAS) == "1"


@pytest.fixture(autouse=True)
def no_storage_setting_from_outside(monkeypatch):
    """Every test starts without the variables the reader reads its
    settings from, whatever the shell that runs the tests exports: a test
    that wants one sets it."""
    for variable in stores.SETTINGS:
        monkeypatch.delenv(variable, raising=False)


# The variable a client reads its token from where it is given none: the
# command line's, for `--remote-token`.
REMOTE_TOKEN = "QF_REMOTE_TOKEN"


@pytest.fixture(autouse=True)
def no_token_from_outside(monkeypatch):
    """Every test starts without the variable a client reads its token
    from, whatever the shell that runs the tests exports: a test that wants
    one sets it."""
    monkeypatch.delenv(REMOTE_TOKEN, raising=False)


@pytest.fixture(scope="session")
def protocol_codes(repository) -> dict[str, int]:
    """Every code of the protocol's error envelope, with its HTTP status, in
    the protocol's order: the table the crate's own tests hold its
    `ErrorCode` to."""
    source = read(repository / "src" / "protocol" / "errors.rs")
    ((count, rows),) = re.findall(
        r"const CODES: \[\(ErrorCode, &str, u16\); (\d+)\] = \[(.*?)\];", source, re.S
    )
    codes = {name: int(status) for name, status in re.findall(r'"(\w+)", (\d+)\)', rows)}
    assert len(codes) == int(count) == 10
    return codes


@pytest.fixture(scope="session")
def protocol_states(repository) -> list[str]:
    """Every state the protocol names for a job, in its order, as it is
    written on the wire: the variants of the crate's `JobState` but the one
    no server sends."""
    source = read(repository / "src" / "protocol" / "jobs" / "status.rs")
    (variants,) = re.findall(r"^pub enum JobState \{\n(.*?)^\}", source, re.S | re.M)
    named = re.findall(r"^    (\w+)(?: \{|,)$", variants, re.M)
    assert named[-1] == "Unrecognized" and len(named) == 6
    return [name.lower() for name in named[:-1]]


@pytest.fixture(scope="session")
def postgres_server(storage_backends):
    """The PostgreSQL server the reader's tests write and read stores on:
    the one `QUANTFORGE_TEST_POSTGRES_URL` names. Without it a test that
    needs it skips, unless the run was told that PostgreSQL has to be
    tested, and the SQLite-only build, which opens no such store, always
    skips."""
    if "postgres" not in storage_backends:
        pytest.skip("the SQLite-only build opens no PostgreSQL store")
    server = stores.server()
    if server is None:
        pytest.skip(f"{stores.POSTGRES_URL} is unset")
    return server


@pytest.fixture(params=["sqlite", "postgres"])
def place(request, tmp_path):
    """Where a store lives, on each backend in turn: a SQLite file that is
    not there yet, and a PostgreSQL database of its own with no table yet,
    which is dropped after the test. A test writes it with the command
    line, `place.store(klines)`."""
    if request.param == "sqlite":
        yield stores.Place.sqlite(tmp_path)
        return
    with stores.Database(request.getfixturevalue("postgres_server")) as database:
        yield stores.Place.postgres(database, tmp_path)


# The functions that give or take a pandas DataFrame, in the order `__all__`
# gives them.
FRAMES = [
    "candles_to_dataframe",
    "dataframe_to_candles",
    "trades_to_dataframe",
    "equity_to_dataframe",
]


@pytest.fixture(scope="module")
def no_warning_drawn_by_the_package():
    """Makes any warning drawn while one of the four frame functions runs an
    error, for the length of a module's tests.

    pandas and NumPy say with a warning that what was asked of them is going
    to change. A frame is the same on every pandas because nothing it is
    made with is, so making one or reading one draws no warning: whichever
    code the warning names, the package's own, pandas' or NumPy's, and
    whatever is asked on the way, a batch's arrays among it. A warning a
    test's own code draws, outside those four, is the test's own, and is
    left a warning: a test may use what pandas is about to change.

    The four are held so under the names the package publishes them by, so
    a test calls each through `quantforge_py`, looked up as the test runs:
    one taken from the package as a module of tests is imported is the
    function as it was before."""

    def strict(function):
        @functools.wraps(function)
        def called(*args, **kwargs):
            with warnings.catch_warnings():
                warnings.simplefilter("error")
                return function(*args, **kwargs)

        return called

    with pytest.MonkeyPatch.context() as patched:
        for name in FRAMES:
            patched.setattr(quantforge_py, name, strict(getattr(quantforge_py, name)))
        yield


@pytest.fixture(scope="session")
def refused():
    """Holds a call to its refusal: this exception itself, not one derived
    from it, and this message word for word."""

    def refused(kind, message, call, *args, **kwargs):
        with pytest.raises(kind) as caught:
            call(*args, **kwargs)
        assert type(caught.value) is kind
        assert str(caught.value) == message

    return refused


@pytest.fixture(scope="session")
def evaluated():
    """What a `repr` evaluates back to, where the package's own names and
    `Decimal` are all it may name."""
    names = {name: getattr(quantforge_py, name) for name in quantforge_py.__all__}
    names["Decimal"] = decimal.Decimal

    def evaluated(text):
        return eval(text, {"__builtins__": {}}, names)

    return evaluated


@pytest.fixture(scope="session")
def as_given():
    """Writes a result as the shared fixtures write one, to be compared with
    what the engine's own tests wrote: every amount as the text of its
    decimal, with every digit it has and no exponent."""

    def written(value) -> str:
        assert type(value) is decimal.Decimal, value
        return format(value, "f")

    def as_given(result) -> dict:
        assert type(result) is quantforge_py.BacktestResult, result
        given = {
            field: written(getattr(result, field))
            for field in ("initial_cash", "final_equity", "total_return_pct", "max_drawdown_pct")
        }
        given["trade_count"] = result.trade_count
        given["trades"] = [
            dict(
                symbol=str(trade.symbol),
                entry_time_ms=trade.entry_time_ms,
                exit_time_ms=trade.exit_time_ms,
                entry_order_id=trade.entry_order_id,
                exit_order_id=trade.exit_order_id,
                **{
                    field: written(getattr(trade, field))
                    for field in ("entry_price", "exit_price", "qty", "gross_quote_pnl")
                },
            )
            for trade in result.trades
        ]
        given["equity_curve"] = [
            dict(open_time_ms=point.open_time_ms, equity=written(point.equity))
            for point in result.equity_curve
        ]
        return given

    return as_given


@pytest.fixture(scope="session")
def bench():
    """The engine run on a Python strategy, on either kind of host:
    `bench(market, candles, strategy, config, *, embedded=False)` gives the
    result.

    As Python hosts a run, it is `run_backtest` itself, the function a user
    calls: on the calling thread, where a callback that fails raises its own
    exception. As a host that embeds Python runs one (`embedded=True`), it
    is the extension module's private entry, which takes the same arguments:
    on a thread of the engine's own, where a failure is a `RuntimeError` of
    the engine's error text, which is all such a host has."""

    def bench(market, candles, strategy, config, *, embedded=False):
        run = _native._embedded_backtest if embedded else quantforge_py.run_backtest
        return run(candles, strategy, config, market=market)

    return bench


@pytest.fixture(params=[False, True], ids=["hosted by python", "embedded"])
def embedded(request) -> bool:
    """Each of the two ways a run is hosted, in turn."""
    return request.param


@pytest.fixture(scope="session")
def market() -> quantforge_py.MarketId:
    return quantforge_py.MarketId("binance_spot", "BTCUSDT", "1m")


@pytest.fixture(scope="session")
def minutes():
    """One-minute candles, the first at time zero, from each one's open and
    close."""

    def minutes(*prices) -> list[quantforge_py.Candle]:
        return [
            quantforge_py.Candle(
                open_time_ms=index * 60_000,
                close_time_ms=index * 60_000 + 59_999,
                open=open,
                high=max(open, close),
                low=min(open, close),
                close=close,
                volume=1,
            )
            for index, (open, close) in enumerate(prices)
        ]

    return minutes


@pytest.fixture
def run(bench, market, minutes, embedded):
    """Runs a strategy on the host under test: over five quiet bars, with
    1000 in cash and no fee, unless told otherwise."""
    quiet = minutes(*[(100, 100)] * 5)

    def run(strategy, candles=None, *, initial_cash=1000, fee_bps=0, close_out_at_end=True):
        config = quantforge_py.BacktestConfig(
            initial_cash=initial_cash, fee_bps=fee_bps, close_out_at_end=close_out_at_end
        )
        candles = quiet if candles is None else candles
        return bench(market, candles, strategy, config, embedded=embedded)

    return run
