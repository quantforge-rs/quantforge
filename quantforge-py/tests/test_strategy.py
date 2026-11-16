"""The strategy a Python class implements, and what its callbacks are handed
and hand back: `Strategy`, `StrategyContext` and `TargetPosition`.

These are the types alone. How the engine drives a class derived from
`Strategy` is `test_strategy_cases.py`, what a callback raises or returns
`test_strategy_failures.py`, and the threads it runs on
`test_strategy_hosts.py`."""

import dataclasses
import inspect
from decimal import Decimal

import pytest

import quantforge_py as qf

I64_MIN, I64_MAX = -(2**63), 2**63 - 1

MARKET = qf.MarketId("binance_spot", "BTCUSDT", "1m")

CONTEXT = dict(
    market=MARKET,
    now_ms=1_700_000_060_000,
    cash=Decimal("0.000000000000000000000000010"),
    position_qty=Decimal("99.800299600499400699200899"),
)

# For each field of a context, a value that is not the one above.
OTHER_CONTEXT = dict(
    market=qf.MarketId("binance_spot", "BTCUSDT", "5m"),
    now_ms=1_700_000_120_000,
    cash=Decimal("0.000000000000000000000000011"),
    position_qty=Decimal("99.800299600499400699200898"),
)


def context(**changed) -> qf.StrategyContext:
    return qf.StrategyContext(**{**CONTEXT, **changed})


def exact(value):
    """A value as it is, a decimal down to its digits and decimal places."""
    return value.as_tuple() if isinstance(value, Decimal) else value


class Decides(qf.Strategy):
    """The least a strategy is: a class that defines `on_bar`."""

    def on_bar(self, ctx, bar):
        return qf.TargetPosition.LONG_ALL_IN


def test_there_are_two_targets_and_no_other():
    targets = qf.TargetPosition
    assert type(targets.ALL) is tuple
    assert targets.ALL == (targets.FLAT, targets.LONG_ALL_IN)
    assert all(type(target) is targets for target in targets.ALL)
    named = {name for name, value in vars(targets).items() if type(value) is targets}
    assert named == {"FLAT", "LONG_ALL_IN"}
    assert targets.ALL[0] is targets.FLAT
    assert targets.ALL[1] is targets.LONG_ALL_IN


def test_a_target_is_written_as_its_constant(evaluated):
    assert repr(qf.TargetPosition.FLAT) == "TargetPosition.FLAT"
    assert repr(qf.TargetPosition.LONG_ALL_IN) == "TargetPosition.LONG_ALL_IN"
    for target in qf.TargetPosition.ALL:
        # It has no text of its own: `str` writes what `repr` does, which
        # names the constant itself.
        assert str(target) == repr(target)
        assert evaluated(repr(target)) is target


def test_a_target_is_equal_to_itself_alone_and_is_a_key():
    flat, long = qf.TargetPosition.ALL
    assert flat == flat and long == long
    assert flat != long
    assert flat != "FLAT" and flat != "TargetPosition.FLAT" and flat != 0
    assert long != True  # noqa: E712
    assert hash(flat) == hash(qf.TargetPosition.FLAT)
    assert {flat: "out", long: "in"}[qf.TargetPosition.LONG_ALL_IN] == "in"
    assert len({flat, long, qf.TargetPosition.FLAT}) == 2
    # Neither is true or false by its meaning: both are objects like any other.
    assert flat and long


def test_a_target_is_not_built_or_changed():
    for arguments in [(), ("FLAT",), (qf.TargetPosition.FLAT,)]:
        with pytest.raises(TypeError, match="cannot create 'quantforge_py.TargetPosition'"):
            qf.TargetPosition(*arguments)
    flat = qf.TargetPosition.FLAT
    with pytest.raises(AttributeError):
        flat.name = "flat"
    with pytest.raises(AttributeError):
        del flat.ALL


def test_a_context_gives_back_what_it_was_given():
    made = context()
    assert type(made.market) is qf.MarketId
    assert made.market == MARKET
    assert made.now_ms == 1_700_000_060_000 and type(made.now_ms) is int
    for field in ["cash", "position_qty"]:
        back = getattr(made, field)
        assert type(back) is Decimal
        assert exact(back) == exact(CONTEXT[field]), field
    # An int is the whole number it is, and a context holds what it is
    # given: nothing says an amount is one an engine would hand over.
    odd = context(cash=-5, position_qty=0, now_ms=I64_MIN)
    assert (odd.cash, odd.position_qty, odd.now_ms) == (-5, 0, I64_MIN)
    assert context(now_ms=I64_MAX).now_ms == I64_MAX


def test_a_contexts_fields_are_named_required_and_of_their_own_types(refused):
    with pytest.raises(TypeError, match="takes 0 positional arguments"):
        qf.StrategyContext(*CONTEXT.values())
    for field in CONTEXT:
        without = {name: value for name, value in CONTEXT.items() if name != field}
        with pytest.raises(TypeError, match=f"missing 1 required keyword argument: '{field}'"):
            qf.StrategyContext(**without)
    with pytest.raises(TypeError, match="unexpected keyword argument"):
        context(equity=Decimal("1"))
    # A market is a `MarketId`: its three parts are not read from anything else.
    for value, named in [
        ("binance_spot", "str"),
        (("binance_spot", "BTCUSDT", "1m"), "tuple"),
        (None, "NoneType"),
        (qf.Symbol("BTCUSDT"), "Symbol"),
    ]:
        refused(TypeError, f"market must be a MarketId, not {named}", context, market=value)
    refused(TypeError, "now_ms must be an int, not float", context, now_ms=0.0)
    refused(TypeError, "now_ms must be an int, not bool", context, now_ms=True)
    message = f"invalid now_ms {I64_MAX + 1}: must be between {I64_MIN} and {I64_MAX}"
    refused(ValueError, message, context, now_ms=I64_MAX + 1)
    for field in ["cash", "position_qty"]:
        message = (
            f"{field} must be a decimal.Decimal or an int, not float: a float is never read as "
            "a decimal; convert it with decimal_from_f64"
        )
        refused(TypeError, message, context, **{field: 1.5})
        message = f"{field} must be a decimal.Decimal or an int, not str"
        refused(TypeError, message, context, **{field: "1.5"})
        message = f"invalid {field} NaN: must be a finite number"
        refused(ValueError, message, context, **{field: Decimal("NaN")})


def test_contexts_are_equal_when_every_field_is_and_are_not_keys():
    assert context() == context()
    # Decimals are compared by value.
    assert context(cash=Decimal("1.10")) == context(cash=Decimal("1.1"))
    for field, other in OTHER_CONTEXT.items():
        assert context() != context(**{field: other}), field
    assert context() != CONTEXT
    assert context() != tuple(CONTEXT.values())
    with pytest.raises(TypeError, match="unhashable"):
        hash(context())


def test_a_context_cannot_be_changed():
    made = context()
    for field, other in OTHER_CONTEXT.items():
        with pytest.raises(AttributeError):
            setattr(made, field, other)
        with pytest.raises(AttributeError):
            delattr(made, field)
    with pytest.raises(AttributeError):
        made.equity = Decimal("1")
    assert made == context()


def test_a_contexts_repr_builds_it_again(evaluated):
    made = context()
    assert repr(made) == (
        "StrategyContext(market=MarketId(ExchangeId('binance_spot'), Symbol('BTCUSDT'), "
        "Interval('1m')), now_ms=1700000060000, cash=Decimal('1.0E-26'), "
        "position_qty=Decimal('99.800299600499400699200899'))"
    )
    back = evaluated(repr(made))
    assert back == made and back is not made
    for field in ["cash", "position_qty"]:
        assert exact(getattr(back, field)) == exact(getattr(made, field)), field


def test_a_strategy_is_a_class_derived_from_strategy_that_defines_on_bar(refused):
    made = Decides()
    assert isinstance(made, qf.Strategy)
    assert type(made) is Decides and Decides.__mro__ == (Decides, qf.Strategy, object)

    message = "Strategy cannot be built: derive a class from it that defines on_bar"
    refused(TypeError, message, qf.Strategy)

    class Undecided(qf.Strategy):
        def on_start(self, ctx):
            pass

    refused(TypeError, "Undecided cannot be built: it does not define on_bar", Undecided)

    # What counts is that the class has one, wherever it is defined.
    class Inherits(Decides):
        pass

    class Shares:
        def on_bar(self, ctx, bar):
            return None

    class Mixed(Shares, qf.Strategy):
        pass

    class Later(Undecided):
        def on_bar(self, ctx, bar):
            return qf.TargetPosition.FLAT

    for cls in [Inherits, Mixed, Later]:
        assert isinstance(cls(), qf.Strategy), cls


def test_a_strategy_takes_the_arguments_its_own_init_does(refused):
    class Crossover(qf.Strategy):
        def __init__(self, fast, slow=50, *, label="sma"):
            self.fast, self.slow, self.label = fast, slow, label

        def on_bar(self, ctx, bar):
            return None

    assert vars(Crossover(20)) == dict(fast=20, slow=50, label="sma")
    assert vars(Crossover(5, 8, label="x")) == dict(fast=5, slow=8, label="x")
    # As a strategy bundle's parameters reach it.
    assert vars(Crossover(**{"fast": 2, "slow": 3})) == dict(fast=2, slow=3, label="sma")
    # What its `__init__` does not take is refused by Python itself.
    with pytest.raises(TypeError, match=r"__init__\(\) missing 1 required positional argument"):
        Crossover()
    with pytest.raises(TypeError, match="unexpected keyword argument 'period'"):
        Crossover(20, period=7)

    @dataclasses.dataclass
    class Settings(qf.Strategy):
        fast: int
        slow: int = 50

        def on_bar(self, ctx, bar):
            return None

    assert (Settings(20).fast, Settings(20).slow, Settings(fast=2, slow=3).slow) == (20, 50, 3)

    # A class without an `__init__` takes nothing, as any class of Python.
    refused(TypeError, "Decides() takes no arguments", Decides, 20)
    refused(TypeError, "Decides() takes no arguments", Decides, fast=20)


def test_the_callbacks_a_class_leaves_out_do_nothing():
    made = Decides()
    ctx = context()
    assert made.on_start(ctx) is None
    assert made.on_finish(ctx) is None
    assert made.on_start(ctx=ctx) is None

    # The one a class must define decides nothing either, where its own
    # reaches for it.
    class Defers(qf.Strategy):
        def on_bar(self, ctx, bar):
            return super().on_bar(ctx, bar)

    assert Defers().on_bar(ctx, None) is None
    assert Decides().on_bar(ctx, None) is qf.TargetPosition.LONG_ALL_IN


def test_a_strategy_has_three_callbacks_and_nothing_beside_them():
    public = sorted(name for name in vars(qf.Strategy) if not name.startswith("_"))
    assert public == ["on_bar", "on_finish", "on_start"]
    signatures = {name: str(inspect.signature(getattr(qf.Strategy, name))) for name in public}
    assert signatures == {
        "on_start": "(self, /, ctx)",
        "on_bar": "(self, /, ctx, bar)",
        "on_finish": "(self, /, ctx)",
    }
    for name in public:
        assert getattr(qf.Strategy, name).__doc__, name
    # A context is the engine's state and no way to reach the engine: four
    # values, and nothing to call.
    fields = sorted(name for name in vars(qf.StrategyContext) if not name.startswith("_"))
    assert fields == ["cash", "market", "now_ms", "position_qty"]
    assert all(
        type(vars(qf.StrategyContext)[name]).__name__ == "getset_descriptor" for name in fields
    )


def test_a_strategy_keeps_its_own_state_and_is_an_object_like_any_other():
    class Counts(qf.Strategy):
        def __init__(self):
            self.bars = 0

        def on_bar(self, ctx, bar):
            self.bars += 1
            return None

    made = Counts()
    made.on_bar(context(), None)
    made.on_bar(context(), None)
    made.seen = ["anything"]
    assert vars(made) == dict(bars=2, seen=["anything"])
    # Two strategies are two objects, however alike.
    other = Counts()
    assert made != other and made == made
    assert len({made, other, made}) == 2

    class Slotted(qf.Strategy):
        __slots__ = ("bars",)

        def __init__(self):
            self.bars = 0

        def on_bar(self, ctx, bar):
            return None

    slotted = Slotted()
    assert slotted.bars == 0
    with pytest.raises(AttributeError):
        slotted.other = 1
