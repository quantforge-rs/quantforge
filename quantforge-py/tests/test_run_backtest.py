"""`run_backtest`, the function a strategy is run with: what it takes, the
order it checks that in, what the engine refuses before any callback is
called, what a run gives, and that the same run gives the same result.

What the engine calls a strategy with and what its decisions come to is
held elsewhere, through this same function: the shared cases in
`test_strategy_cases.py`, a strategy that decides from its bars in
`test_end_to_end.py`, what a callback raises in `test_strategy_failures.py`,
and the threads of a run in `test_strategy_hosts.py`."""

import inspect
import re
from decimal import Decimal

import pytest

import quantforge_py as qf
from quantforge_py import _native

# The engine's refusal of a market that is not Spot, word for word.
NOT_SPOT = "candle strategies run on spot markets only, and {exchange} trades {category} futures"
NO_CANDLES = "no candles provided"

SPOT = [exchange for exchange in qf.ExchangeId.ALL if not exchange.category.is_futures]
FUTURES = [exchange for exchange in qf.ExchangeId.ALL if exchange.category.is_futures]


class Notes(qf.Strategy):
    """Goes long on the first bar and flat on the third, and writes down
    every callback with what it was handed."""

    def __init__(self):
        self.calls, self.contexts, self.bars = [], [], []

    def on_start(self, ctx):
        self.calls.append("on_start")
        self.contexts.append(ctx)

    def on_bar(self, ctx, bar):
        self.calls.append("on_bar")
        self.contexts.append(ctx)
        self.bars.append(bar)
        if len(self.bars) == 1:
            return qf.TargetPosition.LONG_ALL_IN
        if len(self.bars) == 3:
            return qf.TargetPosition.FLAT
        return None

    def on_finish(self, ctx):
        self.calls.append("on_finish")
        self.contexts.append(ctx)


class Holds(qf.Strategy):
    """Asks for a long position on every bar."""

    def __init__(self):
        self.calls = []

    def on_bar(self, ctx, bar):
        self.calls.append("on_bar")
        return qf.TargetPosition.LONG_ALL_IN

    def on_finish(self, ctx):
        self.calls.append("on_finish")


@pytest.fixture(scope="module")
def config() -> qf.BacktestConfig:
    return qf.BacktestConfig(initial_cash=1000, fee_bps=0, close_out_at_end=True)


@pytest.fixture(scope="module")
def four(minutes) -> list[qf.Candle]:
    """The four bars of the engine's documented example: an entry on the
    first fills at 100, and an exit on the third at 125."""
    return minutes((100, 100), (100, 110), (110, 120), (125, 125))


def written(result: qf.BacktestResult) -> list:
    """A result down to its digits: every field as it is written."""
    return [
        *(str(getattr(result, field)) for field in RESULT_FIELDS[:5]),
        [repr(trade) for trade in result.trades],
        [repr(point) for point in result.equity_curve],
    ]


RESULT_FIELDS = [
    "initial_cash",
    "final_equity",
    "total_return_pct",
    "trade_count",
    "max_drawdown_pct",
    "trades",
    "equity_curve",
]


def test_it_takes_three_arguments_by_position_and_the_market_by_name(four, config, market):
    assert str(inspect.signature(qf.run_backtest)) == "(candles, strategy, config, *, market)"
    result = qf.run_backtest(four, Notes(), config, market=market)
    assert type(result) is qf.BacktestResult
    assert result.final_equity == 1250
    # Every argument by its name, in any order, is the same run.
    named = qf.run_backtest(market=market, config=config, strategy=Notes(), candles=four)
    assert named == result
    # The market is given by name alone: nothing is taken for it by its
    # place, and nothing is assumed where it is left out.
    with pytest.raises(TypeError, match="takes 3 positional arguments but 4 were given"):
        qf.run_backtest(four, Notes(), config, market)
    with pytest.raises(TypeError, match="missing 1 required keyword argument: 'market'"):
        qf.run_backtest(four, Notes(), config)
    for field, given in [
        ("candles", dict(strategy=Notes(), config=config)),
        ("strategy", dict(candles=four, config=config)),
        ("config", dict(candles=four, strategy=Notes())),
    ]:
        with pytest.raises(TypeError, match=f"missing 1 required positional argument: '{field}'"):
            qf.run_backtest(market=market, **given)
    with pytest.raises(TypeError, match="unexpected keyword argument 'embedded'"):
        qf.run_backtest(four, Notes(), config, market=market, embedded=True)


def test_it_says_what_it_is_and_how_it_is_called():
    assert qf.run_backtest.__name__ == "run_backtest"
    said = " ".join(qf.run_backtest.__doc__.split())
    assert "`run_backtest(candles, strategy, config, *, market)`" in said
    # Each argument is named with the type it takes, and each failure with
    # the exception it is.
    for words in [
        "`candles` is a `CandleBatch`, or anything that iterates `Candle`s",
        "`strategy` is an object of a class derived from `Strategy`",
        "`config` is a `BacktestConfig`",
        "`market` is the `MarketId` the candles are of, a Spot market",
        "`TypeError`",
        "`ValueError`",
        "`OverflowError`",
    ]:
        assert words in said, words


def test_the_run_is_the_engines_documented_example(four, config, market):
    strategy = Notes()
    result = qf.run_backtest(four, strategy, config, market=market)
    # The long decision on the first bar fills at the second bar's open,
    # 100, and the flat decision on the third at the fourth's, 125.
    assert strategy.calls == ["on_start", *["on_bar"] * 4, "on_finish"]
    assert [(ctx.now_ms, ctx.cash, ctx.position_qty) for ctx in strategy.contexts] == [
        (0, 1000, 0),
        (0, 1000, 0),
        (60_000, 0, 10),
        (120_000, 0, 10),
        (180_000, 1250, 0),
        (180_000, 1250, 0),
    ]
    assert all(ctx.market == market for ctx in strategy.contexts)
    assert strategy.bars == four
    assert (result.initial_cash, result.final_equity) == (1000, 1250)
    assert (result.total_return_pct, result.max_drawdown_pct) == (25, 0)
    assert result.trade_count == 1
    (trade,) = result.trades
    assert trade == qf.ClosedTrade(
        symbol="BTCUSDT",
        entry_time_ms=60_000,
        exit_time_ms=180_000,
        entry_price=100,
        exit_price=125,
        qty=10,
        gross_quote_pnl=250,
    )
    assert [(point.open_time_ms, point.equity) for point in result.equity_curve] == [
        (0, 1000),
        (60_000, 1100),
        (120_000, 1200),
        (180_000, 1250),
    ]


class Iterates:
    """Anything that iterates: it has an `__iter__` and nothing else."""

    def __init__(self, items):
        self.items = items

    def __iter__(self):
        return iter(self.items)


class Indexes:
    """A sequence of the oldest kind: it is read by index until it runs out."""

    def __init__(self, items):
        self.items = items

    def __getitem__(self, index):
        return self.items[index]


def test_candles_are_a_batch_or_anything_that_iterates_candles(four, config, market):
    expected = qf.run_backtest(four, Notes(), config, market=market)
    batch = qf.CandleBatch.from_candles(four)
    for candles in [
        batch,
        tuple(four),
        iter(four),
        (candle for candle in four),
        Iterates(four),
        Indexes(four),
        dict(enumerate(four)).values(),
    ]:
        strategy = Notes()
        result = qf.run_backtest(candles, strategy, config, market=market)
        assert result == expected
        assert written(result) == written(expected)
        # The strategy is handed the candles given, in the order given.
        assert strategy.bars == four
    # What iterates once is read to its end, and holds nothing afterwards.
    once = iter(four)
    qf.run_backtest(once, Notes(), config, market=market)
    assert list(once) == []


def test_a_batch_runs_as_its_candles_do_digit_for_digit(config, market):
    exact = [
        qf.Candle(
            open_time_ms=index * 60_000,
            close_time_ms=index * 60_000 + 59_999,
            open=Decimal(open),
            high=Decimal("82010.00000001"),
            low=Decimal("81700.00"),
            close=Decimal(close),
            volume=Decimal("0.17235000"),
            trades=trades,
        )
        for index, (open, close, trades) in enumerate(
            [
                ("81940.01000000", "81955.25", None),
                ("81955.25000000", "82001.10", 412),
                ("82001.1", "81750.50000000", 0),
            ]
        )
    ]
    listed, batched = Notes(), Notes()
    from_list = qf.run_backtest(exact, listed, config, market=market)
    from_batch = qf.run_backtest(qf.CandleBatch.from_candles(exact), batched, config, market=market)
    assert written(from_batch) == written(from_list)
    # A bar out of a batch is the candle that went into it: every decimal
    # with its places, and a trade count that is unknown still unknown.
    assert [repr(bar) for bar in batched.bars] == [repr(candle) for candle in exact]
    assert [bar.trades for bar in batched.bars] == [None, 412, 0]


def test_the_candles_are_replayed_in_the_order_given_and_are_not_checked(minutes, config, market):
    # Out of order, one of them twice, and a low above its high: the engine
    # checks no candle, and hands each one over as it is.
    first, second, third = minutes((100, 100), (50, 50), (200, 200))
    upside_down = qf.Candle(
        open_time_ms=0, close_time_ms=59_999, open=10, high=5, low=20, close=10, volume=0
    )
    given = [third, first, first, upside_down, second]
    strategy = Notes()
    result = qf.run_backtest(given, strategy, config, market=market)
    assert strategy.bars == given
    assert [point.open_time_ms for point in result.equity_curve] == [120_000, 0, 0, 0, 60_000]
    assert [ctx.now_ms for ctx in strategy.contexts[1:-1]] == [120_000, 0, 0, 0, 60_000]


def test_what_is_no_candles_is_refused_before_anything_is_called(four, config, market, refused):
    def run(candles, strategy):
        return qf.run_backtest(candles, strategy, config, market=market)

    strategy = Notes()
    takes = "a CandleBatch or an iterable of Candle"
    for value, named in [
        (5, "int"),
        (None, "NoneType"),
        (four[0], "Candle"),
        (Decimal("1"), "Decimal"),
        (qf.CandleBatch, "type"),
        (market, "MarketId"),
        # Text iterates too, one character at a time, and is never candles:
        # not the path of a file of them, and not text that holds nothing.
        ("candles.csv", "str"),
        ("", "str"),
    ]:
        refused(TypeError, f"candles must be {takes}, not {named}", run, value, strategy)
    # Each candle is named by its place.
    for value, place, named in [
        ([1], 0, "int"),
        ([four[0], four[1], None], 2, "NoneType"),
        ([four[0], {"open": 100}], 1, "dict"),
        ([[four[0]]], 0, "list"),
        ([qf.CandleBatch.from_candles(four)], 0, "CandleBatch"),
        (["candles.csv"], 0, "str"),
        (b"candles", 0, "int"),
        ((candle.open for candle in four), 0, "Decimal"),
    ]:
        message = f"candles[{place}] must be a Candle, not {named}"
        refused(TypeError, message, run, value, strategy)
    assert strategy.calls == []


class Opens:
    """A source of candles that fails as it is opened: its `__iter__` raises."""

    def __init__(self, raised):
        self.raised = raised

    def __iter__(self):
        raise self.raised


class RunsDry:
    """A source of candles that fails after the first of them."""

    def __init__(self, first, raised):
        self.first, self.raised, self.read = first, raised, 0

    def __iter__(self):
        return self

    def __next__(self):
        self.read += 1
        if self.read == 1:
            return self.first
        raise self.raised


class Misindexed:
    """A sequence read by index that fails at its second candle."""

    def __init__(self, first, raised):
        self.first, self.raised = first, raised

    def __getitem__(self, index):
        if index == 0:
            return self.first
        raise self.raised


@pytest.mark.parametrize(
    "raised",
    [
        OSError("the store is closed"),
        LookupError("the source ran dry"),
        TypeError("the source's own"),
        MemoryError(),
        KeyboardInterrupt(),
    ],
    ids=lambda raised: type(raised).__name__,
)
def test_what_reading_the_candles_raises_is_raised_as_it_is(raised, four, config, market):
    def dries_up():
        yield four[0]
        yield four[1]
        raise raised

    strategy = Notes()
    for candles in [
        Opens(raised),
        RunsDry(four[0], raised),
        Misindexed(four[0], raised),
        dries_up(),
    ]:
        # The source's own error, the object it raised: a source that fails
        # is no argument of the wrong type, and an interrupt is an interrupt.
        with pytest.raises(type(raised)) as caught:
            qf.run_backtest(candles, strategy, config, market=market)
        assert caught.value is raised
        # A host that is not Python reads the candles the same way, before
        # its engine has anything to say.
        if not isinstance(candles, type(dries_up())):
            with pytest.raises(type(raised)) as caught:
                _native._embedded_backtest(candles, strategy, config, market=market)
            assert caught.value is raised
    assert strategy.calls == []


@pytest.mark.parametrize("kind", [KeyboardInterrupt, OSError, TypeError])
def test_a_source_that_takes_its_own_iteration_away_as_it_fails_still_fails_as_itself(
    kind, config, market
):
    class Withdraws:
        """A source whose `__iter__` removes itself from its class, and then
        raises: once it has run, its class says it does not iterate."""

        def __iter__(self):
            type(self).__iter__ = None
            raise raised

    raised = kind("as it was opened")
    strategy = Notes()
    # What it raised is what is raised: whether it iterates was looked at
    # before it was asked to, and not afterwards.
    with pytest.raises(kind) as caught:
        qf.run_backtest(Withdraws(), strategy, config, market=market)
    assert caught.value is raised
    assert Withdraws.__iter__ is None
    # Asked again, it is a value that does not iterate.
    with pytest.raises(TypeError) as caught:
        qf.run_backtest(Withdraws(), strategy, config, market=market)
    assert str(caught.value) == (
        "candles must be a CandleBatch or an iterable of Candle, not Withdraws"
    )
    assert strategy.calls == []


def test_a_value_that_does_not_iterate_is_no_candles(four, config, market, refused):
    class Unlisted:
        """A value that says it does not iterate."""

        __iter__ = None

    class Unlistable(Unlisted):
        """One that could be read by index, and still says it does not."""

        def __getitem__(self, index):
            return four[index]

    # A value Python reads by a key and does not iterate: a match of a
    # regular expression has a `__getitem__`, and no place among sequences.
    found = re.match("candles", "candles")
    for value, named in [(Unlisted(), "Unlisted"), (Unlistable(), "Unlistable"), (found, "Match")]:
        message = f"candles must be a CandleBatch or an iterable of Candle, not {named}"
        refused(TypeError, message, qf.run_backtest, value, Notes(), config, market=market)


def test_a_run_of_no_candles_is_refused_in_the_engines_words(config, market, refused):
    strategy = Notes()
    for nothing in [[], (), {}, range(0), iter(()), qf.CandleBatch.from_candles([]), Iterates([])]:
        refused(ValueError, NO_CANDLES, qf.run_backtest, nothing, strategy, config, market=market)
    assert strategy.calls == []
    # A host that is not Python is told the same.
    with pytest.raises(RuntimeError) as told:
        _native._embedded_backtest([], strategy, config, market=market)
    assert (type(told.value), str(told.value)) == (RuntimeError, NO_CANDLES)
    assert strategy.calls == []


def test_there_are_spot_and_futures_product_lines_to_tell_apart():
    assert len(SPOT) >= 2 and len(FUTURES) >= 4
    assert {exchange.category for exchange in SPOT} == {qf.Category.SPOT}
    assert {exchange.category for exchange in FUTURES} == {qf.Category.LINEAR, qf.Category.INVERSE}


@pytest.mark.parametrize("exchange", FUTURES, ids=str)
def test_a_market_that_is_not_spot_is_refused_before_anything_is_called(
    exchange, four, config, refused
):
    futures = qf.MarketId(exchange, "BTCUSDT", "1m")
    message = NOT_SPOT.format(exchange=exchange, category=exchange.category)
    strategy = Notes()
    refused(ValueError, message, qf.run_backtest, four, strategy, config, market=futures)
    assert strategy.calls == []
    # Whatever the candles are: the market is the first thing the engine
    # looks at, before it finds that there are none.
    refused(ValueError, message, qf.run_backtest, [], strategy, config, market=futures)
    batch = qf.CandleBatch.from_candles(four)
    refused(ValueError, message, qf.run_backtest, batch, strategy, config, market=futures)
    # A host that is not Python is told the same, in the same words.
    with pytest.raises(RuntimeError) as told:
        _native._embedded_backtest(four, strategy, config, market=futures)
    assert (type(told.value), str(told.value)) == (RuntimeError, message)
    assert strategy.calls == []


def test_the_refusal_names_the_product_line_and_its_family():
    said = {
        str(exchange): NOT_SPOT.format(exchange=exchange, category=exchange.category)
        for exchange in FUTURES
    }
    # Written out for one line of each family, so that the words above are
    # held to more than themselves.
    assert said["bybit_linear"] == (
        "candle strategies run on spot markets only, and bybit_linear trades linear futures"
    )
    assert said["bybit_inverse"] == (
        "candle strategies run on spot markets only, and bybit_inverse trades inverse futures"
    )


@pytest.mark.parametrize("exchange", SPOT, ids=str)
def test_every_spot_market_runs_and_is_the_market_of_the_run(exchange, four, config, market):
    spot = qf.MarketId(exchange, "ethusdt", "8h")
    strategy = Notes()
    result = qf.run_backtest(four, strategy, config, market=spot)
    # Every callback is handed the market, and a trade takes its symbol.
    assert len(strategy.contexts) == 6
    assert all(ctx.market == spot for ctx in strategy.contexts)
    assert [trade.symbol for trade in result.trades] == [qf.Symbol("ETHUSDT")]
    # Nothing else of a run depends on which Spot market it is.
    other = qf.run_backtest(four, Notes(), config, market=market)
    assert other.trades[0].symbol == qf.Symbol("BTCUSDT")
    assert written(result)[:5] == written(other)[:5]
    assert result.equity_curve == other.equity_curve


def test_the_market_is_a_market_id_and_nothing_that_names_one(four, config, refused):
    strategy = Notes()
    for value, named in [
        ("binance_spot", "str"),
        (("binance_spot", "BTCUSDT", "1m"), "tuple"),
        (qf.ExchangeId.BINANCE_SPOT, "ExchangeId"),
        (qf.Symbol("BTCUSDT"), "Symbol"),
        (None, "NoneType"),
        (qf.MarketId, "type"),
    ]:
        message = f"market must be a MarketId, not {named}"
        refused(TypeError, message, qf.run_backtest, four, strategy, config, market=value)
    assert strategy.calls == []


def test_the_config_is_a_backtest_config_and_nothing_that_holds_its_settings(
    four, config, market, refused
):
    strategy = Notes()
    settings = dict(initial_cash=1000, fee_bps=0, close_out_at_end=True)
    for value, named in [
        (settings, "dict"),
        (tuple(settings.values()), "tuple"),
        (None, "NoneType"),
        (qf.BacktestConfig, "type"),
        (Decimal("1000"), "Decimal"),
        (qf.run_backtest(four, Notes(), config, market=market), "BacktestResult"),
    ]:
        message = f"config must be a BacktestConfig, not {named}"
        refused(TypeError, message, qf.run_backtest, four, strategy, value, market=market)
    assert strategy.calls == []


def test_the_arguments_are_checked_in_the_order_written_and_then_the_engines_refusals(
    four, config, market, refused
):
    futures = qf.MarketId(FUTURES[0], "BTCUSDT", "1m")
    wrong = dict(candles=5, strategy="sma_cross", config={}, market="binance_spot")
    right = dict(candles=[], strategy=Notes(), config=config, market=futures)
    # With every argument of the wrong type, each is refused in its turn as
    # the ones before it are put right.
    for field, message in [
        ("candles", "candles must be a CandleBatch or an iterable of Candle, not int"),
        ("strategy", "strategy must be a Strategy, not str"),
        ("config", "config must be a BacktestConfig, not dict"),
        ("market", "market must be a MarketId, not str"),
    ]:
        refused(TypeError, message, qf.run_backtest, **wrong)
        wrong[field] = right[field]
    # Every argument is of its type now, and the engine refuses the run:
    # for its market first, and then for its candles.
    assert wrong == right
    message = NOT_SPOT.format(exchange=FUTURES[0], category=FUTURES[0].category)
    refused(ValueError, message, qf.run_backtest, **right)
    refused(ValueError, NO_CANDLES, qf.run_backtest, **{**right, "market": market})
    assert qf.run_backtest(**{**right, "market": market, "candles": four}).trade_count == 1


def test_the_candles_are_read_before_the_other_arguments_are_looked_at(four, market):
    # An iterable that is read once is read to its end even by a call that
    # is then refused for another argument.
    once = iter(four)
    with pytest.raises(TypeError, match="config must be a BacktestConfig, not NoneType"):
        qf.run_backtest(once, Notes(), None, market=market)
    assert list(once) == []


def test_an_amount_beyond_the_engines_decimal_is_an_overflow_error(minutes, config, market):
    # 1000 over 1e-28 is about 1e31, more than the engine's decimal holds.
    tiny = Decimal("0.0000000000000000000000000001")
    candles = minutes((1, 1), (tiny, tiny))
    strategy = Holds()
    with pytest.raises(OverflowError) as caught:
        qf.run_backtest(candles, strategy, config, market=market)
    assert type(caught.value) is OverflowError
    assert str(caught.value) == "decimal overflow computing the entry"
    # The run ended at the fill it could not compute: the first bar had been
    # handed over, and nothing was called after it.
    assert strategy.calls == ["on_bar"]
    assert not hasattr(caught.value, "__notes__")

    # 1e20 buys 1e28 at 1e-8, which fits; valued at a close of 100 it is
    # 1e30, which does not.
    rich = qf.BacktestConfig(initial_cash=10**20, fee_bps=0, close_out_at_end=True)
    candles = minutes((1, 1), (Decimal("0.00000001"), 100))
    with pytest.raises(OverflowError) as caught:
        qf.run_backtest(candles, Holds(), rich, market=market)
    assert str(caught.value) == "decimal overflow computing the equity mark"

    # A host that is not Python is told the same, as the engine's error.
    with pytest.raises(RuntimeError) as told:
        _native._embedded_backtest(candles, Holds(), rich, market=market)
    assert (type(told.value), str(told.value)) == (
        RuntimeError,
        "decimal overflow computing the equity mark",
    )

    # The figures of the result are worked out after the last callback: a
    # gain of 1e28 over a starting cash of 1e-28 is a return no decimal
    # holds, and the run that told its strategy it was finished has no
    # result.
    poor = qf.BacktestConfig(initial_cash=tiny, fee_bps=0, close_out_at_end=True)
    candles = minutes((1, 1), (1, Decimal("1E28")))
    strategy = Holds()
    with pytest.raises(OverflowError) as caught:
        qf.run_backtest(candles, strategy, poor, market=market)
    assert str(caught.value) == "decimal overflow computing the return"
    assert strategy.calls == ["on_bar", "on_bar", "on_finish"]


def test_a_result_holds_one_point_for_every_bar_and_counts_its_trades(minutes, market):
    config = qf.BacktestConfig(initial_cash=Decimal("5000.00"), fee_bps=25, close_out_at_end=True)
    candles = minutes((50, 50), (50, 55), (55, Decimal("60.5")))
    result = qf.run_backtest(candles, Holds(), config, market=market)
    # The starting cash is the config's own, digit for digit.
    assert str(result.initial_cash) == "5000.00"
    assert result.trade_count == len(result.trades) == 1
    assert [point.open_time_ms for point in result.equity_curve] == [0, 60_000, 120_000]
    # The position was closed out at the last close, and its fee is what the
    # last point of the curve does not count.
    (trade,) = result.trades
    assert (trade.exit_time_ms, trade.exit_price) == (179_999, Decimal("60.5"))
    assert (trade.entry_order_id, trade.exit_order_id) == (None, None)
    fee = trade.qty * trade.exit_price * Decimal("0.0025")
    assert 0 < result.equity_curve[-1].equity - result.final_equity
    assert abs(result.equity_curve[-1].equity - result.final_equity - fee) < Decimal("1e-20")

    # Without close-out the position is only valued, and no trade is told.
    open_ended = qf.BacktestConfig(initial_cash=5000, fee_bps=25, close_out_at_end=False)
    held = qf.run_backtest(candles, Holds(), open_ended, market=market)
    assert (held.trade_count, held.trades) == (0, ())
    assert held.final_equity == held.equity_curve[-1].equity


def test_the_same_run_gives_the_same_result_every_time_on_either_host(market, minutes):
    config = qf.BacktestConfig(
        initial_cash=Decimal("2500"), fee_bps=Decimal("7.5"), close_out_at_end=True
    )
    candles = minutes(
        *[
            (
                Decimal(100 + index * 7 % 23) + Decimal(index * 37 % 100) / 100,
                100 + (index + 1) * 7 % 23,
            )
            for index in range(120)
        ]
    )

    class Alternates(qf.Strategy):
        def __init__(self):
            self.bars = 0

        def on_bar(self, ctx, bar):
            self.bars += 1
            if self.bars % 9 == 2:
                return qf.TargetPosition.LONG_ALL_IN
            if self.bars % 9 == 7:
                return qf.TargetPosition.FLAT
            return None

    first = qf.run_backtest(candles, Alternates(), config, market=market)
    assert first.trade_count > 10, "the candles trade"
    for again in range(4):
        result = qf.run_backtest(candles, Alternates(), config, market=market)
        assert result == first
        # Equal down to the digits: a result is not only equal in value.
        assert written(result) == written(first)
    embedded = _native._embedded_backtest(candles, Alternates(), config, market=market)
    assert written(embedded) == written(first)


def test_a_run_changes_nothing_it_was_given(four, config, market):
    candles = list(four)
    before = [repr(candle) for candle in candles]
    batch = qf.CandleBatch.from_candles(candles)
    settings = repr(config)
    qf.run_backtest(candles, Notes(), config, market=market)
    qf.run_backtest(batch, Notes(), config, market=market)
    assert [repr(candle) for candle in candles] == before
    assert all(given is kept for given, kept in zip(candles, four, strict=True))
    assert batch.to_candles() == four
    assert repr(config) == settings
    assert market == qf.MarketId("binance_spot", "BTCUSDT", "1m")


def test_the_candles_of_a_run_are_those_it_was_given_when_it_started(four, config, market):
    class Empties(Notes):
        """Takes away the candles it was run over, as soon as it is started."""

        def __init__(self, candles):
            super().__init__()
            self.candles = candles

        def on_start(self, ctx):
            super().on_start(ctx)
            self.candles.clear()

    expected = qf.run_backtest(four, Notes(), config, market=market)
    candles = list(four)
    strategy = Empties(candles)
    result = qf.run_backtest(candles, strategy, config, market=market)
    # The list is the caller's own, and was read before the first callback:
    # what becomes of it during the run changes nothing of the run.
    assert candles == []
    assert strategy.bars == four
    assert result == expected


def test_a_strategys_state_is_its_own_from_one_run_to_the_next(four, config, market):
    # The engine keeps nothing of a run. What a strategy counted in one, it
    # still holds in the next, which is why each run is given a new one.
    fresh = qf.run_backtest(four, Notes(), config, market=market)
    strategy = Notes()
    assert qf.run_backtest(four, strategy, config, market=market) == fresh
    again = qf.run_backtest(four, strategy, config, market=market)
    assert len(strategy.bars) == 8
    # Its fifth to eighth bars are neither its first nor its third: it
    # decided nothing this time.
    assert again.trade_count == 0
    assert again != fresh


def rust_fields(repository, path: str, name: str) -> list[str]:
    """The public fields of a Rust struct, in the order it declares them."""
    source = (repository / path).read_text(encoding="utf-8")
    body = re.search(rf"^pub struct {name} \{{\n(.*?)^\}}", source, re.MULTILINE | re.DOTALL)
    assert body, f"{path} declares {name}"
    return re.findall(r"^    pub (\w+): ", body.group(1), re.MULTILINE)


# The engine's own types, and the file each is declared in.
ENGINE_TYPES = {
    "BacktestResult": "src/engine/backtest.rs",
    "EquityPoint": "src/engine/backtest.rs",
    "BacktestConfig": "src/engine/backtest.rs",
    "ClosedTrade": "src/model/run.rs",
    "Candle": "src/model/candle.rs",
    "StrategyContext": "src/sdk/strategy.rs",
}


@pytest.mark.parametrize("name", ENGINE_TYPES)
def test_a_type_of_a_run_has_the_engines_fields_one_for_one(name, repository):
    declared = rust_fields(repository, ENGINE_TYPES[name], name)
    assert len(declared) >= 2, declared
    bound = getattr(qf, name)
    # Built with the engine's fields, by their names and in their order, and
    # read by the same names: none is added, left out or renamed.
    assert list(inspect.signature(bound).parameters) == declared
    read = [
        field
        for field, member in vars(bound).items()
        if type(member).__name__ == "getset_descriptor"
    ]
    assert sorted(read) == sorted(declared)


def test_the_result_of_a_run_is_the_engines_result_field_for_field(repository):
    # What the story's acceptance names: final equity, total return, max
    # drawdown and the trade list, with the rest of what the engine reports.
    assert rust_fields(repository, "src/engine/backtest.rs", "BacktestResult") == RESULT_FIELDS
