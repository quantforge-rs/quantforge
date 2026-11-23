"""The runtime a strategy bundle runs in, in this process: a bundle's
strategy imported from its files and built from its parameters, what its
own code may import, what its failures say, and the lines it writes
(docs/python-worker.md). A server's worker runs the same, which
`test_worker_jobs.py` holds it to through a real server."""

import importlib
import importlib.metadata
import os
import sys

import pytest

import quantforge_py as qf
from quantforge_py import _runtime

STRATEGY = """
import quantforge_py as qf


class Hold(qf.Strategy):
    def __init__(self, window=3, label="hold"):
        self.window = window
        self.label = label

    def on_bar(self, ctx, bar):
        return None
"""


def offered() -> dict:
    """What a server offers of the packages a bundle may import, in this
    environment."""
    return _runtime.prepare(["numpy", "pandas"])["packages"]


class Loaded:
    """Bundles built in `tmp_path`, each in a directory of its own, and
    everything a bundle changes as it is built taken back as the test ends:
    its guard, its root on the import path, its modules, and the working
    directory."""

    def __init__(self, tmp_path, monkeypatch):
        self.tmp_path = tmp_path
        self.bundles = []
        self.count = 0
        # The working directory the bundle moves to comes back.
        monkeypatch.chdir(tmp_path)

    def bundle(self, files, entry="strategy.py:Hold", *, params="{}", deps=None, offers=None):
        # One bundle at a time, as a worker holds one.
        self.leave()
        self.count += 1
        root = self.tmp_path / f"bundle-{self.count}"
        for path, text in files.items():
            (root / path).parent.mkdir(parents=True, exist_ok=True)
            (root / path).write_text(text.lstrip("\n"), encoding="utf-8")
        module = entry.rpartition(":")[0].removesuffix(".py").removesuffix("/__init__")
        bundle = _runtime.Bundle(
            str(root),
            entry,
            module.replace("/", "."),
            entry.rpartition(":")[2],
            params,
            deps or {},
            offered() if offers is None else offers,
        )
        self.bundles.append(bundle)
        return bundle

    def strategy(self, files, entry="strategy.py:Hold", **given):
        return self.bundle(files, entry, **given).strategy()

    def refused(self, files, entry="strategy.py:Hold", **given) -> _runtime.Refused:
        with pytest.raises(_runtime.Refused) as caught:
            self.strategy(files, entry, **given)
        return caught.value

    def leave(self):
        for bundle in self.bundles:
            bundle.leave()
            # The bundle's own modules, and never one of the same name that
            # this process has from elsewhere.
            inside = os.path.join(bundle.root, "")
            for name, module in list(sys.modules.items()):
                if (getattr(module, "__file__", None) or "").startswith(inside):
                    del sys.modules[name]
        self.bundles.clear()


@pytest.fixture
def loaded(tmp_path, monkeypatch):
    held = Loaded(tmp_path, monkeypatch)
    yield held
    held.leave()


def test_a_bundles_strategy_is_built_from_its_entry_and_its_params(loaded):
    files = {"pkg/__init__.py": "", "pkg/strategy.py": STRATEGY}
    bundle = loaded.bundle(files, "pkg/strategy.py:Hold", params='{"window": 5, "label": "x"}')
    strategy = bundle.strategy()
    assert type(strategy).__name__ == "Hold"
    assert isinstance(strategy, qf.Strategy)
    assert (strategy.window, strategy.label) == (5, "x")
    # The bundle's root is where its code runs, and what it imports from.
    assert os.getcwd() == bundle.root
    assert sys.path[0] == bundle.root
    # A package's `__init__.py` names the package itself.
    package = loaded.strategy(
        {"pack/__init__.py": STRATEGY}, "pack/__init__.py:Hold", params='{"window": 7}'
    )
    assert package.window == 7


def test_a_bundle_imports_the_standard_library_its_own_modules_and_quantforge_py(loaded):
    files = {}
    files["strategy.py"] = (
        "from __future__ import annotations\n"
        "import asyncio, collections, decimal, json, statistics\n"
        "import importlib\n"
        "import quantforge_py as qf\n"
        "from quantforge_py import _native\n"
        "from helpers import window\n"
        "from tools.maths import mean\n"
        "statistics_module = importlib.import_module('statistics')\n"
        "class Hold(qf.Strategy):\n"
        "    def on_bar(self, ctx, bar):\n"
        "        import fractions\n"
        "        return None\n"
    )
    files["helpers.py"] = "window = 3\n"
    files["tools/__init__.py"] = ""
    files["tools/maths.py"] = "from . import more\nfrom .more import total\nmean = None\n"
    files["tools/more.py"] = "total = 0\n"
    strategy = loaded.strategy(files)
    # An import inside a callback is held to the same rules, and allowed.
    assert strategy.on_bar(None, None) is None


@pytest.mark.skipif(importlib.util.find_spec("numpy") is None, reason="NumPy is not installed")
def test_a_bundle_imports_a_package_its_manifest_declares_and_what_that_imports_is_its_own(
    loaded,
):
    version = importlib.metadata.version("numpy")
    files = {
        "strategy.py": "import numpy\n" + STRATEGY,
    }
    loaded.strategy(files, deps={"numpy": version})
    if importlib.util.find_spec("pandas") is not None:
        # pandas imports packages of its own, dateutil among them, which no
        # bundle declares: they are pandas', and allowed.
        pandas = importlib.metadata.version("pandas")
        loaded.strategy({"strategy.py": "import pandas\n" + STRATEGY}, deps={"pandas": pandas})


@pytest.mark.skipif(importlib.util.find_spec("numpy") is None, reason="NumPy is not installed")
def test_a_package_on_offer_that_the_manifest_does_not_declare_is_refused_with_what_to_add(
    loaded,
):
    import numpy  # noqa: F401  # loaded already, as a worker loads it ahead of jobs

    version = importlib.metadata.version("numpy")
    said = (
        f"the bundle imports numpy, which its manifest does not declare: add numpy = "
        f'"{version}" to [deps] in quantforge.toml'
    )
    for statement in ("import numpy", "import numpy.linalg", "from numpy import linalg"):
        refused = loaded.refused({"strategy.py": f"{statement}\n" + STRATEGY})
        assert refused.message == (
            f"the bundle's strategy.py could not be imported: ImportError: {said}; its "
            "traceback is in the job's log"
        )
        assert refused.traceback.splitlines() == [
            "Traceback (most recent call last):",
            '  File "strategy.py", line 1, in <module>',
            f"    {statement}",
            f"ImportError: {said}",
        ]
    # Inside a callback, as an import statement or through importlib.
    for inside in ("import numpy", "importlib.import_module('numpy')"):
        strategy = loaded.strategy(
            {
                "strategy.py": "import importlib\nimport quantforge_py as qf\n"
                "class Hold(qf.Strategy):\n"
                "    def on_bar(self, ctx, bar):\n"
                f"        {inside}\n"
            }
        )
        with pytest.raises(ImportError) as caught:
            strategy.on_bar(None, None)
        assert str(caught.value) == said


def test_a_package_the_server_does_not_offer_is_refused_however_it_is_imported(loaded):
    offers = offered()
    shown = ", ".join(f"{name} {version}" for name, version in sorted(offers.items()))
    for name in ("pytest", "a_module_no_one_installed"):
        said = (
            f"the bundle imports {name}, which this server does not offer: a bundle imports the "
            "standard library, its own modules, quantforge_py and the packages its manifest "
            f"declares, and this server offers {shown or 'no package beside those'}"
        )
        refused = loaded.refused({"strategy.py": f"import {name}\n" + STRATEGY})
        assert refused.message.endswith(f"ImportError: {said}; its traceback is in the job's log")
    # importlib asks the same rules of a module not loaded yet, as every
    # module but the package's own is in a worker as its job starts.
    refused = loaded.refused(
        {"strategy.py": "import importlib\nimportlib.import_module('a_module_no_one_installed')\n"
         + STRATEGY}
    )
    assert "the bundle imports a_module_no_one_installed, which this server does not" in (
        refused.message
    )
    assert refused.traceback.splitlines()[1:3] == [
        '  File "strategy.py", line 2, in <module>',
        "    importlib.import_module('a_module_no_one_installed')",
    ]
    # With nothing on offer, the refusal says so.
    refused = loaded.refused({"strategy.py": "import pytest\n" + STRATEGY}, offers={})
    assert refused.message.endswith(
        "and this server offers no package beside those; its traceback is in the job's log"
    )


@pytest.mark.skipif(importlib.util.find_spec("numpy") is None, reason="NumPy is not installed")
def test_what_quantforge_py_imports_for_a_bundle_is_held_to_the_bundles_rules(loaded):
    # A batch's arrays are NumPy's: a bundle that asks for them uses NumPy,
    # and has to declare it, whether the package imports it from Rust or
    # from its own Python.
    source = (
        "import quantforge_py as qf\n"
        "class Hold(qf.Strategy):\n"
        "    def on_bar(self, ctx, bar):\n"
        "        batch = qf.CandleBatch.from_candles([bar])\n"
        "        return batch.to_numpy()\n"
    )
    candle = qf.Candle(
        open_time_ms=0, close_time_ms=59_999, open=1, high=1, low=1, close=1, volume=1
    )
    undeclared = loaded.strategy({"strategy.py": source})
    with pytest.raises(ImportError, match="the bundle imports numpy, which its manifest does"):
        undeclared.on_bar(None, candle)
    # Declared, it is the bundle's to use; and what the standard library
    # and the package import for themselves is theirs.
    declared = loaded.strategy(
        {"strategy.py": source}, deps={"numpy": importlib.metadata.version("numpy")}
    )
    assert set(declared.on_bar(None, candle)) >= {"open_time_ms", "close"}


def test_a_bundle_module_named_like_one_the_server_has_is_refused(loaded):
    # The standard library's, the package's own, and an installed one's.
    names = ["json", "quantforge_py", "pytest"]
    if importlib.util.find_spec("numpy") is not None:
        names.append("numpy")
    for name in names:
        refused = loaded.refused({"strategy.py": STRATEGY, f"{name}.py": ""})
        assert refused.message == (
            f"the bundle's module {name} has the name of a module this server's Python already "
            "has: rename it"
        )
        assert refused.traceback == ""


def test_an_entry_that_names_no_strategy_is_refused(loaded):
    refused = loaded.refused({"strategy.py": STRATEGY}, "strategy.py:Missing")
    assert refused.message == "the bundle's strategy.py has no Missing"
    for source in (
        "Hold = 3\n",
        "class Hold:\n    def on_bar(self, ctx, bar):\n        return None\n",
        "from quantforge_py import Strategy as Hold\n",
    ):
        refused = loaded.refused({"strategy.py": source})
        assert refused.message == (
            "Hold in the bundle's strategy.py is not a strategy: make it a class derived from "
            "quantforge_py.Strategy"
        )


def test_a_strategy_its_params_cannot_build_is_refused_with_its_traceback(loaded):
    # A name like none the class takes, which no Python suggests another for.
    refused = loaded.refused({"strategy.py": STRATEGY}, params='{"speed": 5}')
    assert refused.message == (
        "Hold could not be built from the bundle's params: TypeError: Hold.__init__() got an "
        "unexpected keyword argument 'speed'; its traceback is in the job's log"
    )
    assert refused.traceback.splitlines()[-1] == (
        "TypeError: Hold.__init__() got an unexpected keyword argument 'speed'"
    )
    # A class without on_bar, which the SDK refuses to build.
    refused = loaded.refused(
        {"strategy.py": "import quantforge_py as qf\nclass Hold(qf.Strategy):\n    pass\n"}
    )
    assert refused.message == (
        "Hold could not be built from the bundle's params: TypeError: Hold cannot be built: it "
        "does not define on_bar; its traceback is in the job's log"
    )


def test_a_module_that_fails_as_it_is_imported_says_so_with_its_own_frames(loaded):
    refused = loaded.refused({"strategy.py": "x = 1\nraise ValueError('no data')\n"})
    assert refused.message == (
        "the bundle's strategy.py could not be imported: ValueError: no data; its traceback is "
        "in the job's log"
    )
    assert refused.traceback.splitlines() == [
        "Traceback (most recent call last):",
        '  File "strategy.py", line 2, in <module>',
        "    raise ValueError('no data')",
        "ValueError: no data",
    ]
    refused = loaded.refused({"strategy.py": "def broken(:\n"})
    assert refused.message.startswith(
        "the bundle's strategy.py could not be imported: SyntaxError: "
    )
    assert '  File "strategy.py", line 1' in refused.traceback.splitlines()


def test_a_callbacks_failure_is_worded_where_it_failed_with_its_traceback(tmp_path):
    root = str(tmp_path / "bundle")
    path = os.path.join(root, "strategy.py")
    code = compile("def on_bar():\n    return 1 / 0\n", path, "exec")
    scope = {}
    exec(code, scope)
    try:
        scope["on_bar"]()
    except ZeroDivisionError as error:
        error.add_note("the strategy 'Hold' failed in on_bar, at the bar that opened at 60000")
        message, written = _runtime.failed(error, error.__traceback__, root)
    assert message == (
        "the strategy 'Hold' failed in on_bar, at the bar that opened at 60000: "
        "ZeroDivisionError: division by zero; its traceback is in the job's log"
    )
    lines = written.splitlines()
    assert lines[0] == "Traceback (most recent call last):"
    assert '  File "strategy.py", line 2, in on_bar' in lines
    assert lines[-2:] == [
        "ZeroDivisionError: division by zero",
        "the strategy 'Hold' failed in on_bar, at the bar that opened at 60000",
    ]
    # An exception of a module's own is named with its module.
    assert _runtime.line(_runtime.Refused("x")) == "quantforge_py._runtime.Refused: x"
    assert _runtime.line(KeyError()) == "KeyError"


def test_what_a_strategy_writes_goes_to_the_log_a_line_at_a_time():
    said = []
    lines = _runtime.Lines(said.append)
    assert lines.write("one\ntwo") == 7
    assert said == ["one"]
    lines.write(" more\r\nthree\n")
    assert said == ["one", "two more", "three"]
    lines.write("last")
    lines.finish()
    lines.finish()
    assert said[-1] == "last" and len(said) == 4
    # A line that never ends is said in pieces, never held without bound.
    lines.write("x" * (2 * _runtime.LINE_HELD + 5))
    assert said[-2:] == ["x" * _runtime.LINE_HELD] * 2
    lines.finish()
    assert said[-1] == "xxxxx"
    with pytest.raises(TypeError):
        lines.write(b"bytes")
    assert (lines.encoding, lines.writable(), lines.isatty()) == ("utf-8", True, False)


def test_a_worker_offers_its_python_what_it_holds_of_the_packages_and_its_own_files():
    found = _runtime.prepare(["numpy", "pandas", "a-package-no-one-installed"])
    assert found["python"] == "%d.%d" % sys.version_info[:2]
    for name in ("numpy", "pandas"):
        if importlib.util.find_spec(name) is None:
            assert name not in found["packages"]
        else:
            assert found["packages"][name] == importlib.metadata.version(name)
    assert "a-package-no-one-installed" not in found["packages"]
    readable = found["readable"]
    assert readable == sorted(set(readable))
    for directory in (sys.prefix, sys.base_prefix):
        assert os.path.abspath(directory) in readable
    package = os.path.dirname(os.path.dirname(os.path.abspath(qf.__file__)))
    assert any(package == path or package.startswith(path + os.sep) for path in readable)
    assert all(os.path.exists(path) for path in readable)
