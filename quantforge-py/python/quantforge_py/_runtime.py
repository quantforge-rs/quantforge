"""The runtime a strategy bundle runs in (docs/python-worker.md): what a host
does with a bundle's files before the engine drives its strategy, and what
it says of a strategy that failed. A server's worker is its first host; the
extension module reaches it from there (`src/runtime.rs`). Private.

- `prepare(packages)`: the environment as a worker offers it, with NumPy
  imported ahead of any job.
- `Bundle`: one bundle's strategy, imported from its files, held to what
  a bundle may import, and built from its parameters.
- `Guard`: what a bundle's own code may import, best effort: the process
  sandbox is the boundary.
- `Lines`: a stream whose every line goes to a job's log.
- `failed(error, root)`: a callback's exception as the job's failure, and
  its traceback.
"""

import builtins
import importlib
import importlib.metadata
import io
import json
import os
import re
import sys
import traceback

# Linux's own libraries, which a module a job imports may still load once
# the sandbox has closed every other file: read, and never changed.
SYSTEM_LIBRARIES = ("/lib", "/lib64", "/usr/lib", "/usr/lib64", "/usr/local/lib")

# The import machinery's own modules, whose frames stand between an import
# and the code that asked for it.
MACHINERY = frozenset(
    {
        "importlib",
        "importlib._bootstrap",
        "importlib._bootstrap_external",
        "_frozen_importlib",
        "_frozen_importlib_external",
    }
)

# The most text a line without its end is held for: past it, the text goes
# to the log as a line of its own, as a worker cuts no longer line.
LINE_HELD = 64 << 10

# The SDK every strategy derives from, which any bundle imports.
SDK = "quantforge_py"

# What the environment installs, read once: reading it takes tens of
# milliseconds, which a worker spends as it readies itself, not as its job
# starts.
_installed = None


def installed():
    """The distributions that install each top-level module, by the
    module's name, as `importlib.metadata.packages_distributions` gives
    them, read the first time they are asked for."""
    global _installed
    if _installed is None:
        _installed = importlib.metadata.packages_distributions()
    return _installed


def prepare(packages):
    """What a worker offers, once its environment is ready: its Python,
    `MAJOR.MINOR`; those of `packages`, by their distribution names, it has
    installed, each at its version; and the directories its interpreter
    reads its own files from.

    What the environment installs is read now, before any job, and NumPy,
    where it is offered, is imported: a job that asks for it finds it
    loaded, and the libraries it loads are loaded before the sandbox closes
    the files around them."""
    offered = {}
    for name in packages:
        try:
            offered[name] = importlib.metadata.version(name)
        except importlib.metadata.PackageNotFoundError:
            continue
    # What every job's run uses, read and loaded while every file can be.
    import decimal  # noqa: F401

    installed()
    if "numpy" in offered:
        import numpy  # noqa: F401
    return {
        "python": "%d.%d" % sys.version_info[:2],
        "packages": offered,
        "readable": readable(),
    }


def readable():
    """The directories this interpreter reads its own files from: its
    installation, its environment, every directory on its path, and on
    Linux the system's libraries."""
    found = {sys.prefix, sys.exec_prefix, sys.base_prefix, sys.base_exec_prefix}
    found.update(path for path in sys.path if path)
    if sys.platform.startswith("linux"):
        found.update(SYSTEM_LIBRARIES)
    return sorted({os.path.abspath(path) for path in found if os.path.exists(path)})


def normalized(name):
    """A distribution's name as a manifest's `deps` writes it (PEP 503)."""
    return re.sub(r"[-_.]+", "-", name).lower()


class Refused(Exception):
    """Why a bundle's strategy could not be built: what the job's failure
    says, and the traceback, where there is one, for the job's log."""

    def __init__(self, message, written=""):
        super().__init__(message)
        self.message = message
        self.traceback = written


def line(error):
    """An exception's own line, as Python writes it last in a traceback:
    `ZeroDivisionError: division by zero`."""
    kind = type(error)
    name = kind.__qualname__
    if kind.__module__ not in ("builtins", "__main__"):
        name = f"{kind.__module__}.{name}"
    said = str(error)
    return f"{name}: {said}" if said else name


def machinery(filename):
    """Whether a frame of this file is the runtime's own, or the import
    system's, which stand between a bundle's code and what called it."""
    return (
        filename == __file__
        or filename == importlib.__file__
        or filename.startswith("<frozen importlib.")
    )


def trimmed(shown, seen):
    """Leaves out of an exception's traceback, its causes' and its group's
    the runtime's own frames."""
    if id(shown) in seen:
        return
    seen.add(id(shown))
    shown.stack = traceback.StackSummary.from_list(
        [frame for frame in shown.stack if not machinery(frame.filename)]
    )
    for other in (shown.__cause__, shown.__context__, *(getattr(shown, "exceptions", None) or ())):
        if other is not None:
            trimmed(other, seen)


def written(error, root, tb=None):
    """An exception as Python writes it, traceback, causes and notes, the
    runtime's own frames left out and the bundle's files named from the
    bundle's root, as their author named them. `tb` is its traceback where
    the exception does not hold it, as one the engine's adapter kept does
    not before Python 3.12."""
    shown = traceback.TracebackException(
        type(error), error, tb if tb is not None else error.__traceback__, compact=True
    )
    trimmed(shown, set())
    text = "".join(shown.format())
    return text.replace(os.path.join(root, ""), "").rstrip("\n")


def failed(error, tb, root):
    """A callback's exception as the job's failure: where it failed, which
    the engine's adapter noted on it, and the exception's own line; then
    the whole of it, its traceback `tb`, for the job's log."""
    notes = getattr(error, "__notes__", None) or ["the strategy failed"]
    message = f"{notes[-1]}: {line(error)}; its traceback is in the job's log"
    return message, written(error, root, tb)


class Lines(io.TextIOBase):
    """A stream whose every line goes to a job's log, through `say`: what a
    strategy prints, or writes to standard error. A line without its end
    waits for it, or for the job's end."""

    def __init__(self, say):
        super().__init__()
        self._say = say
        self._held = ""

    @property
    def encoding(self):
        return "utf-8"

    @property
    def errors(self):
        return "strict"

    def writable(self):
        return True

    def write(self, text):
        if not isinstance(text, str):
            raise TypeError(f"write() argument must be str, not {type(text).__name__}")
        self._held += text
        *done, self._held = self._held.split("\n")
        for each in done:
            self._say(each.removesuffix("\r"))
        while len(self._held) > LINE_HELD:
            self._say(self._held[:LINE_HELD])
            self._held = self._held[LINE_HELD:]
        return len(text)

    def finish(self):
        """Says the last line, where it has no end."""
        if self._held:
            self._say(self._held)
            self._held = ""


def capture(out, err):
    """Sends what is written to standard output to `out`, and to standard
    error to `err`, a line at a time, and gives nothing to a read of
    standard input: the worker's own streams carry its frames. Answers the
    two streams, to be finished as the job ends."""
    sys.stdout, sys.stderr = Lines(out), Lines(err)
    sys.stdin = io.StringIO()
    return sys.stdout, sys.stderr


class Guard:
    """What a bundle's own code may import: the standard library, its own
    modules, `quantforge_py`, and the packages its manifest declares. Any
    other import it asks for is refused, by `import` and by `importlib`,
    with what to do instead. What `quantforge_py` imports for it, as
    `CandleBatch.to_numpy` imports NumPy, is held to the same rules; what
    the other modules it may import go on to import is theirs.

    It is best effort: code that goes around the import system, or reads
    `sys.modules`, is not stopped here. The worker's sandbox is the
    boundary."""

    def __init__(self, own, declared, offered):
        """`own`: the bundle's top-level modules; `declared`: what its
        manifest's `deps` pins, by distribution name; `offered`: what the
        server offers, by distribution name, at the version it holds."""
        self.own = frozenset(own)
        self.offered = dict(offered)
        allowed = set(sys.stdlib_module_names) | self.own | {SDK}
        # A module's top-level name, for each distribution that installs it.
        self.offered_as = {}
        for top, names in installed().items():
            for name in map(normalized, names):
                if name in declared:
                    allowed.add(top)
                if name in self.offered:
                    self.offered_as.setdefault(top, name)
        self.allowed = frozenset(allowed)
        self._original = None
        self._import_module = None

    def install(self):
        self._original = builtins.__import__
        self._import_module = importlib.import_module
        builtins.__import__ = self.importing
        importlib.import_module = self.import_module
        sys.meta_path.insert(0, self)

    def uninstall(self):
        if self._original is not None:
            builtins.__import__ = self._original
            importlib.import_module = self._import_module
            self._original = self._import_module = None
        if self in sys.meta_path:
            sys.meta_path.remove(self)

    def from_bundle(self, frame):
        """Whether an import the code of `frame` asks for is the bundle's:
        asked for by one of its own modules, or by `quantforge_py` for one,
        whatever stands between."""
        while frame is not None:
            name = (frame.f_globals.get("__name__") or "").partition(".")[0]
            if name in self.own:
                return True
            if name != SDK:
                return False
            frame = frame.f_back
        return False

    def check(self, name):
        """Refuses `name` unless the bundle may import it."""
        top = name.partition(".")[0]
        if top in self.allowed:
            return
        package = self.offered_as.get(top)
        if package is not None:
            raise ImportError(
                f"the bundle imports {top}, which its manifest does not declare: add "
                f'{package} = "{self.offered[package]}" to [deps] in quantforge.toml',
                name=top,
            )
        offers = ", ".join(f"{each} {version}" for each, version in sorted(self.offered.items()))
        raise ImportError(
            f"the bundle imports {top}, which this server does not offer: a bundle imports the "
            "standard library, its own modules, quantforge_py and the packages its manifest "
            f"declares, and this server offers {offers or 'no package beside those'}",
            name=top,
        )

    def importing(self, name, globals=None, locals=None, fromlist=(), level=0):
        """`builtins.__import__`, which every `import` statement calls:
        held to the rules where a module of the bundle asks."""
        if level == 0 and self.from_bundle(sys._getframe(1)):
            self.check(name)
        return self._original(name, globals, locals, fromlist, level)

    def import_module(self, name, package=None):
        """`importlib.import_module`, which answers a module already
        loaded without asking a finder: held to the rules where a module of
        the bundle asks."""
        if not name.startswith(".") and self.from_bundle(sys._getframe(1)):
            self.check(name)
        return self._import_module(name, package)

    def find_spec(self, name, path=None, target=None):
        """A finder ahead of every other, for an import `importlib` makes
        for the bundle's code of a module not loaded yet: held to the rules,
        and then left to the finders after it."""
        if path is None:
            frame = sys._getframe(1)
            while frame is not None and frame.f_globals.get("__name__") in MACHINERY:
                frame = frame.f_back
            if self.from_bundle(frame):
                self.check(name)
        return None


def own_modules(root):
    """The bundle's top-level modules: each `.py` file at its root, and each
    directory that holds one, at any depth."""
    own = set()
    for entry in os.scandir(root):
        if entry.is_file() and entry.name.endswith(".py"):
            own.add(entry.name.removesuffix(".py"))
        elif entry.is_dir() and any(
            name.endswith(".py") for _, _, names in os.walk(entry.path) for name in names
        ):
            own.add(entry.name)
    return own


class Bundle:
    """One bundle's strategy, as a host builds it from the bundle's files
    in `root`: `module` imported, `cls` taken from it and built from the
    parameters, `params` as JSON, the imports of the bundle's own code held
    to the packages its manifest's `deps` declares, of those the server
    `offered`. The file `entry` names is the one messages name."""

    def __init__(self, root, entry, module, cls, params, deps, offered):
        self.root = os.path.abspath(root)
        self.file = entry.rpartition(":")[0]
        self.module = module
        self.cls = cls
        self.params = json.loads(params)
        self.deps = {normalized(name): version for name, version in deps.items()}
        self.offered = dict(offered)
        self.guard = None

    def strategy(self):
        """The strategy, built: the bundle's root its working directory and
        on the import path, and its imports guarded, from now on. Raises
        `Refused`, with what the job's failure says, where the bundle cannot
        give one."""
        own = own_modules(self.root)
        taken = set(sys.stdlib_module_names) | set(installed())
        for name in sorted(own):
            if name == SDK or name in taken:
                raise Refused(
                    f"the bundle's module {name} has the name of a module this server's Python "
                    "already has: rename it"
                )
        self.guard = Guard(own, self.deps, self.offered)
        self.guard.install()
        sys.path.insert(0, self.root)
        os.chdir(self.root)
        try:
            module = importlib.import_module(self.module)
        except BaseException as error:
            raise Refused(
                f"the bundle's {self.file} could not be imported: {line(error)}; its traceback "
                "is in the job's log",
                written(error, self.root),
            ) from None
        found = getattr(module, self.cls, None)
        if found is None:
            raise Refused(f"the bundle's {self.file} has no {self.cls}")
        from quantforge_py import Strategy

        if not isinstance(found, type) or not issubclass(found, Strategy) or found is Strategy:
            raise Refused(
                f"{self.cls} in the bundle's {self.file} is not a strategy: make it a class "
                "derived from quantforge_py.Strategy"
            )
        try:
            return found(**self.params)
        except BaseException as error:
            raise Refused(
                f"{self.cls} could not be built from the bundle's params: {line(error)}; its "
                "traceback is in the job's log",
                written(error, self.root),
            ) from None

    def leave(self):
        """Takes the guard and the bundle's root off the import path: for a
        host that goes on after the bundle, as the tests do."""
        if self.guard is not None:
            self.guard.uninstall()
        if self.root in sys.path:
            sys.path.remove(self.root)
