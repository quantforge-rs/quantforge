"""What the text of a file reads and does not define
(`docs/python-bundles.md`): the names a class sent by its source would be
missing where a server runs it, and what to do about each.

A class is sent as the text of its statement, below the imports its caller
declares. Whatever that text reads has to be defined in it: by the imports,
by the class itself, or as one of Python's builtins.

- **The answer is about the text, and the same on whichever Python reads
  it.** A bundle runs on its server's Python. What Python's own table of
  symbols tells differently from one version to the next is written plainly
  before the table is asked: an annotation as the expression it is, where
  the Pythons that evaluate one as its statement runs evaluate it, and a
  comprehension as the scope of its own that it is.
- **A class's body is read in the order it is written.** It looks a name up
  among those the class has set so far, and then outside the class: a name
  it reads before it has set it is read outside, though the name is the
  class's own.
- **Inside a function, a lambda or a comprehension of the class's body,**
  Python does not look in the class at all: a name the class sets is read
  outside it there.
- **Nothing is run, and nothing is asked.** The text is parsed and its
  symbols read. What stands under a name where the class was written is
  told by its type and by what Python itself keeps of it: an object whose
  class answers for its attributes with code of its own is asked nothing.

Importing this module imports nothing: `ast`, `symtable` and `inspect` are
imported when a class is first submitted.

This module is private.
"""

# The names every module has without defining them.
_MODULE_NAMES = frozenset(
    {"__name__", "__doc__", "__file__", "__package__", "__spec__", "__loader__", "__builtins__"}
)


def _listed(names):
    """Names as a sentence lists them: `a`, `a and b`, `a, b and c`."""
    names = list(names)
    if len(names) < 3:
        return " and ".join(names)
    return f"{', '.join(names[:-1])} and {names[-1]}"


def _kept(value, name):
    """What Python keeps of an object under a name it gives objects itself,
    `__name__` or `__module__`, where that is text; `None` where it keeps
    none so. Read without asking the object, so that no class answers for
    it with code of its own."""
    import inspect
    import types

    try:
        found = inspect.getattr_static(value, name)
    except AttributeError:
        return None
    # Python's own types keep such a name in a slot, which a descriptor of
    # the type reads: its code, and none of the object's.
    if type(found) in (types.GetSetDescriptorType, types.MemberDescriptorType):
        try:
            found = found.__get__(value)
        except (AttributeError, TypeError):
            return None
    return found if type(found) is str else None


class _ClassBody:
    """One class's body, read in the order it is written, for the names it
    reads before it has set them.

    A class's body looks a name up among those the class has set so far,
    then in the module, then among Python's builtins: so `LIMIT = LIMIT`
    reads the module's `LIMIT`, though the name is the class's own in
    Python's table of symbols. Only what is certain is reported. A read
    inside a loop, which may run after a later statement of the loop has, is
    left alone; a name that any statement written before the read sets, in
    whichever branch, counts as set; and a statement this reader does not
    know sets what it names and reads nothing.

    An annotation is read where its statement stands, as Python up to 3.13
    evaluates one: a bundle runs on its server's Python, whichever reads the
    class here."""

    def __init__(self):
        self.bound, self.early, self.outer, self.loops = set(), set(), set(), 0
        # What the body makes the class's own without setting it: a name it
        # annotates and gives no value, and one it deletes.
        self.own = set()

    def names(self, body):
        """The names the body reads before it has set them, of those that
        are the class's own. One that is not is the module's, or a
        function's around the class: which of the two, the table of symbols
        says."""
        self.statements(body)
        return (self.early & (self.bound | self.own)) - self.outer

    def read(self, name):
        if name not in self.bound and not self.loops:
            self.early.add(name)

    def statements(self, body):
        for statement in body:
            self.statement(statement)

    def statement(self, node):
        import ast

        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            # What a function is decorated with, its defaults and its
            # annotations are the class's to evaluate; its body is not.
            arguments = node.args
            named = [*arguments.posonlyargs, *arguments.args, *arguments.kwonlyargs]
            named += [arguments.vararg, arguments.kwarg]
            annotations = [each.annotation for each in named if each is not None]
            defaults = [*arguments.defaults, *arguments.kw_defaults]
            for each in (*node.decorator_list, *defaults, *annotations, node.returns):
                self.expression(each)
            self.bound.add(node.name)
        elif isinstance(node, ast.ClassDef):
            keywords = [keyword.value for keyword in node.keywords]
            for each in (*node.decorator_list, *node.bases, *keywords):
                self.expression(each)
            self.bound.add(node.name)
        elif isinstance(node, ast.Assign):
            self.expression(node.value)
            for target in node.targets:
                self.target(target)
        elif isinstance(node, ast.AugAssign):
            if isinstance(node.target, ast.Name):
                self.read(node.target.id)
            self.expression(node.value)
            self.target(node.target)
        elif isinstance(node, ast.AnnAssign):
            # The value, then the name, then what annotates it. Without a
            # value the name is the class's own, and nothing is set.
            if node.value is not None:
                self.expression(node.value)
                self.target(node.target)
            elif isinstance(node.target, ast.Name):
                self.own.add(node.target.id)
            else:
                self.expression(node.target)
            self.expression(node.annotation)
        elif isinstance(node, ast.Delete):
            for target in node.targets:
                if isinstance(target, ast.Name):
                    self.read(target.id)
                    self.own.add(target.id)
                else:
                    self.expression(target)
        elif isinstance(node, (ast.Import, ast.ImportFrom)):
            for alias in node.names:
                self.bound.add((alias.asname or alias.name).partition(".")[0])
        elif isinstance(node, ast.Nonlocal):
            self.outer.update(node.names)
        elif isinstance(node, ast.If):
            self.expression(node.test)
            self.statements(node.body)
            self.statements(node.orelse)
        elif isinstance(node, (ast.For, ast.AsyncFor)):
            self.expression(node.iter)
            self.loops += 1
            self.target(node.target)
            self.statements(node.body)
            self.statements(node.orelse)
            self.loops -= 1
        elif isinstance(node, ast.While):
            self.loops += 1
            self.expression(node.test)
            self.statements(node.body)
            self.statements(node.orelse)
            self.loops -= 1
        elif isinstance(node, (ast.With, ast.AsyncWith)):
            for item in node.items:
                self.expression(item.context_expr)
                if item.optional_vars is not None:
                    self.target(item.optional_vars)
            self.statements(node.body)
        elif isinstance(node, ast.Try) or type(node).__name__ == "TryStar":
            self.statements(node.body)
            for handler in node.handlers:
                self.expression(handler.type)
                if handler.name:
                    self.bound.add(handler.name)
                self.statements(handler.body)
            self.statements(node.orelse)
            self.statements(node.finalbody)
        elif isinstance(node, (ast.Expr, ast.Assert, ast.Raise, ast.Pass, ast.Global)):
            for child in ast.iter_child_nodes(node):
                self.expression(child)
        else:
            self.unknown(node)

    def unknown(self, node):
        """A statement this reader does not know, a `match` and what a later
        Python adds: every name it could set counts as set, and nothing it
        reads is reported."""
        import ast

        for inner in ast.walk(node):
            if isinstance(inner, ast.Name) and not isinstance(inner.ctx, ast.Load):
                self.bound.add(inner.id)
            elif isinstance(inner, ast.alias):
                self.bound.add((inner.asname or inner.name).partition(".")[0])
            for field in ("name", "rest"):
                named = getattr(inner, field, None)
                if type(named) is str:
                    self.bound.add(named)

    def target(self, node):
        """Sets what an assignment's target names, and reads what a target
        is made of: the object of an attribute, the container of an item."""
        import ast

        if isinstance(node, ast.Name):
            self.bound.add(node.id)
        elif isinstance(node, (ast.Tuple, ast.List)):
            for each in node.elts:
                self.target(each)
        elif isinstance(node, ast.Starred):
            self.target(node.value)
        else:
            self.expression(node)

    def expression(self, node):
        """Reads an expression as the class's body evaluates it: the names
        it loads there, in the order they are written."""
        import ast

        if node is None:
            return
        if isinstance(node, ast.Name):
            if isinstance(node.ctx, ast.Load):
                self.read(node.id)
        elif isinstance(node, ast.NamedExpr):
            self.expression(node.value)
            self.bound.add(node.target.id)
        elif isinstance(node, ast.Lambda):
            # Its defaults are evaluated here; its body is a scope of its own.
            for default in (*node.args.defaults, *node.args.kw_defaults):
                self.expression(default)
        elif isinstance(node, (ast.ListComp, ast.SetComp, ast.DictComp, ast.GeneratorExp)):
            # Its first iterable is evaluated here; the rest is a scope of
            # its own, which sees no name of the class's.
            self.expression(node.generators[0].iter)
        else:
            for child in ast.iter_child_nodes(node):
                self.expression(child)


def _read_early(tree):
    """The names that a class's body, in a module's statements, reads before
    the class has set them, the class's own and any class's inside it; and
    the names that the classes written at the module's top level make their
    own, by setting them or by annotating them."""
    import ast

    early, owned = set(), set()
    for node in ast.walk(tree):
        if isinstance(node, ast.ClassDef):
            body = _ClassBody()
            early |= body.names(node.body)
            if node in tree.body:
                owned |= body.bound | body.own
    return early, owned


def _plainly(tree):
    """A module's statements as text that Python's table of symbols says the
    same of on every Python, with the same names read from the same scopes.

    Two things are written otherwise, which the table tells differently
    from one Python to the next:

    - **An annotation** is evaluated where its statement stands up to
      Python 3.13, and from 3.14 on when it is asked for, in a scope of its
      own. It is written as the expression it is, where the earlier Pythons
      evaluate it: beside its assignment in a class's body, and before a
      function whose signature it is of. Inside a function's body, where no
      Python evaluates one, it is left out.
    - **A comprehension** in a class's body is a scope of its own, which
      sees no name of the class's. From Python 3.12 on the table no longer
      shows it as one. It is written as the generator expression of the
      same parts, which every Python shows as a scope.

    The statements are changed in place: they are read for nothing else
    afterwards."""
    import ast

    def expressions(node):
        for field, value in ast.iter_fields(node):
            if isinstance(value, list):
                value[:] = [
                    expressions(each) if isinstance(each, ast.AST) else each for each in value
                ]
            elif isinstance(value, ast.AST):
                setattr(node, field, expressions(value))
        if isinstance(node, (ast.ListComp, ast.SetComp)):
            return ast.GeneratorExp(node.elt, node.generators)
        if isinstance(node, ast.DictComp):
            return ast.GeneratorExp(ast.Tuple([node.key, node.value], ast.Load()), node.generators)
        return node

    def unset(target):
        # A name that is annotated and given no value is its scope's own,
        # and is not set: as one that is deleted is.
        if isinstance(target, ast.Name):
            return ast.Delete([ast.Name(target.id, ast.Del())])
        return ast.Expr(target)

    def block(statements, in_function):
        written = []
        for node in statements:
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
                arguments = node.args
                named = [*arguments.posonlyargs, *arguments.args, *arguments.kwonlyargs]
                for each in (*named, arguments.vararg, arguments.kwarg):
                    if each is not None and each.annotation is not None:
                        written.append(ast.Expr(each.annotation))
                        each.annotation = None
                if node.returns is not None:
                    written.append(ast.Expr(node.returns))
                    node.returns = None
                node.body = block(node.body, True)
            elif isinstance(node, ast.ClassDef):
                node.body = block(node.body, False)
            elif isinstance(node, ast.AnnAssign):
                if not in_function:
                    written.append(ast.Expr(node.annotation))
                if node.value is None:
                    node = unset(node.target)
                else:
                    node = ast.Assign([node.target], node.value)
            else:
                for field in ("body", "orelse", "finalbody"):
                    inner = getattr(node, field, None)
                    if isinstance(inner, list) and inner and isinstance(inner[0], ast.stmt):
                        setattr(node, field, block(inner, in_function))
                for part in (*getattr(node, "handlers", ()), *getattr(node, "cases", ())):
                    part.body = block(part.body, in_function)
            written.append(node)
        return written

    tree.body = block(tree.body, False)
    return ast.unparse(ast.fix_missing_locations(expressions(tree)))


def _undefined(module):
    """The names the text of a module reads and nothing in it defines: not
    its imports, not its class, and no builtin of Python's. With them, those
    a class's body reads before the class has set them, and those the class
    sets in its body and reads where Python does not look there: inside a
    function, a lambda or a comprehension. Read from the text's own table
    of symbols and from its statements in their order; nothing of it is
    run, and the answer is the same on whichever Python reads the text."""
    import ast
    import builtins
    import symtable

    tree = ast.parse(module)
    early, owned = _read_early(tree)
    top = symtable.symtable(_plainly(tree), "<class>", "exec")
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
    unknown = (read | early) - defined - set(dir(builtins)) - _MODULE_NAMES
    return sorted(unknown), early & unknown, (owned & unknown) - early


def _advice(around, module, names, early, within):
    """What to do about names a class's source reads and does not define,
    by what each is: one the class reads before it has set it, `early`; one
    the class sets and reads where Python does not look in the class,
    `within`; or, by what stands under the name in `around`, the names
    defined where the class was written, a module or a name that an import
    statement gives, something defined beside the class, or nothing.
    `module` is the name of the module the class says it is of. What stands
    under a name is told by its type and by what Python itself keeps of it,
    and is asked nothing."""
    import sys
    import types

    imported, beside, nowhere, sooner, inside = [], [], [], [], []
    for name in names:
        if name in early:
            sooner.append(name)
            continue
        if name in within:
            inside.append(name)
            continue
        if name not in around:
            nowhere.append(name)
            continue
        value = around[name]
        known = _kept(value, "__name__")
        if type(value) is types.ModuleType and known is not None:
            alias = "" if name == known else f" as {name}"
            imported.append(f"import {known}{alias}")
            continue
        home = _kept(value, "__module__")
        there = sys.modules.get(home) if home is not None else None
        if (
            known is not None
            # A builtin under another name was given that name here.
            and home not in (None, module, "builtins")
            and type(there) is types.ModuleType
            and vars(there).get(known) is value
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
    if inside:
        is_or_are, it = ("is", "it") if len(inside) == 1 else ("are", "them")
        advice.append(
            f"{_listed(inside)} {is_or_are} set in the class's body, where a function, a lambda "
            f"or a comprehension inside the class does not look: there Python reads {it} outside "
            "the class"
        )
    if sooner:
        it = "it" if len(sooner) == 1 else "them"
        advice.append(
            f"the class reads {_listed(sooner)} before it sets {it}, where Python looks outside "
            f"the class: set {it} from what the class itself defines"
        )
    return "; ".join(advice)
