"""What becomes of a callback that raises, or that returns what it may not,
and what the adapter refuses before a run starts.

A run that Python hosts, which is `run_backtest`, raises the callback's own
exception: the object the callback raised, its traceback as it was, and one
note that says where the strategy failed. A host that is not Python has the
engine's error, a `StrategyError`, whose text is that line and then the
exception as Python itself writes it, traceback included; the bench's
`embedded` runs raise that text as a `RuntimeError`."""

import gc
import traceback
import weakref
from decimal import Decimal

import pytest

import quantforge_py as qf

BAR = 60_000

FAILED_IN = {
    "on_start": 'the strategy "{name}" failed in on_start',
    "on_bar": 'the strategy "{name}" failed in on_bar, at the bar that opened at {bar}',
    "on_finish": 'the strategy "{name}" failed in on_finish',
}


def said(callback: str, name: str, bar: int = 2 * BAR) -> str:
    """The line that says where a strategy failed."""
    return FAILED_IN[callback].format(name=name, bar=bar)


class Raises(qf.Strategy):
    """Raises a given exception in one callback: for `on_bar`, on the third
    bar."""

    def __init__(self, raised, where="on_bar"):
        self.raised, self.where, self.calls = raised, where, []

    def fail(self):
        raise self.raised

    def on_start(self, ctx):
        self.calls.append("on_start")
        if self.where == "on_start":
            self.fail()

    def on_bar(self, ctx, bar):
        self.calls.append("on_bar")
        if self.where == "on_bar" and bar.open_time_ms == 2 * BAR:
            self.fail()

    def on_finish(self, ctx):
        self.calls.append("on_finish")
        if self.where == "on_finish":
            self.fail()


# What the engine had called, over five bars, by the time each callback
# failed.
CALLS = {
    "on_start": ["on_start"],
    "on_bar": ["on_start", "on_bar", "on_bar", "on_bar"],
    "on_finish": ["on_start", *["on_bar"] * 5, "on_finish"],
}


class Returns(qf.Strategy):
    """Returns a given value from one callback: for `on_bar`, on the third
    bar."""

    def __init__(self, value, where="on_bar"):
        self.value, self.where, self.calls = value, where, []

    def on_start(self, ctx):
        self.calls.append("on_start")
        return self.value if self.where == "on_start" else None

    def on_bar(self, ctx, bar):
        self.calls.append("on_bar")
        return self.value if self.where == "on_bar" and bar.open_time_ms == 2 * BAR else None

    def on_finish(self, ctx):
        self.calls.append("on_finish")
        return self.value if self.where == "on_finish" else None


def frames(raised: BaseException) -> list[str]:
    """The functions an exception's traceback passes through, outermost first."""
    return [frame.name for frame in traceback.extract_tb(raised.__traceback__)]


def as_the_callback_raised_it(raised: BaseException, callback: str) -> str:
    """An exception as Python writes it, from the callback that raised it
    inward and before any note was added: what the engine is told."""
    trace = raised.__traceback__
    while trace is not None and trace.tb_frame.f_code.co_name != callback:
        trace = trace.tb_next
    assert trace is not None, f"{callback} is in the traceback"
    notes = raised.__dict__.pop("__notes__", None)
    try:
        return "".join(traceback.format_exception(type(raised), raised, trace)).rstrip()
    finally:
        if notes is not None:
            raised.__notes__ = notes


@pytest.mark.parametrize("where", ["on_start", "on_bar", "on_finish"])
def test_a_callback_that_raises_ends_the_run_with_its_own_exception(where, bench, market, minutes):
    candles = minutes(*[(100, 100)] * 5)
    config = qf.BacktestConfig(initial_cash=1000, fee_bps=0, close_out_at_end=True)
    raised = ZeroDivisionError("division by zero")
    strategy = Raises(raised, where)
    # Called as a user calls it, so that nothing of the tests' own lies
    # between this frame and the callback's.
    with pytest.raises(ZeroDivisionError) as caught:
        qf.run_backtest(candles, strategy, config, market=market)
    # The object the callback raised, and nothing wrapped around it.
    assert caught.value is raised
    assert caught.value.__cause__ is None and caught.value.__context__ is None
    # The traceback as it was: from this test, into the callback, to the
    # line that raised. The engine between them is no frame of Python's.
    assert frames(caught.value) == [
        "test_a_callback_that_raises_ends_the_run_with_its_own_exception",
        where,
        "fail",
    ]
    assert caught.value.__notes__ == [said(where, "Raises")]
    # No callback follows the one that raised.
    assert strategy.calls == CALLS[where]

    # A host that is not Python is told the same, as text: where, and then
    # the exception as Python writes it.
    again = Raises(ZeroDivisionError("division by zero"), where)
    with pytest.raises(RuntimeError) as told:
        bench(market, candles, again, config, embedded=True)
    assert type(told.value) is RuntimeError
    assert again.calls == CALLS[where]
    expected = said(where, "Raises") + "\n" + as_the_callback_raised_it(caught.value, where)
    assert str(told.value) == expected
    lines = str(told.value).splitlines()
    assert lines[1] == "Traceback (most recent call last):"
    assert lines[2].startswith(f'  File "{__file__}", line ')
    assert lines[2].endswith(f", in {where}")
    assert lines[-1] == "ZeroDivisionError: division by zero"
    assert not hasattr(told.value, "__notes__")


@pytest.mark.parametrize("kind", [KeyboardInterrupt, SystemExit, GeneratorExit])
def test_an_interrupt_or_an_exit_raised_in_a_callback_is_itself(kind, run, embedded):
    raised = kind(3) if kind is SystemExit else kind()
    # As Python writes each: its name, and for an exit its code.
    written = "SystemExit: 3" if kind is SystemExit else kind.__name__
    strategy = Raises(raised)
    with pytest.raises(BaseException) as caught:
        run(strategy)
    assert strategy.calls == CALLS["on_bar"]
    if embedded:
        assert type(caught.value) is RuntimeError
        assert str(caught.value).splitlines()[-1] == written
    else:
        # Not wrapped in anything an `except Exception` would catch: an
        # interrupt of a run interrupts what called it.
        assert caught.value is raised
        assert caught.value.__notes__ == [said("on_bar", "Raises")]


def test_what_an_exception_came_from_is_written_with_it(bench, market, minutes):
    class Chains(qf.Strategy):
        def on_bar(self, ctx, bar):
            try:
                return {}["missing"]
            except KeyError as error:
                raise ValueError("no setting") from error

    candles = minutes((100, 100))
    config = qf.BacktestConfig(initial_cash=1000, fee_bps=0, close_out_at_end=True)
    with pytest.raises(ValueError) as caught:
        bench(market, candles, Chains(), config)
    assert type(caught.value.__cause__) is KeyError
    with pytest.raises(RuntimeError) as told:
        bench(market, candles, Chains(), config, embedded=True)
    text = str(told.value)
    written = as_the_callback_raised_it(caught.value, "on_bar")
    assert text == said("on_bar", "Chains", 0) + "\n" + written
    assert "KeyError: 'missing'" in text
    assert "The above exception was the direct cause of the following exception:" in text
    assert text.splitlines()[-1] == "ValueError: no setting"


def test_a_group_of_exceptions_and_one_that_cannot_write_itself_are_written_too(run, embedded):
    class Unwritable(Exception):
        def __str__(self):
            raise RuntimeError("no words")

    group = ExceptionGroup("two at once", [ValueError("one"), TypeError("two")])
    for raised, last in [
        (group, "    +------------------------------------"),
        (Unwritable(), f"{__name__}.{Unwritable.__qualname__}: <exception str() failed>"),
    ]:
        strategy = Raises(raised)
        with pytest.raises(BaseException) as caught:
            run(strategy)
        if embedded:
            assert type(caught.value) is RuntimeError
            lines = str(caught.value).splitlines()
            assert lines[0] == said("on_bar", "Raises")
            assert lines[-1] == last
        else:
            assert caught.value is raised


def test_a_traceback_python_cannot_write_leaves_the_exceptions_own_line(
    bench, market, minutes, monkeypatch
):
    def broken(*args, **kwargs):
        raise OSError("no source to read")

    monkeypatch.setattr(traceback, "format_exception", broken)
    candles = minutes(*[(100, 100)] * 5)
    config = qf.BacktestConfig(initial_cash=1000, fee_bps=0, close_out_at_end=True)
    with pytest.raises(RuntimeError) as told:
        bench(market, candles, Raises(LookupError("no such bar")), config, embedded=True)
    assert str(told.value) == said("on_bar", "Raises") + "\nLookupError: no such bar"
    # The exception itself needs no writing, and is raised as it was.
    raised = LookupError("no such bar")
    with pytest.raises(LookupError) as caught:
        bench(market, candles, Raises(raised), config)
    assert caught.value is raised


def test_an_exception_that_takes_no_note_is_raised_without_one(bench, market, minutes):
    class Noteless(Exception):
        def add_note(self, note):
            raise RuntimeError("no notes here")

    raised = Noteless("as it is")
    with pytest.raises(Noteless) as caught:
        bench(
            market,
            minutes(*[(100, 100)] * 5),
            Raises(raised),
            qf.BacktestConfig(initial_cash=1000, fee_bps=0, close_out_at_end=True),
        )
    assert caught.value is raised
    assert not hasattr(caught.value, "__notes__")


def test_a_note_is_added_to_the_notes_an_exception_has(run, embedded):
    raised = ValueError("bad bar")
    raised.add_note("the strategy's own note")
    with pytest.raises(BaseException) as caught:
        run(Raises(raised))
    if embedded:
        # The exception's own notes are part of what Python writes.
        last = str(caught.value).splitlines()[-2:]
        assert last == ["ValueError: bad bar", "the strategy's own note"]
    else:
        assert caught.value.__notes__ == ["the strategy's own note", said("on_bar", "Raises")]


# What `on_bar` may not return, with the name of its type.
NOT_A_DECISION = [
    (1, "int"),
    (0, "int"),
    (True, "bool"),
    (False, "bool"),
    ("FLAT", "str"),
    ("TargetPosition.FLAT", "str"),
    (Decimal("1"), "Decimal"),
    ((qf.TargetPosition.FLAT,), "tuple"),
    ([qf.TargetPosition.LONG_ALL_IN], "list"),
    (qf.TargetPosition, "type"),
    (qf.TargetPosition.ALL, "tuple"),
    (NotImplemented, "NotImplementedType"),
]


@pytest.mark.parametrize("value, named", NOT_A_DECISION, ids=lambda value: repr(value))
def test_on_bar_returns_a_target_or_none_and_nothing_else(value, named, run, embedded):
    message = f"on_bar must return a TargetPosition or None, not {named}"
    strategy = Returns(value)
    with pytest.raises((TypeError, RuntimeError)) as caught:
        run(strategy)
    # The run ends there, as with a callback that raises.
    assert strategy.calls == CALLS["on_bar"]
    if embedded:
        assert type(caught.value) is RuntimeError
        assert str(caught.value) == said("on_bar", "Returns") + f"\nTypeError: {message}"
    else:
        assert type(caught.value) is TypeError
        assert str(caught.value) == message
        assert caught.value.__notes__ == [said("on_bar", "Returns")]


@pytest.mark.parametrize("where", ["on_start", "on_finish"])
@pytest.mark.parametrize(
    "value, named",
    [(qf.TargetPosition.LONG_ALL_IN, "TargetPosition"), (0, "int"), (False, "bool"), ("", "str")],
)
def test_on_start_and_on_finish_return_none(where, value, named, run, embedded):
    # A decision returned from either would be dropped without a word.
    message = f"{where} must return None, not {named}"
    strategy = Returns(value, where)
    with pytest.raises((TypeError, RuntimeError)) as caught:
        run(strategy)
    assert strategy.calls == CALLS[where]
    if embedded:
        assert str(caught.value) == said(where, "Returns") + f"\nTypeError: {message}"
    else:
        assert type(caught.value) is TypeError
        assert str(caught.value) == message
        assert caught.value.__notes__ == [said(where, "Returns")]


@pytest.mark.filterwarnings("ignore:coroutine .* was never awaited:RuntimeWarning")
def test_a_coroutine_or_a_callback_of_other_arguments_fails_on_its_first_call(run, embedded):
    class Awaits(qf.Strategy):
        async def on_bar(self, ctx, bar):
            return None

    class Misses(qf.Strategy):
        def on_bar(self, bar):
            return None

    for strategy, kind, message in [
        (Awaits(), TypeError, "on_bar must return a TargetPosition or None, not coroutine"),
        (Misses(), TypeError, "on_bar() takes 2 positional arguments but 3 were given"),
    ]:
        with pytest.raises((kind, RuntimeError)) as caught:
            run(strategy)
        if embedded:
            assert str(caught.value).splitlines()[0] == said("on_bar", type(strategy).__name__, 0)
            assert str(caught.value).splitlines()[-1].endswith(message)
        else:
            assert type(caught.value) is kind
            assert str(caught.value).endswith(message)


def test_a_strategy_is_named_by_its_name_or_by_its_class(run, embedded):
    class Fails(qf.Strategy):
        def on_bar(self, ctx, bar):
            raise ValueError("at once")

    class Labelled(Fails):
        name = "sma_cross"

    class Inherits(Labelled):
        pass

    class Sets(Fails):
        def __init__(self, name):
            self.name = name

    class Computes(Fails):
        @property
        def name(self):
            return "computed"

    for strategy, name in [
        (Fails(), "Fails"),
        (Labelled(), "sma_cross"),
        (Inherits(), "sma_cross"),
        (Sets("fast 20, slow 50"), "fast 20, slow 50"),
        (Sets("ünïcode"), "ünïcode"),
        (Sets(""), ""),
        # Written so that it stays on its line, whatever it holds.
        (Sets('a "quoted"\nname'), 'a \\"quoted\\"\\nname'),
        (Computes(), "computed"),
    ]:
        with pytest.raises((ValueError, RuntimeError)) as caught:
            run(strategy)
        line = said("on_bar", name, 0)
        if embedded:
            assert str(caught.value).splitlines()[0] == line
        else:
            assert caught.value.__notes__ == [line]


def test_a_name_that_is_not_text_is_refused_before_anything_is_called(refused, run):
    class Named(qf.Strategy):
        def __init__(self, name):
            self.name, self.calls = name, []

        def on_start(self, ctx):
            self.calls.append("on_start")

        def on_bar(self, ctx, bar):
            self.calls.append("on_bar")

    for name, named in [(5, "int"), (None, "NoneType"), (b"sma", "bytes"), (["sma"], "list")]:
        strategy = Named(name)
        refused(TypeError, f"a strategy's name must be a str, not {named}", run, strategy)
        assert strategy.calls == []

    class Nameless(Named):
        @property
        def name(self):
            raise LookupError("no name yet")

        @name.setter
        def name(self, value):
            pass

    strategy = Nameless("ignored")
    refused(LookupError, "no name yet", run, strategy)
    assert strategy.calls == []


def test_what_is_not_a_strategy_is_refused_before_anything_is_called(refused, run):
    class Decides(qf.Strategy):
        def on_bar(self, ctx, bar):
            return None

    class Duck:
        """Everything a strategy has, and no strategy: it derives from nothing."""

        name = "duck"

        def on_start(self, ctx):
            raise AssertionError("never called")

        def on_bar(self, ctx, bar):
            raise AssertionError("never called")

        def on_finish(self, ctx):
            raise AssertionError("never called")

    for value, named in [
        (Duck(), "Duck"),
        (None, "NoneType"),
        ("Decides", "str"),
        (lambda ctx, bar: None, "function"),
        (Duck, "type"),
        (qf.Strategy, "type"),
        (qf.TargetPosition.FLAT, "TargetPosition"),
    ]:
        refused(TypeError, f"strategy must be a Strategy, not {named}", run, value)
    # The slip of handing over a class in place of a strategy built from it.
    message = "strategy must be a Strategy, not the class Decides: build one from it"
    refused(TypeError, message, run, Decides)

    class Undecided(qf.Strategy):
        pass

    message = "strategy must be a Strategy, not the class Undecided: build one from it"
    refused(TypeError, message, run, Undecided)


def test_the_engine_refuses_a_run_of_no_candles_before_anything_is_called(bench, market, embedded):
    strategy = Raises(AssertionError("never called"), "on_start")
    config = qf.BacktestConfig(initial_cash=1000, fee_bps=0, close_out_at_end=True)
    with pytest.raises((ValueError, RuntimeError)) as caught:
        bench(market, [], strategy, config, embedded=embedded)
    assert type(caught.value) is (RuntimeError if embedded else ValueError)
    assert str(caught.value) == "no candles provided"
    assert strategy.calls == []


def test_a_strategy_and_what_it_raised_are_let_go_of_when_the_run_is_over(run, embedded):
    class Kept(Exception):
        pass

    def once(raises: bool):
        strategy = Raises(Kept("held by nothing"), "on_bar" if raises else "never")
        seen = weakref.ref(strategy), weakref.ref(strategy.raised)
        try:
            run(strategy)
        except (Kept, RuntimeError):
            assert raises
        else:
            assert not raises
        return seen

    for raises in (False, True):
        strategy, raised = once(raises)
        # An exception that was raised holds its strategy, through the
        # frames of its traceback, and the strategy holds it: Python's
        # collector takes the two apart. The adapter held the one for the
        # run and the other until it was raised again or dropped, and holds
        # neither now.
        gc.collect()
        assert strategy() is None
        assert raised() is None
    # A strategy runs again after it failed: its state is its own.
    again = Raises(Kept("once more"))
    for _ in range(2):
        with pytest.raises((Kept, RuntimeError)):
            run(again)
    assert again.calls == CALLS["on_bar"] * 2
