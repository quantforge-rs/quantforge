"""A strategy a user would write, run end to end: fixture candles, a class
that decides from its bars, `run_backtest`, and the result.

The candles are those of the one shared case a rule decides
(`tests/fixtures/strategy_cases.json`, the case with a `rule`): four hours
of one-minute bars, with the prices, volumes and trade counts of an
exchange's candles. The crate's tests write that case from the engine's run
of the rule written in Rust (`tests/strategy_cases.rs`, `Breakout`). Here
the rule is written again in Python, as a strategy that keeps the bars it
is handed and compares decimals, and has to decide the same on every bar
and bring the same result, digit for digit.

Then every field of that result is held to what the engine says it means,
worked out again from the trades and from what the strategy saw."""

import json
import pathlib
from decimal import Decimal
from fractions import Fraction

import pytest

import quantforge_py as qf

REPOSITORY = pathlib.Path(__file__).resolve().parents[2]
FIXTURE = json.loads((REPOSITORY / "tests" / "fixtures" / "strategy_cases.json").read_text("utf-8"))
RULED = [case for case in FIXTURE["cases"] if case["rule"] is not None]

CANDLE_DECIMALS = ["open", "high", "low", "close", "volume"]

TARGETS = {"LongAllIn": qf.TargetPosition.LONG_ALL_IN, "Flat": qf.TargetPosition.FLAT, None: None}

# How far a figure of the engine's may lie from the same figure worked out
# exactly: the engine keeps the digits a decimal holds, 28 or 29 of them,
# and rounds the last, when it divides, when it multiplies, and when it adds
# or subtracts amounts of different sizes.
ROUNDING = Fraction(1, 10**20)


class Breakout(qf.Strategy):
    """Goes long when a bar closes above the highest high of the
    `enter_bars` bars before it, and flat, while it holds a position, when
    a bar closes below the lowest low of the `exit_bars` bars before it."""

    name = "breakout"

    def __init__(self, enter_bars: int, exit_bars: int):
        self.enter_bars, self.exit_bars = enter_bars, exit_bars
        self.highs, self.lows = [], []

    def on_bar(self, ctx, bar):
        decision = self.decide(ctx, bar)
        self.highs.append(bar.high)
        self.lows.append(bar.low)
        return decision

    def decide(self, ctx, bar):
        if ctx.position_qty == 0:
            highs = last(self.highs, self.enter_bars)
            if highs and bar.close > max(highs):
                return qf.TargetPosition.LONG_ALL_IN
        else:
            lows = last(self.lows, self.exit_bars)
            if lows and bar.close < min(lows):
                return qf.TargetPosition.FLAT
        return None


def last(values: list, count: int) -> list:
    """The last `count` of `values`, and none of them while there are fewer:
    a rule that looks back over more bars than it has been handed decides
    nothing yet."""
    return values[len(values) - count :] if len(values) >= count else []


class Watched(Breakout):
    """The strategy above, with every callback written down: what it was
    handed, and for a bar what it decided."""

    def __init__(self, enter_bars: int, exit_bars: int):
        super().__init__(enter_bars, exit_bars)
        self.calls, self.decisions, self.seen = [], [], []

    def called(self, callback, bar, ctx):
        self.calls.append(
            dict(
                callback=callback,
                bar=bar,
                now_ms=ctx.now_ms,
                cash=format(ctx.cash, "f"),
                position_qty=format(ctx.position_qty, "f"),
            )
        )

    def on_start(self, ctx):
        self.called("on_start", None, ctx)

    def on_bar(self, ctx, bar):
        self.called("on_bar", len(self.decisions), ctx)
        self.seen.append((ctx.cash, ctx.position_qty))
        decision = super().on_bar(ctx, bar)
        self.decisions.append(decision)
        return decision

    def on_finish(self, ctx):
        self.called("on_finish", None, ctx)


def candles_of(case: dict) -> list[qf.Candle]:
    return [
        qf.Candle(
            open_time_ms=given["open_time_ms"],
            close_time_ms=given["close_time_ms"],
            trades=given["trades"],
            **{field: Decimal(given[field]) for field in CANDLE_DECIMALS},
        )
        for given in case["candles"]
    ]


def config_of(case: dict) -> qf.BacktestConfig:
    settings = case["config"]
    return qf.BacktestConfig(
        initial_cash=Decimal(settings["initial_cash"]),
        fee_bps=Decimal(settings["fee_bps"]),
        close_out_at_end=settings["close_out_at_end"],
    )


@pytest.fixture(scope="module")
def case() -> dict:
    (case,) = RULED
    return case


@pytest.fixture(scope="module")
def ran(case):
    """The case run once as a user runs it: the strategy, and its result."""
    strategy = Watched(case["rule"]["enter_bars"], case["rule"]["exit_bars"])
    result = qf.run_backtest(
        candles_of(case), strategy, config_of(case), market=qf.MarketId(**case["market"])
    )
    return strategy, result


def test_the_fixture_holds_candles_like_an_exchanges_and_a_rule_to_decide_on_them(case):
    assert case["rule"] == dict(name="breakout", enter_bars=20, exit_bars=10)
    candles = candles_of(case)
    assert len(candles) == 240
    # One minute after another from midnight, 2026-01-01, UTC.
    assert candles[0].open_time_ms == 1_767_225_600_000
    assert all(
        (after.open_time_ms, bar.close_time_ms)
        == (bar.open_time_ms + 60_000, bar.open_time_ms + 59_999)
        for bar, after in zip(candles, candles[1:])
    )
    # Prices to the hundredth, volumes to eight places, and a trade count
    # some bars do not have.
    for bar in candles:
        assert bar.low <= min(bar.open, bar.close) <= max(bar.open, bar.close) <= bar.high
        assert {
            price.as_tuple().exponent for price in (bar.open, bar.high, bar.low, bar.close)
        } == {-2}
        assert bar.volume.as_tuple().exponent == -8
    unknown = sum(bar.trades is None for bar in candles)
    assert 10 < unknown < 60
    # The engine was not told what to decide: the rule was.
    assert case["fails"] is None and case["error"] is None


def test_a_strategy_that_reads_its_bars_decides_as_the_same_rule_written_in_rust(case, ran):
    strategy, _ = ran
    assert strategy.decisions == [TARGETS[decision] for decision in case["decisions"]]
    # It entered and left more than once, at bars the rule alone chose.
    made = [(index, decision) for index, decision in enumerate(strategy.decisions) if decision]
    assert made == [
        (51, qf.TargetPosition.LONG_ALL_IN),
        (97, qf.TargetPosition.FLAT),
        (133, qf.TargetPosition.LONG_ALL_IN),
        (163, qf.TargetPosition.FLAT),
        (215, qf.TargetPosition.LONG_ALL_IN),
    ]


def test_it_is_called_as_the_engines_own_strategy_was_and_brings_the_same_result(
    case, ran, as_given
):
    strategy, result = ran
    # Every callback, in order, with the state the engine handed it.
    assert len(strategy.calls) == 242
    assert strategy.calls == case["calls"]
    # And the engine's result of the run, digit for digit.
    assert as_given(result) == case["result"]
    assert result.trade_count == 3
    assert str(result.final_equity) == "9998.670979465278896412314620"


@pytest.mark.parametrize("given", ["a list", "a batch", "a generator", "the plain strategy"])
def test_the_result_is_the_same_however_the_candles_are_handed_over(given, case, ran, as_given):
    _, expected = ran
    candles = candles_of(case)
    rule = case["rule"]
    strategy = Watched(rule["enter_bars"], rule["exit_bars"])
    if given == "a batch":
        candles = qf.CandleBatch.from_candles(candles)
    elif given == "a generator":
        candles = (candle for candle in candles)
    elif given == "the plain strategy":
        # The strategy a user would write, with nothing watching it.
        strategy = Breakout(rule["enter_bars"], rule["exit_bars"])
    result = qf.run_backtest(
        candles, strategy, config_of(case), market=qf.MarketId(**case["market"])
    )
    assert result == expected
    assert as_given(result) == case["result"]


def exact(value: Decimal) -> Fraction:
    return Fraction(value)


def test_every_trade_is_a_decision_filled_at_the_next_bars_open(case, ran):
    strategy, result = ran
    candles = candles_of(case)
    fee = exact(config_of(case).fee_bps) / 10_000
    entries = [
        at for at, made in enumerate(strategy.decisions) if made is qf.TargetPosition.LONG_ALL_IN
    ]
    exits = [at for at, made in enumerate(strategy.decisions) if made is qf.TargetPosition.FLAT]
    assert len(result.trades) == len(entries) == len(exits) + 1
    for number, trade in enumerate(result.trades):
        entered = candles[entries[number] + 1]
        assert (trade.entry_time_ms, trade.entry_price) == (entered.open_time_ms, entered.open)
        if number < len(exits):
            left = candles[exits[number] + 1]
            assert (trade.exit_time_ms, trade.exit_price) == (left.open_time_ms, left.open)
            cash_after = strategy.seen[exits[number] + 1][0]
        else:
            # The last position was still open, and was closed out at the
            # last bar's close.
            assert (trade.exit_time_ms, trade.exit_price) == (
                candles[-1].close_time_ms,
                candles[-1].close,
            )
            cash_after = result.final_equity
        assert trade.symbol == qf.Symbol(case["market"]["symbol"])
        assert (trade.entry_order_id, trade.exit_order_id) == (None, None)
        # All the cash bought the quantity, fee included: what is left of
        # it is what the division could not place.
        cash_before, _ = strategy.seen[entries[number]]
        _, held = strategy.seen[entries[number] + 1]
        assert held == trade.qty
        spent = exact(trade.qty) * exact(trade.entry_price) * (1 + fee)
        assert abs(spent - exact(cash_before)) < ROUNDING
        # Its PnL is the change in cash over the trade, both fees taken off.
        changed = exact(cash_after) - exact(cash_before)
        assert abs(exact(trade.gross_quote_pnl) - changed) < ROUNDING
        received = exact(trade.qty) * exact(trade.exit_price) * (1 - fee)
        assert abs(exact(trade.gross_quote_pnl) - (received - spent)) < ROUNDING


def test_every_figure_of_the_result_is_what_the_engine_says_it_is(case, ran):
    strategy, result = ran
    candles = candles_of(case)
    config = config_of(case)
    initial = exact(config.initial_cash)
    assert result.initial_cash == config.initial_cash
    assert result.trade_count == len(result.trades)

    # The curve: one point for every bar, named by the bar's open time, and
    # worth the cash and the position the strategy was handed at that bar,
    # the position at the bar's close.
    assert [point.open_time_ms for point in result.equity_curve] == [
        bar.open_time_ms for bar in candles
    ]
    for point, bar, (cash, held) in zip(result.equity_curve, candles, strategy.seen, strict=True):
        marked = exact(cash) + exact(held) * exact(bar.close)
        assert abs(exact(point.equity) - marked) < ROUNDING

    # The final equity: the run ended flat, so it is the starting cash and
    # every trade's PnL.
    pnl = sum(exact(trade.gross_quote_pnl) for trade in result.trades)
    assert abs(exact(result.final_equity) - (initial + pnl)) < ROUNDING
    # The return: the gain over the starting cash, in percent.
    gained = (exact(result.final_equity) - initial) / initial * 100
    assert abs(exact(result.total_return_pct) - gained) < ROUNDING
    assert result.total_return_pct < 0, "the fixture's run ends in a small loss"
    # The drawdown: the largest fall of the curve from an earlier peak.
    peak, deepest = Fraction(0), Fraction(0)
    for point in result.equity_curve:
        peak = max(peak, exact(point.equity))
        deepest = max(deepest, (peak - exact(point.equity)) / peak)
    assert abs(exact(result.max_drawdown_pct) - deepest * 100) < ROUNDING
    assert 0 < result.max_drawdown_pct < 1
    # The last point of the curve leaves out the fee of the close-out, which
    # the final equity has paid.
    closed_out = result.trades[-1]
    fee = exact(closed_out.qty) * exact(closed_out.exit_price) * exact(config.fee_bps) / 10_000
    over = exact(result.equity_curve[-1].equity) - exact(result.final_equity)
    assert abs(over - fee) < ROUNDING and fee > 0


def test_the_run_is_the_same_on_a_host_that_embeds_python(case, ran, bench, as_given):
    _, expected = ran
    rule = case["rule"]
    strategy = Watched(rule["enter_bars"], rule["exit_bars"])
    market = qf.MarketId(**case["market"])
    result = bench(market, candles_of(case), strategy, config_of(case), embedded=True)
    assert strategy.calls == case["calls"]
    assert as_given(result) == as_given(expected)
