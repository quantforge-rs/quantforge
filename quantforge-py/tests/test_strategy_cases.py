"""A strategy written in Python is driven as one written in Rust is: the
shared cases of the strategy boundary, through the adapter.

`tests/fixtures/strategy_cases.json` is written by the engine's own tests
(`tests/strategy_cases.rs`), from runs of a strategy written in Rust that
plays a script back: for every case, the run's inputs, the decision on each
bar, every callback in the order the engine made it with the state it was
handed, and the result or the error. Here the same script is played back by
a class derived from `Strategy`, which has to be called the same way, call
for call and digit for digit, and to bring the same result.

A run Python hosts is `run_backtest` itself, and one a host that embeds
Python would make is the bench's. One case's decisions were made by a rule
and not listed: here they are played back like any other's, and
`test_end_to_end.py` holds a strategy that follows the rule to them."""

import json
import pathlib
from decimal import Decimal

import pytest

import quantforge_py as qf

# Read as the module is: its cases are the parameters of a test below.
REPOSITORY = pathlib.Path(__file__).resolve().parents[2]
FIXTURE = json.loads((REPOSITORY / "tests" / "fixtures" / "strategy_cases.json").read_text("utf-8"))
CASES = {case["id"]: case for case in FIXTURE["cases"]}

# A decision as the fixture spells it, which is the engine's name for it.
TARGETS = {
    "Flat": qf.TargetPosition.FLAT,
    "LongAllIn": qf.TargetPosition.LONG_ALL_IN,
    None: None,
}

CANDLE_DECIMALS = ["open", "high", "low", "close", "volume"]
CANDLE_FIELDS = ["open_time_ms", "close_time_ms", *CANDLE_DECIMALS, "trades"]


class ScriptFails(Exception):
    """What a script raises where its case says it fails."""


def written(value) -> str:
    """A decimal as the engine writes it: every digit, and no exponent."""
    assert type(value) is Decimal, value
    return format(value, "f")


def candle_of(given: dict) -> qf.Candle:
    decimals = {field: Decimal(given[field]) for field in CANDLE_DECIMALS}
    return qf.Candle(
        open_time_ms=given["open_time_ms"],
        close_time_ms=given["close_time_ms"],
        trades=given["trades"],
        **decimals,
    )


def candle_as_given(candle: qf.Candle) -> dict:
    """A candle as the fixture writes one, each decimal digit for digit."""
    assert type(candle) is qf.Candle, candle
    return {
        field: written(getattr(candle, field))
        if field in CANDLE_DECIMALS
        else getattr(candle, field)
        for field in CANDLE_FIELDS
    }


class Scripted(qf.Strategy):
    """Plays a case's script back and writes down every callback, as the
    strategy of the engine's own tests does. It holds each callback to what
    the fixture leaves unsaid: the market of every context, and each bar
    being the candle at its place."""

    def __init__(self, case: dict):
        self.case = case
        self.market = qf.MarketId(**case["market"])
        self.bars = 0
        self.calls = []
        self.raised = None

    def called(self, callback, bar, ctx):
        assert type(ctx) is qf.StrategyContext, ctx
        assert ctx.market == self.market
        assert type(ctx.now_ms) is int
        self.calls.append(
            dict(
                callback=callback,
                bar=bar,
                now_ms=ctx.now_ms,
                cash=written(ctx.cash),
                position_qty=written(ctx.position_qty),
            )
        )
        if self.case["fails"] == dict(callback=callback, bar=bar):
            self.raised = ScriptFails(FIXTURE["failure"])
            raise self.raised

    def on_start(self, ctx):
        self.called("on_start", None, ctx)

    def on_bar(self, ctx, bar):
        index = self.bars
        self.bars += 1
        assert candle_as_given(bar) == self.case["candles"][index]
        self.called("on_bar", index, ctx)
        return TARGETS[self.case["decisions"][index]]

    def on_finish(self, ctx):
        self.called("on_finish", None, ctx)


def played(case: dict, bench, embedded: bool):
    """Runs a case's script in the engine: the strategy, and the result or
    what was raised."""
    strategy = Scripted(case)
    candles = [candle_of(given) for given in case["candles"]]
    settings = case["config"]
    config = qf.BacktestConfig(
        initial_cash=Decimal(settings["initial_cash"]),
        fee_bps=Decimal(settings["fee_bps"]),
        close_out_at_end=settings["close_out_at_end"],
    )
    try:
        return strategy, bench(strategy.market, candles, strategy, config, embedded=embedded), None
    except (ScriptFails, RuntimeError) as raised:
        return strategy, None, raised


def test_the_shared_cases_are_read_and_name_every_target():
    assert FIXTURE["format"] == 2
    assert len(CASES) == len(FIXTURE["cases"]) > 15
    # A case's decisions are listed by its script, or made by the rule it
    # names: one case has one.
    assert [case["id"] for case in CASES.values() if case["rule"]] == ["a_rule_over_four_hours"]
    decided = {decision for case in CASES.values() for decision in case["decisions"]}
    assert decided == set(TARGETS)
    assert {TARGETS[decision] for decision in decided if decision} == set(qf.TargetPosition.ALL)
    # Some succeed and some fail, in each callback.
    failing = [case["fails"] for case in CASES.values() if case["fails"]]
    assert sorted(fails["callback"] for fails in failing) == ["on_bar", "on_finish", "on_start"]
    assert sum(case["result"] is not None for case in CASES.values()) > 10


@pytest.mark.parametrize("case", CASES.values(), ids=CASES.keys())
def test_a_python_strategy_is_called_and_obeyed_as_the_engines_own_is(
    case, bench, embedded, as_given
):
    strategy, result, raised = played(case, bench, embedded)
    # Every callback, in order, with the state the engine handed it.
    assert strategy.calls == case["calls"]
    if case["fails"] is None:
        assert raised is None
        assert as_given(result) == case["result"]
        return
    # The run failed where the script did, and the engine was told so.
    assert result is None
    assert case["error"] == FIXTURE["failure"]
    if embedded:
        assert type(raised) is RuntimeError
        assert str(raised).splitlines()[-1] == f"{__name__}.ScriptFails: {FIXTURE['failure']}"
    else:
        assert raised is strategy.raised


# The engine's documented example, worked by hand: the long decision on the
# first bar fills at the second bar's open, 100, and the flat decision on
# the third at the fourth's, 125.
def test_the_engines_documented_example_runs_to_its_documented_result(bench, embedded):
    strategy, result, raised = played(CASES["enter_then_exit"], bench, embedded)
    assert raised is None
    seen = [
        (call["callback"], call["now_ms"], call["cash"], call["position_qty"])
        for call in strategy.calls
    ]
    assert seen == [
        ("on_start", 0, "1000", "0"),
        ("on_bar", 0, "1000", "0"),
        ("on_bar", 60_000, "0", "10"),
        ("on_bar", 120_000, "0", "10"),
        ("on_bar", 180_000, "1250", "0"),
        ("on_finish", 180_000, "1250", "0"),
    ]
    assert result.final_equity == Decimal("1250")
    assert result.trade_count == 1
    (trade,) = result.trades
    assert (trade.entry_price, trade.exit_price, trade.qty) == (100, 125, 10)
    assert (trade.entry_time_ms, trade.exit_time_ms) == (60_000, 180_000)
    assert trade.symbol == qf.Symbol("BTCUSDT")
    assert [point.equity for point in result.equity_curve] == [1000, 1100, 1200, 1250]


def test_a_run_is_the_same_every_time_and_on_either_host(bench, as_given):
    case = CASES["sixty_bars"]
    runs = [played(case, bench, embedded) for embedded in (False, True, False, True)]
    first_strategy, first_result, _ = runs[0]
    assert len(first_strategy.calls) == 62
    for strategy, result, raised in runs[1:]:
        assert raised is None
        assert strategy.calls == first_strategy.calls
        assert result == first_result
        assert as_given(result) == as_given(first_result)


def test_every_callback_is_handed_a_context_and_a_bar_of_its_own(run, minutes):
    class Keeps(qf.Strategy):
        def __init__(self):
            self.contexts, self.bars = [], []

        def on_start(self, ctx):
            self.contexts.append(ctx)

        def on_bar(self, ctx, bar):
            self.contexts.append(ctx)
            self.bars.append(bar)
            return qf.TargetPosition.LONG_ALL_IN if len(self.bars) == 1 else None

        def on_finish(self, ctx):
            self.contexts.append(ctx)

    candles = minutes((100, 100), (100, 110), (110, 120))
    strategy = Keeps()
    run(strategy, candles, close_out_at_end=False)
    # Each is an object of its own, and what it said then it still says:
    # the engine moved on, and no context it handed out moved with it.
    assert len({id(ctx) for ctx in strategy.contexts}) == 5
    assert [(ctx.now_ms, ctx.cash, ctx.position_qty) for ctx in strategy.contexts] == [
        (0, 1000, 0),
        (0, 1000, 0),
        (60_000, 0, 10),
        (120_000, 0, 10),
        (120_000, 0, 10),
    ]
    # A bar is the candle the run was given, as a candle built anew.
    assert strategy.bars == candles
    assert all(seen is not given for seen, given in zip(strategy.bars, candles, strict=True))
    with pytest.raises(AttributeError):
        strategy.contexts[0].cash = Decimal("1")
    with pytest.raises(AttributeError):
        strategy.bars[0].close = Decimal("1")
