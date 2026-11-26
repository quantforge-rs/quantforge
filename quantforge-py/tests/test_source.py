"""A class as `submit_class` sends one, in the test's own process
(docs/python-bundles.md): the source text of its `class` statement, found
in its module's file or in the notebook's cell that defined it, written
below the imports its caller declares, and every class and every name that
is refused before anything is sent.

A notebook is stood in for by what its kernel does for a class's source: a
cell is compiled under a name of its own, whose text Python keeps, and run
in a module that has no file (`Notebook`). The same cells run in a real
IPython shell find their classes the same way; one test runs a program
whose `__main__` has no file, as a kernel's has none.

What a client then does with the file, the bundle it makes of it and the
requests it sends, is in `test_bundles.py` and `test_submit.py`, and a
class submitted to a real server in `test_submit_jobs.py`.
"""

import builtins
import functools
import hashlib
import importlib.util
import itertools
import linecache
import random
import subprocess
import sys
import textwrap
import types

import pytest

import jobs
import quantforge_py as qf
from quantforge_py import _names, _source

QF = "import quantforge_py as qf"

CAVEATS = (
    "submit_class sends the source text of the class as the one file of a bundle, and nothing "
    "else of where the class was written: no import, no global and no variable around it "
    "travels unless imports declares it, and the server runs that text, not the class as it is "
    "in memory here. Pass source_only=True to say that this is what is meant; a strategy kept "
    "in files is submitted with submit()"
)

_numbered = itertools.count(1)


class Notebook:
    """A notebook's kernel, as far as the source of a class goes: every cell
    is compiled under a name of its own and run in one module that has no
    file, and Python keeps each cell's text, as it does a cell of
    IPython's."""

    def __init__(self):
        self.name = f"notebook_{next(_numbered)}"
        self.module = types.ModuleType(self.name)
        sys.modules[self.name] = self.module
        self.cells = []

    def run(self, text: str):
        """Runs a cell, and gives the module's names."""
        text = textwrap.dedent(text).lstrip("\n")
        name = f"<{self.name}-cell-{len(self.cells) + 1}>"
        linecache.cache[name] = (len(text), None, text.splitlines(True), name)
        self.cells.append(name)
        exec(compile(text, name, "exec"), vars(self.module))
        return self.module

    def close(self):
        sys.modules.pop(self.name, None)
        for name in self.cells:
            linecache.cache.pop(name, None)


@pytest.fixture
def notebook():
    made = Notebook()
    made.run(QF)
    yield made
    made.close()


@pytest.fixture
def module_of(tmp_path):
    """Writes a module's file and imports it: `module_of(text)` gives the
    module, which is forgotten again as the test ends."""
    made = []

    def module_of(text: str):
        name = f"strategies_{next(_numbered)}"
        path = tmp_path / f"{name}.py"
        path.write_text(textwrap.dedent(text).lstrip("\n"), encoding="utf-8")
        spec = importlib.util.spec_from_file_location(name, path)
        module = importlib.util.module_from_spec(spec)
        sys.modules[name] = module
        made.append(name)
        spec.loader.exec_module(module)
        return module

    yield module_of
    for name in made:
        sys.modules.pop(name, None)


def file_of(strategy, imports=QF) -> str:
    """The text of the file `submit_class` writes for a class."""
    (name, text), entry = _source._class_file(strategy, imports, True)
    assert name == "strategy.py" and entry == f"strategy.py:{strategy.__name__}"
    return text.decode("utf-8")


def run_from(text: str, name: str, **params):
    """Runs the class a written file defines over a few bars, as a worker
    would: the file's text executed where nothing else is defined."""
    names = {"__name__": "strategy"}
    exec(compile(text, "strategy.py", "exec"), names)
    candles = [
        qf.Candle(
            open_time_ms=index * 60_000,
            close_time_ms=index * 60_000 + 59_999,
            open=100 + index,
            high=101 + index,
            low=99 + index,
            close=100 + index,
            volume=1,
        )
        for index in range(8)
    ]
    config = qf.BacktestConfig(initial_cash=1000, fee_bps=0, close_out_at_end=True)
    market = qf.MarketId("binance_spot", "BTCUSDT", "1m")
    return qf.run_backtest(candles, names[name](**params), config, market=market)


# Where a class's source is found.


def test_a_class_written_in_a_file_is_sent_as_its_own_statement_below_its_imports(module_of):
    module = module_of(
        '''
        """A module with more in it than the class."""

        import functools

        import quantforge_py as qf

        THRESHOLD = 3


        def helper():
            return THRESHOLD


        class Other(qf.Strategy):
            def on_bar(self, ctx, bar):
                return helper()


        @functools.total_ordering
        class Mine(qf.Strategy):
            """Goes long on the second bar.

        A line of its docstring that starts at the margin.
            """

            name = "mine"  # as its job's failures name it

            def __init__(self, after: int = 2):
                self.after, self.seen = after, 0

            def __lt__(self, other):
                return False

            def on_bar(self, ctx, bar):
                self.seen += 1
                if self.seen == self.after:
                    return qf.TargetPosition.LONG_ALL_IN
                return None


        AFTER = "defined after the class, and no part of it"
        '''
    )
    text = file_of(module.Mine, "import functools\nimport quantforge_py as qf")
    assert text == textwrap.dedent(
        '''
        import functools
        import quantforge_py as qf


        @functools.total_ordering
        class Mine(qf.Strategy):
            """Goes long on the second bar.

        A line of its docstring that starts at the margin.
            """

            name = "mine"  # as its job's failures name it

            def __init__(self, after: int = 2):
                self.after, self.seen = after, 0

            def __lt__(self, other):
                return False

            def on_bar(self, ctx, bar):
                self.seen += 1
                if self.seen == self.after:
                    return qf.TargetPosition.LONG_ALL_IN
                return None
        '''
    ).lstrip("\n")
    # The file stands alone: run where nothing else is defined, its class
    # trades as the class it was written from.
    there = run_from(text, "Mine", after=3)
    config = qf.BacktestConfig(initial_cash=1000, fee_bps=0, close_out_at_end=True)
    assert there.trade_count == 1 and there.trades[0].entry_time_ms == 3 * 60_000
    assert there.final_equity != config.initial_cash
    # The same class gives the same file, every time it is asked.
    assert file_of(module.Mine, "import functools\nimport quantforge_py as qf") == text


def test_the_goldens_strategy_is_sent_as_a_file_that_makes_the_goldens_trades(module_of):
    module = module_of((jobs.GOLDEN / "sma_cross.py").read_text(encoding="utf-8"))
    imports = "from collections import deque\n\nimport quantforge_py as qf"
    text = file_of(module.SmaCross, imports)
    assert text.startswith(f"{imports}\n\n\nclass SmaCross(qf.Strategy):\n")
    assert text.endswith("            return qf.TargetPosition.FLAT\n        return None\n")
    names = {"__name__": "strategy"}
    exec(compile(text, "strategy.py", "exec"), names)
    # Over the goldens' frozen day it decides as the file it came from.
    klines = jobs.golden_klines()
    candles = [
        qf.Candle(
            open_time_ms=row[0],
            close_time_ms=row[6],
            open=__import__("decimal").Decimal(row[1]),
            high=__import__("decimal").Decimal(row[2]),
            low=__import__("decimal").Decimal(row[3]),
            close=__import__("decimal").Decimal(row[4]),
            volume=__import__("decimal").Decimal(row[5]),
        )
        for row in klines
    ]
    config = qf.BacktestConfig(initial_cash=10000, fee_bps=10, close_out_at_end=True)
    market = qf.MarketId("binance_spot", "BTCUSDT", "1m")
    sent = qf.run_backtest(candles, names["SmaCross"](fast=20, slow=50), config, market=market)
    expected = jobs.golden()["result"]
    assert format(sent.final_equity, "f") == expected["final_equity"]
    assert sent.trade_count == expected["trade_count"] == 15
    # Without the import its source needs, it is refused, with the line to
    # add.
    with pytest.raises(ValueError) as refused:
        _source._class_file(module.SmaCross, QF, True)
    assert str(refused.value) == (
        "SmaCross uses deque, which its source does not define: submit_class sends the class's "
        "own source below imports, and nothing else of where it was written; add 'from "
        "collections import deque' to imports"
    )


def test_a_class_of_a_notebooks_cell_is_found_in_the_cell_that_defined_it(notebook):
    module = notebook.run(
        '''
        import math

        LIMIT = 3


        class First(qf.Strategy):
            def on_bar(self, ctx, bar):
                return None


        class Mine(qf.Strategy):
            """Holds nothing."""

            def __init__(self, fast=2):
                self.fast = fast

            def on_bar(self, ctx, bar):
                return None
        '''
    )
    # Python's own `inspect` finds no source of a class whose module has no
    # file, as a notebook's has none.
    with pytest.raises((OSError, TypeError)):
        __import__("inspect").getsource(module.Mine)
    assert file_of(module.Mine) == textwrap.dedent(
        '''
        import quantforge_py as qf


        class Mine(qf.Strategy):
            """Holds nothing."""

            def __init__(self, fast=2):
                self.fast = fast

            def on_bar(self, ctx, bar):
                return None
        '''
    ).lstrip("\n")
    assert file_of(module.First).endswith(
        "class First(qf.Strategy):\n    def on_bar(self, ctx, bar):\n        return None\n"
    )


def test_a_class_defined_again_in_a_later_cell_is_sent_as_the_later_one(notebook):
    first = notebook.run(
        """
        class Mine(qf.Strategy):
            def on_bar(self, ctx, bar):
                return None
        """
    ).Mine
    second = notebook.run(
        """
        class Mine(qf.Strategy):
            def on_start(self, ctx):
                self.started = True

            def on_bar(self, ctx, bar):
                return qf.TargetPosition.FLAT
        """
    ).Mine
    assert "qf.TargetPosition.FLAT" in file_of(second) and "on_start" in file_of(second)
    # A class is sent as the cell that made it, whichever name it has now.
    assert "FLAT" not in file_of(first) and "return None" in file_of(first)


def test_a_class_written_twice_in_one_cell_is_sent_as_the_statement_that_made_it(notebook):
    module = notebook.run(
        """
        class Mine(qf.Strategy):
            def on_bar(self, ctx, bar):
                return None


        Earlier = Mine


        class Mine(qf.Strategy):
            def on_bar(self, ctx, bar):
                return qf.TargetPosition.FLAT
        """
    )
    assert "FLAT" in file_of(module.Mine) and "return None" not in file_of(module.Mine)
    assert "return None" in file_of(module.Earlier) and "FLAT" not in file_of(module.Earlier)


def test_a_class_found_by_its_name_alone_is_refused_where_its_file_defines_the_name_twice(
    module_of,
):
    # A class that defines no function of its own is found as `inspect`
    # finds one. From Python 3.13 on a class says which line it starts on.
    # Before, its name is all there is, and the first statement of that
    # name was taken: the text of another class than the one submitted.
    module = module_of(
        """
        import quantforge_py as qf


        class Mine(qf.Strategy):
            LIMIT = 3


        class Mine(qf.Strategy):
            LIMIT = 5


        class Once(qf.Strategy):
            LIMIT = 7
        """
    )
    assert module.Mine.LIMIT == 5
    if sys.version_info >= (3, 13):
        assert file_of(module.Mine) == f"{QF}\n\n\nclass Mine(qf.Strategy):\n    LIMIT = 5\n"
    else:
        with pytest.raises(ValueError) as refused:
            _source._class_file(module.Mine, QF, True)
        assert str(refused.value) == (
            f"Mine is defined 2 times in {module.__file__}, and defines no function of its own "
            "by which the statement that made it would be found: before Python 3.13 a class says "
            "no more of where it was written than its name. Give each of them a name of its own, "
            "or submit the file with submit()"
        )
    # One that is defined once is found by its name on every Python.
    assert file_of(module.Once) == f"{QF}\n\n\nclass Once(qf.Strategy):\n    LIMIT = 7\n"
    # And one with a function of its own is found by that, on every Python,
    # whichever of two statements of its name made it.
    module = module_of(
        """
        import quantforge_py as qf


        class Mine(qf.Strategy):
            LIMIT = 3

            def on_bar(self, ctx, bar):
                return None


        Earlier = Mine


        class Mine(qf.Strategy):
            LIMIT = 5

            def on_bar(self, ctx, bar):
                return None
        """
    )
    assert "LIMIT = 5" in file_of(module.Mine) and "LIMIT = 3" not in file_of(module.Mine)
    assert "LIMIT = 3" in file_of(module.Earlier) and "LIMIT = 5" not in file_of(module.Earlier)


def test_a_class_is_found_by_any_function_of_its_own_whatever_it_is_wrapped_in(notebook):
    module = notebook.run(
        """
        class Labelled(qf.Strategy):
            limit = 3

            @property
            def label(self):
                return "x"


        class Counted(qf.Strategy):
            @staticmethod
            def count():
                return 1


        class Made(qf.Strategy):
            @classmethod
            def make(cls):
                return cls()
        """
    )
    for name in ("Labelled", "Counted", "Made"):
        text = file_of(getattr(module, name))
        assert text.startswith(f"{QF}\n\n\nclass {name}(qf.Strategy):\n"), name
        assert text.count("class ") == 1, name


def test_a_cell_written_with_other_line_endings_is_sent_with_line_feeds(notebook):
    text = "class Mine(qf.Strategy):\r\n    def on_bar(self, ctx, bar):\r\n        return None\r\n"
    name = f"<{notebook.name}-crlf>"
    linecache.cache[name] = (len(text), None, text.splitlines(True), name)
    notebook.cells.append(name)
    exec(compile(text, name, "exec"), vars(notebook.module))
    assert file_of(notebook.module.Mine) == (
        f"{QF}\n\n\nclass Mine(qf.Strategy):\n    def on_bar(self, ctx, bar):\n        return None\n"
    )


def test_a_class_of_a_program_whose_main_module_has_no_file_is_found_as_a_notebooks_is(tmp_path):
    # A kernel's `__main__` has no file, and its classes say they are
    # `__main__`'s: so do those of a program handed to Python as text.
    program = textwrap.dedent(
        """
        import inspect, linecache, sys

        import quantforge_py as qf
        from quantforge_py import _source

        cell = "class Mine(qf.Strategy):\\n    def on_bar(self, ctx, bar):\\n        return None\\n"
        linecache.cache["<cell-1>"] = (len(cell), None, cell.splitlines(True), "<cell-1>")
        exec(compile(cell, "<cell-1>", "exec"))
        assert Mine.__module__ == "__main__" and not hasattr(sys.modules["__main__"], "__file__")
        try:
            inspect.getsource(Mine)
        except OSError as error:
            print("inspect:", error)
        (name, text), entry = _source._class_file(Mine, "import quantforge_py as qf", True)
        print(entry)
        print(text.decode(), end="")
        """
    )
    run = subprocess.run(
        [sys.executable, "-c", program], capture_output=True, text=True, timeout=120, cwd=tmp_path
    )
    assert run.returncode == 0, run.stderr
    assert run.stdout == (
        "inspect: source code not available\n"
        "strategy.py:Mine\n"
        f"{QF}\n\n\nclass Mine(qf.Strategy):\n    def on_bar(self, ctx, bar):\n        return None\n"
    )


def test_finding_a_class_runs_nothing_of_it_again(notebook):
    module = notebook.run(
        """
        defined = []


        class Mine(qf.Strategy):
            defined.append("the class's body ran")

            def on_bar(self, ctx, bar):
                defined.append("a bar")
                return None
        """
    )
    assert module.defined == ["the class's body ran"]
    with pytest.raises(ValueError, match="^Mine uses defined, which its source does not define"):
        _source._class_file(module.Mine, QF, True)
    assert module.defined == ["the class's body ran"]


@pytest.fixture
def probes():
    """A module, `probes`, whose `Probe` answers for every attribute it has
    not got with code of its own: it notes what it was asked for in `asked`,
    and raises."""
    module = types.ModuleType("probes")
    exec(
        textwrap.dedent(
            """
            asked = []


            class Probe:
                def __getattr__(self, name):
                    asked.append(name)
                    raise RuntimeError(f"a probe was asked for {name}")
            """
        ),
        vars(module),
    )
    sys.modules["probes"] = module
    yield module
    sys.modules.pop("probes", None)


def test_nothing_is_asked_of_what_a_class_holds_or_of_what_stands_beside_it(notebook, probes):
    # What a class's attributes hold is told by its type. An object whose
    # class answers for its attributes with code of its own was asked for
    # `__func__`, which ran that code and, where it raised, ended the
    # submission.
    declared = f"{QF}\nimport probes"
    module = notebook.run(
        """
        import probes

        gadget = probes.Probe()


        class Mine(qf.Strategy):
            probe = probes.Probe()

            def on_bar(self, ctx, bar):
                return None


        class Bare(qf.Strategy):
            probe = probes.Probe()
            limit = None


        class Leans(qf.Strategy):
            probe = probes.Probe()

            def on_bar(self, ctx, bar):
                return gadget
        """
    )
    text = file_of(module.Mine, declared)
    assert text.endswith(
        "    probe = probes.Probe()\n\n    def on_bar(self, ctx, bar):\n        return None\n"
    )
    assert probes.asked == []
    # A class that reads a name it does not define is told what to do about
    # it from what stands under the name where it was written: which is
    # asked nothing either, whatever it is.
    with pytest.raises(ValueError) as refused:
        _source._class_file(module.Leans, declared, True)
    assert str(refused.value).endswith("; define gadget inside the class")
    assert probes.asked == []

    # Nor is anything asked to say what a class defined inside a function
    # takes from it, or where a class without a function of its own was
    # written.
    def make(threshold):
        class Captures(qf.Strategy):
            probe = probes.Probe()

            def on_bar(self, ctx, bar):
                return None if bar.close > threshold else None

        return Captures

    with pytest.raises(ValueError, match="^Captures is defined inside the function make, and"):
        _source._class_file(make(3), declared, True)
    assert _source._written_in(module.Bare) is vars(module)
    assert probes.asked == []

    # What Python's own wrappers hold is still found, and nothing else is
    # taken for a function: not what merely answers to their names.
    def function(self):
        return None

    assert _source._own_function(function) is function
    for wrapped in (staticmethod(function), classmethod(function), property(function)):
        assert _source._own_function(wrapped) is function
    lookalike = types.SimpleNamespace(__func__=function, fget=function)
    for other in (lookalike, probes.Probe(), property(), functools.partial(function), len, None):
        assert _source._own_function(other) is None
    assert probes.asked == []


# What a class's source may read.


def test_every_name_a_class_reads_is_defined_by_its_imports_by_itself_or_by_python(notebook):
    module = notebook.run(
        """
        class Mine(qf.Strategy):
            '''Reads nothing that is not its own, an import's or a builtin.'''

            LEVELS = [1, 2, 3]
            WIDEST = max(LEVELS)
            label = __name__

            def __init__(self, fast: int = 2, slow: "int" = 5, *levels, **more):
                super().__init__()
                self.fast, self.slow = fast, slow
                self.levels = [level * 2 for level in levels if isinstance(level, int)]
                self.pick = lambda value, default=None: value or default
                self.more = dict(more)

            def average(self, values):
                import statistics as st

                total = 0

                def add(value):
                    nonlocal total
                    total += value
                    return total

                for value in values:
                    add(value)
                return st.mean(values) if values else total

            def on_bar(self, ctx, bar):
                try:
                    closes = [float(bar.close)]
                except (TypeError, ValueError) as refused:
                    raise RuntimeError(str(refused)) from None
                if self.average(closes) > Mine.WIDEST and __debug__:
                    return qf.TargetPosition.LONG_ALL_IN
                return None
        """
    )
    text = file_of(module.Mine)
    assert run_from(text, "Mine", fast=2).trade_count == 1


@pytest.mark.parametrize(
    "body, uses, advice",
    [
        (
            "limit = THRESHOLD",
            "THRESHOLD",
            "define THRESHOLD inside the class",
        ),
        (
            "def on_start(self, ctx):\n        self.x = np.zeros(THRESHOLD) + helper()",
            "THRESHOLD, helper and np",
            "add 'import statistics as np' to imports; define THRESHOLD and helper inside the class",
        ),
        (
            "def on_start(self, ctx):\n        self.x = Decimal(1) + D(2) + st.mean([1]) + sqrt(4)",
            "D, Decimal, sqrt and st",
            "add 'from decimal import Decimal as D', 'from decimal import Decimal', 'from math "
            "import sqrt' and 'import statistics as st' to imports",
        ),
        (
            "def on_start(self, ctx):\n        return never_defined + also_missing",
            "also_missing and never_defined",
            "also_missing and never_defined are defined nowhere the class was written",
        ),
        (
            "def on_start(self, ctx):\n        return [value for value in SERIES]",
            "SERIES",
            "SERIES is defined nowhere the class was written",
        ),
        (
            "def on_start(self, ctx):\n        global COUNTER\n        COUNTER = COUNTER + 1",
            "COUNTER",
            "COUNTER is defined nowhere the class was written",
        ),
        (
            "LIMIT = 3\n\n    def on_start(self, ctx):\n        return LIMIT",
            "LIMIT",
            "LIMIT is set in the class's body, where a function, a lambda or a comprehension "
            "inside the class does not look: there Python reads it outside the class",
        ),
        (
            "def on_start(self, ctx, depth=THRESHOLD):\n        return depth",
            "THRESHOLD",
            "define THRESHOLD inside the class",
        ),
        (
            "@decorated\n    def on_start(self, ctx):\n        return None",
            "decorated",
            "define decorated inside the class",
        ),
    ],
)
def test_a_name_a_class_reads_and_does_not_define_is_refused_with_what_to_do(
    notebook, body, uses, advice
):
    module = notebook.run(
        """
        import statistics as np
        import statistics as st
        from decimal import Decimal
        from decimal import Decimal as D
        from math import sqrt

        THRESHOLD = 3


        def helper():
            return 1


        def decorated(function):
            return function
        """
    )
    notebook.run(
        f"class Mine(qf.Strategy):\n    {body}\n\n    def on_bar(self, ctx, bar):\n        return None\n"
    )
    with pytest.raises(ValueError) as refused:
        _source._class_file(module.Mine, QF, True)
    assert str(refused.value) == (
        f"Mine uses {uses}, which its source does not define: submit_class sends the class's own "
        f"source below imports, and nothing else of where it was written; {advice}"
    )


# What the notebook of the two tests below defines beside the class: every
# name a body of theirs reads, so that each class is one Python makes here.
BESIDE = """
    LIMIT = 3
    SERIES = [1, 2]
    depth = 1
    count = 0
    limit = 1
    wrap = staticmethod
    GUARD = memoryview(b"")
    _Mine__limit = 7
"""


@pytest.mark.parametrize(
    "body, names",
    [
        ("LIMIT = LIMIT", ["LIMIT"]),
        ("depth = depth + 1", ["depth"]),
        ("count += 1", ["count"]),
        ("first, LIMIT = LIMIT, 1", ["LIMIT"]),
        ("def scaled(self, by=LIMIT):\n        return by\n\n    LIMIT = 4", ["LIMIT"]),
        ("pick = lambda value, default=LIMIT: value\n    LIMIT = 4", ["LIMIT"]),
        ("@wrap\n    def wrapped():\n        return 1\n\n    wrap = None", ["wrap"]),
        ("values = [value for value in SERIES]\n    SERIES = []", ["SERIES"]),
        ("if LIMIT:\n        pass\n    LIMIT = 4", ["LIMIT"]),
        ("with GUARD:\n        pass\n    GUARD = None", ["GUARD"]),
        ("class Inner:\n        depth = depth", ["depth"]),
        ("class Inner(SERIES.__class__):\n        pass\n\n    SERIES = []", ["SERIES"]),
        ("LIMIT = LIMIT\n    depth = depth", ["LIMIT", "depth"]),
        # A name that is annotated and given no value is the class's own,
        # and is never set.
        ("limit: int\n    double = limit", ["limit"]),
        ("double = limit\n    limit: int", ["limit"]),
        # What an item is added to is read before the value, which here sets
        # the name the index reads.
        ("items = [0, 0]\n    items[limit] += (limit := 1)", ["limit"]),
        # A name the class deletes is set no longer, and neither is the name
        # an exception was given, once its handler has ended.
        ("limit = 1\n    del limit\n    value = limit", ["limit"]),
        (
            "try:\n        raise ValueError\n    except ValueError as limit:\n        pass\n"
            "    value = limit",
            ["limit"],
        ),
        # A loop's test is read once for certain, before its body runs.
        ("while limit:\n        break\n    limit = 0", ["limit"]),
        # The name one clause of an `except*` was given is cleared before
        # the next clause runs.
        (
            'try:\n        raise ExceptionGroup("both", [ValueError(), TypeError()])\n'
            "    except* ValueError as limit:\n        pass\n    except* TypeError:\n"
            "        value = limit",
            ["limit"],
        ),
        (
            'try:\n        raise ExceptionGroup("both", [ValueError(), TypeError()])\n'
            "    except* ValueError as limit:\n        limit = 3\n    except* TypeError:\n"
            "        value = limit",
            ["limit"],
        ),
        # A private name is looked up under the class's own.
        ("__limit = __limit", ["_Mine__limit"]),
    ],
)
def test_a_name_a_class_reads_before_it_sets_it_is_read_outside_the_class(notebook, body, names):
    # A class's body looks a name up among those the class has set so far,
    # and then outside the class: `LIMIT = LIMIT` reads the notebook's
    # `LIMIT`, which does not travel, though the name is the class's own.
    module = notebook.run(BESIDE)
    statement = f"class Mine(qf.Strategy):\n    {body}\n\n    def on_bar(self, ctx, bar):\n        return None\n"
    notebook.run(statement)
    with pytest.raises(ValueError) as refused:
        _source._class_file(module.Mine, QF, True)
    listed = " and ".join(names)
    it = "it" if len(names) == 1 else "them"
    assert str(refused.value) == (
        f"Mine uses {listed}, which its source does not define: submit_class sends the class's "
        "own source below imports, and nothing else of where it was written; the class reads "
        f"{listed} before it sets {it}, where Python looks outside the class: set {it} from what "
        "the class itself defines"
    )
    # And it is so: the file such a class would be sent as does not run.
    with pytest.raises(NameError) as failed:
        exec(compile(f"{QF}\n\n\n{statement}", "strategy.py", "exec"), {"__name__": "strategy"})
    assert failed.value.name in names


@pytest.mark.parametrize(
    "body",
    [
        "LIMIT = 3\n    WIDEST = LIMIT * 2",
        "counter = 0\n    counter += 1",
        "a = b = 1\n    c = a + b",
        "first, *rest = [1, 2]\n    most = len(rest)",
        "limit: int\n    other = 1",
        "limit: int = 3\n    double = limit * 2",
        "max = max",
        "z = (y := 1) + y",
        "import math\n    root = math.sqrt",
        "from statistics import mean as average\n    middle = average",
        "def helper(value):\n        return value\n\n    made = helper(1)",
        "class Inner:\n        pass\n\n    inner = Inner",
        "squares = [n * n for n in range(3)]",
        "key = lambda item, default=None: item or default",
        # Set in whichever branch a statement written before the read took.
        "if qf:\n        LIMIT = 1\n    else:\n        LIMIT = 2\n    double = LIMIT * 2",
        "try:\n        import statistics as st\n    except ImportError:\n        st = None\n    mean = st",
        "try:\n        value = int('x')\n    except ValueError as refused:\n        value = str(refused)\n    shown = value",
        "with memoryview(b'x') as view:\n        first = view[0]\n    kept = first",
        "match 1:\n        case found:\n            pass\n    kept = found",
        # What a statement this reader does not know sets counts as set, also
        # where the class sets the same name again below.
        "match 1:\n        case found:\n            pass\n    kept = found\n    found = 2",
        "match [1, 2]:\n        case [first, *others]:\n            pass\n    kept = others\n    others = []",
        # Inside a loop a read may come after a statement written below it.
        "for index in range(2):\n        if index:\n            seen = total\n        total = index",
        "while False:\n        late = early\n        early = 1\n    early = 2",
        "for index in range(3):\n        pass\n    last = index",
        # A name of a function around a class is that function's.
        "def scale(self):\n        factor = 2\n\n        class Inner:\n            value = factor\n\n"
        "        return Inner.value",
        # A name a class says is the function's around it is that function's.
        "def scale(self):\n        total = 1\n\n        class Inner:\n            nonlocal total\n"
        "            total = total + 1\n\n        return total",
        # What annotates a name is read where its statement stands, by
        # which the class has set what it reads; and inside a function's
        # body no Python evaluates an annotation at all.
        "Kind = int\n    kind: Kind = 3",
        "kind: kind = int",
        # A key and then its value, pair by pair: the first value sets what
        # the second key reads.
        'pairs = {"first": (limit := 2), limit: None}',
        "items = [0, 0]\n    limit = 0\n    items[limit] += (limit := 1)",
        # A name is unset for certain only where every way there unsets it.
        "limit = 1\n    value = limit\n    del limit",
        "limit = 1\n    if not qf:\n        del limit\n    value = limit",
        "limit = 1\n    with memoryview(b''):\n        del limit\n        limit = 2\n    value = limit",
        "limit = 5\n    try:\n        pass\n    except ValueError as limit:\n        pass\n"
        "    value = limit",
        "try:\n        value = int('x')\n    except ValueError as limit:\n        seen = str(limit)\n"
        "    limit = 3\n    other = limit",
        "for index in ():\n        pass\n    else:\n        last = 0\n    kept = last",
        "limit = 1\n    for index in ():\n        del limit\n    value = limit",
        # A block that may be left before its end may have been left at any
        # statement of it: what it set anywhere may be set after it, though
        # it unsets it further down. The body of a `try`, for its handlers
        # and its `finally`; and the body of a loop that is broken out of.
        "try:\n        limit = 1\n        1 / 0\n        del limit\n    except ZeroDivisionError:\n"
        "        value = limit",
        "try:\n        limit = 1\n        1 / 0\n        del limit\n    except ZeroDivisionError:\n"
        "        pass\n    finally:\n        value = limit",
        "try:\n        try:\n            limit = 1\n            1 / 0\n            del limit\n"
        "        finally:\n            value = limit\n    except ZeroDivisionError:\n        pass",
        "for index in (0, 1):\n        limit = 1\n        if index:\n            break\n"
        "        del limit\n    value = limit",
        # Of the clauses of an `except*`, every one that matches a part of
        # what was raised runs, one after the other.
        'try:\n        raise ExceptionGroup("both", [ValueError(), TypeError()])\n'
        "    except* ValueError:\n        limit = 1\n    except* TypeError:\n        value = limit",
        # A loop's `else` is passed over where the loop is broken out of.
        "limit = 1\n    for index in (0,):\n        break\n    else:\n        del limit\n"
        "    value = limit",
        "__limit = 3\n    double = __limit * 2",
        # A name with two underscores at either end is no private name.
        '__doc__ = __doc__ or "kept"',
        "Kind = int\n\n    def on_start(self, ctx: Kind) -> Kind:\n        return None",
        "if qf:\n        limit: int = 3",
        "try:\n        limit: int = 3\n    except Exception:\n        limit: int = 4",
        "def on_start(self, ctx):\n        level: NeverDefined = 1\n        self.level = level",
    ],
)
def test_a_name_a_class_sets_before_it_reads_it_is_its_own(notebook, body):
    module = notebook.run(BESIDE)
    notebook.run(
        f"class Mine(qf.Strategy):\n    {body}\n\n    def on_bar(self, ctx, bar):\n        return None\n"
    )
    text = file_of(module.Mine)
    # And it is so: the file the class is sent as runs, where nothing but
    # its imports is defined.
    names = {"__name__": "strategy"}
    exec(compile(text, "strategy.py", "exec"), names)
    assert issubclass(names["Mine"], qf.Strategy)


@pytest.mark.parametrize(
    "body",
    [
        "LIMIT = 4\n    doubled = [LIMIT * value for value in range(3)]",
        "LIMIT = 4\n    doubled = {LIMIT * value for value in range(3)}",
        "LIMIT = 4\n    doubled = {value: LIMIT for value in range(3)}",
        "LIMIT = 4\n    doubled = list(LIMIT * value for value in range(3))",
        "LIMIT = 4\n    doubled = [value for value in range(3) if value < LIMIT]",
        "LIMIT = 4\n    doubled = (lambda value: value * LIMIT)(2)",
        "LIMIT = 4\n\n    def on_start(self, ctx):\n        self.limit = LIMIT",
    ],
)
def test_a_name_a_class_sets_is_not_seen_inside_a_function_or_a_comprehension_of_its_body(
    notebook, body
):
    # A function, a lambda and a comprehension inside a class's body look a
    # name up in themselves and then outside the class, never in it. Python
    # from 3.12 on no longer shows a comprehension as a scope of its own in
    # its table of symbols, where such a read then passed.
    module = notebook.run(BESIDE)
    statement = f"class Mine(qf.Strategy):\n    {body}\n\n    def on_bar(self, ctx, bar):\n        return None\n"
    notebook.run(statement)
    with pytest.raises(ValueError) as refused:
        _source._class_file(module.Mine, QF, True)
    assert str(refused.value) == (
        "Mine uses LIMIT, which its source does not define: submit_class sends the class's own "
        "source below imports, and nothing else of where it was written; LIMIT is set in the "
        "class's body, where a function, a lambda or a comprehension inside the class does not "
        "look: there Python reads it outside the class"
    )
    # And it is so, on every Python: the file such a class would be sent as
    # fails as the class is made, or as its function is called.
    names = {"__name__": "strategy"}
    with pytest.raises(NameError) as failed:
        exec(compile(f"{QF}\n\n\n{statement}", "strategy.py", "exec"), names)
        names["Mine"]().on_start(None)
    assert failed.value.name == "LIMIT"


@pytest.mark.parametrize(
    "body",
    [
        "kind: Kind = 3\n    Kind = int",
        "def on_start(self, ctx: Kind) -> None:\n        return None\n\n    Kind = int",
        "def on_start(self, ctx) -> Kind:\n        return None\n\n    Kind = int",
    ],
)
def test_what_annotates_a_name_is_read_where_its_statement_stands_on_every_python(notebook, body):
    # Up to Python 3.13 an annotation is evaluated as its statement runs;
    # later Pythons evaluate it when it is asked for, in a scope of its own.
    # A bundle runs on its server's Python, whichever reads the class here:
    # the answer is the earlier Pythons', on every one.
    module = notebook.run("Kind = int")
    statement = f"class Mine(qf.Strategy):\n    {body}\n\n    def on_bar(self, ctx, bar):\n        return None\n"
    notebook.run(statement)
    with pytest.raises(ValueError) as refused:
        _source._class_file(module.Mine, QF, True)
    assert str(refused.value).endswith(
        "; the class reads Kind before it sets it, where Python looks outside the class: set it "
        "from what the class itself defines"
    )
    names = {"__name__": "strategy"}
    program = compile(f"{QF}\n\n\n{statement}", "strategy.py", "exec")
    if sys.version_info >= (3, 14):
        exec(program, names)
    else:
        with pytest.raises(NameError):
            exec(program, names)


def test_the_names_a_text_reads_are_the_same_on_whichever_python_reads_it():
    # What the table of symbols of one Python says and another's does not:
    # a name it makes up for annotations inside a block, a name that only
    # annotates a variable of a function, and a name that is deleted.
    def said(body):
        return _names._undefined(f"{QF}\n\n\nclass Mine(qf.Strategy):\n    {body}\n")

    nothing = ([], set(), set())
    assert said("if qf:\n        limit: int = 3\n    else:\n        limit: float = 4.0") == nothing
    assert said("def on_start(self, ctx):\n        level: NeverDefined = 1") == nothing
    assert said("Kind = int\n    kind: Kind = 3\n    other: Kind") == nothing
    assert said("kind: NeverDefined = 3") == (["NeverDefined"], set(), set())
    assert said("del gone") == (["gone"], {"gone"}, set())
    assert said("LIMIT = 4\n    doubled = [LIMIT for _ in range(3)]") == (
        ["LIMIT"],
        set(),
        {"LIMIT"},
    )
    assert said("LIMIT = 4\n    doubled = [value for value in range(LIMIT)]") == nothing
    # A name a function annotates and gives no value is that function's,
    # and no name the module is asked for.
    assert said("def on_start(self, ctx):\n        level: int\n        return level") == nothing
    # And a name a class inside the class sets is not the class's own.
    inner = "class Inner:\n        depth = 1\n\n    def on_start(self, ctx):\n        return depth"
    assert said(inner) == (["depth"], set(), set())
    # A private name is the class's under the class's own name, in its body
    # as in its functions; an import supplies it under that name and no other.
    private = "__limit = 3\n\n    def on_start(self, ctx):\n        return __limit"
    assert said(private) == (["_Mine__limit"], set(), {"_Mine__limit"})

    def under(imports):
        return _names._undefined(
            f"{imports}\n\n\nclass Mine(qf.Strategy):\n    __limit = __limit\n"
        )

    assert under(f"{QF}\nfrom math import pi as _Mine__limit") == nothing
    hidden = f"{QF}\n\n\nclass __Hidden(qf.Strategy):\n    __limit = __limit\n"
    assert _names._undefined(hidden)[0] == ["_Hidden__limit"]
    # A name the class says is the module's is the module's, and no name of
    # the class's read too soon.
    assert said("global counter\n    counter = counter + 1") == (["counter"], set(), set())
    assert under(f"{QF}\nfrom math import pi as __limit") == (
        ["_Mine__limit"],
        {"_Mine__limit"},
        set(),
    )


def test_the_builtins_a_class_may_read_are_those_every_python_has_on_every_system():
    # A bundle runs on its server's Python, on its server's system: a name
    # that only a later Python has for a builtin, or only Windows, is no
    # name every server has, on whichever Python and system the class is
    # read.
    def said(body, imports=QF):
        return _names._undefined(f"{imports}\n\n\nclass Mine(qf.Strategy):\n    {body}\n")[0]

    method = "def on_start(self, ctx):\n        return {}"
    assert said(method.format("PythonFinalizationError")) == ["PythonFinalizationError"]
    assert said(method.format("WindowsError")) == ["WindowsError"]
    assert said(method.format("ExceptionGroup, aiter, sorted, __debug__")) == []
    # A declared import supplies such a name, as any other.
    declared = f"{QF}\nfrom builtins import PythonFinalizationError"
    assert said(method.format("PythonFinalizationError"), declared) == []
    # The names are Python 3.11's, the oldest the package runs on, and every
    # later Python still has each of them. They are asked of a Python just
    # started: one that has run a session keeps the session's last value
    # among its builtins, as `_`.
    listing = "import builtins; print(' '.join(dir(builtins)))"
    fresh = subprocess.run([sys.executable, "-c", listing], capture_output=True, text=True)
    assert fresh.returncode == 0, fresh.stderr
    here = set(fresh.stdout.split())
    assert _names._BUILTINS <= here <= set(dir(builtins))
    if sys.version_info[:2] == (3, 11):
        assert here - {"WindowsError"} == _names._BUILTINS
    assert len(_names._BUILTINS) == 157


def test_a_read_that_only_some_way_through_a_body_leaves_unset_is_taken(notebook):
    # Only what is certain is refused. Whether a loop runs, and which way a
    # branch goes, the text does not say: a class whose read some way
    # through its body satisfies is taken, and one that is left unset along
    # another fails where it runs, with its traceback in the job's log. To
    # refuse these would be to refuse the same class where its loop runs.
    module = notebook.run(BESIDE)
    for body, runs in [
        ("for limit in range(1):\n        pass\n    value = limit", True),
        ("for limit in range(0):\n        pass\n    value = limit", False),
        ("if qf:\n        limit = 2\n    value = limit", True),
        ("if not qf:\n        limit = 2\n    value = limit", False),
    ]:
        notebook.run(
            f"class Mine(qf.Strategy):\n    {body}\n\n    def on_bar(self, ctx, bar):\n        return None\n"
        )
        text = file_of(module.Mine)
        names = {"__name__": "strategy"}
        if runs:
            exec(compile(text, "strategy.py", "exec"), names)
        else:
            with pytest.raises(NameError):
                exec(compile(text, "strategy.py", "exec"), names)


def test_what_a_block_may_be_ended_before_is_not_unset_for_certain(notebook):
    # What manages a `with` block may end it at any statement and go on, as
    # `contextlib.suppress` does: a name deleted further down the block is
    # not unset for certain after it.
    declared = f"{QF}\nimport contextlib"
    module = notebook.run(
        """
        import contextlib


        class Mine(qf.Strategy):
            limit = 1
            with contextlib.suppress(ZeroDivisionError):
                1 / 0
                del limit
            value = limit

            def on_bar(self, ctx, bar):
                return None


        class Inside(qf.Strategy):
            with contextlib.suppress(ZeroDivisionError):
                limit = 2
                1 / 0
                del limit
            value = limit

            def on_bar(self, ctx, bar):
                return None
        """
    )
    text = file_of(module.Mine, declared)
    names = {"__name__": "strategy"}
    exec(compile(text, "strategy.py", "exec"), names)
    assert names["Mine"].value == 1
    # And what the block set before it was ended is set after it, though a
    # statement further down the block unsets it.
    text = file_of(module.Inside, declared)
    exec(compile(text, "strategy.py", "exec"), names)
    assert names["Inside"].value == 2


@pytest.mark.skipif(sys.version_info < (3, 12), reason="a function is written so from 3.12 on")
def test_a_generic_functions_type_parameters_are_names_where_its_annotations_are_read(notebook):
    # A function's type parameters are names in its annotations and in its
    # body, and nowhere in the class's: where the annotations are read as
    # the class's body would read them, they are no names to look for.
    module = notebook.run(
        """
        class Mine(qf.Strategy):
            Kind = int

            def identity[T](self, value: T, kind: Kind = 0) -> T:
                held: T = value
                return held

            def first[*Ts, **P](self, *values: *Ts) -> tuple[*Ts]:
                return values

            # What an annotation sets under a type parameter's name is the
            # annotation's own.
            def third[T](self, value: [T for T in (int,)] = None):
                return value

            # A name the class sets afterwards is another name than the
            # function's type parameter of the same spelling.
            T = 3

            def on_bar(self, ctx, bar):
                return None
        """
    )
    text = file_of(module.Mine)
    names = {"__name__": "strategy"}
    exec(compile(text, "strategy.py", "exec"), names)
    assert names["Mine"]().identity(3) == 3
    # A name such a signature reads and nothing defines is refused as in
    # any other, and so is a type parameter's name read outside its function.
    notebook.run(
        """
        Missing = int


        class Other(qf.Strategy):
            def identity[T](self, value: T) -> Missing:
                return value

            def on_bar(self, ctx, bar):
                return T
        """
    )
    with pytest.raises(ValueError, match="^Other uses Missing and T, which its source does not"):
        _source._class_file(module.Other, QF, True)


def bodies(seed: int, count: int):
    """Class bodies put together from a seed: statements that each run once,
    in the order written, over three names of the class's own; with each,
    whether an annotation in it reads one of them."""
    rng = random.Random(seed)
    names = ["a", "b", "c"]

    def simple(index):
        n, m, k = rng.choice(names), rng.choice(names), rng.choice(names)
        return rng.choice(
            [
                (f"{n} = []", False),
                (f"{n} = [{m}]", False),
                (f"{n} = ({m}, {k})", False),
                (f"{n}, {m} = [], []", False),
                (f"{n} = [x for x in {m}]", False),
                (f"{n} = [{m} for _ in [0]]", False),
                (f"{n} = (lambda value={m}: value)()", False),
                (f"{n} = ({m} := [])", False),
                (f"{n} = [{m}, ({k} := [])]", False),
                (f"{n} = [({k} := []), {m}]", False),
                (f"{n}: list = {m}", False),
                (f"{n}: list", False),
                (f"{n}: {m} = []", True),
                (f"{n}: {m}", True),
                (f"def h_{index}(self, value: {m}) -> {k}:\n    return value", True),
                (f"def f_{index}(self, value={m}):\n    return value", False),
                (
                    f"@(lambda function, by={m}: function)\ndef g_{index}(self):\n    return None",
                    False,
                ),
                (f"class C_{index}:\n    inner = {m}", False),
                (f"class D_{index}:\n    {n} = {n}", False),
                (f"class E_{index}(([{m}], object)[1]):\n    pass", False),
                (f"from string import digits as {n}", False),
                (f"assert [{m}] is not None", False),
                (f"[{m}]", False),
                (f"{n} *= 1", False),
                (f"[[]][0][len({m}):] += ({m} := [])", False),
                (f"{n} = [{{0: ({m} := ()), len({m}): 1}}]", False),
                (f"{n} = [{{len({m}): 0, 0: ({m} := ())}}]", False),
            ]
        )

    def statement(index, depth=0):
        if depth == 0 and rng.random() < 0.08:
            # Unset for certain only where nothing stands around it.
            return f"del {rng.choice(names)}", False
        if depth > 1 or rng.random() < 0.6:
            inner, annotated = simple(index)
        else:
            inner, annotated = statement(index, depth + 1)
        block = textwrap.indent(inner, "    ")
        wrapper = rng.choice(["plain", "plain", "if", "try", "with", "handler", "as"])
        if wrapper == "if":
            inner = f"if True:\n{block}"
        elif wrapper == "try":
            inner = f"try:\n{block}\nfinally:\n    pass"
        elif wrapper == "with":
            inner = f"with memoryview(b''):\n{block}"
        elif wrapper == "handler":
            inner = f"try:\n    raise KeyError\nexcept KeyError:\n{block}"
        elif wrapper == "as":
            inner = f"with contextlib.nullcontext([]) as {rng.choice(names)}:\n{block}"
        return inner, annotated

    for _ in range(count):
        made = [statement(index) for index in range(rng.randint(1, 6))]
        body = textwrap.indent("\n".join(inner for inner, _ in made), "    ")
        yield (
            f"import contextlib\n\n{QF}\n\n\nclass Mine(qf.Strategy):\n{body}\n\n"
            "    def on_bar(self, ctx, bar):\n        return None\n",
            any(annotated for _, annotated in made),
        )


# What the reader says of `bodies(BODIES_SEED, BODIES)`, every answer in
# order, as one hash: the same on every Python that reads them.
BODIES_SEED, BODIES = 0x5EED_0B0D, 4000
BODIES_SAID = "c729e89d0b2a9765b8862848e48d09c12762b9fc1d5af9e6f5f554868db8269a"


def test_what_a_class_body_reads_outside_itself_is_what_python_finds_missing_as_it_runs():
    # The reader is held to Python itself. Each body's statements run once,
    # in the order written: so the file runs exactly when the reader finds
    # no name to refuse, and where it does not run, the name Python misses
    # is one the reader named.
    said = hashlib.sha256()
    ran = 0
    for text, annotated in bodies(BODIES_SEED, BODIES):
        unknown, early, within = _names._undefined(text)
        said.update(repr((unknown, sorted(early), sorted(within))).encode())
        if annotated and sys.version_info >= (3, 14):
            # A later Python evaluates an annotation when it is asked for:
            # the answer for such a body is the earlier Pythons', which the
            # hash below holds this one to.
            continue
        try:
            exec(compile(text, "strategy.py", "exec"), {"__name__": "strategy"})
        except NameError as missing:
            assert missing.name in unknown, text
        else:
            ran += 1
            assert unknown == [], text
    assert 200 < ran < BODIES - 200
    # And what it says is the same on whichever Python reads the bodies.
    assert said.hexdigest() == BODIES_SAID


def test_what_a_class_reads_too_soon_is_told_beside_what_it_does_not_define_at_all(notebook):
    module = notebook.run(
        """
        LIMIT = 3
        THRESHOLD = 4


        class Mine(qf.Strategy):
            floor = THRESHOLD
            LIMIT = LIMIT

            def on_bar(self, ctx, bar):
                return None
        """
    )
    with pytest.raises(ValueError) as refused:
        _source._class_file(module.Mine, QF, True)
    assert str(refused.value).endswith(
        "; define THRESHOLD inside the class; the class reads LIMIT before it sets it, where "
        "Python looks outside the class: set it from what the class itself defines"
    )


def test_a_name_an_annotation_reads_is_a_name_the_source_reads(notebook):
    # Up to Python 3.13 an annotation of a class and of a function's
    # signature is evaluated as its statement runs, and fails there where
    # its name is not defined; later Pythons evaluate it when it is asked
    # for. A bundle runs on its server's Python, whichever this one is: the
    # name is asked for on every one.
    module = notebook.run(
        """
        from decimal import Decimal

        Context = Target = object


        class Mine(qf.Strategy):
            limit: Decimal = 3

            def on_bar(self, ctx: Context, bar: "Quoted") -> Target:
                return None
        """
    )
    with pytest.raises(ValueError) as refused:
        _source._class_file(module.Mine, QF, True)
    assert str(refused.value) == (
        "Mine uses Context, Decimal and Target, which its source does not define: submit_class "
        "sends the class's own source below imports, and nothing else of where it was written; "
        "add 'from decimal import Decimal' to imports; define Context and Target inside the class"
    )
    # One written as text is text, which nothing reads.
    assert "Quoted" not in str(refused.value)


def test_what_a_class_is_derived_from_and_decorated_with_is_read_as_any_name(notebook):
    module = notebook.run(
        """
        class Base(qf.Strategy):
            def on_bar(self, ctx, bar):
                return None


        class Mine(Base):
            def on_start(self, ctx):
                self.started = True


        class Bare(qf.Strategy):
            def on_bar(self, ctx, bar):
                return None
        """
    )
    with pytest.raises(ValueError) as refused:
        _source._class_file(module.Mine, QF, True)
    assert str(refused.value).startswith("Mine uses Base, which its source does not define: ")
    assert str(refused.value).endswith("; define Base inside the class")
    # The package itself is declared as anything else is: without an
    # import, the name the class derives from is no name at all.
    with pytest.raises(ValueError) as refused:
        _source._class_file(module.Bare, "", True)
    assert str(refused.value) == (
        "Bare uses qf, which its source, which has no imports, does not define: submit_class "
        "sends the class's own source below imports, and nothing else of where it was written; "
        "add 'import quantforge_py as qf' to imports"
    )
    for imports in ("import quantforge_py", "from quantforge_py import Strategy"):
        with pytest.raises(ValueError, match="^Bare uses qf, which its source does not define"):
            _source._class_file(module.Bare, imports, True)


def test_the_imports_are_written_as_given_above_the_class(notebook):
    module = notebook.run(
        """
        import statistics as st
        from decimal import Decimal


        class Mine(qf.Strategy):
            def on_bar(self, ctx, bar):
                return None if st.mean([Decimal(1)]) else None
        """
    )
    imports = "\n\n  \nimport quantforge_py as qf\r\nimport statistics as st  # the mean\nfrom decimal import (\n    Decimal,\n)\n\n"
    text = file_of(module.Mine, imports)
    assert text.startswith(
        "import quantforge_py as qf\nimport statistics as st  # the mean\nfrom decimal import (\n"
        "    Decimal,\n)\n\n\nclass Mine(qf.Strategy):\n"
    )
    # A future statement is an import, where it comes first.
    future = "from __future__ import annotations\nimport quantforge_py as qf\nimport statistics as st\nfrom decimal import Decimal"
    assert file_of(module.Mine, future).startswith("from __future__ import annotations\n")
    with pytest.raises(ValueError) as refused:
        _source._class_file(
            module.Mine,
            future.replace("\n", "\n" * 1).split("\n", 1)[1]
            + "\nfrom __future__ import annotations",
            True,
        )
    assert str(refused.value).startswith("the source of Mine does not compile below its imports: ")
    assert str(refused.value).endswith(", at line 4 of the file submit_class writes")


@pytest.mark.parametrize(
    "imports, kind, said",
    [
        (None, TypeError, "imports must be a str of import statements, not NoneType"),
        (
            ["import quantforge_py as qf"],
            TypeError,
            "imports must be a str of import statements, not list",
        ),
        (
            b"import quantforge_py as qf",
            TypeError,
            "imports must be a str of import statements, not bytes",
        ),
        (
            "import quantforge_py as qf\nTHRESHOLD = 3",
            ValueError,
            "line 2 of imports is no import statement that names what it imports: imports holds "
            "import statements alone, none of them relative and none with a *",
        ),
        (
            "from math import *",
            ValueError,
            "line 1 of imports is no import statement that names what it imports: imports holds "
            "import statements alone, none of them relative and none with a *",
        ),
        (
            "import quantforge_py as qf\nfrom . import helpers",
            ValueError,
            "line 2 of imports is no import statement that names what it imports: imports holds "
            "import statements alone, none of them relative and none with a *",
        ),
        (
            "import quantforge_py as qf\nprint('run on the server')",
            ValueError,
            "line 2 of imports is no import statement that names what it imports: imports holds "
            "import statements alone, none of them relative and none with a *",
        ),
        (
            "import quantforge_py as",
            ValueError,
            "imports is not Python: invalid syntax, at line 1; write import statements, one a "
            "line, such as 'import quantforge_py as qf'",
        ),
        (
            "import quantforge_py as qf\x00",
            ValueError,
            "imports is not Python: source code string cannot contain null bytes; write import "
            "statements, one a line, such as 'import quantforge_py as qf'",
        ),
    ],
)
def test_imports_are_import_statements_and_nothing_else(notebook, imports, kind, said, refused):
    module = notebook.run(
        """
        class Mine(qf.Strategy):
            def on_bar(self, ctx, bar):
                return None
        """
    )
    refused(kind, said, _source._class_file, module.Mine, imports, True)


# What is no class to send.


def test_a_class_is_sent_only_where_its_caller_says_that_its_source_is_all_that_is_sent(
    notebook, refused
):
    module = notebook.run(
        """
        class Mine(qf.Strategy):
            def on_bar(self, ctx, bar):
                return None
        """
    )
    refused(ValueError, CAVEATS, _source._class_file, module.Mine, QF, False)
    for said in (None, 1, "yes", "True"):
        with pytest.raises(TypeError) as raised:
            _source._class_file(module.Mine, QF, said)
        assert str(raised.value) == f"source_only must be a bool, not {type(said).__name__}"
    # It is asked before anything else is looked at: what is no class at all
    # is told the same.
    refused(ValueError, CAVEATS, _source._class_file, lambda: None, 7, False)
    assert _source._class_file(module.Mine, QF, True)[1] == "strategy.py:Mine"


def test_what_is_no_class_derived_from_strategy_is_refused(notebook, refused):
    module = notebook.run(
        """
        class Mine(qf.Strategy):
            def on_bar(self, ctx, bar):
                return None


        class Plain:
            def on_bar(self, ctx, bar):
                return None


        def function(ctx, bar):
            return None
        """
    )
    for given, kind in [
        (lambda ctx, bar: None, "function"),
        (module.function, "function"),
        ("./strategy.py", "str"),
        (None, "NoneType"),
        (module, "module"),
        (functools.partial(module.Mine), "partial"),
    ]:
        refused(
            TypeError,
            f"strategy must be a class derived from quantforge_py.Strategy, not {kind}",
            _source._class_file,
            given,
            QF,
            True,
        )
    refused(
        TypeError,
        "strategy must be a class derived from quantforge_py.Strategy, not an object of one: "
        "pass Mine itself, and what it is built from as params",
        _source._class_file,
        module.Mine(),
        QF,
        True,
    )
    for given, name in [(module.Plain, "Plain"), (qf.Strategy, "Strategy"), (int, "int")]:
        refused(
            TypeError,
            f"{name} is not a strategy: submit_class sends a class derived from "
            "quantforge_py.Strategy",
            _source._class_file,
            given,
            QF,
            True,
        )


def test_a_class_that_captures_the_variables_of_a_function_is_refused_with_their_names(
    notebook, refused
):
    module = notebook.run(
        """
        def make(threshold, window=5):
            scale = 2

            class Captures(qf.Strategy):
                def on_start(self, ctx):
                    self.limit = threshold * scale

                def on_bar(self, ctx, bar):
                    return None if window else super().on_bar(ctx, bar)

            return Captures


        def plain():
            class Inner(qf.Strategy):
                def on_bar(self, ctx, bar):
                    return None

            return Inner


        class Outer:
            class Nested(qf.Strategy):
                def on_bar(self, ctx, bar):
                    return None


        Captures, Inner = make(3), plain()
        """
    )
    refused(
        ValueError,
        "Captures is defined inside the function make, and uses its scale, threshold and window: "
        "a class is sent as its own source, and what it takes from around it stays behind. Write "
        "it at the top level of a module or of a notebook's cell, with what it needs inside it",
        _source._class_file,
        module.Captures,
        QF,
        True,
    )
    refused(
        ValueError,
        "Inner is defined inside the function plain: a class is sent as its own source, and what "
        "it takes from around it stays behind. Write it at the top level of a module or of a "
        "notebook's cell, with what it needs inside it",
        _source._class_file,
        module.Inner,
        QF,
        True,
    )
    refused(
        ValueError,
        "Nested is defined inside the class Outer: submit_class sends a class written at the top "
        "level of a module or of a notebook's cell",
        _source._class_file,
        module.Outer.Nested,
        QF,
        True,
    )


def test_a_class_python_keeps_no_source_of_is_refused(notebook, refused):
    no_source = (
        "the source of {} cannot be read: Python keeps none of it, as of a class made by calling "
        "type() or typed at a prompt that keeps no source. submit_class sends a class written in "
        "a file or in a notebook's cell; a strategy kept in files is submitted with submit()"
    )
    module = notebook.run(
        """
        Made = type("Made", (qf.Strategy,), {"on_bar": lambda self, ctx, bar: None})


        def on_bar(self, ctx, bar):
            return None


        Assembled = type("Assembled", (qf.Strategy,), {"on_bar": on_bar})
        """
    )
    for name in ("Made", "Assembled"):
        refused(
            ValueError, no_source.format(name), _source._class_file, getattr(module, name), QF, True
        )
    # A class run from text Python keeps none of, as one typed at a prompt.
    names = {"qf": qf, "__name__": notebook.name}
    exec("class Typed(qf.Strategy):\n    def on_bar(self, ctx, bar):\n        return None\n", names)
    refused(ValueError, no_source.format("Typed"), _source._class_file, names["Typed"], QF, True)
    # And one whose only functions are another class's.
    notebook.run(
        """
        class Mine(qf.Strategy):
            def on_bar(self, ctx, bar):
                return None


        class Borrowed(qf.Strategy):
            on_bar = Mine.on_bar
        """
    )
    refused(
        ValueError, no_source.format("Borrowed"), _source._class_file, module.Borrowed, QF, True
    )


def test_a_class_written_inside_another_statement_is_refused(module_of, notebook, refused):
    inside = (
        "the source of Mine is not one class statement at the top level of its module or cell: "
        "submit_class sends a class written there, and not one written inside another statement"
    )
    text = """
        import quantforge_py as qf

        if True:

            class Mine(qf.Strategy):
                def on_bar(self, ctx, bar):
                    return None
        """
    refused(ValueError, inside, _source._class_file, module_of(text).Mine, QF, True)
    # In a notebook's cell too, where it is looked for among the cell's own
    # statements.
    cell = notebook.run(text.replace("import quantforge_py as qf", "")).Mine
    with pytest.raises(ValueError, match="^the source of Mine "):
        _source._class_file(cell, QF, True)
