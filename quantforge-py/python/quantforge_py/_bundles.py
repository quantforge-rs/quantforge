"""A strategy bundle as a client makes one before it submits a job
(`docs/python-bundles.md`): the files a caller names, the manifest written
for them where they hold none, and `BundleError`, which the package
publishes, for a bundle that is refused before anything of it is sent.

- **A bundle is collected, checked and packed by the engine's own code,**
  `quantforge_py._native._bundle`: the code a server checks an upload with.
  The same files give the same bundle, the same code hash and the same
  refusal on both sides, whatever the Python that runs the client.
- **A manifest is written by the writer every client shares,**
  `quantforge_py._native._manifest`, in its one form. The same entry,
  parameters and packages write the same manifest here as the command
  line's `backtest --remote` writes for the same flags, and so make the
  same bundle.
- **Nothing is pickled, and nothing is run.** A bundle is files and a
  manifest. A parameter is a value JSON holds, written into the manifest
  as TOML writes it, and no file of a strategy is imported or executed
  here.
- **Everything a caller named is read and checked before a server is
  asked anything,** and again, as it is then, for the server's runtime:
  what is uploaded is what was checked last.

This module is private: import its names from `quantforge_py`.
"""

from . import _native
from ._errors import _published
from ._wire import _wrong_type

# The manifest's path, at a bundle's root.
_MANIFEST = "quantforge.toml"

# What a written manifest is checked with before the server's runtime is
# known: any Python version does, since only admission reads it, and any
# version of a package that is named without one.
_PROVISIONAL_PYTHON = "3.11"
_PROVISIONAL_PIN = "0"

# The largest whole number a parameter holds, so that every JSON reader
# reads it exactly, and how deep a manifest's values may lie: the bundle
# spec's own, which the tests hold these to.
_MOST_SAFE_INTEGER = 2**53 - 1
_MOST_DEPTH = 32

# What a parameter is, as a refusal says it.
_PARAM = "a str, a bool, an int, a float, a list or a dict"

# The extensions a bundle's files have beside `.py`, which no entry names.
_OTHER_EXTENSIONS = frozenset({"toml", "json", "txt"})

# The spec's codes for a bundle a server cannot run, as against one that
# breaks the spec wherever it is sent.
_ADMISSION = frozenset(
    {
        "dependency_not_allowed",
        "dependency_version_unavailable",
        "engine_mismatch",
        "python_mismatch",
    }
)


@_published
class BundleError(ValueError):
    """A strategy bundle that is refused before anything of it is sent: it
    breaks the bundle spec, or the server it was made for cannot run it.

    `str()` of one says every problem found, each with what to change.

    - `problems`: the problems, in the order the bundle spec checks them,
      as a tuple of pairs: the spec's code, such as `"file_too_large"` or
      `"dependency_not_allowed"`, and its message. The codes are the
      spec's contract; a message is for the person fixing the bundle.

    It is a `ValueError`: what was given cannot be submitted as it is.
    """

    def __init__(self, message, problems=()):
        super().__init__(message)
        self.problems = tuple((code, said) for code, said in problems)


def _refused(problems, server=None):
    """The exception a refused bundle is raised as. One that a server's
    runtime refuses, and the spec does not, says which server."""
    cannot_run = server is not None and all(code in _ADMISSION for code, _ in problems)
    lead = (
        f"the server at {server} cannot run the bundle" if cannot_run else "the bundle is refused"
    )
    if len(problems) == 1:
        return BundleError(f"{lead}: {problems[0][1]}", problems)
    listed = "".join(f"\n  - {said}" for _, said in problems)
    return BundleError(f"{lead} for {len(problems)} reasons:{listed}", problems)


def _shown_key(key):
    """A key as a refusal names it in a path: bare where TOML writes it
    bare."""
    if key and all(char.isascii() and (char.isalnum() or char in "_-") for char in key):
        return key
    quoted = key.replace("\\", "\\\\").replace('"', '\\"')
    return f'"{quoted}"'


def _text(value, at):
    """Text a manifest holds: any `str` but one with a lone surrogate, which
    is no text at all once it is written."""
    try:
        value.encode("utf-8")
    except UnicodeEncodeError:
        raise ValueError(f"{at} holds a lone surrogate, which no file holds as text") from None
    return value


def _named(name):
    """A file's name as a bundle's path. One that is no text, as a file
    system may hand one over, is named as the spec's own walk of a directory
    names it, with a mark where its bytes are none, for the path rules to
    refuse."""
    return "".join("\ufffd" if "\ud800" <= char <= "\udfff" else char for char in name)


def _param(value, at, depth):
    """A parameter as a manifest holds one, copied: a value of exactly one
    of the types JSON has, and nothing that merely converts to one. `depth`
    is the level a list or a dict lies at in the manifest."""
    kind = type(value)
    if kind is str:
        return _text(value, at)
    if kind is bool:
        return value
    if kind is int:
        if abs(value) > _MOST_SAFE_INTEGER:
            raise ValueError(
                f"{at} is an integer beyond 2^53 - 1, which not every JSON reader holds exactly; "
                "write it as a str"
            )
        return value
    if kind is float:
        if value != value or value in (float("inf"), float("-inf")):
            raise ValueError(f"{at} is not a finite number")
        return value
    if kind is list or kind is dict:
        if depth > _MOST_DEPTH:
            raise ValueError(
                f"{at} lies more than {_MOST_DEPTH} levels deep in the manifest, whose own table "
                "is the first level and params the second"
            )
        if kind is dict:
            return _table(value, at, depth)
        return [_param(item, f"{at}[{index}]", depth + 1) for index, item in enumerate(value)]
    raise TypeError(
        f"{at} must be {_PARAM}, not {kind.__name__}: a strategy's parameters reach it as JSON, "
        "and an amount that must stay exact goes in a str"
    )


def _table(table, at, depth):
    """A dict of parameters, copied: every key a `str` that does not start
    with `$`, which the spec reserves."""
    copied = {}
    for key, value in table.items():
        if type(key) is not str:
            raise TypeError(f"a key of {at} must be a str, not {type(key).__name__}")
        named = f"{at}.{_shown_key(_text(key, f'a key of {at}'))}"
        if key.startswith("$"):
            raise ValueError(f"{named} starts with $, which is reserved")
        copied[key] = _param(value, named, depth + 1)
    return copied


def _checked_params(params):
    """A strategy's parameters as a caller gives them, checked and copied:
    a dict, or `None` for none."""
    if params is None:
        return {}
    if type(params) is not dict:
        raise _wrong_type("params", "a dict or None", params)
    return _table(params, "params", 2)


def _checked_deps(deps):
    """The packages a strategy imports as a caller names them, each with
    the version it pins, or with `None` where only its name was given and
    the server's version is to be pinned."""
    if deps is None:
        return {}
    if type(deps) is dict:
        pins = {}
        for name, version in deps.items():
            if type(name) is not str:
                raise TypeError(f"a key of deps must be a str, not {type(name).__name__}")
            _text(name, "a key of deps")
            if type(version) is not str:
                raise TypeError(
                    f"deps[{name!r}] must be a str, the version to pin, not "
                    f"{type(version).__name__}"
                )
            pins[name] = _text(version, f"deps[{name!r}]")
        return pins
    if type(deps) is list or type(deps) is tuple:
        names = {}
        for index, name in enumerate(deps):
            if type(name) is not str:
                raise TypeError(
                    f"deps[{index}] must be a str, a package's name, not {type(name).__name__}"
                )
            if _text(name, f"deps[{index}]") in names:
                raise ValueError(f"deps names {name} twice")
            names[name] = None
        return names
    raise _wrong_type(
        "deps",
        "a dict of each package's version, a list or a tuple of package names, or None",
        deps,
    )


def _checked_entry(entry, example):
    """A strategy's entry as a caller names it: text that names a `.py`
    file and a class, as a manifest does. The rest of what an entry is, the
    bundle's own check says."""
    if type(entry) is not str:
        raise _wrong_type("entry", "a str", entry)
    _text(entry, "entry")
    file, colon, named = entry.partition(":")
    if colon and file.endswith(".py"):
        return entry
    parts = file.split(".")
    is_a_module = all(part.isascii() and part.isidentifier() for part in parts)
    # A file of another kind is no module, though it reads like one.
    if colon and named and is_a_module and parts[-1] not in _OTHER_EXTENSIONS:
        # A module and a class, as Python imports them.
        written = f"{'/'.join(parts)}.py:{named}"
        raise ValueError(
            f"entry {entry!r} names a module: name the strategy's file and its class, as a "
            f"bundle's manifest does, such as {written!r}"
        )
    raise ValueError(
        f"entry {entry!r} names no .py file and class: write the strategy's file, a colon and "
        f"its class, such as {example!r}"
    )


def _same(one, other):
    """Whether two values of parameters are the same value of the same
    type, as a strategy would be handed them: `1`, `1.0` and `True` are
    three values, and so are the two zeros of a float."""
    if type(one) is not type(other):
        return False
    if type(one) is dict:
        return one.keys() == other.keys() and all(_same(one[key], other[key]) for key in one)
    if type(one) is list:
        return len(one) == len(other) and all(map(_same, one, other))
    if type(one) is float:
        return one == other and str(one) == str(other)
    return one == other


class _Draft:
    """A strategy as a caller named it, read and checked before any server
    is asked anything, and ready to be built for a server's runtime: its
    files, in a directory or in memory, and, where they hold no manifest,
    what one is written from."""

    __slots__ = ("_root", "_files", "_written")

    def __init__(self, root, files, written):
        self._root = root
        self._files = files
        # What a manifest is written from: the entry, the parameters and
        # the packages; `None` where the files hold a manifest of their own.
        self._written = written
        self.built()

    @classmethod
    def of_path(cls, strategy, entry, params, deps):
        """What `strategy` names: a `.py` file, bundled beside a manifest
        written from `entry`, `params` and `deps`, or a directory, bundled
        as the spec collects one, with its own `quantforge.toml` where it
        holds one."""
        import os
        import stat

        try:
            path = os.fspath(strategy)
        except TypeError:
            path = None
        if not isinstance(path, str):
            raise _wrong_type("strategy", "a str or an os.PathLike", strategy)
        if not path:
            raise ValueError(
                "strategy is empty: name a strategy's .py file, or the directory its files are in"
            )
        try:
            mode = os.stat(path).st_mode
        except FileNotFoundError:
            raise FileNotFoundError(
                f"there is no file or directory at {path}: strategy names a strategy's .py file, "
                "or the directory its files are in"
            ) from None
        given = [
            name
            for name, value in (("entry", entry), ("params", params), ("deps", deps))
            if value is not None
        ]
        if stat.S_ISDIR(mode):
            own = os.path.join(path, _MANIFEST)
            if os.path.lexists(own):
                if given:
                    raise ValueError(
                        f"{own} names the strategy's entry, its params and its deps: leave out "
                        f"{' and '.join(given)}, or remove the file to have it written from them"
                    )
                return cls(path, [], None)
            if entry is None:
                raise ValueError(
                    'entry must name the strategy, such as entry="strategy.py:MyStrategy": '
                    f"{path} holds no {_MANIFEST} that names it"
                )
            written = cls._written_from(entry, params, deps, "strategy.py:MyStrategy")
            return cls(path, [], written)
        if not stat.S_ISREG(mode):
            raise ValueError(
                f"{path} is neither a file nor a directory: strategy names a strategy's .py "
                "file, or the directory its files are in"
            )
        name = _named(os.path.basename(path))
        # A file is the one Python file of its bundle, which the entry
        # names: a file of another kind is no strategy to bundle alone.
        if not name.endswith(".py"):
            raise ValueError(
                f"{path} is no .py file: strategy names a strategy's .py file, or the directory "
                "its files are in"
            )
        example = f"{name}:MyStrategy"
        if entry is None:
            raise ValueError(
                f"entry must name the strategy, such as entry={example!r}: a file holds no "
                f"{_MANIFEST} that names it"
            )
        written = cls._written_from(entry, params, deps, example)
        with open(path, "rb") as file:
            return cls(None, [(name, file.read())], written)

    @classmethod
    def of_files(cls, files, entry, params, deps):
        """A bundle of `files` given in memory, each a path and its bytes,
        beside a manifest written from `entry`, `params` and `deps`."""
        written = cls._written_from(entry, params, deps, "strategy.py:MyStrategy")
        return cls(None, list(files), written)

    @staticmethod
    def _written_from(entry, params, deps, example):
        return _checked_entry(entry, example), _checked_params(params), _checked_deps(deps)

    def _pinned(self, offered, server):
        """The packages a written manifest declares, each with its version:
        the one given, or, for a package named alone, the one the server
        offers. One it does not offer keeps a version no server has, for
        the admission check to refuse in its own words."""
        pins = {}
        for name, version in self._written[2].items():
            if version is None:
                versions = (offered or {}).get("packages", {}).get(name, ())
                if len(versions) > 1:
                    raise ValueError(
                        f"deps names {name} without a version, and the server at {server} offers "
                        f"{' and '.join(versions)} of it: pin one, as in "
                        f"deps={{{name!r}: {versions[-1]!r}}}"
                    )
                version = versions[0] if versions else _PROVISIONAL_PIN
            pins[name] = version
        return pins

    def built(self, offered=None, answer=None, server=None):
        """The bundle, built and checked as it is now: for the runtime
        `offered`, which `answer` is the server's own answer of, or, with
        neither, for no server in particular. A `BundleError` where it is
        refused; else its code hash, its canonical tar and what its
        manifest says."""
        files = list(self._files)
        if self._written is not None:
            entry, params, _ = self._written
            python = _PROVISIONAL_PYTHON if offered is None else offered["python"]
            text = _native._manifest(entry, python, params, self._pinned(offered, server))
            files.append((_MANIFEST, text.encode("utf-8")))
        passed, problems = _native._bundle(self._root, files, None, answer)
        if passed is None:
            raise _refused(problems, server)
        if self._written is not None:
            import json

            # What the server will hand the strategy is what was given: the
            # manifest was written and read back by two pieces of code, and
            # a difference between them is nothing to send.
            if not _same(json.loads(passed["params"]), self._written[1]):
                raise RuntimeError(
                    "the manifest written for the bundle does not read back as the params it "
                    "was given, which is a fault of quantforge_py's own: nothing was sent"
                )
        return passed
