"""`docs/python-types.md`, held to the package: the types it says are bound,
every signature, constant and field, which types are keys, each refusal it
shows, its examples, and its statement that no futures execution type is
public in Python."""

import copy
import doctest
import inspect
import pickle
import re
from decimal import Decimal

import pytest

import quantforge_py as qf

PAGE = "docs/python-types.md"

# What the package says about itself comes first in `__all__`; the rest is
# what this page is about.
ABOUT = ["__version__", "ENGINE_VERSION", "STORAGE_BACKENDS"]

# The Rust modules the page says stay in Rust, and the file each one's
# public names are declared in.
INTERNAL = {
    "quantforge::model::futures": "src/model/futures/mod.rs",
    "quantforge::model::execution": "src/model/execution/mod.rs",
    "quantforge::ports::futures": "src/ports/futures.rs",
    "quantforge::automation": "src/automation/mod.rs",
}


def bound() -> list[str]:
    return [name for name in qf.__all__ if name not in ABOUT]


def classes() -> dict[str, type]:
    return {name: getattr(qf, name) for name in bound() if isinstance(getattr(qf, name), type)}


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
    return {
        "Symbol": [qf.Symbol("BTCUSDT")],
        "Interval": [qf.Interval.M1],
        "Category": [qf.Category.INVERSE],
        "ExchangeId": [qf.ExchangeId.BYBIT_INVERSE],
        "MarketId": [qf.MarketId("bybit_inverse", "BTCUSD", "8h")],
        "Candle": [known, unknown],
        "CandleBatch": [qf.CandleBatch.from_candles([known, unknown])],
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
        assert quoted(rust) == f"quantforge::{quoted(python)}"
        assert quoted(python) in published, python
        assert described


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
    assert [quoted(name) for name, _, _ in rows] == list(classes())
    made = samples()
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
    """A value equal to `value` and not the same object: its repr evaluated."""
    again = eval(repr(value), {"__builtins__": {}}, {name: getattr(qf, name) for name in bound()})
    assert again is not value
    return again


def test_every_refusal_the_page_shows_is_refused_so(page, quoted):
    rows = page(PAGE).rows("refusals")
    assert len(rows) > 20
    names = {"qf": qf, "Decimal": Decimal, "__builtins__": {"float": float}}
    kinds = {"ValueError": ValueError, "TypeError": TypeError}
    for written, refusal in rows:
        kind, _, message = quoted(refusal).partition(": ")
        with pytest.raises(kinds[kind]) as caught:
            eval(quoted(written), names)
        assert type(caught.value) is kinds[kind], written
        assert str(caught.value) == message, written
    assert {quoted(refusal).partition(": ")[0] for _, refusal in rows} == set(kinds)


def test_every_example_on_the_page_runs_as_written(page):
    examples = page(PAGE).fences("python")
    assert len(examples) > 5
    assert all(">>> " in example for example in examples)
    # One session from top to bottom: a later example uses an earlier one's
    # names, as a reader does.
    session = doctest.DocTestParser().get_doctest("\n".join(examples), {}, PAGE, PAGE, 0)
    assert len(session.examples) > 25
    said = []
    results = doctest.DocTestRunner(verbose=False).run(session, out=said.append)
    assert results.failed == 0, "".join(said)
    assert results.attempted == len(session.examples)


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
