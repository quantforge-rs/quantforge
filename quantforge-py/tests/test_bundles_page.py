"""`docs/python-bundles.md`, held to the package: the two calls' signatures
and arguments, the limits, the manifest it says is written, what a
parameter is, the requests a submission makes in order, the file
`submit_class` writes, what is raised, every refusal it shows, and its
examples, which run against a real server that stores the day of candles
the page says it does, from a directory that holds the strategy it says it
submits.

What the two calls do beyond what the page shows is in `test_bundles.py`,
`test_source.py` and `test_submit.py`, and against a real server on either
backend in `test_submit_jobs.py`."""

import decimal
import doctest
import inspect
import linecache
import shutil
import textwrap

import pytest

import jobs
import quantforge_py as qf
import stores
from conftest import REMOTE_TOKEN
from quantforge_py import _bundles, _native, _source
from standin import Serving
from test_source import Notebook

PAGE = "docs/python-bundles.md"
SPEC = "docs/bundle-spec.md"
CLIENT_PAGE = "docs/python-client.md"
PACKAGE_PAGE = "docs/python-package.md"

# Where the page's examples say their server is.
SHOWN = "127.0.0.1:9600"


@pytest.fixture(scope="module")
def session(page, tmp_path_factory):
    """The page's examples, run as one session from top to bottom against a
    real server that stores the goldens' day of candles, in a directory
    that holds `my_strategy`, with that server's address in the place of
    the one the page shows. Gives what the run came to, what it said, how
    many examples it had, the names it left, and the server."""
    directory = tmp_path_factory.mktemp("bundles-page")
    for name in ("place", "scratch", "work"):
        (directory / name).mkdir()
    place = stores.Place.sqlite(directory / "place")
    with stores.Exchange({("BTCUSDT", "1m"): jobs.golden_klines()}) as exchange:
        place.sync(exchange)
    strategy = directory / "work" / "my_strategy"
    strategy.mkdir()
    shutil.copy(jobs.GOLDEN / "sma_cross.py", strategy / "sma_cross.py")
    examples = [example for example in page(PAGE).fences("python") if ">>> " in example]
    assert len(examples) >= 8
    with (
        jobs.served(place, directory / "scratch") as server,
        pytest.MonkeyPatch.context() as patched,
    ):
        patched.delenv(REMOTE_TOKEN, raising=False)
        patched.chdir(directory / "work")
        there = f"127.0.0.1:{server.port}"
        text = "\n".join(examples)
        assert text.count(SHOWN) == 2
        parsed = doctest.DocTestParser().get_doctest(text.replace(SHOWN, there), {}, PAGE, PAGE, 0)
        # Python keeps the text of what a session ran for as long as the
        # session lasts, which is how a class defined in it is found; the
        # tests below go on after it, and keep it themselves.
        kept = []
        for index, example in enumerate(parsed.examples):
            name = f"<doctest {parsed.name}[{index}]>"
            lines = example.source.splitlines(True)
            linecache.cache[name] = (len(example.source), None, lines, name)
            kept.append(name)
        said = []
        results = doctest.DocTestRunner(verbose=False).run(
            parsed, out=said.append, clear_globs=False
        )
        try:
            yield results, "".join(said), len(parsed.examples), parsed.globs, server
        finally:
            for name in kept:
                linecache.cache.pop(name, None)


def test_every_example_on_the_page_runs_as_written(session):
    results, said, examples, names, server = session
    assert examples > 20
    assert results.failed == 0, said
    assert results.attempted == examples
    # What the examples submitted is on the server of the test: the two
    # backtests of the directory and of its file, and the class's.
    client = qf.Client(f"http://127.0.0.1:{server.port}")
    listed = client.backtest.list()
    assert len(listed) == 3
    assert (
        names["job"].status.state == "succeeded"
        and names["again"].status.bundle != names["job"].status.bundle
    )
    by_id = {job.id: job for job in listed}
    assert by_id[names["again"].id].status.bundle == listed[-1].status.bundle
    # The first ran over the whole day the server stores, and made the
    # goldens' trades: the strategy the page says it submits.
    first = listed[-1]
    expected = jobs.golden()["result"]
    assert server.result(first.id)["metrics"] == {field: expected[field] for field in jobs.METRICS}


def called(function) -> str:
    """A method's signature as it is called, without `self`."""
    signature = inspect.signature(function)
    return str(signature.replace(parameters=list(signature.parameters.values())[1:]))


def test_the_page_gives_the_two_calls_as_python_does(page, quoted, session):
    *_, names, _ = session
    backtests = type(names["client"].backtest)
    lines = page(PAGE).section("signatures")
    assert (lines[0], lines[-1]) == ("```text", "```")
    assert lines[1:-1] == [
        f"Client.backtest.submit{called(backtests.submit)}",
        f"Client.backtest.submit_class{called(backtests.submit_class)}",
    ]
    # Every argument of either, once, in the order the calls take them, with
    # the call it is an argument of.
    taken = {
        "submit": list(inspect.signature(backtests.submit).parameters)[1:],
        "submit_class": list(inspect.signature(backtests.submit_class).parameters)[1:],
    }
    rows = [(quoted(argument), of) for argument, of, _ in page(PAGE).rows("arguments")]
    for call, arguments in taken.items():
        listed = [argument for argument, of in rows if of in ("both", f"`{call}`")]
        assert listed == arguments, call
    assert [argument for argument, of in rows if of == "both"] == [
        argument for argument in taken["submit"] if argument in taken["submit_class"]
    ]
    assert len(rows) == len({argument for argument, _ in rows}) == 10
    # What is required of a caller, and what is not.
    required = [
        name
        for name, parameter in inspect.signature(backtests.submit).parameters.items()
        if parameter.default is inspect.Parameter.empty and name != "self"
    ]
    assert required == ["strategy", "market", "config"]
    assert (
        inspect.signature(backtests.submit_class).parameters["imports"].default == _source._IMPORTS
    )
    assert inspect.signature(backtests.submit_class).parameters["source_only"].default is False
    # Each says what it is, to `help()` as on the page.
    for call in (backtests.submit, backtests.submit_class):
        said = " ".join(call.__doc__.split())
        for argument in inspect.signature(call).parameters:
            assert argument == "self" or f"`{argument}`" in said, (call.__name__, argument)
    assert "nothing is pickled" in " ".join(backtests.submit_class.__doc__.split())


def test_the_limits_on_the_page_are_the_bundle_specs(page, quoted):
    rows = page(PAGE).rows("limits")
    spec = {quoted(name): int(default) for name, default, _ in page(SPEC).rows("limits")}
    assert [quoted(name) for name, _ in rows] == [name for name in spec if name.startswith("max_")]
    for name, most in rows:
        assert most.startswith(f"{spec[quoted(name)]} "), name


def test_the_manifest_on_the_page_is_the_one_the_package_writes(page, session):
    lines = page(PAGE).section("written manifest")
    assert (lines[0], lines[-1]) == ("```toml", "```")
    shown = "".join(f"{line}\n" for line in lines[1:-1])
    written = _native._manifest(
        "sma_cross.py:SmaCross", "3.12", {"slow": 50, "fast": 20}, {"numpy": "2.4.6"}
    )
    assert shown == written
    assert f'engine = "=={qf.ENGINE_VERSION}"\n' in shown
    # And it is a bundle's manifest, for a server that runs that Python and
    # offers that NumPy: with it the strategy's file is admitted.
    with Serving(python="3.12", packages={"numpy": ["2.4.6"]}) as server:
        *_, names, _ = session
        job = qf.Client(server.url).backtest.submit(
            "my_strategy",
            entry="sma_cross.py:SmaCross",
            params={"fast": 20, "slow": 50},
            deps=["numpy"],
            market=names["market"],
            config=names["config"],
        )
        files = {
            "quantforge.toml": shown.encode(),
            "sma_cross.py": (jobs.GOLDEN / "sma_cross.py").read_bytes(),
        }
        assert job.status.bundle == jobs.code_hash(jobs.canonical_tar(files))


def test_the_page_lists_what_a_parameter_is_as_a_refusal_does(page, quoted):
    kinds = [quoted(kind) for kind, _ in page(PAGE).rows("params")]
    assert kinds == ["str", "bool", "int", "float", "list", "dict"]
    assert _bundles._PARAM == "a str, a bool, an int, a float, a list or a dict"
    # Each is taken, and nothing else is.
    values = {"str": "x", "bool": True, "int": 2, "float": 2.0, "list": [1], "dict": {"k": 1}}
    assert sorted(values) == sorted(kinds)
    for kind, value in values.items():
        assert _bundles._checked_params({"x": value}) == {"x": value}, kind
    for value in ((1, 2), None, decimal.Decimal(1), b"x", {1, 2}):
        with pytest.raises(TypeError):
            _bundles._checked_params({"x": value})
    said = page(PAGE).text
    assert f"at most {_bundles._MOST_DEPTH} levels deep" in said
    assert "within ±(2^53 − 1)" in said and _bundles._MOST_SAFE_INTEGER == 2**53 - 1


def test_the_page_lists_the_requests_of_a_submission_in_their_order(page, quoted, session):
    *_, names, _ = session
    rows = page(PAGE).rows("steps")
    assert [step for step, _, _ in rows] == [str(index) for index in range(1, len(rows) + 1)]
    asked = [request for _, request, _ in rows]
    assert asked[0] == "—"
    with Serving() as server:
        job = qf.Client(server.url).backtest.submit(
            "my_strategy",
            entry="sma_cross.py:SmaCross",
            market=names["market"],
            config=names["config"],
        )
        made = [route.replace(job.status.bundle, "{code_hash}") for route in server.routes]
    assert [quoted(request) for request in asked[1:]] == made
    # What the first step refuses asks nothing at all.
    with Serving() as server:
        with pytest.raises(qf.BundleError):
            qf.Client(server.url).backtest.submit(
                "my_strategy",
                entry="other.py:SmaCross",
                market=names["market"],
                config=names["config"],
            )
        assert server.asked == []


def test_the_file_on_the_page_is_the_one_submit_class_writes_for_the_class_it_shows(page, session):
    *_, names, _ = session
    lines = page(PAGE).section("class file")
    assert (lines[0], lines[-1]) == ("```python", "```")
    shown = "".join(f"{line}\n" for line in lines[1:-1])
    # The class of the page's own example, as the session defined it.
    (file, text), entry = _source._class_file(names["Momentum"], _source._IMPORTS, True)
    assert (file, entry) == ("strategy.py", "strategy.py:Momentum")
    assert text.decode() == shown
    assert shown.startswith(f"{_source._IMPORTS}\n\n\nclass Momentum(qf.Strategy):\n")
    assert f"`{file}`" in page(PAGE).text and f"`{entry}`" in page(PAGE).text
    # And as a notebook's cell defines the same text.
    notebook = Notebook()
    try:
        notebook.run(_source._IMPORTS)
        cell = notebook.run(shown.split("\n\n\n", 1)[1]).Momentum
        assert _source._class_file(cell, _source._IMPORTS, True)[0][1].decode() == shown
    finally:
        notebook.close()
    # What the page says of a call without the caller's word is what such
    # a call is told, word for word.
    assert " ".join(_source._CAVEATS.split()) in " ".join(page(PAGE).text.split())


def test_the_page_names_what_is_raised_and_a_bundle_error_is_what_it_says(page, quoted):
    rows = page(PAGE).rows("raises")
    kinds = [quoted(kind) for kind, _ in rows]
    assert kinds == [
        "TypeError",
        "ValueError",
        "FileNotFoundError",
        "BundleError",
        "ServerUnavailable",
        "ValidationError",
        "RemoteError",
    ]
    for kind in kinds:
        raised = getattr(qf, kind, None) or getattr(__import__("builtins"), kind)
        assert isinstance(raised, type) and issubclass(raised, Exception), kind
    assert issubclass(qf.BundleError, ValueError) and not issubclass(qf.BundleError, qf.RemoteError)
    # The codes the page quotes are the spec's.
    codes = [quoted(code) for code, _ in page(SPEC).rows("problem codes")]
    for code in ("file_too_large", "entry_not_found", "dependency_not_allowed"):
        assert code in codes and f"`{code}`" in page(PAGE).text
    said = " ".join(qf.BundleError.__doc__.split())
    assert "`problems`" in said and "`ValueError`" in said


def test_every_refusal_on_the_page_is_raised_as_written(page, session, quoted):
    *_, names, server = session
    rows = page(PAGE).rows("refusals")
    assert len(rows) >= 25
    names = {**names, "Decimal": decimal.Decimal}
    before = len(qf.Client(f"http://127.0.0.1:{server.port}").backtest.list())
    shown = set()
    for call, raises in rows:
        call, raises = quoted(call), quoted(raises)
        kind, _, message = raises.partition(": ")
        with pytest.raises(Exception) as caught:
            eval(call, names)
        assert (type(caught.value).__name__, str(caught.value)) == (kind, message), call
        shown.add(kind)
    assert shown == {"TypeError", "ValueError", "FileNotFoundError", "BundleError"}
    # None of them reached the server as a job.
    assert len(qf.Client(f"http://127.0.0.1:{server.port}").backtest.list()) == before


def test_what_the_page_leaves_to_later_stories_is_what_the_other_pages_do(page):
    def left(name: str) -> dict[str, str]:
        read = page(name)
        at = read.lines.index("## What is left")
        rows = [
            [cell.strip() for cell in line.strip().strip("|").split("|")]
            for line in read.lines[at + 1 :]
            if line.startswith("|")
        ]
        return dict(rows[2:])

    here = left(PAGE)
    assert sorted(here.values()) == ["QUA-345", "QUA-468"]
    # The client's page leaves the same, in the same words, and the
    # package's page lists the client's own story among what is left of it.
    assert left(CLIENT_PAGE) == here
    package = left(PACKAGE_PAGE)
    assert "QUA-462" not in package.values()
    assert {what for what, story in package.items() if story == "QUA-468"} == {
        what for what, story in here.items() if story == "QUA-468"
    }


def test_the_page_says_what_the_strategy_of_its_examples_is(page):
    said = textwrap.fill(" ".join(page(PAGE).text.split()), 10_000)
    assert "`my_strategy`, a directory beside them that holds one file, `sma_cross.py`" in said
    assert (
        "**Nothing is pickled, and nothing of a strategy is run where it is submitted from.**"
        in said
    )
