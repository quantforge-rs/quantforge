"""What the package's tests share: where the checkout is, its manifests, the
distribution under test, how a refusal and a `repr` are checked, how a
strategy is run in the engine, and how a result is compared with a fixture's.

The tests run against `quantforge_py` as it is installed, from a wheel or by
`maturin develop`, and hold it to the files of the checkout they are in.
"""

import decimal
import importlib.metadata
import json
import os
import pathlib
import tomllib

import pytest

import quantforge_py
from quantforge_py import _native

# quantforge-py/, and the repository it is a member of.
PACKAGE_DIR = pathlib.Path(__file__).resolve().parents[1]
REPOSITORY = PACKAGE_DIR.parent

DISTRIBUTION = "quantforge-py"

# Says that the build under test is the SQLite-only one
# (`--no-default-features`). Unset, the tests expect the package as it is
# published, with both stores.
SQLITE_ONLY = "QUANTFORGE_PY_TEST_SQLITE_ONLY"


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


def pytest_configure(config):
    """Stops the run on a value of the variable that says neither build."""
    said = os.environ.get(SQLITE_ONLY)
    if said not in (None, "1"):
        raise pytest.UsageError(
            f"{SQLITE_ONLY} is {said!r}: set it to 1 for the SQLite-only build, or leave it unset"
        )


@pytest.fixture(scope="session")
def storage_backends() -> tuple[str, ...]:
    """The stores the build under test is expected to open."""
    if os.environ.get(SQLITE_ONLY) == "1":
        return ("sqlite",)
    return ("sqlite", "postgres")


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
