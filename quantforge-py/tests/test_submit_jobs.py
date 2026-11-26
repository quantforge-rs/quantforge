"""A backtest submitted from Python to a real QuantForge server:
`quantforge serve run` on a store of either backend, its workers this
interpreter, which runs what `client.backtest.submit` and `submit_class`
send it (docs/python-bundles.md).

- **A strategy's directory** is bundled, uploaded, submitted and run, and
  makes the goldens' trades, equity curve and summary digit for digit, on
  SQLite and on PostgreSQL.
- **The code hash the client computes** is the one the server holds the
  bundle under, the one the command line's `backtest --remote` computes
  for the same files and the same flags, and the one the test's own
  builder computes, on the standard library's `tarfile`.
- **What a strategy is handed** is what the caller gave as `params`, value
  for value and type for type, as its worker says in its job's log.
- **A class** is submitted by its source and runs as the file it came from;
  one that takes a variable from around it is refused, and sent nothing of.
- **A bundle that is too large, and one that needs a package the server
  does not offer,** are refused here, and the server's store holds no
  bundle afterwards.

What the client does with a server that answers otherwise is in
`test_submit.py`, against servers of the test's own.
"""

import ast
import importlib.util
import os
import re
import shutil
import subprocess
import sys
import time

import pytest

import jobs
import quantforge_py as qf
import stores
from quantforge_py import _bundles

# The goldens' frozen day: 1,440 minutes from noon.
FIRST, LAST = stores.NOON, stores.NOON + 1439 * stores.MINUTE
THIRTIETH = FIRST + 29 * stores.MINUTE

PYTHON = "%d.%d" % sys.version_info[:2]
MARKET = qf.MarketId("binance_spot", "BTCUSDT", "1m")
SETTINGS = qf.BacktestConfig(initial_cash=10000, fee_bps=10, close_out_at_end=True)

# A strategy that holds nothing, and says what it was built from.
SAYS = b'''import quantforge_py as qf


class Says(qf.Strategy):
    """Holds nothing, and says what it was built from as it starts."""

    def __init__(self, **params):
        self.params = params

    def on_start(self, ctx):
        print(repr(sorted(self.params.items())))

    def on_bar(self, ctx, bar):
        return None
'''


def ended(job: qf.Job) -> qf.JobStatus:
    """Asks a job's server where it stands until the job has ended."""
    deadline = time.monotonic() + jobs.PATIENCE
    while not job.refresh().ended:
        assert time.monotonic() < deadline, job
        time.sleep(0.02)
    return job.status


def scratch(directory):
    made = directory / "scratch"
    made.mkdir(exist_ok=True)
    return made


def with_minutes(place, count=30):
    with stores.Exchange({("BTCUSDT", "1m"): jobs.golden_klines()[:count]}) as exchange:
        place.sync(exchange)
    return place


def golden_directory(directory):
    """The goldens' strategy as a user keeps one: a directory with its
    file, and no manifest."""
    root = directory / "my_strategy"
    root.mkdir()
    shutil.copy(jobs.GOLDEN / "sma_cross.py", root / "sma_cross.py")
    return root


def says_directory(directory):
    root = directory / "says"
    root.mkdir()
    (root / "says.py").write_bytes(SAYS)
    return root


@pytest.fixture(scope="module")
def standing(tmp_path_factory):
    """One server for the tests of this module that need no store of their
    own, on SQLite, with the goldens' first thirty minutes."""
    directory = tmp_path_factory.mktemp("submit")
    (directory / "place").mkdir()
    place = with_minutes(stores.Place.sqlite(directory / "place"))
    with jobs.served(place, scratch(directory)) as server:
        yield server


def client_of(server) -> qf.Client:
    return qf.Client(f"http://127.0.0.1:{server.port}")


def offered(server) -> dict[str, str]:
    """The packages a server's workers offer, each at its version, as it
    said as it started."""
    return dict(each.split(" ", 1) for each in server.said["job_runtime"].split(", ")[2:])


def test_a_strategy_directory_is_bundled_submitted_and_run_and_makes_the_goldens_trades(
    place, tmp_path
):
    with stores.Exchange({("BTCUSDT", "1m"): jobs.golden_klines()}) as exchange:
        place.sync(exchange)
    strategy = golden_directory(tmp_path)
    expected = jobs.golden()["result"]
    with jobs.served(place, scratch(tmp_path)) as server:
        client = client_of(server)
        job = client.backtest.submit(
            strategy,
            entry="sma_cross.py:SmaCross",
            params={"fast": 20, "slow": 50},
            market=MARKET,
            config=SETTINGS,
        )
        # The server queued it over every bar it stores of the market.
        assert type(job) is qf.Job and not job.status.ended
        assert (job.status.market, job.status.config) == (MARKET, SETTINGS)
        assert (job.status.start_open_ms, job.status.end_open_ms) == (FIRST, LAST)
        status = ended(job)
        assert status.state == "succeeded", status.error_message
        result = server.result(job.id)
        provenance = server.json("GET", f"/v1/backtests/{job.id}/provenance")
        # It is the server's job like any other: listed, and got by its ID.
        assert [each.id for each in client.backtest.list()] == [job.id]
        assert client.backtest.get(job.id).status == status
    summary = {field: expected[field] for field in jobs.METRICS}
    assert result["listed"] == summary and result["metrics"] == summary
    assert result["trades"] == [
        {column: trade[column] for column in jobs.TRADE_COLUMNS} for trade in expected["trades"]
    ]
    assert result["equity"] == expected["equity_curve"]
    assert len(result["equity"]) == 1440 and result["metrics"]["trade_count"] == 15
    # What the server recorded of the job is the bundle the client named,
    # and the manifest written for it: the entry and the params as given.
    assert provenance["inputs"]["code_hash"] == status.bundle
    assert provenance["strategy"] == {
        "entry": "sma_cross.py:SmaCross",
        "params": {"fast": 20, "slow": 50},
    }
    assert provenance["inputs"]["runtime"] == {
        "engine": qf.ENGINE_VERSION,
        "python": PYTHON,
        "deps": {},
    }
    # The code hash is the one the test's own builder gives the same files:
    # the strategy as it is, and the manifest in its one form.
    manifest = (
        f'format = 1\nentry = "sma_cross.py:SmaCross"\nengine = "=={qf.ENGINE_VERSION}"\n'
        f'python = "{PYTHON}"\n\n[params]\nfast = 20\nslow = 50\n'
    ).encode()
    files = {"quantforge.toml": manifest, "sma_cross.py": (strategy / "sma_cross.py").read_bytes()}
    assert status.bundle == jobs.code_hash(jobs.canonical_tar(files))
    # Nothing was written beside the strategy's file.
    assert [path.name for path in strategy.iterdir()] == ["sma_cross.py"]


def command_line(server, cwd, *arguments) -> tuple[str, str]:
    """Runs `quantforge backtest --remote` against a server, without
    waiting for the job, and gives the bundle it named and whether the
    server stored it then or held it already."""
    environment = {name: os.environ[name] for name in ("PATH", "SYSTEMROOT") if name in os.environ}
    run = subprocess.run(
        [
            str(stores.binary()),
            "--log-level",
            "error",
            "backtest",
            "--symbol",
            "BTCUSDT",
            "--remote",
            f"http://127.0.0.1:{server.port}",
            *arguments,
            "--no-wait",
        ],
        cwd=cwd,
        env=environment,
        capture_output=True,
        text=True,
        encoding="utf-8",
        timeout=jobs.PATIENCE,
    )
    assert run.returncode == 0, run.stderr
    (bundle,) = [line for line in run.stdout.splitlines() if line.startswith("bundle: ")]
    named, _, rest = bundle.removeprefix("bundle: ").partition(" (")
    return named, rest.rstrip(")").partition(", ")[2]


# The parameters of one submission, as Python gives them and as the command
# line's `--params` writes the same.
PARAMS = [
    ({"fast": 20, "slow": 50}, "slow=50,fast=20"),
    (
        {"ratio": 0.1, "big": 1e20, "tiny": 1e-7, "whole": 2.0, "on": True, "n": -7},
        "ratio=0.1,big=1e20,tiny=1e-7,whole=2.0,on=true,n=-7",
    ),
    (
        {
            "name": 'a "q" \\ é\ttab',
            "levels": [1, 2.5, "x"],
            "window": {"size": 5, "kind": "ema"},
            "odd key": 1,
            "empty": {},
        },
        'name="a \\"q\\" \\\\ é\\ttab",levels=[1, 2.5, "x"],window={size=5, kind="ema"},'
        '"odd key"=1,empty={}',
    ),
    (
        # Floats where a text and a float part ways: one between two
        # shortest texts that are equally near, and those at the edges of
        # where an exponent is written.
        {
            "tie": 1669054756414022.2,
            "edge": 1e16,
            "below": 9999999999999998.0,
            "small": 0.0001,
            "smaller": 0.00001,
            "negative": -0.0,
        },
        "tie=1669054756414022.2,edge=1e16,below=9999999999999998.0,small=0.0001,smaller=0.00001,"
        "negative=-0.0",
    ),
    ({}, None),
]


def test_the_clients_code_hash_is_the_servers_and_the_command_lines_for_the_same_files(
    standing, tmp_path
):
    client = client_of(standing)
    strategy = says_directory(tmp_path)
    elsewhere = tmp_path / "elsewhere"
    elsewhere.mkdir()
    seen = set()
    for params, flags in PARAMS:
        job = client.backtest.submit(
            strategy,
            entry="says.py:Says",
            params=params,
            market=MARKET,
            end=FIRST + stores.MINUTE,
            config=SETTINGS,
        )
        named = job.status.bundle
        seen.add(named)
        # The server took the SHA-256 of what it received, read it back
        # strictly as a canonical tar, and holds it under that name.
        status = ended(job)
        assert status.state == "succeeded", status.error_message
        provenance = standing.json("GET", f"/v1/backtests/{job.id}/provenance")
        assert provenance["inputs"]["code_hash"] == named
        # Its strategy was handed what was given, value for value and type
        # for type: a float as the float it is, a whole number as one. A
        # dict among them comes with its keys in the manifest's order.
        (line,) = standing.log(job.id)["lines"]
        handed = dict(ast.literal_eval(line["message"]))
        assert _bundles._same(handed, params), (handed, params)
        # The command line, given the same files and the same flags, names
        # the same bundle, which the server holds already: the manifest
        # either writes is the same text.
        arguments = ["--strategy", str(strategy), "--entry", "says.py:Says"]
        if flags is not None:
            arguments += ["--params", flags]
        assert command_line(standing, elsewhere, *arguments) == (named, "already held"), params
    assert len(seen) == len(PARAMS)
    # One file, under its own name, is the bundle of the directory that
    # holds it alone.
    of_the_file = client.backtest.submit(
        strategy / "says.py", entry="says.py:Says", market=MARKET, config=SETTINGS
    )
    of_the_directory = client.backtest.submit(
        strategy, entry="says.py:Says", market=MARKET, config=SETTINGS
    )
    assert of_the_file.status.bundle == of_the_directory.status.bundle
    named = command_line(
        standing, elsewhere, "--strategy", str(strategy / "says.py"), "--entry", "says.py:Says"
    )
    assert named == (of_the_file.status.bundle, "already held")
    # And a directory with a manifest of its own is sent as it is, by both.
    own = (
        f'# Written by hand.\nformat = 1\nentry = "says.py:Says"\n'
        f'engine = ">={qf.ENGINE_VERSION}"\npython = "{PYTHON}"\n\n[params]\nfast = 2\n'
    ).encode()
    (strategy / "quantforge.toml").write_bytes(own)
    job = client.backtest.submit(strategy, market=MARKET, config=SETTINGS)
    files = {"quantforge.toml": own, "says.py": SAYS}
    assert job.status.bundle == jobs.code_hash(jobs.canonical_tar(files))
    assert command_line(standing, elsewhere, "--strategy", str(strategy)) == (
        job.status.bundle,
        "already held",
    )
    assert list(elsewhere.iterdir()) == []


def golden_module():
    """The goldens' strategy file, imported as a module of its own."""
    spec = importlib.util.spec_from_file_location("golden_sma_cross", jobs.GOLDEN / "sma_cross.py")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def test_a_class_is_submitted_by_its_source_and_makes_the_trades_of_the_file_it_came_from(
    place, tmp_path
):
    with stores.Exchange({("BTCUSDT", "1m"): jobs.golden_klines()}) as exchange:
        place.sync(exchange)
    expected = jobs.golden()["result"]
    module = golden_module()
    try:
        with jobs.served(place, scratch(tmp_path)) as server:
            client = client_of(server)
            job = client.backtest.submit_class(
                module.SmaCross,
                imports="from collections import deque\n\nimport quantforge_py as qf",
                params={"fast": 20, "slow": 50},
                market=MARKET,
                config=SETTINGS,
                source_only=True,
            )
            status = ended(job)
            assert status.state == "succeeded", status.error_message
            result = server.result(job.id)
            provenance = server.json("GET", f"/v1/backtests/{job.id}/provenance")
            # Without the import its source needs, nothing is sent.
            before = [each.id for each in client.backtest.list()]
            with pytest.raises(ValueError, match="^SmaCross uses deque, which its source does not"):
                client.backtest.submit_class(
                    module.SmaCross, market=MARKET, config=SETTINGS, source_only=True
                )
            assert [each.id for each in client.backtest.list()] == before == [job.id]
    finally:
        sys.modules.pop("golden_sma_cross", None)
    assert result["metrics"] == {field: expected[field] for field in jobs.METRICS}
    assert result["trades"] == [
        {column: trade[column] for column in jobs.TRADE_COLUMNS} for trade in expected["trades"]
    ]
    assert result["equity"] == expected["equity_curve"]
    # The bundle is one file of the class's source, under one name.
    assert provenance["strategy"] == {
        "entry": "strategy.py:SmaCross",
        "params": {"fast": 20, "slow": 50},
    }


class Holds(qf.Strategy):
    """Holds nothing: a class written at the top level of its module."""

    def on_bar(self, ctx, bar):
        return None


def test_a_class_that_takes_a_variable_from_around_it_is_refused_and_sent_nothing_of(standing):
    client = client_of(standing)
    before = [job.id for job in client.backtest.list()]
    threshold, window = 3, 5

    class Captures(qf.Strategy):
        def on_bar(self, ctx, bar):
            return None if threshold > window else None

    with pytest.raises(ValueError) as refused:
        client.backtest.submit_class(Captures, market=MARKET, config=SETTINGS, source_only=True)
    assert str(refused.value) == (
        "Captures is defined inside the function "
        "test_a_class_that_takes_a_variable_from_around_it_is_refused_and_sent_nothing_of, and "
        "uses its threshold and window: a class is sent as its own source, and what it takes "
        "from around it stays behind. Write it at the top level of a module or of a notebook's "
        "cell, with what it needs inside it"
    )
    # Nor is any class sent before its caller has said what is sent of it.
    with pytest.raises(ValueError, match="Pass source_only=True to say that this is what is meant"):
        client.backtest.submit_class(Holds, market=MARKET, config=SETTINGS)
    assert [job.id for job in client.backtest.list()] == before
    # The same class written at the top level is sent, and runs.
    job = client.backtest.submit_class(Holds, market=MARKET, config=SETTINGS, source_only=True)
    assert ended(job).state == "succeeded"
    provenance = standing.json("GET", f"/v1/backtests/{job.id}/provenance")
    assert provenance["strategy"] == {"entry": "strategy.py:Holds", "params": {}}


def test_a_bundle_too_large_and_one_the_server_cannot_run_are_refused_before_any_upload(
    place, tmp_path
):
    with_minutes(place)
    strategy = says_directory(tmp_path)
    with jobs.served(place, scratch(tmp_path)) as server:
        client = client_of(server)
        url = f"http://127.0.0.1:{server.port}/"

        def refused(**arguments):
            given = {"entry": "says.py:Says", "market": MARKET, "config": SETTINGS, **arguments}
            with pytest.raises(qf.BundleError) as caught:
                client.backtest.submit(strategy, **given)
            return caught.value

        # A package this server does not offer, whatever it does offer.
        error = refused(deps=["scipy"])
        assert [code for code, _ in error.problems] == ["dependency_not_allowed"]
        assert str(error).startswith(
            f"the server at {url} cannot run the bundle: the bundle imports scipy, which this "
            "server does not offer; it offers "
        )
        error = refused(deps={"scipy": "1.14.0", "torch": "2.5.0"})
        assert [code for code, _ in error.problems] == ["dependency_not_allowed"] * 2
        for name, version in offered(server).items():
            # And another version of one it does offer.
            error = refused(deps={name: f"{version}.post99"})
            assert error.problems == (
                (
                    "dependency_version_unavailable",
                    f"the bundle pins {name} {version}.post99, but this server offers {name} "
                    f"{version} only",
                ),
            )
        # A file over the most one file holds, files that hold too much
        # together, a file no bundle holds, and too many files.
        (strategy / "big.txt").write_bytes(b"x" * (1024 * 1024 + 1))
        assert [code for code, _ in refused().problems] == ["file_too_large"]
        (strategy / "big.txt").unlink()
        for index in range(9):
            (strategy / f"part{index}.txt").write_bytes(b"x" * 1024 * 1024)
        assert [code for code, _ in refused().problems] == ["bundle_too_large"]
        for index in range(9):
            (strategy / f"part{index}.txt").unlink()
        (strategy / "model.pkl").write_bytes(b"\x80\x04N.")
        error = refused()
        assert error.problems == (
            (
                "extension_not_allowed",
                '"model.pkl" cannot be in a bundle: a bundle holds only .py, .toml, .json, and '
                ".txt files, so leave it out or convert it",
            ),
        )
        (strategy / "model.pkl").unlink()
        for index in range(256):
            (strategy / f"z{index}.py").write_bytes(b"")
        assert [code for code, _ in refused().problems] == ["too_many_files"]
        for index in range(256):
            (strategy / f"z{index}.py").unlink()
        # No job came of any of it, and the directory is a bundle again.
        assert client.backtest.list() == []
    # The server's store holds no bundle: nothing was uploaded.
    assert place.value("SELECT count(*) FROM bundles") == "0"
    with jobs.served(place, scratch(tmp_path)) as server:
        job = client_of(server).backtest.submit(
            strategy, entry="says.py:Says", market=MARKET, config=SETTINGS
        )
        assert ended(job).state == "succeeded"
    assert place.value("SELECT count(*) FROM bundles") == "1"


def test_a_package_named_alone_is_pinned_at_the_version_the_server_offers_and_imported(
    standing, tmp_path, without_numpy
):
    if without_numpy:
        pytest.skip("this run's workers offer no NumPy")
    version = offered(standing)["numpy"]
    root = tmp_path / "averages"
    root.mkdir()
    (root / "averages.py").write_text(
        "import numpy as np\n\nimport quantforge_py as qf\n\n\n"
        "class Averages(qf.Strategy):\n"
        "    def on_start(self, ctx):\n"
        "        print(np.__version__, float(np.mean([1.0, 2.0, 6.0])))\n\n"
        "    def on_bar(self, ctx, bar):\n"
        "        return None\n",
        encoding="utf-8",
    )
    client = client_of(standing)
    job = client.backtest.submit(
        root, entry="averages.py:Averages", deps=["numpy"], market=MARKET, config=SETTINGS
    )
    assert ended(job).state == "succeeded", job.status.error_message
    (line,) = standing.log(job.id)["lines"]
    assert line["message"] == f"{version} 3.0"
    provenance = standing.json("GET", f"/v1/backtests/{job.id}/provenance")
    assert provenance["inputs"]["runtime"]["deps"] == {"numpy": version}
    # The same bundle as the pin written out.
    pinned = client.backtest.submit(
        root, entry="averages.py:Averages", deps={"numpy": version}, market=MARKET, config=SETTINGS
    )
    assert pinned.status.bundle == job.status.bundle
    # Without the declaration the bundle is admitted, and its import is
    # refused as it runs, in the worker's words.
    undeclared = client.backtest.submit(
        root, entry="averages.py:Averages", market=MARKET, config=SETTINGS
    )
    status = ended(undeclared)
    assert (status.state, status.error_code) == ("failed", "invalid_request")
    assert f'add numpy = "{version}" to [deps] in quantforge.toml' in status.error_message


def test_the_window_is_read_within_the_bars_a_real_server_stores(standing, tmp_path):
    client = client_of(standing)
    strategy = says_directory(tmp_path)
    url = f"http://127.0.0.1:{standing.port}/"

    def submit(**arguments):
        given = {"entry": "says.py:Says", "market": MARKET, "config": SETTINGS, **arguments}
        return client.backtest.submit(strategy, **given)

    whole = submit()
    assert (whole.status.start_open_ms, whole.status.end_open_ms) == (FIRST, THIRTIETH)
    part = submit(start="2026-01-01T12:05:00Z", end=FIRST + 9 * stores.MINUTE)
    assert (part.status.start_open_ms, part.status.end_open_ms) == (
        FIRST + 5 * stores.MINUTE,
        FIRST + 9 * stores.MINUTE,
    )
    beyond = submit(start="2025-12-31T00:00:00Z", end="2026-01-02T00:00:00Z")
    assert (beyond.status.start_open_ms, beyond.status.end_open_ms) == (FIRST, THIRTIETH)
    for job in (whole, part, beyond):
        assert ended(job).state == "succeeded"
    listed = len(client.backtest.list(limit=1000))
    with pytest.raises(qf.ValidationError) as refused:
        submit(start="2026-01-02T00:00:00Z")
    assert str(refused.value) == (
        f"the server at {url} stores binance spot BTCUSDT 1m from 2026-01-01T12:00:00Z to "
        "2026-01-01T12:29:00Z, outside the window from 2026-01-02T00:00:00Z to "
        "2026-01-01T12:29:00Z"
    )
    with pytest.raises(qf.ValidationError) as refused:
        submit(market=qf.MarketId("binance_spot", "ETHUSDT", "1m"))
    assert str(refused.value) == f"the server at {url} stores no bars of binance spot ETHUSDT 1m"
    assert len(client.backtest.list(limit=1000)) == listed


def test_a_server_started_without_a_python_is_sent_no_bundle(tmp_path):
    place = with_minutes(stores.Place.sqlite(tmp_path), 2)
    strategy = says_directory(tmp_path)
    with jobs.served(place, scratch(tmp_path), python=False) as server:
        with pytest.raises(qf.ServerUnavailable) as refused:
            client_of(server).backtest.submit(
                strategy, entry="says.py:Says", market=MARKET, config=SETTINGS
            )
        assert str(refused.value) == (
            f"the server at http://127.0.0.1:{server.port}/ runs no Python bundles: it was "
            "started without --job-python"
        )
    assert place.value("SELECT count(*) FROM bundles") == "0"


def test_the_readmes_example_of_a_submission_runs_against_a_real_server(
    page, tmp_path, monkeypatch
):
    # What PyPI shows of a submission is what a reader copies: the readme's
    # last example, which goes on from the client, the market and the
    # settings the examples before it made.
    readme = page("quantforge-py/README.md")
    *before, of_the_client, example = readme.fences("python")
    made_before = "\n".join([*before, of_the_client])
    for line in [
        'market = qf.MarketId("binance_spot", "btcusdt", "1m")',
        "config = qf.BacktestConfig(initial_cash=1000, fee_bps=0, close_out_at_end=True)",
        'client = qf.Client("http://127.0.0.1:9600")',
    ]:
        assert line in made_before, line
    place = stores.Place.sqlite(tmp_path)
    with stores.Exchange({("BTCUSDT", "1m"): jobs.golden_klines()}) as exchange:
        place.sync(exchange)
    golden_directory(tmp_path)
    monkeypatch.chdir(tmp_path)
    with jobs.served(place, scratch(tmp_path)) as server:
        names = {
            "qf": qf,
            "client": client_of(server),
            "market": qf.MarketId("binance_spot", "btcusdt", "1m"),
            "config": qf.BacktestConfig(initial_cash=1000, fee_bps=0, close_out_at_end=True),
        }
        exec(compile(example, readme.name, "exec"), names)
        job = names["job"]
        assert type(job) is qf.Job and job.status.bundle.startswith("sha256:")
        # From the first bar the server stores on, to the last.
        assert (job.status.start_open_ms, job.status.end_open_ms) == (FIRST, LAST)
        assert ended(job).state == "succeeded"
        assert server.result(job.id)["metrics"]["trade_count"] == 15
    # The exception it names is the one a bundle is refused with, and no
    # other of the package's is named beside it.
    said_of_it = readme.text.split(example)[1]
    assert re.findall(r"`qf\.(\w+)`", said_of_it) == ["BundleError"]
    assert "`source_only=True`" in said_of_it and "Nothing is pickled" in said_of_it
