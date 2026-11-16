"""The golden parity test: the SMA cross written in Python makes the trades
the Rust one made, digit for digit.

`tests/fixtures/golden/` holds one frozen day of candles (`candles.csv`),
what QuantForge's built-in Rust `SmaCrossStrategy` made of them
(`sma_cross.json`), and the same strategy written in Python
(`sma_cross.py`). Here the Python one is run over the candles through
`run_backtest`, and as a host that embeds Python runs one. It has to decide
as the Rust one did on every bar and to bring the same result: every trade,
the equity at every bar, the final equity and the drawdown, each compared
as the text of its decimal. Nothing is compared within a tolerance, and
nothing as a float.

The two data files are frozen (`docs/golden-parity.md`). The crate's own
tests hold them to the Rust strategy for as long as it exists, and to the
engine alone (`tests/golden/`). A failure here is a change in the engine,
in what crosses the strategy boundary, or in the strategy written in
Python, and is never mended by changing the files.

The fixture is then shown to tell apart what it should: a strategy that
differs from the Rust one in a small way trades differently on it, whether
it takes equal averages for a signal, decides too early or a bar late,
rounds its averages to the cent, or averages floats in each of three ways.
A fourth way of averaging floats makes the same trades, and is caught by
its decisions: the goldens record one for every bar."""

import ast
import csv
import decimal
import doctest
import functools
import hashlib
import importlib.util
import inspect
import itertools
import json
import math
import pathlib
import sys
from collections import deque
from decimal import Decimal
from fractions import Fraction

import pytest

import quantforge_py as qf

PAGE = "docs/golden-parity.md"

REPOSITORY = pathlib.Path(__file__).resolve().parents[2]
GOLDEN = REPOSITORY / "tests" / "fixtures" / "golden"
EXPECTED = json.loads((GOLDEN / "sma_cross.json").read_text("utf-8"))

CANDLE_DECIMALS = ["open", "high", "low", "close", "volume"]

LONG, FLAT = qf.TargetPosition.LONG_ALL_IN, qf.TargetPosition.FLAT

# A decision as the file spells it, which is the engine's name for it.
TARGETS = {"LongAllIn": LONG, "Flat": FLAT, None: None}


def entry(path: pathlib.Path, name: str) -> type:
    """A strategy's class, from its file: the file is imported by its path,
    under no name another import could find, and no bytecode is written
    beside it, so the fixture's directory holds what it held."""
    spec = importlib.util.spec_from_file_location(path.stem, path)
    module = importlib.util.module_from_spec(spec)
    written, sys.dont_write_bytecode = sys.dont_write_bytecode, True
    try:
        spec.loader.exec_module(module)
    finally:
        sys.dont_write_bytecode = written
    return getattr(module, name)


SmaCross = entry(GOLDEN / "sma_cross.py", "SmaCross")


def rows() -> list[dict]:
    """The dataset's lines, each field as the file writes it."""
    with open(GOLDEN / "candles.csv", newline="", encoding="utf-8") as file:
        return list(csv.DictReader(file))


def candle_of(row: dict) -> qf.Candle:
    return qf.Candle(
        open_time_ms=int(row["open_time_ms"]),
        close_time_ms=int(row["close_time_ms"]),
        # A trade count the exchange did not report is an empty field.
        trades=int(row["trades"]) if row["trades"] else None,
        **{field: Decimal(row[field]) for field in CANDLE_DECIMALS},
    )


@pytest.fixture(scope="module")
def candles() -> list[qf.Candle]:
    return [candle_of(row) for row in rows()]


@pytest.fixture(scope="module")
def config() -> qf.BacktestConfig:
    settings = EXPECTED["config"]
    return qf.BacktestConfig(
        initial_cash=Decimal(settings["initial_cash"]),
        fee_bps=Decimal(settings["fee_bps"]),
        close_out_at_end=settings["close_out_at_end"],
    )


@pytest.fixture(scope="module")
def golden_market() -> qf.MarketId:
    return qf.MarketId(**EXPECTED["dataset"]["market"])


def watched(strategy: type) -> type:
    """A strategy's class, with every decision it makes written down."""

    class Watched(strategy):
        def __init__(self, *args, **kwargs):
            super().__init__(*args, **kwargs)
            self.decisions = []

        def on_bar(self, ctx, bar):
            decision = super().on_bar(ctx, bar)
            self.decisions.append(decision)
            return decision

    return Watched


@pytest.fixture(scope="module")
def ran(bench, candles, config, golden_market):
    """Runs a strategy's class over the frozen candles as the goldens were
    run: the strategy, with its decisions, and the result. The windows are
    the goldens' unless others are named."""

    def ran(strategy: type, *, embedded=False, **windows):
        made = watched(strategy)(**(windows or EXPECTED["strategy"]["params"]))
        return made, bench(golden_market, candles, made, config, embedded=embedded)

    return ran


def recorded() -> list:
    """What the Rust strategy decided on each bar."""
    return [TARGETS[decision] for decision in EXPECTED["decisions"]]


def first_difference(decisions: list) -> tuple | None:
    """The first bar a strategy decided otherwise than the Rust one did:
    its place, what was decided there, and what the Rust one decided."""
    assert len(decisions) == len(EXPECTED["decisions"])
    for bar, (made, given) in enumerate(zip(decisions, recorded(), strict=True)):
        if made is not given:
            return bar, made, given
    return None


def test_the_frozen_files_are_a_day_of_candles_and_the_rust_strategys_results(candles):
    assert EXPECTED["format"] == 1
    dataset = EXPECTED["dataset"]
    # The results are of these candles, byte for byte.
    assert hashlib.sha256((GOLDEN / "candles.csv").read_bytes()).hexdigest() == dataset["sha256"]
    assert dataset["market"] == dict(exchange="binance_spot", symbol="BTCUSDT", interval="1m")
    assert len(candles) == dataset["candles"] == 1440
    # One minute after another from noon, 2026-01-01, UTC.
    assert candles[0].open_time_ms == dataset["first_open_ms"] == 1_767_268_800_000
    assert candles[-1].open_time_ms == dataset["last_open_ms"]
    assert all(
        (after.open_time_ms, bar.close_time_ms)
        == (bar.open_time_ms + 60_000, bar.open_time_ms + 59_999)
        for bar, after in itertools.pairwise(candles)
    )
    # Every digit of the file is in the candles: a price with its eight
    # decimal places, and a volume of zero with the eight it has.
    for row, candle in zip(rows(), candles, strict=True):
        for field in CANDLE_DECIMALS:
            assert format(getattr(candle, field), "f") == row[field]
        assert str(candle.trades) == row["trades"]
    assert rows()[0]["open"] == "82499.72000000"
    assert any(row["volume"] == "0.00000000" for row in rows())
    # They are the run of the built-in Rust strategy, at the command line's
    # defaults.
    assert EXPECTED["oracle"]["strategy"] == "quantforge::SmaCrossStrategy"
    assert EXPECTED["strategy"] == dict(name="sma_cross", params=dict(fast=20, slow=50))
    assert EXPECTED["config"] == dict(initial_cash="10000", fee_bps="10", close_out_at_end=True)
    assert len(EXPECTED["decisions"]) == len(EXPECTED["result"]["equity_curve"]) == 1440
    assert set(EXPECTED["decisions"]) == set(TARGETS)


def test_the_strategy_is_one_file_with_one_class_as_a_bundle_enters_it():
    assert issubclass(SmaCross, qf.Strategy)
    # It is named as the Rust one is, and takes its two windows by name, as
    # a bundle's parameters reach a strategy.
    assert SmaCross.name == EXPECTED["strategy"]["name"]
    taken = inspect.signature(SmaCross).parameters
    assert list(taken) == list(EXPECTED["strategy"]["params"]) == ["fast", "slow"]
    assert all(window.default is inspect.Parameter.empty for window in taken.values())
    # Nothing is needed beside the file but the package and Python itself.
    source = ast.parse((GOLDEN / "sma_cross.py").read_text("utf-8"))
    imported = {
        (node.module if isinstance(node, ast.ImportFrom) else alias.name).partition(".")[0]
        for node in ast.walk(source)
        if isinstance(node, ast.Import | ast.ImportFrom)
        for alias in node.names
    }
    assert imported == {"collections", "quantforge_py"}
    classes = [node.name for node in source.body if isinstance(node, ast.ClassDef)]
    assert classes == ["SmaCross"]


def test_the_python_strategy_decides_as_the_rust_one_did_on_every_bar(ran, embedded):
    strategy, _ = ran(SmaCross, embedded=embedded)
    differs = first_difference(strategy.decisions)
    assert differs is None, f"bar {differs[0]}: decided {differs[1]}, and the Rust one {differs[2]}"
    # All three decisions are among them: long, flat, and none.
    assert set(strategy.decisions) == {LONG, FLAT, None}


def test_it_makes_the_same_trades_final_equity_and_drawdown_digit_for_digit(
    ran, embedded, as_given
):
    _, result = ran(SmaCross, embedded=embedded)
    given, expected = as_given(result), EXPECTED["result"]
    # The trade list, the final equity and the drawdown, each amount as the
    # text of its decimal.
    assert given["trades"] == expected["trades"]
    assert given["final_equity"] == expected["final_equity"] == "9921.202457428483046329087897"
    assert given["max_drawdown_pct"] == expected["max_drawdown_pct"]
    assert given["max_drawdown_pct"] == "1.8091109103945370264444945700"
    # And the rest of the result: the return, and the equity at every bar.
    assert given["total_return_pct"] == "-0.7879754257151695367091210300"
    assert given["equity_curve"] == expected["equity_curve"]
    assert given == expected
    assert result.trade_count == len(result.trades) == 15


@functools.cache
def exactly(fast: int, slow: int) -> list:
    """What comparing the two averages of the dataset's closes decides in
    exact arithmetic, where no sum and no division is rounded."""
    held = [Fraction(row["close"]) for row in rows()]
    decisions = []
    for bars in range(1, len(held) + 1):
        if bars < slow:
            decisions.append(None)
            continue
        quick = sum(held[bars - fast : bars]) / fast
        slower = sum(held[bars - slow : bars]) / slow
        decisions.append(LONG if quick > slower else FLAT if quick < slower else None)
    return decisions


# The windows of the goldens, 20 and 50, divide a sum of prices evenly: no
# average of theirs is rounded at all. The other two pairs do not, so their
# averages are rounded, to 28 digits here and to the 28 or 29 the engine's
# decimal holds in the Rust strategy. Both still compare as exact arithmetic
# does, which the crate's tests show of the Rust one over the same windows
# (`tests/golden/oracle.rs`).
@pytest.mark.parametrize("fast, slow, even", [(20, 50, True), (7, 30, False), (3, 13, False)])
def test_its_averages_compare_as_exact_arithmetic_would(ran, fast, slow, even):
    with decimal.localcontext(decimal.Context()) as context:
        strategy, _ = ran(SmaCross, fast=fast, slow=slow)
        rounded = bool(context.flags[decimal.Inexact])
    assert rounded is not even
    assert strategy.decisions == exactly(fast, slow)
    if (fast, slow) == tuple(EXPECTED["strategy"]["params"].values()):
        assert first_difference(strategy.decisions) is None


def averages(strategy) -> tuple[Decimal, Decimal]:
    """The fast and the slow average of the closes a strategy holds."""
    closes = list(strategy.closes)
    return sum(closes[-strategy.fast :]) / strategy.fast, sum(closes) / strategy.slow


class EqualIsLong(SmaCross):
    """Takes equal averages for a reason to be long."""

    def on_bar(self, ctx, bar):
        self.closes.append(bar.close)
        if len(self.closes) < self.slow:
            return None
        fast, slow = averages(self)
        return LONG if fast >= slow else FLAT


class EqualIsFlat(SmaCross):
    """Takes equal averages for a reason to be flat."""

    def on_bar(self, ctx, bar):
        self.closes.append(bar.close)
        if len(self.closes) < self.slow:
            return None
        fast, slow = averages(self)
        return LONG if fast > slow else FLAT


class NoWarmUp(SmaCross):
    """Averages the closes it has, before the slow window is full."""

    def on_bar(self, ctx, bar):
        self.closes.append(bar.close)
        closes = list(self.closes)
        recent = closes[-self.fast :]
        fast, slow = sum(recent) / len(recent), sum(closes) / len(closes)
        return LONG if fast > slow else FLAT if fast < slow else None


class WithoutTheBar(SmaCross):
    """Decides from the bars before the one it is handed."""

    def on_bar(self, ctx, bar):
        decision = None
        if len(self.closes) == self.slow:
            fast, slow = averages(self)
            decision = LONG if fast > slow else FLAT if fast < slow else None
        self.closes.append(bar.close)
        return decision


class ToTheCent(SmaCross):
    """Rounds its two averages to the cent, the market's tick, before it
    compares them."""

    def on_bar(self, ctx, bar):
        self.closes.append(bar.close)
        if len(self.closes) < self.slow:
            return None
        fast, slow = (average.quantize(Decimal("0.01")) for average in averages(self))
        return LONG if fast > slow else FLAT if fast < slow else None


class InFloats(SmaCross):
    """Keeps its two sums in floats, the newest close added and the oldest
    taken off, as the Rust indicator keeps its own in decimals: the sums
    drift apart in their last bits, the same on every Python."""

    def __init__(self, fast, slow):
        super().__init__(fast, slow)
        self.sums = {fast: 0.0, slow: 0.0}
        self.kept = {fast: deque(), slow: deque()}

    def on_bar(self, ctx, bar):
        close = float(bar.close)
        for window, kept in self.kept.items():
            kept.append(close)
            self.sums[window] += close
            if len(kept) > window:
                self.sums[window] -= kept.popleft()
        if len(self.kept[self.slow]) < self.slow:
            return None
        fast, slow = self.sums[self.fast] / self.fast, self.sums[self.slow] / self.slow
        return LONG if fast > slow else FLAT if fast < slow else None


def summed_in_floats(add) -> type:
    """A strategy that averages floats, adding the closes of each window up
    anew on every bar with `add`."""

    class SummedInFloats(SmaCross):
        def on_bar(self, ctx, bar):
            self.closes.append(float(bar.close))
            if len(self.closes) < self.slow:
                return None
            closes = list(self.closes)
            fast, slow = add(closes[-self.fast :]) / self.fast, add(closes) / self.slow
            return LONG if fast > slow else FLAT if fast < slow else None

    return SummedInFloats


# The first hour in which the market stands still begins at bar 392, and
# from bar 440 the slow average holds that one price alone: the two averages
# are equal there for the first time, while the strategy is long.
FIRST_EQUAL = 440

# The quiet stretch begins at bar 766, and from bar 814 both averages hold
# its two prices alone. On bar 816 the two are equal, though neither window
# holds one price alone. On bar 817 the fast one is three ten-thousandths
# below, and that is the first difference rounding to the cent hides: on
# bars 814 and 815 the two are as close, and round to different cents.
FIRST_EQUAL_OF_OTHER_BARS = 816
FIRST_HIDDEN_BY_ROUNDING = 817

# Each strategy that differs, with the first bar it decides otherwise at,
# what it decides there, and what the Rust strategy decided.
DIFFERS = {
    "equal averages taken for long": (EqualIsLong, FIRST_EQUAL, LONG, None),
    "equal averages taken for flat": (EqualIsFlat, FIRST_EQUAL, FLAT, None),
    "no warm-up": (NoWarmUp, 20, LONG, None),
    "without the bar's own close": (WithoutTheBar, 49, None, LONG),
    "averages rounded to the cent": (ToTheCent, FIRST_HIDDEN_BY_ROUNDING, None, FLAT),
    "running sums kept in floats": (InFloats, FIRST_EQUAL, FLAT, None),
    # Summed exactly, floats still average the same two prices to one float
    # over twenty bars and to another over fifty: the fault is in the float,
    # not in the adding up.
    "floats summed exactly": (
        summed_in_floats(math.fsum),
        FIRST_EQUAL_OF_OTHER_BARS,
        FLAT,
        None,
    ),
}


@pytest.mark.parametrize("differs", DIFFERS.values(), ids=DIFFERS.keys())
def test_a_strategy_that_differs_in_a_small_way_trades_differently(ran, as_given, differs):
    strategy, bar, decided, given = differs
    made, result = ran(strategy)
    assert first_difference(made.decisions) == (bar, decided, given)
    # And the difference reaches the result: other trades, and another
    # final equity.
    assert as_given(result)["trades"] != EXPECTED["result"]["trades"]
    assert format(result.final_equity, "f") != EXPECTED["result"]["final_equity"]


def test_pythons_own_sum_of_floats_does_not_reproduce_the_goldens_on_any_version(ran, as_given):
    # Python's `sum` adds floats one after another up to 3.11, and with a
    # correction that makes it far more accurate from 3.12 on. The first
    # finds the averages of one price apart, in the first still hour; the
    # second finds them equal there, and is caught where the two windows
    # hold other bars.
    made, result = ran(summed_in_floats(sum))
    corrected = sys.version_info >= (3, 12)
    first = FIRST_EQUAL_OF_OTHER_BARS if corrected else FIRST_EQUAL
    assert first_difference(made.decisions) == (first, FLAT, None)
    assert as_given(result)["trades"] != EXPECTED["result"]["trades"]


class FromOneTotal(SmaCross):
    """Keeps one running total of its closes in floats, and takes the sum of
    a window as the difference of two of its totals."""

    def __init__(self, fast, slow):
        super().__init__(fast, slow)
        self.totals = [0.0]

    def on_bar(self, ctx, bar):
        self.totals.append(self.totals[-1] + float(bar.close))
        if len(self.totals) <= self.slow:
            return None
        fast = (self.totals[-1] - self.totals[-1 - self.fast]) / self.fast
        slow = (self.totals[-1] - self.totals[-1 - self.slow]) / self.slow
        return LONG if fast > slow else FLAT if fast < slow else None


def test_a_strategy_can_make_the_same_trades_and_is_still_told_by_its_decisions(ran, as_given):
    # This one finds the fast average above the slow one on 18 bars where the
    # two are equal, and decides to be long on each. It is long already on
    # every one of them, so its result is the goldens', digit for digit. The
    # trades alone would not tell it from the Rust strategy; the decisions,
    # which the goldens record for every bar, do.
    made, result = ran(FromOneTotal)
    assert first_difference(made.decisions) == (FIRST_EQUAL, LONG, None)
    otherwise = [
        bar
        for bar, (decided, given) in enumerate(zip(made.decisions, recorded(), strict=True))
        if decided is not given
    ]
    assert len(otherwise) == 18
    assert all(made.decisions[bar] is LONG and recorded()[bar] is None for bar in otherwise)
    assert as_given(result) == EXPECTED["result"]


def test_equal_averages_are_met_while_long_and_while_flat(ran):
    # Once its slow average was full, the Rust strategy decided nothing only
    # where the two averages were equal: on twelve bars of each still hour,
    # and on eight of the quiet stretch.
    equal = [bar for bar, given in enumerate(recorded()) if bar >= 49 and given is None]
    assert len(equal) == 32 and equal[0] == FIRST_EQUAL
    assert FIRST_EQUAL_OF_OTHER_BARS in equal
    # Taken for long, equal averages change nothing while the strategy is
    # long, and put it in while it is flat; taken for flat, the other way
    # round. The dataset holds both, so each mistake decides otherwise on
    # just those bars, and each costs a trade.
    for strategy in (EqualIsLong, EqualIsFlat):
        made, result = ran(strategy)
        otherwise = [
            bar
            for bar, (decided, given) in enumerate(zip(made.decisions, recorded(), strict=True))
            if decided is not given
        ]
        assert otherwise == equal
        assert result.trade_count == EXPECTED["result"]["trade_count"] + 1


# A run Python hosts is on the thread that asked for it, and the strategy's
# own arithmetic follows that thread's decimal context. What the context has
# to hold is the sum of fifty prices, of nine digits; from there on, with
# windows of 20 and 50, nothing the strategy computes is rounded at all.
@pytest.mark.parametrize("precision", [9, 28, 60])
def test_the_result_is_the_same_at_any_precision_that_holds_the_sum_of_fifty_prices(
    ran, as_given, precision
):
    with decimal.localcontext(decimal.Context(prec=precision)) as context:
        strategy, result = ran(SmaCross)
        given = as_given(result)
        rounded = bool(context.flags[decimal.Inexact])
    assert not rounded
    assert first_difference(strategy.decisions) is None
    assert given == EXPECTED["result"]


def test_a_precision_that_holds_each_price_and_not_their_sum_changes_the_trades(ran, as_given):
    # Eight digits hold every price, of seven, and not the sum of fifty of
    # them. The two sums are rounded apart, so where the averages are equal
    # the strategy finds one below the other, and decides: the context a
    # strategy computes in is one of its inputs.
    with decimal.localcontext(decimal.Context(prec=8)) as context:
        strategy, result = ran(SmaCross)
        given = as_given(result)
        rounded = bool(context.flags[decimal.Inexact])
    assert rounded
    assert first_difference(strategy.decisions) == (FIRST_EQUAL, FLAT, None)
    assert given["trades"] != EXPECTED["result"]["trades"]
    assert result.trade_count != EXPECTED["result"]["trade_count"]


ROUNDINGS = [
    decimal.ROUND_HALF_EVEN,
    decimal.ROUND_HALF_UP,
    decimal.ROUND_HALF_DOWN,
    decimal.ROUND_UP,
    decimal.ROUND_DOWN,
    decimal.ROUND_CEILING,
    decimal.ROUND_FLOOR,
    decimal.ROUND_05UP,
]


# Where an average is rounded, how it is rounded has no say either. Two
# averages of these closes that differ at all differ by a tick over the
# product of the two windows, at least 0.01 / (7 * 30), and at eleven digits
# a quotient is right to a millionth.
@pytest.mark.parametrize("rounding", ROUNDINGS)
@pytest.mark.parametrize("precision", [11, 28])
@pytest.mark.parametrize("fast, slow", [(7, 30), (3, 13)])
def test_rounded_averages_compare_as_exact_ones_in_every_rounding_mode(
    ran, fast, slow, precision, rounding
):
    with decimal.localcontext(decimal.Context(prec=precision, rounding=rounding)) as context:
        strategy, _ = ran(SmaCross, fast=fast, slow=slow)
        rounded = bool(context.flags[decimal.Inexact])
    assert rounded
    assert strategy.decisions == exactly(fast, slow)


class Replays(qf.Strategy):
    """Plays the Rust strategy's decisions back, one a bar, and computes
    nothing of its own: it only writes down what it is handed."""

    def __init__(self):
        self.decisions = iter(recorded())
        self.handed = []

    def on_bar(self, ctx, bar):
        self.handed.append(
            (format(bar.close, "f"), format(ctx.cash, "f"), format(ctx.position_qty, "f"))
        )
        return next(self.decisions)


def test_the_engines_own_figures_follow_no_decimal_context(
    bench, candles, config, golden_market, embedded, as_given
):
    # A context of three digits that refuses to round at all: any figure
    # computed or read under it in Python's arithmetic would raise. The
    # engine's are computed in its own decimal and handed over as they are,
    # to a callback as in the result.
    strict = decimal.Context(prec=3, traps=[decimal.Inexact, decimal.Rounded])
    strategy = Replays()
    with decimal.localcontext(strict):
        result = bench(golden_market, candles, strategy, config, embedded=embedded)
        given = as_given(result)
    assert given == EXPECTED["result"]
    # Every bar came with its close as the file writes it, and the cash and
    # the position with the digits the engine holds: the entry at bar 50
    # leaves a residue of cash in the last of them.
    assert [close for close, _, _ in strategy.handed] == [row["close"] for row in rows()]
    cash, held = strategy.handed[50][1:]
    assert Decimal(0) < Decimal(cash) < Decimal("1E-20") and len(held) > 28


def test_the_strategy_refuses_windows_that_cannot_cross(refused):
    # In the Rust strategy's words.
    zero = "fast and slow windows must be greater than zero"
    refused(ValueError, zero, SmaCross, 0, 50)
    refused(ValueError, zero, SmaCross, 20, 0)
    refused(ValueError, zero, SmaCross, fast=-1, slow=50)
    ordered = "fast window must be smaller than slow window"
    refused(ValueError, ordered, SmaCross, 50, 50)
    refused(ValueError, ordered, SmaCross, fast=50, slow=20)
    # A window is a whole number of bars: an `int`, and nothing that would
    # have to be read as one. `True` is no window of one bar.
    whole = "fast and slow windows must be whole numbers of bars"
    for window in (20.0, "20", None, Decimal(20), True, False):
        refused(TypeError, whole, SmaCross, fast=window, slow=50)
        refused(TypeError, whole, SmaCross, fast=20, slow=window)
    built = SmaCross(fast=20, slow=50)
    assert (built.fast, built.slow) == (20, 50)


def test_the_run_the_page_shows_is_the_goldens(page, monkeypatch):
    # The page's example reads the files by their paths from the
    # repository's root, as a reader runs it there, and imports the strategy
    # from its directory. Here that leaves nothing behind: no bytecode
    # beside the fixture, and no module or path for another test to find.
    monkeypatch.chdir(REPOSITORY)
    left_before = sorted(path.name for path in GOLDEN.iterdir())
    monkeypatch.setattr(sys, "dont_write_bytecode", True)
    monkeypatch.setattr(sys, "path", list(sys.path))
    monkeypatch.delitem(sys.modules, "sma_cross", raising=False)
    # Its other Python fence is the strategy's file, shown in full.
    examples = [example for example in page(PAGE).fences("python") if ">>> " in example]
    assert len(examples) == 1
    session = doctest.DocTestParser().get_doctest("\n".join(examples), {}, PAGE, PAGE, 0)
    said = []
    try:
        results = doctest.DocTestRunner(verbose=False).run(session, out=said.append)
    finally:
        sys.modules.pop("sma_cross", None)
    assert sorted(path.name for path in GOLDEN.iterdir()) == left_before
    assert results.failed == 0, "".join(said)
    assert results.attempted == len(session.examples) > 5
