"""A backtest's settings and what one produces: `BacktestConfig`,
`ClosedTrade`, `EquityPoint` and `BacktestResult`.

A config is refused where the engine would refuse to run it, in the
engine's words. The other three hold a result as the engine reports it:
every amount an exact decimal, and an order ID that never reached a venue
unknown."""

import sys
from decimal import Decimal

import pytest

import quantforge_py as qf

I64_MIN, I64_MAX = -(2**63), 2**63 - 1

CONFIG = dict(initial_cash=Decimal("10000.00"), fee_bps=Decimal("7.5"), close_out_at_end=True)

TRADE = dict(
    symbol=qf.Symbol("BTCUSDT"),
    entry_time_ms=1_700_000_060_000,
    exit_time_ms=1_700_000_180_000,
    entry_price=Decimal("100.00"),
    exit_price=Decimal("125.5"),
    qty=Decimal("9.98501497753369945082376436"),
    gross_quote_pnl=Decimal("251.5"),
    entry_order_id="8389765519",
    exit_order_id="1321003749386327552",
)

# For each field of a trade, a value that is not the one above.
OTHER_TRADE = dict(
    symbol=qf.Symbol("ETHUSDT"),
    entry_time_ms=1_700_000_120_000,
    exit_time_ms=1_700_000_240_000,
    entry_price=Decimal("100.01"),
    exit_price=Decimal("125.6"),
    qty=Decimal("9.98501497753369945082376437"),
    gross_quote_pnl=Decimal("-251.5"),
    entry_order_id="8389765520",
    exit_order_id=None,
)

TRADE_DECIMALS = ["entry_price", "exit_price", "qty", "gross_quote_pnl"]
RESULT_DECIMALS = ["initial_cash", "final_equity", "total_return_pct", "max_drawdown_pct"]


def config(**changed) -> qf.BacktestConfig:
    return qf.BacktestConfig(**{**CONFIG, **changed})


def trade(**changed) -> qf.ClosedTrade:
    return qf.ClosedTrade(**{**TRADE, **changed})


def point(index: int, equity: str) -> qf.EquityPoint:
    return qf.EquityPoint(open_time_ms=index * 60_000, equity=Decimal(equity))


def result_fields() -> dict:
    """One run's result: one closed trade over four bars."""
    return dict(
        initial_cash=Decimal("1000"),
        final_equity=Decimal("1250.00"),
        total_return_pct=Decimal("25.000"),
        trade_count=1,
        max_drawdown_pct=Decimal("4.5454545454545454545454545455"),
        trades=[trade()],
        equity_curve=[point(0, "1000"), point(1, "1100.0"), point(2, "1050"), point(3, "1250.00")],
    )


def result(**changed) -> qf.BacktestResult:
    return qf.BacktestResult(**{**result_fields(), **changed})


def exact(value):
    """A value as it is, a decimal down to its digits and decimal places."""
    return value.as_tuple() if isinstance(value, Decimal) else value


def test_a_config_gives_back_what_it_was_given(evaluated):
    made = config()
    assert exact(made.initial_cash) == exact(Decimal("10000.00"))
    assert exact(made.fee_bps) == exact(Decimal("7.5"))
    assert made.close_out_at_end is True
    assert config(close_out_at_end=False).close_out_at_end is False
    assert repr(made) == (
        "BacktestConfig(initial_cash=Decimal('10000.00'), fee_bps=Decimal('7.5'), "
        "close_out_at_end=True)"
    )
    back = evaluated(repr(made))
    assert back == made
    assert exact(back.initial_cash) == exact(made.initial_cash)


def test_a_config_the_engine_would_not_run_is_refused_in_the_engines_words(refused):
    for cash in ["0", "0.00", "-0.01", "-10000"]:
        message = f"invalid configuration: initial_cash must be greater than 0, got {cash}"
        refused(ValueError, message, config, initial_cash=Decimal(cash))
    for fee in ["-0.1", "-10"]:
        message = f"invalid configuration: fee_bps must be zero or greater, got {fee}"
        refused(ValueError, message, config, fee_bps=Decimal(fee))
    # The cash is looked at first.
    message = "invalid configuration: initial_cash must be greater than 0, got 0"
    refused(ValueError, message, config, initial_cash=0, fee_bps=-1)
    # The smallest cash and no fee are a config the engine runs.
    least = config(initial_cash=Decimal("0.0000000000000000000000000001"), fee_bps=0)
    assert least.fee_bps == 0


def test_a_configs_three_settings_are_named_required_and_of_their_own_types(refused):
    with pytest.raises(TypeError, match="takes 0 positional arguments"):
        qf.BacktestConfig(*CONFIG.values())
    for field in CONFIG:
        without = {name: value for name, value in CONFIG.items() if name != field}
        with pytest.raises(TypeError, match=f"missing 1 required keyword argument: '{field}'"):
            qf.BacktestConfig(**without)
    for value, named in [(1, "int"), (0, "int"), ("true", "str"), (None, "NoneType")]:
        message = f"close_out_at_end must be a bool, not {named}"
        refused(TypeError, message, config, close_out_at_end=value)
    for field in ["initial_cash", "fee_bps"]:
        message = (
            f"{field} must be a decimal.Decimal or an int, not float: a float is never read as "
            "a decimal; convert it with decimal_from_f64"
        )
        refused(TypeError, message, config, **{field: 10.0})
        message = f"{field} must be a decimal.Decimal or an int, not str"
        refused(TypeError, message, config, **{field: "10"})


def test_configs_are_equal_when_their_settings_are_and_cannot_be_changed():
    assert config() == config(initial_cash=10_000, fee_bps=Decimal("7.50"))
    assert config() != config(initial_cash=Decimal("10000.01"))
    assert config() != config(fee_bps=Decimal("7.6"))
    assert config() != config(close_out_at_end=False)
    assert config() != CONFIG
    with pytest.raises(TypeError, match="unhashable"):
        hash(config())
    made = config()
    for field in CONFIG:
        with pytest.raises(AttributeError):
            setattr(made, field, CONFIG[field])


def test_a_trade_gives_back_every_field_as_it_was_given():
    made = trade()
    for field, value in TRADE.items():
        back = getattr(made, field)
        assert type(back) is type(value), field
        assert exact(back) == exact(value), field


def test_a_trades_symbol_is_a_symbol_or_the_text_of_one(refused):
    assert trade(symbol=" btcusdt ") == trade()
    assert type(trade(symbol="btcusdt").symbol) is qf.Symbol
    refused(ValueError, "invalid symbol: empty", trade, symbol="  ")
    refused(TypeError, "symbol must be a Symbol or a str, not NoneType", trade, symbol=None)


def test_an_order_that_never_reached_a_venue_has_no_id(refused):
    # As in a backtest, and as a trade is built when no ID is given.
    without = {name: value for name, value in TRADE.items() if not name.endswith("_order_id")}
    simulated = qf.ClosedTrade(**without)
    assert simulated.entry_order_id is None
    assert simulated.exit_order_id is None
    assert simulated == trade(entry_order_id=None, exit_order_id=None)
    assert simulated != trade()
    # A venue's ID is its own text, kept as it is: a number on one venue is
    # not one on another, and none is computed with.
    for venue_id in ["8389765519", "0", "a1b2-c3", "x" * 128]:
        assert trade(entry_order_id=venue_id).entry_order_id == venue_id
        assert trade(exit_order_id=venue_id).exit_order_id == venue_id
    for field in ["entry_order_id", "exit_order_id"]:
        for venue_id in ["", "has space", "tab\tbed", "x" * 129]:
            message = (
                f'invalid venue order id "{venue_id}": 1 to 128 characters without spaces'
            ).replace("\t", "\\t")
            refused(ValueError, message, trade, **{field: venue_id})
        for value, named in [(8389765519, "int"), (Decimal("1"), "Decimal"), (b"1", "bytes")]:
            message = f"{field} must be a str or None, not {named}"
            refused(TypeError, message, trade, **{field: value})


def test_a_trades_times_and_amounts_are_held_like_any_other(refused):
    for field in ["entry_time_ms", "exit_time_ms"]:
        assert getattr(trade(**{field: I64_MIN}), field) == I64_MIN
        message = f"invalid {field} {I64_MAX + 1}: must be between {I64_MIN} and {I64_MAX}"
        refused(ValueError, message, trade, **{field: I64_MAX + 1})
        refused(TypeError, f"{field} must be an int, not float", trade, **{field: 1.5})
    for field in TRADE_DECIMALS:
        message = (
            f"{field} must be a decimal.Decimal or an int, not float: a float is never read as "
            "a decimal; convert it with decimal_from_f64"
        )
        refused(TypeError, message, trade, **{field: 1.5})
        message = f"invalid {field} Infinity: must be a finite number"
        refused(ValueError, message, trade, **{field: Decimal("Infinity")})
    # A loss is a negative PnL, and nothing checks a trade's arithmetic.
    lost = trade(gross_quote_pnl=Decimal("-12.5"), exit_time_ms=0, qty=0)
    assert (lost.gross_quote_pnl, lost.exit_time_ms, lost.qty) == (Decimal("-12.5"), 0, 0)


def test_trades_are_equal_when_every_field_is_and_are_not_keys(evaluated):
    assert trade() == trade(entry_price=Decimal("100"), exit_price=Decimal("125.50"))
    for field, other in OTHER_TRADE.items():
        assert trade() != trade(**{field: other}), field
    assert trade() != TRADE
    with pytest.raises(TypeError, match="unhashable"):
        hash(trade())
    assert repr(trade(entry_order_id=None)) == (
        "ClosedTrade(symbol=Symbol('BTCUSDT'), entry_time_ms=1700000060000, "
        "exit_time_ms=1700000180000, entry_price=Decimal('100.00'), exit_price=Decimal('125.5'), "
        "qty=Decimal('9.98501497753369945082376436'), gross_quote_pnl=Decimal('251.5'), "
        "entry_order_id=None, exit_order_id='1321003749386327552')"
    )
    for made in [trade(), trade(entry_order_id=None, exit_order_id=None)]:
        back = evaluated(repr(made))
        assert back == made
        for field in TRADE:
            assert exact(getattr(back, field)) == exact(getattr(made, field)), field


def test_a_trades_fields_are_named_and_cannot_be_changed():
    with pytest.raises(TypeError, match="takes 0 positional arguments"):
        qf.ClosedTrade(*TRADE.values())
    for field in TRADE:
        without = {name: value for name, value in TRADE.items() if name != field}
        if field.endswith("_order_id"):
            assert getattr(qf.ClosedTrade(**without), field) is None
        else:
            with pytest.raises(TypeError, match=f"missing 1 required keyword argument: '{field}'"):
                qf.ClosedTrade(**without)
    made = trade()
    for field in TRADE:
        with pytest.raises(AttributeError):
            setattr(made, field, TRADE[field])


def test_an_equity_point_is_a_bars_open_time_and_its_equity(evaluated, refused):
    made = qf.EquityPoint(open_time_ms=120_000, equity=Decimal("1050.50"))
    assert made.open_time_ms == 120_000
    assert exact(made.equity) == exact(Decimal("1050.50"))
    assert repr(made) == "EquityPoint(open_time_ms=120000, equity=Decimal('1050.50'))"
    assert exact(evaluated(repr(made)).equity) == exact(made.equity)
    assert made == qf.EquityPoint(open_time_ms=120_000, equity=Decimal("1050.5"))
    assert made != qf.EquityPoint(open_time_ms=180_000, equity=Decimal("1050.50"))
    assert made != qf.EquityPoint(open_time_ms=120_000, equity=Decimal("1050.51"))
    assert made != (120_000, Decimal("1050.50"))
    # Equity below zero is a number like any other.
    assert qf.EquityPoint(open_time_ms=0, equity=Decimal("-1")).equity == -1
    with pytest.raises(TypeError, match="unhashable"):
        hash(made)
    with pytest.raises(AttributeError):
        made.equity = Decimal("0")
    with pytest.raises(TypeError, match="takes 0 positional arguments"):
        qf.EquityPoint(120_000, Decimal("1050.50"))
    with pytest.raises(TypeError, match="missing 1 required keyword argument: 'equity'"):
        qf.EquityPoint(open_time_ms=0)
    message = "open_time_ms must be an int, not str"
    refused(TypeError, message, qf.EquityPoint, open_time_ms="0", equity=1)


def test_a_result_gives_back_every_field_as_it_was_given():
    fields = result_fields()
    made = result()
    for field in RESULT_DECIMALS:
        back = getattr(made, field)
        assert type(back) is Decimal
        assert exact(back) == exact(fields[field]), field
    assert made.trade_count == 1
    assert type(made.trades) is tuple
    assert type(made.equity_curve) is tuple
    assert made.trades == tuple(fields["trades"])
    assert made.equity_curve == tuple(fields["equity_curve"])
    for back, given in zip(made.equity_curve, fields["equity_curve"], strict=True):
        assert type(back) is qf.EquityPoint
        assert exact(back.equity) == exact(given.equity)
    for back, given in zip(made.trades, fields["trades"], strict=True):
        assert type(back) is qf.ClosedTrade
        for field in TRADE:
            assert exact(getattr(back, field)) == exact(getattr(given, field)), field


def test_a_result_takes_anything_that_iterates_its_trades_and_its_points():
    fields = result_fields()
    assert result(trades=tuple(fields["trades"])) == result()
    assert result(trades=iter(fields["trades"])) == result()
    assert result(equity_curve=(made for made in fields["equity_curve"])) == result()
    empty = result(trades=[], equity_curve=(), trade_count=0)
    assert (empty.trades, empty.equity_curve, empty.trade_count) == ((), (), 0)


def test_a_result_holds_what_it_is_given_and_recounts_nothing():
    # The engine counts its own trades. A result built by hand says what it
    # is told, as the engine's own type does.
    odd = result(trade_count=5, final_equity=Decimal("-1"), total_return_pct=Decimal("0"))
    assert odd.trade_count == 5
    assert len(odd.trades) == 1
    assert (odd.final_equity, odd.total_return_pct) == (-1, 0)


def test_results_are_equal_when_every_field_is_and_are_not_keys():
    fields = result_fields()
    assert result() == result()
    assert result() == result(initial_cash=Decimal("1000.0"), final_equity=1250)
    for field in RESULT_DECIMALS:
        assert result() != result(**{field: fields[field] + 1}), field
    assert result() != result(trade_count=2)
    assert result() != result(trades=[])
    assert result() != result(trades=[trade(), trade()])
    assert result() != result(trades=[trade(qty=Decimal("1"))])
    assert result() != result(equity_curve=fields["equity_curve"][:-1])
    assert result() != result(equity_curve=fields["equity_curve"][::-1])
    assert result() != fields
    with pytest.raises(TypeError, match="unhashable"):
        hash(result())


def test_a_results_repr_counts_its_trades_and_its_points():
    head = (
        "BacktestResult(initial_cash=Decimal('1000'), final_equity=Decimal('1250.00'), "
        "total_return_pct=Decimal('25.000'), trade_count=1, "
        "max_drawdown_pct=Decimal('4.5454545454545454545454545455'), "
    )
    assert repr(result()) == head + "trades=<1 trade>, equity_curve=<4 points>)"
    one = result(trades=[], equity_curve=[point(0, "1000")])
    assert repr(one) == head + "trades=<0 trades>, equity_curve=<1 point>)"
    many = result(trades=[trade(), trade()], equity_curve=[])
    assert repr(many) == head + "trades=<2 trades>, equity_curve=<0 points>)"


def test_a_results_fields_are_named_required_and_of_their_own_types(refused):
    fields = result_fields()
    with pytest.raises(TypeError, match="takes 0 positional arguments"):
        qf.BacktestResult(*fields.values())
    for field in fields:
        without = {name: value for name, value in fields.items() if name != field}
        with pytest.raises(TypeError, match=f"missing 1 required keyword argument: '{field}'"):
            qf.BacktestResult(**without)
    for field in RESULT_DECIMALS:
        message = (
            f"{field} must be a decimal.Decimal or an int, not float: a float is never read as "
            "a decimal; convert it with decimal_from_f64"
        )
        refused(TypeError, message, result, **{field: 25.0})
    refused(TypeError, "trade_count must be an int, not float", result, trade_count=1.0)
    # A count is as wide as the platform's addresses.
    message = f"invalid trade_count -1: must be between 0 and {2 * sys.maxsize + 1}"
    refused(ValueError, message, result, trade_count=-1)
    refused(TypeError, "trades must be an iterable of ClosedTrade, not int", result, trades=1)
    message = "trades[1] must be a ClosedTrade, not EquityPoint"
    refused(TypeError, message, result, trades=[trade(), point(0, "1")])
    message = "equity_curve must be an iterable of EquityPoint, not NoneType"
    refused(TypeError, message, result, equity_curve=None)
    message = "equity_curve[0] must be an EquityPoint, not tuple"
    refused(TypeError, message, result, equity_curve=[(0, Decimal("1"))])


def test_a_result_cannot_be_changed_and_what_it_gives_is_the_readers_own():
    trades = [trade()]
    made = result(trades=trades)
    trades.append(trade())
    assert len(made.trades) == 1
    for field in result_fields():
        with pytest.raises(AttributeError):
            setattr(made, field, None)
    with pytest.raises(AttributeError):
        made.sharpe = Decimal("1")
