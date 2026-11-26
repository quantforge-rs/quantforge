"""A class as `submit_class` sends one (`docs/python-bundles.md`): its own
source text, below the imports its caller declares, as the one Python file
of a bundle.

A class travels as the text of its `class` statement and as nothing else.
No object is serialized, and nothing is pickled: what a server runs is that
text, read again by its Python, and what the text needs has to be in it.

- **The source is the statement that defined the class,** found where
  Python keeps it: in the file, or the notebook's cell, that the class's
  own functions were compiled from. A class no source is kept of, one made
  by calling `type` among them, is refused.
- **A class is written at the top level** of its module or cell. One
  defined inside a function or inside another class is refused: what it
  takes from around it does not travel.
- **Its imports are declared, never carried over.** `imports` is import
  statements, written above the class in the file. The imports, the
  globals and the variables of where the class was written stay behind.
- **Every name the text reads has to be defined in it:** by `imports`, by
  the class itself, or as one of Python's builtins. Any other name is a
  refusal that says which, before anything is sent. The check reads the
  text and runs nothing: a name looked up some other way, by `globals()`
  or by `eval`, is not seen by it.

Importing this module imports nothing but the package's own: `inspect`,
`ast` and `symtable` are imported when a class is first submitted.

This module is private.
"""

from ._native import Strategy
from ._wire import _wrong_type

# The file a submitted class is written to, at its bundle's root, and the
# imports written above a class whose caller declares none.
_FILE = "strategy.py"
_IMPORTS = "import quantforge_py as qf"

# The names every module has without defining them.
_MODULE_NAMES = frozenset(
    {"__name__", "__doc__", "__file__", "__package__", "__spec__", "__loader__", "__builtins__"}
)

# What `submit_class` says where its caller has not said that it knows.
_CAVEATS = (
    "submit_class sends the source text of the class as the one file of a bundle, and nothing "
    "else of where the class was written: no import, no global and no variable around it "
    "travels unless imports declares it, and the server runs that text, not the class as it is "
    "in memory here. Pass source_only=True to say that this is what is meant; a strategy kept "
    "in files is submitted with submit()"
)


def _listed(names):
    """Names as a sentence lists them: `a`, `a and b`, `a, b and c`."""
    names = list(names)
    if len(names) < 3:
        return " and ".join(names)
    return f"{', '.join(names[:-1])} and {names[-1]}"


def _acknowledged(source_only):
    """Refuses a submission whose caller has not said that only the class's
    source is sent."""
    if type(source_only) is not bool:
        raise _wrong_type("source_only", "a bool", source_only)
    if not source_only:
        raise ValueError(_CAVEATS)


def _a_strategy_class(strategy):
    """Refuses what is no class derived from the package's `Strategy`, and
    a class that is not written at the top level of its module or cell."""
    if not isinstance(strategy, type):
        if isinstance(strategy, Strategy):
            raise TypeError(
                f"strategy must be a class derived from quantforge_py.Strategy, not an object of "
                f"one: pass {type(strategy).__name__} itself, and what it is built from as params"
            )
        raise _wrong_type("strategy", "a class derived from quantforge_py.Strategy", strategy)
    name = strategy.__name__
    if not issubclass(strategy, Strategy) or strategy is Strategy:
        raise TypeError(
            f"{name} is not a strategy: submit_class sends a class derived from "
            "quantforge_py.Strategy"
        )
    outer, _, inner = strategy.__qualname__.rpartition(".")
    if not outer:
        return
    if outer.endswith("<locals>"):
        function = outer.removesuffix(".<locals>").rpartition(".")[2]
        taken = sorted(
            {
                free
                for value in vars(strategy).values()
                for free in getattr(getattr(value, "__code__", None), "co_freevars", ())
                if free != "__class__"
            }
        )
        uses = f", and uses its {_listed(taken)}" if taken else ""
        raise ValueError(
            f"{inner} is defined inside the function {function}{uses}: a class is sent as its "
            "own source, and what it takes from around it stays behind. Write it at the top "
            "level of a module or of a notebook's cell, with what it needs inside it"
        )
    raise ValueError(
        f"{inner} is defined inside the class {outer}: submit_class sends a class written at "
        "the top level of a module or of a notebook's cell"
    )


def _from_a_method(strategy):
    """The statement that defined a class, read from where one of the
    class's own functions was compiled: a file, or a notebook's cell, which
    Python keeps the text of. `None` where it keeps none, or the class
    defines no function of its own."""
    import ast
    import linecache
    import types

    for value in vars(strategy).values():
        function = getattr(value, "__func__", None) or getattr(value, "fget", None) or value
        if not isinstance(function, types.FunctionType):
            continue
        if function.__qualname__ != f"{strategy.__qualname__}.{function.__name__}":
            continue
        code = function.__code__
        # A file's text as it is now, as `inspect` reads one; a cell's is
        # kept as it was run.
        linecache.checkcache(code.co_filename)
        lines = linecache.getlines(code.co_filename)
        try:
            tree = ast.parse("".join(lines))
        except (SyntaxError, ValueError):
            continue
        for node in tree.body:
            if not isinstance(node, ast.ClassDef) or node.name != strategy.__name__:
                continue
            first = min([node.lineno, *(each.lineno for each in node.decorator_list)])
            if first <= code.co_firstlineno <= node.end_lineno:
                return "".join(lines[first - 1 : node.end_lineno])
    return None


def _statement(strategy):
    """The `class` statement that defined a class, as text that ends with a
    line feed: one statement, at the top level, of that class's name."""
    import ast
    import inspect

    name = strategy.__name__
    # Where one of the class's own functions was compiled from says exactly
    # which statement made it, in a file as in a notebook's cell, whose
    # module has no file for `inspect` to read. A class that defines no
    # function is left to `inspect`, which reads its module's file.
    source = _from_a_method(strategy)
    if source is None:
        try:
            source = inspect.getsource(strategy)
        except (OSError, TypeError):
            source = None
    if source is None:
        raise ValueError(
            f"the source of {name} cannot be read: Python keeps none of it, as of a class made "
            "by calling type() or typed at a prompt that keeps no source. submit_class sends a "
            "class written in a file or in a notebook's cell; a strategy kept in files is "
            "submitted with submit()"
        )
    source = source.replace("\r\n", "\n").replace("\r", "\n").rstrip() + "\n"
    try:
        body = ast.parse(source).body
    except (SyntaxError, ValueError):
        body = []
    if len(body) != 1 or not isinstance(body[0], ast.ClassDef) or body[0].name != name:
        raise ValueError(
            f"the source of {name} is not one class statement at the top level of its module or "
            "cell: submit_class sends a class written there, and not one written inside "
            "another statement"
        )
    return source


def _declared(imports):
    """The imports a caller declares, as the text written above the class:
    import statements and nothing else, each naming what it imports."""
    import ast

    if type(imports) is not str:
        raise _wrong_type("imports", "a str of import statements", imports)
    text = imports.replace("\r\n", "\n").replace("\r", "\n").strip()
    try:
        statements = ast.parse(text).body
    except (SyntaxError, ValueError) as error:
        # Text Python does not read at all, a NUL in it, has no line.
        why = getattr(error, "msg", None) or str(error)
        line = getattr(error, "lineno", None)
        where = "" if line is None else f", at line {line}"
        raise ValueError(
            f"imports is not Python: {why}{where}; write import statements, one a line, such as "
            f"{_IMPORTS!r}"
        ) from None
    for statement in statements:
        named = isinstance(statement, ast.Import) or (
            isinstance(statement, ast.ImportFrom)
            and statement.level == 0
            and all(alias.name != "*" for alias in statement.names)
        )
        if not named:
            raise ValueError(
                f"line {statement.lineno} of imports is no import statement that names what it "
                "imports: imports holds import statements alone, none of them relative and "
                "none with a *"
            )
    return text


def _undefined(module):
    """The names the text of a module reads and nothing in it defines: not
    its imports, not its class, and no builtin of Python's. Read from the
    text's own table of symbols; nothing of it is run."""
    import builtins
    import symtable

    top = symtable.symtable(module, _FILE, "exec")
    defined = {
        symbol.get_name()
        for symbol in top.get_symbols()
        if symbol.is_assigned() or symbol.is_imported() or symbol.is_namespace()
    }
    read = set()

    def walk(table, outermost):
        for symbol in table.get_symbols():
            # Inside the class and its functions a name that is neither
            # theirs nor an enclosing function's is looked up in the module.
            if symbol.is_referenced() and (outermost or symbol.is_global()):
                read.add(symbol.get_name())
        for child in table.get_children():
            walk(child, False)

    walk(top, True)
    return sorted(read - defined - set(dir(builtins)) - _MODULE_NAMES)


def _written_in(strategy):
    """The names defined where a class was written: the globals its own
    functions were defined with, or, for a class that has none, those of
    the module it says it is of."""
    import sys
    import types

    for value in vars(strategy).values():
        function = getattr(value, "__func__", None) or getattr(value, "fget", None) or value
        if isinstance(function, types.FunctionType):
            return function.__globals__
    return vars(sys.modules.get(strategy.__module__, types.ModuleType("nowhere")))


def _advice(strategy, names):
    """What to do about names a class's source reads and does not define,
    by what each is where the class was written: a module or a name that an
    import statement gives, something defined beside the class, or
    nothing."""
    import sys
    import types

    around = _written_in(strategy)
    imported, beside, nowhere = [], [], []
    for name in names:
        if name not in around:
            nowhere.append(name)
            continue
        value = around[name]
        if isinstance(value, types.ModuleType):
            alias = "" if name == value.__name__ else f" as {name}"
            imported.append(f"import {value.__name__}{alias}")
            continue
        home, known = getattr(value, "__module__", None), getattr(value, "__name__", None)
        if (
            isinstance(home, str)
            and isinstance(known, str)
            # A builtin under another name was given that name here.
            and home not in (strategy.__module__, "builtins")
            and getattr(sys.modules.get(home), known, None) is value
        ):
            alias = "" if name == known else f" as {name}"
            imported.append(f"from {home} import {known}{alias}")
        else:
            beside.append(name)
    advice = []
    if imported:
        advice.append(f"add {_listed(repr(line) for line in imported)} to imports")
    if beside:
        advice.append(f"define {_listed(beside)} inside the class")
    if nowhere:
        is_or_are = "is" if len(nowhere) == 1 else "are"
        advice.append(f"{_listed(nowhere)} {is_or_are} defined nowhere the class was written")
    return "; ".join(advice)


def _class_file(strategy, imports, source_only):
    """The one Python file of the bundle `submit_class` makes of a class,
    and the entry that names the class in it: `imports`, and the class's
    own source below them. Refuses, before anything is read, a caller that
    has not said `source_only=True`."""
    _acknowledged(source_only)
    _a_strategy_class(strategy)
    declared = _declared(imports)
    name = strategy.__name__
    statement = _statement(strategy)
    module = f"{declared}\n\n\n{statement}" if declared else statement
    try:
        compile(module, _FILE, "exec", dont_inherit=True)
        unknown = _undefined(module)
        text = module.encode("utf-8")
    except SyntaxError as error:
        raise ValueError(
            f"the source of {name} does not compile below its imports: {error.msg}, at line "
            f"{error.lineno} of the file submit_class writes"
        ) from None
    except UnicodeEncodeError:
        raise ValueError(
            f"the source of {name} holds a lone surrogate, which no file holds"
        ) from None
    if unknown:
        its = "its source does" if declared else "its source, which has no imports, does"
        raise ValueError(
            f"{name} uses {_listed(unknown)}, which {its} not define: submit_class sends the "
            f"class's own source below imports, and nothing else of where it was written; "
            f"{_advice(strategy, unknown)}"
        )
    return (_FILE, text), f"{_FILE}:{name}"
