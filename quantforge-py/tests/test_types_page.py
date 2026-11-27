"""`docs/python-types.md`, held to the package: the types it says are bound,
every signature, constant and field, which types are keys, each refusal it
shows, its examples, what it says of a strategy's callbacks and of a run
that fails, what `run_backtest` takes and raises, and its statement that no
futures execution type is public in Python."""

import copy
import doctest
import inspect
import pickle
import re
from decimal import Decimal

import pytest

import quantforge_py as qf

PAGE = "docs/python-types.md"

# What the package says about itself comes first in `__all__`, and last the
# reader of a store's candles, the functions that give or take a pandas
# DataFrame, and the client of a server with what it gives and raises, which
# have pages of their own (`docs/python-reader.md`, `docs/python-pandas.md`,
# `docs/python-client.md`); the rest is what this page is about.
ABOUT = ["__version__", "ENGINE_VERSION", "STORAGE_BACKENDS"]
READER = ["CandleReader"]
FRAMES = [
    "candles_to_dataframe",
    "dataframe_to_candles",
    "trades_to_dataframe",
    "equity_to_dataframe",
]
CLIENT = [
    "Client",
    "Job",
    "JobStatus",
    "LogLine",
    "JobResult",
    "ResultFile",
    "Provenance",
    "ProvenanceSegment",
    "BundleError",
    "RemoteError",
    "AuthError",
    "ValidationError",
    "NotFound",
    "Conflict",
    "ServerUnavailable",
    "VersionMismatch",
]

# What the engine calls a name the package binds under another: a method of
# a type the crate publishes, and the file that declares it.
RUST = {"run_backtest": ("BacktestEngine::run", "src/engine/backtest.rs")}

# The Rust modules the page says stay in Rust, and the file each one's
# public names are declared in.
INTERNAL = {
    "quantforge::model::futures": "src/model/futures/mod.rs",
    "quantforge::model::execution": "src/model/execution/mod.rs",
    "quantforge::ports::futures": "src/ports/futures.rs",
    "quantforge::automation": "src/automation/mod.rs",
}


def bound() -> list[str]:
    return [name for name in qf.__all__ if name not in ABOUT + READER + FRAMES + CLIENT]


def classes() -> dict[str, type]:
    return {name: getattr(qf, name) for name in bound() if isinstance(getattr(qf, name), type)}


def values() -> dict[str, type]:
    """The classes whose objects are values: every one but `Strategy`,
    which is derived from, and whose objects hold a state of their own."""
    return {name: cls for name, cls in classes().items() if cls is not qf.Strategy}


def members(cls, kind: str) -> list[str]:
    """A class's own public members of one kind, as its dict holds them."""
    return [
        name
        for name, member in vars(cls).items()
        if not name.startswith("_") and type(member).__name__ == kind
    ]


def samples() -> dict[str, list]:
    """Values of every type, with and without the fields that may be unknown."""
    times = dict(open_time_ms=1_700_000_000_000, close_time_ms=1_700_000_059_999)
    prices = dict(open=Decimal("81940.01"), high=81990, low=81900, close=81955, volume=1)
    known = qf.Candle(**times, **prices, trades=412)
    unknown = qf.Candle(**times, **prices)
    fill = dict(
        symbol="BTCUSDT",
        entry_time_ms=60_000,
        exit_time_ms=180_000,
        entry_price=100,
        exit_price=125,
        qty=10,
        gross_quote_pnl=250,
    )
    sent = qf.ClosedTrade(**fill, entry_order_id="8389765519", exit_order_id="8389765520")
    simulated = qf.ClosedTrade(**fill)
    point = qf.EquityPoint(open_time_ms=0, equity=1000)
    market = qf.MarketId("bybit_inverse", "BTCUSD", "8h")
    return {
        "Symbol": [qf.Symbol("BTCUSDT")],
        "Interval": [qf.Interval.M1],
        "Category": [qf.Category.INVERSE],
        "ExchangeId": [qf.ExchangeId.BYBIT_INVERSE],
        "MarketId": [market],
        "Candle": [known, unknown],
        "CandleBatch": [
            qf.CandleBatch.from_candles([known, unknown], market=market),
            qf.CandleBatch.from_candles([known, unknown]),
        ],
        "BacktestConfig": [
            qf.BacktestConfig(initial_cash=1000, fee_bps=10, close_out_at_end=True)
        ],
        "ClosedTrade": [sent, simulated],
        "EquityPoint": [point],
        "BacktestResult": [
            qf.BacktestResult(
                initial_cash=1000,
                final_equity=1250,
                total_return_pct=25,
                trade_count=2,
                max_drawdown_pct=0,
                trades=[sent, simulated],
                equity_curve=[point],
            )
        ],
        "TargetPosition": list(qf.TargetPosition.ALL),
        "StrategyContext": [
            qf.StrategyContext(market=market, now_ms=0, cash=Decimal("1000.00"), position_qty=0)
        ],
    }


def test_the_page_lists_every_type_and_function_the_package_binds(page, quoted, repository):
    rows = page(PAGE).rows("bound types")
    assert [quoted(python) for python, _, _ in rows] == bound()
    # Each is the engine's own type under the engine's own name, which the
    # crate publishes at its root.
    root = (repository / "src" / "lib.rs").read_text(encoding="utf-8")
    published = set()
    for names in re.findall(r"^pub use [\w:]+::(\{[^}]*\}|\w+);", root, flags=re.MULTILINE):
        published.update(re.findall(r"\w+", names))
    assert len(published) > 100, "the crate's re-exports were read"
    for python, rust, described in rows:
        path, declared_in = RUST.get(quoted(python), (quoted(python), None))
        assert quoted(rust) == f"quantforge::{path}"
        owner, _, method = path.partition("::")
        assert owner in published, python
        if method:
            source = (repository / declared_in).read_text(encoding="utf-8")
            assert re.search(rf"^impl {owner} \{{$", source, re.MULTILINE), path
            assert f"\n    pub fn {method}(" in source, path
        assert described
    assert set(RUST) <= {quoted(python) for python, _, _ in rows}


def test_the_page_gives_every_signature_python_does(page):
    lines = page(PAGE).section("signatures")
    assert (lines[0], lines[-1]) == ("```text", "```")
    given = []
    for name in bound():
        bound_name = getattr(qf, name)
        if not isinstance(bound_name, type):
            given.append(f"{name}{inspect.signature(bound_name)}")
            continue
        # A class built only by its named constructors has no `__new__`.
        if "__new__" in vars(bound_name):
            given.append(f"{name}{inspect.signature(bound_name)}")
        for method in members(bound_name, "staticmethod"):
            given.append(f"{name}.{method}{inspect.signature(getattr(bound_name, method))}")
        for method in members(bound_name, "method_descriptor"):
            signature = inspect.signature(getattr(bound_name, method))
            taken = list(signature.parameters.values())[1:]
            given.append(f"{name}.{method}{signature.replace(parameters=taken)}")
    assert sorted(lines[1:-1]) == sorted(given)
    assert len(set(given)) == len(given)


def test_the_page_lists_every_constant_in_the_order_all_holds_them(page, quoted):
    rows = page(PAGE).rows("constants")
    listed = {quoted(name): (constants, texts) for name, constants, texts in rows}
    named = {name: cls for name, cls in classes().items() if "ALL" in vars(cls)}
    assert list(listed) == list(named)
    for name, cls in named.items():
        constants, texts = listed[name]
        assert type(cls.ALL) is tuple
        holds = {id(value): constant for constant, value in vars(cls).items() if type(value) is cls}
        assert [quoted(cell) for cell in constants.split(", ")] == [
            holds[id(value)] for value in cls.ALL
        ]
        assert [quoted(cell) for cell in texts.split(", ")] == [str(value) for value in cls.ALL]
        assert len(holds) == len(cls.ALL), "every constant is in ALL"


def gives(value, said: str) -> bool:
    """Whether `value` is what a field's row says the field gives."""
    if said.startswith("tuple of "):
        each = said.removeprefix("tuple of ")
        return type(value) is tuple and all(gives(item, each) for item in value)
    if said.endswith(" or None"):
        return value is None or gives(value, said.removesuffix(" or None"))
    return type(value).__name__ == said


def test_the_page_lists_every_field_with_what_it_gives(page, quoted):
    rows = page(PAGE).rows("fields")
    listed: dict[str, list[tuple[str, str]]] = {name: [] for name in classes()}
    for name, field, said, described in rows:
        listed[quoted(name)].append((quoted(field), quoted(said)))
        assert described
    made = samples()
    for name, cls in classes().items():
        fields = [field for field, _ in listed[name]]
        assert sorted(fields) == sorted(members(cls, "getset_descriptor")), name
        assert len(set(fields)) == len(fields), name
        # A record's fields are the names it is built with, in that order.
        if "__new__" in vars(cls) and fields:
            taken = list(inspect.signature(cls).parameters)
            if taken != ["value"]:
                assert fields[: len(taken)] == taken, name
        for field, said in listed[name]:
            values = [getattr(value, field) for value in made[name]]
            for value in values:
                assert gives(value, said), f"{name}.{field} gives {value!r}, not {said}"
            if said.endswith(" or None"):
                # Both are seen: the field is unknown in one and known in another.
                flat = [
                    item
                    for value in values
                    for item in (value if type(value) is tuple else [value])
                ]
                assert any(item is None for item in flat), f"{name}.{field}"
                assert any(item is not None for item in flat), f"{name}.{field}"


def test_the_page_says_which_types_are_keys(page, quoted):
    rows = page(PAGE).rows("keys")
    assert [quoted(name) for name, _, _ in rows] == list(values())
    made = samples()
    assert list(made) == list(values()), "a sample of every value"
    for name, equal_when, key in rows:
        assert equal_when
        assert key in ("yes", "no"), key
        for value in made[quoted(name)]:
            if key == "yes":
                assert {value: 1}[copy_of(value)] == 1
            else:
                with pytest.raises(TypeError, match="unhashable"):
                    hash(value)


def copy_of(value):
    """A value equal to `value` and not the same object: its repr evaluated.
    A target is the one exception: its repr names its constant, which is the
    target itself."""
    again = eval(repr(value), {"__builtins__": {}}, {name: getattr(qf, name) for name in bound()})
    assert (again is value) == (type(value) is qf.TargetPosition)
    return again


def test_every_refusal_the_page_shows_is_refused_so(page, quoted):
    rows = page(PAGE).rows("refusals")
    assert len(rows) > 20
    names = {"qf": qf, "Decimal": Decimal, "__builtins__": {"float": float, "type": type}}
    kinds = {"ValueError": ValueError, "TypeError": TypeError}
    for written, refusal in rows:
        kind, _, message = quoted(refusal).partition(": ")
        with pytest.raises(kinds[kind]) as caught:
            eval(quoted(written), names)
        assert type(caught.value) is kinds[kind], written
        assert str(caught.value) == message, written
    assert {quoted(refusal).partition(": ")[0] for _, refusal in rows} == set(kinds)


@pytest.fixture(scope="module")
def session(page):
    """The page's examples, run as one session from top to bottom: a later
    example uses an earlier one's names, as a reader does. Gives what the
    run came to, what it said, and the names it left."""
    examples = page(PAGE).fences("python")
    assert len(examples) > 5
    assert all(">>> " in example for example in examples)
    session = doctest.DocTestParser().get_doctest("\n".join(examples), {}, PAGE, PAGE, 0)
    said = []
    runner = doctest.DocTestRunner(verbose=False)
    results = runner.run(session, out=said.append, clear_globs=False)
    return results, "".join(said), len(session.examples), session.globs


def test_every_example_on_the_page_runs_as_written(session):
    results, said, examples, _ = session
    assert examples > 30
    assert results.failed == 0, said
    assert results.attempted == examples


def test_the_page_lists_what_a_run_takes(page, quoted):
    rows = page(PAGE).rows("run arguments")
    taken = inspect.signature(qf.run_backtest).parameters
    assert [quoted(name) for name, _, _, _ in rows] == list(taken)
    left_out = "or nothing where `candles` is a batch that names its market"
    for name, given, takes, described in rows:
        argument = taken[quoted(name)]
        by_name_alone = argument.kind is inspect.Parameter.KEYWORD_ONLY
        assert given == ("By name alone" if by_name_alone else "By position or by name"), name
        # An argument that may be left out is said to be, and where.
        required = argument.default is inspect.Parameter.empty
        assert required == (left_out not in takes), name
        assert required or argument.default is None, name
        assert takes and described
    assert [quoted(name) for name, given, _, _ in rows if given == "By name alone"] == ["market"]
    # The market alone may be left out, and only beside a batch that names
    # one.
    assert [name for name, argument in taken.items() if argument.default is None] == ["market"]


def test_every_run_the_page_shows_refused_raises_so(page, quoted, session):
    rows = page(PAGE).rows("run refusals")
    assert len(rows) >= 8
    kinds = {"TypeError": TypeError, "ValueError": ValueError, "OverflowError": OverflowError}
    # The names are those the page's examples left, as it says.
    *_, names = session
    for written, refusal in rows:
        kind, _, message = quoted(refusal).partition(": ")
        with pytest.raises(kinds[kind]) as caught:
            eval(quoted(written), names)
        assert type(caught.value) is kinds[kind], written
        assert str(caught.value) == message, written
    # Each failure the page lists before a callback's own is shown by rows
    # of its own: the kind of exception, and the words that are that
    # failure's.
    failures = page(PAGE).rows("run failures")
    shown = [tuple(quoted(refusal).partition(": ")[::2]) for _, refusal in rows]
    expected = {
        "An argument of a type it does not take": (
            "TypeError",
            [
                "candles must be ",
                "candles[",
                "strategy must be ",
                "config must be ",
                "market must be ",
            ],
        ),
        "No market named, by `market=` or by a batch": ("TypeError", ["market is required: "]),
        "A market other than the one a batch names": (
            "ValueError",
            ["the candles are of binance_spot BTCUSDT 1m, and market names "],
        ),
        "A market that is not Spot": ("ValueError", ["candle strategies run on spot markets only"]),
        "No candles": ("ValueError", ["no candles provided"]),
        "An amount beyond the engine's decimal": ("OverflowError", ["decimal overflow computing "]),
    }
    assert [what for what, _, _ in failures[: len(expected)]] == list(expected)
    for what, raised, when in failures[: len(expected)]:
        kind, starts = expected[what]
        assert quoted(raised) == kind, what
        assert when
        for start in starts:
            assert any(
                (shown_kind, message[: len(start)]) == (kind, start)
                for shown_kind, message in shown
            ), f"no row shows {what}: {start}"
    assert {kind for kind, _ in shown} == {kind for kind, _ in expected.values()}


def test_a_callback_ends_a_run_with_its_own_exception_as_the_page_says(page, quoted, session):
    *_, raises, returns = page(PAGE).rows("run failures")
    assert raises[:2] == ["A callback that raises", "The callback's own exception"]
    assert returns[0] == "A callback that returns what it may not"
    assert quoted(returns[1]) == "TypeError"
    assert raises[2] and returns[2]
    *_, names = session

    class Unsure(Exception):
        pass

    failure = Unsure("not on this bar")

    class Fails(qf.Strategy):
        def on_bar(self, ctx, bar):
            raise failure

    with pytest.raises(Unsure) as caught:
        qf.run_backtest(names["candles"], Fails(), names["config"], market=names["market"])
    assert caught.value is failure

    class Says(qf.Strategy):
        def on_bar(self, ctx, bar):
            return "LONG_ALL_IN"

    with pytest.raises(TypeError) as caught:
        qf.run_backtest(names["candles"], Says(), names["config"], market=names["market"])
    assert type(caught.value) is TypeError
    assert str(caught.value) == "on_bar must return a TargetPosition or None, not str"


def test_the_page_lists_the_callbacks_a_strategy_has(page, quoted):
    rows = page(PAGE).rows("callbacks")
    written = [quoted(callback) for callback, _, _ in rows]
    # In the order an engine calls them.
    assert written == ["on_start(ctx)", "on_bar(ctx, bar)", "on_finish(ctx)"]
    callbacks = {}
    for name in members(qf.Strategy, "method_descriptor"):
        signature = inspect.signature(getattr(qf.Strategy, name))
        taken = list(signature.parameters.values())[1:]
        callbacks[name] = f"{name}{signature.replace(parameters=taken)}"
    assert sorted(written) == sorted(callbacks.values())
    for _, called, returns in rows:
        assert called and returns
    # The class's own three decide nothing and do nothing: what a strategy
    # gets where its class leaves a callback as it is.
    plain = type("Plain", (qf.Strategy,), {"on_bar": lambda self, ctx, bar: None})()
    assert qf.Strategy.on_start(plain, None) is None
    assert qf.Strategy.on_bar(plain, None, None) is None
    assert qf.Strategy.on_finish(plain, None) is None


def returning(callback: str, value):
    """A strategy that returns `value` from `callback`, the first time it
    is called, and `None` from every other call."""

    def returns(self, *handed):
        if self.returned:
            return None
        self.returned = True
        return value

    body = {"on_bar": lambda self, ctx, bar: None, "returned": False, callback: returns}
    return type("Returns", (qf.Strategy,), body)()


def test_every_return_the_page_shows_fails_the_run_so(page, quoted, bench, market, minutes):
    rows = page(PAGE).rows("returns")
    assert {quoted(callback) for _, callback, _ in rows} == {"on_start", "on_bar", "on_finish"}
    config = qf.BacktestConfig(initial_cash=1000, fee_bps=0, close_out_at_end=True)
    for returned, callback, failure in rows:
        kind, _, message = quoted(failure).partition(": ")
        assert kind == "TypeError"
        value = eval(quoted(returned), {"qf": qf, "__builtins__": {}})
        strategy = returning(quoted(callback), value)
        with pytest.raises(TypeError) as caught:
            bench(market, minutes((100, 100), (100, 100)), strategy, config)
        assert type(caught.value) is TypeError, returned
        assert str(caught.value) == message, returned
    # What the page says is taken is taken: a target, and nothing at all.
    for value in (*qf.TargetPosition.ALL, None):
        bench(market, minutes((100, 100), (100, 100)), returning("on_bar", value), config)


def test_the_failure_the_page_shows_is_what_the_engine_is_told(page, bench):
    lines = page(PAGE).section("failure as told")
    assert (lines[0], lines[-1]) == ("```text", "```")
    shown = lines[1:-1]

    class EnterThenExit(qf.Strategy):
        total = count = 0

        def on_bar(self, ctx, bar):
            average = self.total / self.count
            return qf.TargetPosition.FLAT if average > bar.close else None

    opened = 1_700_000_000_000
    bar = qf.Candle(
        open_time_ms=opened, close_time_ms=opened + 59_999, open=1, high=1, low=1, close=1, volume=1
    )
    market = qf.MarketId("binance_spot", "BTCUSDT", "1m")
    config = qf.BacktestConfig(initial_cash=1000, fee_bps=0, close_out_at_end=True)
    with pytest.raises(RuntimeError) as told:
        bench(market, [bar], EnterThenExit(), config, embedded=True)

    def anywhere(line: str) -> str:
        """A traceback's line without the file and the line number, which
        are the reader's own."""
        return re.sub(r'^  File ".*", line \d+, in ', '  File "...", line ..., in ', line)

    assert [anywhere(line) for line in str(told.value).splitlines()] == [
        anywhere(line) for line in shown
    ]
    assert anywhere(shown[2]) != shown[2], "the page shows where it was raised"
    # The first line is the note Python finds on the exception itself.
    with pytest.raises(ZeroDivisionError) as raised:
        bench(market, [bar], EnterThenExit(), config)
    assert raised.value.__notes__ == [shown[0]]


def public_names(source: str) -> set[str]:
    """The names a Rust module publishes: what it declares, and what it
    re-exports from its own parts."""
    declared = re.findall(
        r"^\s*pub (?:const fn|fn|struct|enum|trait|type|const|static) (\w+)", source, re.MULTILINE
    )
    names = set(declared)
    for group in re.findall(r"^pub use [\w:]+::(\{[^}]*\}|\w+);", source, flags=re.MULTILINE):
        names.update(re.findall(r"\w+", group))
    return names


def test_no_futures_execution_type_is_public_in_python(page, quoted, repository):
    rows = page(PAGE).rows("not bound")
    assert [quoted(module) for module, _ in rows] == list(INTERNAL)
    internal = {}
    for module, path in INTERNAL.items():
        names = public_names((repository / path).read_text(encoding="utf-8"))
        assert len(names) > 5, f"{module}'s public names were read"
        internal[module] = names
    # The reading finds what is there: a name of each module.
    assert "RealizedPnl" in internal["quantforge::model::futures"]
    assert "ExecutionUnit" in internal["quantforge::model::execution"]
    assert "FuturesConnector" in internal["quantforge::ports::futures"]
    assert "PolicyService" in internal["quantforge::automation"]
    for module, names in internal.items():
        crossed = sorted(names & set(qf.__all__))
        assert not crossed, f"{module} is internal, and the package publishes {crossed}"


def test_a_value_cannot_be_pickled_or_deep_copied_as_the_page_says(page):
    assert "**A value cannot be pickled or deep-copied yet.**" in page(PAGE).text
    for name, values in samples().items():
        for value in values:
            with pytest.raises(TypeError, match="cannot pickle"):
                pickle.dumps(value)
            with pytest.raises(TypeError, match="cannot pickle"):
                copy.deepcopy(value)
