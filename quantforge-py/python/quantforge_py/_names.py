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
  outside it there. A comprehension's first iterable is no part of that:
  the class's body evaluates it, as it does a lambda's defaults.
- **Only what is certain is refused.** Whether a loop runs and which way a
  branch goes, the text does not say: a read that some way through the
  body satisfies is no refusal.
- **A block may be left before its end,** and the class's body goes on
  from where it was left: an exception for a handler, a `break` for its
  loop, which no handler takes. What may be set there is told place by
  place; every way out of a `try` passes through its `finally`. And where
  nothing leads, after a `raise` or in the handler of a `try` that nothing
  can leave, nothing is read.
- **Python's builtins are those every Python the package runs on has,**
  on every system: a bundle runs on its server's Python, not on the one
  that reads the class.
- **Nothing is run, and nothing is asked.** The text is parsed and its
  symbols read. What stands under a name where the class was written is
  told by its type and by what Python itself keeps of it: an object whose
  class answers for its attributes with code of its own is asked nothing.

Importing this module imports nothing: `ast`, `symtable` and `inspect` are
imported when a class is first submitted.

This module is private.
"""

# Python's builtins, as every Python the package runs on has them: those of
# Python 3.11, the oldest. A later Python has more, and Windows one of its
# own: a bundle runs on its server's Python, on its server's system.
_BUILTINS = frozenset(
    """
    ArithmeticError AssertionError AttributeError BaseException BaseExceptionGroup
    BlockingIOError BrokenPipeError BufferError BytesWarning ChildProcessError
    ConnectionAbortedError ConnectionError ConnectionRefusedError ConnectionResetError
    DeprecationWarning EOFError Ellipsis EncodingWarning EnvironmentError Exception
    ExceptionGroup False FileExistsError FileNotFoundError FloatingPointError FutureWarning
    GeneratorExit IOError ImportError ImportWarning IndentationError IndexError
    InterruptedError IsADirectoryError KeyError KeyboardInterrupt LookupError MemoryError
    ModuleNotFoundError NameError None NotADirectoryError NotImplemented NotImplementedError
    OSError OverflowError PendingDeprecationWarning PermissionError ProcessLookupError
    RecursionError ReferenceError ResourceWarning RuntimeError RuntimeWarning
    StopAsyncIteration StopIteration SyntaxError SyntaxWarning SystemError SystemExit TabError
    TimeoutError True TypeError UnboundLocalError UnicodeDecodeError UnicodeEncodeError
    UnicodeError UnicodeTranslateError UnicodeWarning UserWarning ValueError Warning
    ZeroDivisionError __build_class__ __debug__ __doc__ __import__ __loader__ __name__
    __package__ __spec__ abs aiter all anext any ascii bin bool breakpoint bytearray bytes
    callable chr classmethod compile complex copyright credits delattr dict dir divmod
    enumerate eval exec exit filter float format frozenset getattr globals hasattr hash help
    hex id input int isinstance issubclass iter len license list locals map max memoryview
    min next object oct open ord pow print property quit range repr reversed round set
    setattr slice sorted staticmethod str sum super tuple type vars zip
    """.split()
)

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


def _mangled(name, owner):
    """A name as Python looks it up inside the class `owner`. One that starts
    with two underscores and does not end with two is private to the class:
    Python writes the class's name before it, less that name's own leading
    underscores."""
    if not name.startswith("__") or name.endswith("__") or "." in name:
        return name
    owner = owner.lstrip("_")
    return f"_{owner}{name}" if owner else name


# What the names that may be set are kept with where the class's body can come
# to the statement being read at all. It is no name: where it is missing,
# nothing leads there, and nothing read there is read.
_HERE = "."


class _ClassBody:
    """One class's body, read in the order it is written, for the names it
    reads before it has set them.

    A class's body looks a name up among those the class has set so far,
    then in the module, then among Python's builtins: so `LIMIT = LIMIT`
    reads the module's `LIMIT`, though the name is the class's own in
    Python's table of symbols.

    **Only what is certain is reported.** The reader keeps the names the
    class may have set by the statement it reads, along whichever way led
    there: after branches of which one is taken, a name may be set if any
    of them may have set it, and a name is unset for certain only where the
    class deletes it, or where the handler of an exception ends, which
    clears the name the exception was given. A read is reported where the
    name cannot have been set.

    **A block may be left before its end**, the body of a `try`, of a
    `with` or of a loop, and the class's body goes on from where it was
    left. An exception goes to a handler, or past a `with` whose manager
    takes it. It may leave a block where a statement of it begins, since
    whatever a statement evaluates may raise, and where a statement has
    set a name and goes on to something that may raise; a `del` is left
    otherwise, since where it fails at a name, that name was not set. A
    `break` and a `continue` go to their loop, and no handler takes them.
    **Every way out of a `try` passes through its `finally`**, and every
    way out of a handler through the clearing of its name: what these
    unset is unset wherever the body goes on from there.

    **Where nothing leads, nothing is read.** What follows a `raise`, a
    `break` or a `continue` in its block does not run, and neither does
    the handler of a `try` that nothing can leave: a read there is no
    read, and what is set there is set on no way. It still makes the name
    the class's own, as it does in Python's table of symbols.

    So a class is never refused for a read that some way through its body
    satisfies; one that fails only along some way, as a read after a loop
    that did not run, is taken, and fails where it runs. A read inside a
    loop, which may come after a later statement of the loop has run, is
    left alone, and a statement this reader does not know sets what it
    names and reads nothing.

    An annotation is read where its statement stands, as Python up to 3.13
    evaluates one: a bundle runs on its server's Python, whichever reads the
    class here. Names are read as the class's body looks them up, a private
    one under the class's name."""

    def __init__(self, owner):
        self.owner = owner
        # The names the class may have set by the statement being read, with
        # `_HERE` where its body can come there; and every name it sets
        # anywhere.
        self.maybe, self.ever = {_HERE}, set()
        # What the body makes the class's own without setting it: a name it
        # annotates and gives no value, and one it deletes.
        self.own = set()
        # What it reads where the name cannot have been set, and what it
        # says is a function's around the class, or the module's.
        self.early, self.outer = set(), set()
        # Above zero where a read is not reported: inside a loop, and in a
        # `finally` that is read a second time.
        self.silent = 0
        # What may be set where a block being read is left before its end,
        # the innermost block last: by an exception, and for the next round
        # of the loop around it or for that loop's end.
        self.exits, self.rounds = [], []

    def names(self, body):
        """The names the body reads before it has set them, of those that
        are the class's own. One that is not is the module's, or a
        function's around the class: which of the two, the table of symbols
        says."""
        self.statements(body)
        return (self.early & (self.ever | self.own)) - self.outer

    def read(self, name):
        name = _mangled(name, self.owner)
        if _HERE in self.maybe and name not in self.maybe and not self.silent:
            self.early.add(name)

    def bind(self, name, last=False):
        """Sets a name. Unless that is the `last` thing its statement does,
        what the statement goes on to may raise: the block being read may
        be left with the name set."""
        name = _mangled(name, self.owner)
        self.ever.add(name)
        if _HERE in self.maybe:
            self.maybe.add(name)
            if not last:
                self.leave()

    def unbind(self, name):
        self.maybe.discard(_mangled(name, self.owner))

    def leave(self, names=None):
        """Notes that an exception may leave the block being read with
        `names` set: unless given, here, with what may be set now."""
        if self.exits:
            self.exits[-1] |= self.maybe if names is None else names

    def go(self, names):
        """Notes that the block being read may be left with `names` set for
        the next round of the loop around it, or for the loop's end."""
        if self.rounds:
            self.rounds[-1] |= names

    def left(self, read):
        """Reads a block that may be left before its end, and gives the
        names that may be set where an exception leaves it. Where the
        class's body goes on from there is its caller's to say: nothing is
        noted for the block around it."""
        self.exits.append(set())
        try:
            read()
        finally:
            exits = self.exits.pop()
        return exits

    def both(self, read):
        """Reads a block as `left` does, and gives beside it the names that
        may be set where the block is left for the loop around it: what
        this block does to those on their way, its caller says."""
        self.rounds.append(set())
        try:
            exits = self.left(read)
        finally:
            rounds = self.rounds.pop()
        return exits, rounds

    def either(self, *ways):
        """Reads ways of which one is taken: after them a name may be set
        if any of them may have set it."""
        before, after = set(self.maybe), set()
        for way in ways:
            self.maybe = set(before)
            way()
            after |= self.maybe
        self.maybe = after

    def statements(self, body):
        for statement in body:
            self.statement(statement)

    def statement(self, node):
        import ast

        still = (ast.Try, ast.TryStar, ast.Pass, ast.Global, ast.Nonlocal, ast.Break, ast.Continue)
        if not isinstance(node, (*still, ast.Delete)):
            # Whatever a statement evaluates may raise as it begins. These
            # evaluate nothing themselves, and a `del` says where it is left.
            self.leave()
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            # What a function is decorated with and its defaults are the
            # class's to evaluate, and its annotations where its type
            # parameters are names; its body is not.
            arguments = node.args
            named = [*arguments.posonlyargs, *arguments.args, arguments.vararg]
            named += [*arguments.kwonlyargs, arguments.kwarg]
            annotations = [each.annotation for each in named if each is not None]
            for each in (*node.decorator_list, *arguments.defaults, *arguments.kw_defaults):
                self.expression(each)
            self.generic(node, [*annotations, node.returns])
            self.bind(node.name, last=True)
        elif isinstance(node, ast.ClassDef):
            for each in node.decorator_list:
                self.expression(each)
            self.generic(node, [*node.bases, *(keyword.value for keyword in node.keywords)])
            self.bind(node.name, last=True)
        elif isinstance(node, ast.Assign):
            self.expression(node.value)
            self.targets(node.targets)
        elif isinstance(node, ast.AugAssign):
            # What is added to is read before the value: a name, or the
            # object and the index of an item.
            if isinstance(node.target, ast.Name):
                self.read(node.target.id)
            else:
                self.expression(node.target)
            self.expression(node.value)
            if isinstance(node.target, ast.Name):
                self.bind(node.target.id, last=True)
        elif isinstance(node, ast.AnnAssign):
            # The value, then the name, then what annotates it. Without a
            # value the name is the class's own, and nothing is set.
            if node.value is not None:
                self.expression(node.value)
                self.targets([node.target], more=True)
            elif isinstance(node.target, ast.Name):
                self.own.add(_mangled(node.target.id, self.owner))
            else:
                self.expression(node.target)
            self.expression(node.annotation)
        elif isinstance(node, ast.Delete):
            self.deleted(node.targets)
        elif isinstance(node, (ast.Import, ast.ImportFrom)):
            # One after the other: a later one may fail to be imported.
            for alias in node.names:
                name = (alias.asname or alias.name).partition(".")[0]
                self.bind(name, last=alias is node.names[-1])
        elif isinstance(node, (ast.Nonlocal, ast.Global)):
            self.outer.update(_mangled(name, self.owner) for name in node.names)
        elif isinstance(node, ast.If):
            self.expression(node.test)
            self.either(lambda: self.statements(node.body), lambda: self.statements(node.orelse))
        elif isinstance(node, (ast.For, ast.AsyncFor, ast.While)):
            self.loop(node)
        elif isinstance(node, (ast.With, ast.AsyncWith)):
            for item in node.items:
                self.expression(item.context_expr)
                if item.optional_vars is not None:
                    self.targets([item.optional_vars], more=True)
            left, gone = self.both(lambda: self.statements(node.body))
            # What manages the block may raise as the block ends, or as it
            # is left for the loop around it; an exception goes on from
            # where it left the block; and where what manages the block
            # takes it, the class's body goes on.
            self.leave(left | gone | self.maybe)
            self.go(gone)
            self.maybe |= left
        elif isinstance(node, (ast.Try, ast.TryStar)):
            self.attempt(node)
        elif isinstance(node, (ast.Break, ast.Continue)):
            # For the next round of its loop, or for the loop's end: no
            # handler takes it, and what follows it does not run.
            self.go(self.maybe)
            self.maybe = set()
        elif isinstance(node, (ast.Expr, ast.Assert, ast.Raise, ast.Pass)):
            for child in ast.iter_child_nodes(node):
                self.expression(child)
            if isinstance(node, ast.Raise):
                # What follows it does not run.
                self.maybe = set()
        else:
            self.unknown(node)

    def loop(self, node):
        """A loop: what it runs over, or its test, is read once for certain;
        its body may run any number of times, or not at all, and its `else`
        runs unless the loop is broken out of."""
        import ast

        self.expression(node.test if isinstance(node, ast.While) else node.iter)
        before = set(self.maybe)

        def body():
            if not isinstance(node, ast.While):
                self.targets([node.target], more=True)
            self.statements(node.body)

        self.silent += 1
        left, gone = self.both(body)
        self.silent -= 1
        # A round ends at the end of the body, or where it is left for the
        # next round or for the loop's end; and an exception goes on from
        # wherever a round was left, or from where the next was asked for.
        gone |= self.maybe
        self.leave(left | gone)
        self.maybe = before | gone
        ran = set(self.maybe)
        self.statements(node.orelse)
        self.maybe |= ran

    def attempt(self, node):
        """A `try`. A handler runs from wherever an exception left the body.
        Which handler takes it is found by evaluating what each names, in
        turn; the handler is given the exception under a name that Python
        clears however the handler ends. Of the handlers of a `try` one
        runs; of the clauses of an `except*` every one that matches a part
        of what was raised, one after the other, and one that raises does
        not keep the next from running. And however the statement is left,
        by an exception or for the loop around it, what is left to
        `finally` runs first."""
        import ast

        every = isinstance(node, ast.TryStar)
        ends = []

        def tried():
            raised = self.left(lambda: self.statements(node.body))
            self.statements(node.orelse)
            ends.append(set(self.maybe))
            self.maybe = raised
            for handler in node.handlers:
                # A handler that names nothing takes whatever was raised:
                # nothing is evaluated for it, and nothing passes it.
                passed = set()
                if handler.type is not None:
                    self.leave()
                    self.expression(handler.type)
                    passed = set(self.maybe)

                def handled():
                    if handler.name:
                        self.bind(handler.name)
                    self.statements(handler.body)

                left, gone = self.both(handled)
                if handler.name:
                    self.unbind(handler.name)
                    left.discard(_mangled(handler.name, self.owner))
                    gone.discard(_mangled(handler.name, self.owner))
                self.go(gone)
                ends.append(set(self.maybe))
                if every:
                    self.maybe |= passed | left
                else:
                    self.leave(left)
                    self.maybe = passed
            # What no handler took goes on from here, and what the clauses
            # of an `except*` raised with it; where every clause took its
            # part, the statement ends here.
            self.leave()
            if every:
                ends.append(set(self.maybe))

        left, gone = self.both(tried)
        ended = set().union(*ends)
        if not node.finalbody:
            self.leave(left)
            self.go(gone)
            self.maybe = ended
            return
        # `finally` runs from wherever the statement ended or was left: what
        # it reads is held to each, and it may be left itself from any.
        began = ended | left | gone
        self.maybe = set(began)
        self.statements(node.finalbody)
        after = sets = self.maybe
        if any(way and way != began for way in (ended, left, gone)):
            # After it, a name is set that `finally` sets itself, or that
            # was set where `finally` began and that it leaves set. What it
            # sets itself is told by reading it again from nothing set, with
            # nothing reported and nowhere left a second time. Python's
            # compiler writes a `finally` out more than once too.
            self.maybe = {_HERE}
            self.silent += 1
            self.both(lambda: self.statements(node.finalbody))
            self.silent -= 1
            sets = self.maybe

        def through(way):
            return after & way | sets if way else set()

        # What left the statement goes on from the end of `finally`, and the
        # class's body goes on from there where the statement had ended.
        self.leave(through(left))
        self.go(through(gone))
        self.maybe = through(ended)

    def generic(self, node, expressions):
        """Reads what a function or a class evaluates where its type
        parameters are names: a function's annotations, a class's bases.
        There they are set, and nowhere else."""
        parameters = {_mangled(each.name, self.owner) for each in getattr(node, "type_params", ())}
        held = set(self.maybe)
        self.maybe |= parameters
        for each in expressions:
            self.expression(each)
        self.maybe = held | (self.maybe - parameters)

    def deleted(self, targets):
        """Unsets what a `del` names, from left to right: a name is read,
        to be deleted, is the class's own from then on, and is set no
        longer. Where the statement fails at a name, the name was not set:
        the block may be left once the first is unset, and not before."""
        import ast

        flat = []

        def each(target):
            if isinstance(target, (ast.Tuple, ast.List)):
                for inner in target.elts:
                    each(inner)
            else:
                flat.append(target)

        for target in targets:
            each(target)
        for at, target in enumerate(flat):
            if isinstance(target, ast.Name):
                self.read(target.id)
                self.own.add(_mangled(target.id, self.owner))
                self.unbind(target.id)
            else:
                self.expression(target)
            if not at:
                self.leave()

    def unknown(self, node):
        """A statement this reader does not know, a `match` and what a later
        Python adds: every name it could set counts as set, and nothing it
        reads is reported. Where it holds a `break` or a `continue`, its
        loop may be left from inside it, with any of those names set."""
        import ast

        leaves = False
        for inner in ast.walk(node):
            if isinstance(inner, ast.Name) and not isinstance(inner.ctx, ast.Load):
                self.bind(inner.id)
            elif isinstance(inner, ast.alias):
                self.bind((inner.asname or inner.name).partition(".")[0])
            for field in ("name", "rest"):
                named = getattr(inner, field, None)
                if type(named) is str:
                    self.bind(named)
            leaves = leaves or isinstance(inner, (ast.Break, ast.Continue))
        if leaves:
            self.go(self.maybe)

    def targets(self, targets, more=False):
        """Sets what the targets of an assignment name, from left to right,
        and reads what a target is made of: the object of an attribute, the
        container of an item. A name is the last thing set where nothing
        after it can fail: no unpacking, no target that is no name, and,
        with `more`, nothing the statement goes on to."""
        import ast

        steps = []

        def each(target):
            if isinstance(target, (ast.Tuple, ast.List)):
                # Unpacked before any of its names is set.
                steps.append(None)
                for inner in target.elts:
                    each(inner)
            elif isinstance(target, ast.Starred):
                each(target.value)
            else:
                steps.append(target)

        for target in targets:
            each(target)
        fails = [at for at, step in enumerate(steps) if not isinstance(step, ast.Name)]
        after = len(steps) if more else max(fails, default=-1)
        for at, step in enumerate(steps):
            if isinstance(step, ast.Name):
                self.bind(step.id, last=at > after)
            elif step is not None:
                self.expression(step)

    def expression(self, node):
        """Reads an expression as the class's body evaluates it: the names
        it loads there, in the order Python evaluates them."""
        import ast

        if node is None:
            return
        if isinstance(node, ast.Name):
            if isinstance(node.ctx, ast.Load):
                self.read(node.id)
        elif isinstance(node, ast.NamedExpr):
            self.expression(node.value)
            self.bind(node.target.id)
        elif isinstance(node, ast.Dict):
            # A key and then its value, pair by pair.
            for key, value in zip(node.keys, node.values):
                self.expression(key)
                self.expression(value)
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
            body = _ClassBody(node.name)
            early |= body.names(node.body)
            if node in tree.body:
                owned |= (body.ever | body.own) - body.outer
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

    def without(node, names):
        # A type parameter is a name where a function's annotations are
        # evaluated, and nowhere they are written out. What an annotation
        # sets under such a name, as a comprehension in it may, is its own.
        if isinstance(node, ast.Name) and isinstance(node.ctx, ast.Load) and node.id in names:
            return ast.Constant(None)
        for field, value in ast.iter_fields(node):
            if isinstance(value, list):
                value[:] = [
                    without(each, names) if isinstance(each, ast.AST) else each for each in value
                ]
            elif isinstance(value, ast.AST):
                setattr(node, field, without(value, names))
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
                parameters = {each.name for each in getattr(node, "type_params", ())}
                named = [*arguments.posonlyargs, *arguments.args, *arguments.kwonlyargs]
                for each in (*named, arguments.vararg, arguments.kwarg):
                    if each is not None and each.annotation is not None:
                        written.append(ast.Expr(without(each.annotation, parameters)))
                        each.annotation = None
                if node.returns is not None:
                    written.append(ast.Expr(without(node.returns, parameters)))
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
    unknown = (read | early) - defined - _BUILTINS - _MODULE_NAMES
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
