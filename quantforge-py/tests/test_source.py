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

import functools
import importlib.util
import itertools
import linecache
import subprocess
import sys
import textwrap
import types

import pytest

import jobs
import quantforge_py as qf
from quantforge_py import _source

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
            "LIMIT is defined nowhere the class was written",
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
