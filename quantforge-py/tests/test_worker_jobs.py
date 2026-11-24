"""Strategy bundles run by a QuantForge server's workers: `quantforge serve
run --job-python` this interpreter, a bundle uploaded and a backtest of it
submitted over the job routes, its result and its log read back
(docs/python-worker.md).

The goldens' SMA cross runs as a bundle on a store of either backend and
makes their trades, equity curve and summary digit for digit; a strategy
that raises fails its job with its traceback in its log; a package the
server does not offer is refused before anything runs, and an import the
bundle may not make as it runs; what a strategy writes is its job's log;
every job has a worker of its own, which holds nothing of its server's
store; how long a job takes to reach its first bar is measured; and the
command line's `backtest --remote` runs the goldens' strategy file on a
server and prints their report."""

import os
import secrets
import statistics
import subprocess
import sys
import time
import urllib.parse

import pytest

import jobs
import quantforge_py as qf
import stores

# The goldens' frozen day: 1,440 minutes from noon, and the last of its
# first 30.
FIRST, LAST = stores.NOON, stores.NOON + 1439 * stores.MINUTE
THIRTIETH = FIRST + 29 * stores.MINUTE

PYTHON = "%d.%d" % sys.version_info[:2]

# A strategy that holds nothing, for jobs whose strategy is not the point.
HOLD = """
import quantforge_py as qf


class Hold(qf.Strategy):
    def on_bar(self, ctx, bar):
        return None
"""


def scratch(tmp_path):
    """Where a server's workers make their scratch directories."""
    directory = tmp_path / "scratch"
    directory.mkdir(exist_ok=True)
    return directory


def with_minutes(place, count=30):
    """The place, holding the first `count` minutes of the goldens' day."""
    with stores.Exchange({("BTCUSDT", "1m"): jobs.golden_klines()[:count]}) as exchange:
        place.sync(exchange)
    return place


def bundle(source, entry="strategy.py:Hold", **manifest):
    return {
        "quantforge.toml": jobs.manifest(entry, **manifest),
        entry.partition(":")[0]: jobs.strategy_file(source),
    }


def messages(log, level=None):
    return [line["message"] for line in log["lines"] if level in (None, line["level"])]


def test_the_goldens_sma_cross_runs_as_a_bundle_and_makes_their_trades_digit_for_digit(
    place, tmp_path
):
    with stores.Exchange({("BTCUSDT", "1m"): jobs.golden_klines()}) as exchange:
        place.sync(exchange)
    expected = jobs.golden()["result"]
    with jobs.served(place, scratch(tmp_path)) as server:
        assert server.said["job_runtime"].startswith(f"python {PYTHON}, engine {qf.ENGINE_VERSION}")
        job = server.submit(server.upload(jobs.golden_bundle()), FIRST, LAST)
        state = server.ended(job)
        assert state["kind"] == "succeeded", state
        result = server.result(job)
        log = server.log(job)
    summary = {field: expected[field] for field in jobs.METRICS}
    assert result["listed"] == summary
    assert result["metrics"] == summary
    assert result["trades"] == [
        {column: trade[column] for column in jobs.TRADE_COLUMNS} for trade in expected["trades"]
    ]
    assert result["equity"] == expected["equity_curve"]
    assert len(result["equity"]) == 1440 and result["metrics"]["trade_count"] == 15
    # Every bar was handed to the strategy, which wrote nothing.
    assert log["progress"] == {"bars_done": 1440, "bars_total": 1440}
    assert log["complete"] and log["lines"] == []


def test_the_command_line_runs_the_goldens_strategy_on_a_server_and_prints_their_report(
    place, tmp_path
):
    with stores.Exchange({("BTCUSDT", "1m"): jobs.golden_klines()}) as exchange:
        place.sync(exchange)
    expected = jobs.golden()["result"]
    strategy = jobs.GOLDEN / "sma_cross.py"
    elsewhere = tmp_path / "elsewhere"
    elsewhere.mkdir()
    environment = {name: os.environ[name] for name in ("PATH", "SYSTEMROOT") if name in os.environ}
    with jobs.served(place, scratch(tmp_path)) as server:
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
                "--strategy",
                str(strategy),
                "--entry",
                "sma_cross.py:SmaCross",
                "--params",
                "fast=20,slow=50",
            ],
            cwd=elsewhere,
            env=environment,
            capture_output=True,
            text=True,
            encoding="utf-8",
            timeout=jobs.PATIENCE,
        )
    assert run.returncode == 0, run.stderr
    lines = run.stdout.splitlines()
    report = lines[lines.index("strategy: sma_cross.py:SmaCross") :]
    trades = [
        "trade: entry={entry_time_ms} @ {entry_price} exit={exit_time_ms} @ {exit_price} "
        "qty={qty} gross_pnl={gross_quote_pnl}".format(**trade)
        for trade in expected["trades"][-5:]
    ]
    assert report == [
        "strategy: sma_cross.py:SmaCross",
        f"final_equity: {expected['final_equity']}",
        f"total_return_pct: {expected['total_return_pct']}",
        f"max_drawdown_pct: {expected['max_drawdown_pct']}",
        f"trade_count: {expected['trade_count']}",
        *trades,
    ]
    assert "state: succeeded" in lines and "progress: 1440 of 1440 bars" in lines
    # The client's code hash is the one this stdlib builder gives the same
    # files: the strategy as it is, and the manifest the flags write for
    # this server's Python and the client's engine.
    manifest = (
        f'format = 1\nentry = "sma_cross.py:SmaCross"\nengine = "=={qf.ENGINE_VERSION}"\n'
        f'python = "{PYTHON}"\n\n[params]\nfast = 20\nslow = 50\n'
    ).encode()
    files = {"quantforge.toml": manifest, "sma_cross.py": strategy.read_bytes()}
    named = jobs.code_hash(jobs.canonical_tar(files))
    bundle = next(line for line in lines if line.startswith("bundle: "))
    assert bundle.startswith(f"bundle: {named} ("), (bundle, named)
    # Nothing was opened where the command ran.
    assert list(elsewhere.iterdir()) == []


def test_a_strategy_that_raises_fails_its_job_with_its_traceback_in_its_log(tmp_path):
    place = with_minutes(stores.Place.sqlite(tmp_path))
    at = FIRST + 2 * stores.MINUTE
    source = f"""
        import quantforge_py as qf


        class Fragile(qf.Strategy):
            name = "fragile"

            def on_bar(self, ctx, bar):
                if bar.open_time_ms == {at}:
                    return self.divide(bar)
                return None

            def divide(self, bar):
                return 1 / 0
    """
    with jobs.served(place, scratch(tmp_path)) as server:
        job = server.submit(
            server.upload(bundle(source, "strategy.py:Fragile")), FIRST, THIRTIETH
        )
        state = server.ended(job)
        log = server.log(job)
    assert state["kind"] == "failed"
    said = f'the strategy "fragile" failed in on_bar, at the bar that opened at {at}'
    assert state["error"] == {
        "code": "invalid_request",
        "message": f"{said}: ZeroDivisionError: division by zero; its traceback is in the "
        "job's log",
    }
    # The traceback, as Python writes it, of the strategy's own frames.
    traceback = messages(log, "error")
    assert traceback[0] == "Traceback (most recent call last):"
    assert '  File "strategy.py", line 9, in on_bar' in traceback
    assert "    return self.divide(bar)" in traceback
    assert '  File "strategy.py", line 13, in divide' in traceback
    assert "    return 1 / 0" in traceback
    assert traceback[-2:] == ["ZeroDivisionError: division by zero", said]
    assert messages(log) == traceback
    # The bars before the one it failed on were run.
    assert log["progress"]["bars_done"] == 2


def test_a_package_the_server_does_not_offer_is_refused_as_the_bundle_is_uploaded_and_submitted(
    tmp_path,
):
    place = with_minutes(stores.Place.sqlite(tmp_path))
    files = bundle(HOLD, deps='requests = "2.32.3"')
    with jobs.served(place, scratch(tmp_path)) as server:
        offered = server.said["job_runtime"].split(", ")[2:]
        packages = ", ".join(each.partition(" ")[0] for each in offered)
        offers = f"it offers {packages}" if packages else "it offers no packages"
        refusal = f"the bundle imports requests, which this server does not offer; {offers}"
        with pytest.raises(jobs.Refused) as refused:
            server.upload(files)
        assert (refused.value.status, refused.value.code) == (400, "invalid_request")
        assert refusal in refused.value.message
        # And a bundle written for another Python.
        with pytest.raises(jobs.Refused) as refused:
            server.upload(bundle(HOLD, python="3.10"))
        assert (
            f"the bundle was written for Python 3.10, but this server runs Python {PYTHON}"
            in refused.value.message
        )
    # A server that runs no bundle stores one it cannot check against a
    # runtime; one that runs them refuses a job of it.
    with jobs.served(place, scratch(tmp_path), python=False) as server:
        assert server.said["job_runtime"] == "none"
        stored = server.upload(files)
    with jobs.served(place, scratch(tmp_path)) as server:
        with pytest.raises(jobs.Refused) as refused:
            server.submit(stored, FIRST, THIRTIETH)
        assert (refused.value.status, refused.value.code) == (400, "invalid_request")
        assert refusal in refused.value.message


def test_an_import_the_bundle_may_not_make_fails_its_job_saying_what_to_do(tmp_path):
    place = with_minutes(stores.Place.sqlite(tmp_path))
    with jobs.served(place, scratch(tmp_path)) as server:
        offered = dict(
            each.split(" ", 1) for each in server.said["job_runtime"].split(", ")[2:]
        )
        shown = ", ".join(f"{name} {version}" for name, version in sorted(offered.items()))
        cases = [
            (
                "import pytest",
                "the bundle imports pytest, which this server does not offer: a bundle imports "
                "the standard library, its own modules, quantforge_py and the packages its "
                f"manifest declares, and this server offers {shown or 'no package beside those'}",
            )
        ]
        if "numpy" in offered:
            # Loaded ahead of every job, and still the bundle's to declare.
            cases.append(
                (
                    "import numpy",
                    "the bundle imports numpy, which its manifest does not declare: add numpy = "
                    f'"{offered["numpy"]}" to [deps] in quantforge.toml',
                )
            )
        for statement, said in cases:
            job = server.submit(server.upload(bundle(f"{statement}\n{HOLD}")), FIRST, THIRTIETH)
            state = server.ended(job)
            assert state["error"] == {
                "code": "invalid_request",
                "message": f"the bundle's strategy.py could not be imported: ImportError: {said}; "
                "its traceback is in the job's log",
            }
            assert messages(server.log(job), "error") == [
                "Traceback (most recent call last):",
                '  File "strategy.py", line 1, in <module>',
                f"    {statement}",
                f"ImportError: {said}",
            ]
        if "numpy" in offered:
            # Declared, it is the bundle's.
            source = "import numpy\n" + HOLD
            declared = bundle(source, deps=f'numpy = "{offered["numpy"]}"')
            job = server.submit(server.upload(declared), FIRST, THIRTIETH)
            assert server.ended(job)["kind"] == "succeeded"


def test_what_a_strategy_writes_is_its_jobs_log(tmp_path):
    place = with_minutes(stores.Place.sqlite(tmp_path), 3)
    source = """
        import json
        import logging
        import sys
        import warnings

        import quantforge_py as qf


        class Talker(qf.Strategy):
            def on_start(self, ctx):
                # The bundle's root is its working directory.
                with open("notes.json", encoding="utf-8") as file:
                    print("notes say", json.load(file)["hello"])
                print("no end yet", end="")
                print(" ... ended")

            def on_bar(self, ctx, bar):
                print(f"bar {bar.open_time_ms} closed at {bar.close}")
                return None

            def on_finish(self, ctx):
                print("to standard error", file=sys.stderr)
                logging.warning("a logged warning")
                warnings.warn("a warning", stacklevel=1)
                sys.stdout.write("a last line without its end")
    """
    files = bundle(source, "strategy.py:Talker")
    files["notes.json"] = b'{"hello": "world"}'
    with jobs.served(place, scratch(tmp_path)) as server:
        job = server.submit(server.upload(files), FIRST, FIRST + 2 * stores.MINUTE)
        assert server.ended(job)["kind"] == "succeeded"
        log = server.log(job)
    closes = [row[4] for row in jobs.golden_klines()[:3]]
    assert messages(log, "info") == [
        "notes say world",
        "no end yet ... ended",
        *[f"bar {FIRST + n * stores.MINUTE} closed at {closes[n]}" for n in range(3)],
        "a last line without its end",
    ]
    warning = messages(log, "warning")
    assert warning[:2] == ["to standard error", "WARNING:root:a logged warning"]
    assert any(line.endswith("UserWarning: a warning") for line in warning[2:])


def test_a_strategys_own_threads_write_whole_lines_beside_its_run(tmp_path):
    # Threads of the strategy's own write to standard output and to standard
    # error at once, each a stream of its own, while the engine runs and says
    # how far it has come: every line reaches the log whole, at its level,
    # and the job ends. 600 KB of lines, all of which a log keeps.
    place = with_minutes(stores.Place.sqlite(tmp_path), 30)
    source = """
        import sys
        import threading

        import quantforge_py as qf


        class Chatty(qf.Strategy):
            def on_start(self, ctx):
                # A whole line a write: `print` writes its text and its end
                # apart, so the texts of two threads printing to one stream
                # at once can make one line, in Python's own streams as here.
                def talk(name, stream):
                    for n in range(150):
                        stream.write(f"{name} {n} " + "x" * 1000 + "\\n")

                self.threads = [
                    threading.Thread(
                        target=talk, args=(f"thread-{t}", sys.stdout if t < 2 else sys.stderr)
                    )
                    for t in range(4)
                ]
                for thread in self.threads:
                    thread.start()

            def on_bar(self, ctx, bar):
                sys.stdout.write(f"bar {bar.open_time_ms}\\n")
                return None

            def on_finish(self, ctx):
                for thread in self.threads:
                    thread.join()
    """
    with jobs.served(place, scratch(tmp_path)) as server:
        job = server.submit(server.upload(bundle(source, "strategy.py:Chatty")), FIRST, THIRTIETH)
        state = server.ended(job)
        log = server.log(job)
    assert state["kind"] == "succeeded", state
    for t, level in [(0, "info"), (1, "info"), (2, "warning"), (3, "warning")]:
        assert [line for line in messages(log, level) if line.startswith(f"thread-{t} ")] == [
            f"thread-{t} {n} " + "x" * 1000 for n in range(150)
        ], t
    assert [line for line in messages(log, "info") if line.startswith("bar ")] == [
        f"bar {FIRST + n * stores.MINUTE}" for n in range(30)
    ]
    assert len(messages(log)) == 4 * 150 + 30


def test_every_job_runs_in_a_worker_of_its_own(tmp_path):
    place = with_minutes(stores.Place.sqlite(tmp_path), 2)
    source = """
        import os

        import quantforge_py as qf


        class Hold(qf.Strategy):
            def on_start(self, ctx):
                print("pid", os.getpid(), "left", os.path.exists("left.txt"))
                with open("left.txt", "w") as file:
                    file.write("by an earlier job")

            def on_bar(self, ctx, bar):
                return None
    """
    files = bundle(source)
    with jobs.served(place, scratch(tmp_path)) as server:
        stored = server.upload(files)
        said = []
        for _ in range(2):
            job = server.submit(stored, FIRST, FIRST + stores.MINUTE)
            assert server.ended(job)["kind"] == "succeeded"
            said.append(messages(server.log(job))[0].split())
    assert [line[3] for line in said] == ["False", "False"]
    assert said[0][1] != said[1][1], said


def test_a_worker_holds_nothing_of_its_servers_store(postgres_server, tmp_path):
    # A role of the test's own, with a password no line could hold by
    # chance: the store's URL names both.
    role = f"qf_sentinel_{os.getpid()}"
    password = secrets.token_hex(16)
    postgres_server.query(f"CREATE ROLE {role} LOGIN PASSWORD '{password}'")
    try:
        with stores.Database(postgres_server) as database:
            database.admin.query(f"ALTER DATABASE {database.name} OWNER TO {role}")
            parts = urllib.parse.urlsplit(database.url)
            netloc = f"{role}:{password}@{parts.hostname}:{parts.port or 5432}"
            url = parts._replace(netloc=netloc).geturl()
            variables = {"QF_STORAGE": "postgres", "QF_POSTGRES_URL": url}
            place = stores.Place("postgres", tmp_path, {}, variables, None)
            with_minutes(place, 2)
            source = """
                import os
                import sys

                import quantforge_py as qf


                class Snoop(qf.Strategy):
                    def __init__(self, **given):
                        print("params", sorted(given.items()))

                    def on_start(self, ctx):
                        for name, value in sorted(os.environ.items()):
                            print("env", name, value)
                        print("argv", sys.argv)
                        print("cwd", os.getcwd(), sorted(os.listdir(".")))

                    def on_bar(self, ctx, bar):
                        raise RuntimeError(repr(dict(os.environ)))
            """
            files = bundle(source, "strategy.py:Snoop", params='window = 3\nlabel = "x"')
            with jobs.served(place, scratch(tmp_path)) as server:
                job = server.submit(server.upload(files), FIRST, FIRST + stores.MINUTE)
                state = server.ended(job)
                log = server.log(job)
                status = server.status(job)
    finally:
        postgres_server.query(f"DROP ROLE IF EXISTS {role}")
    assert state["kind"] == "failed"
    said = repr(state) + repr(log) + repr(status)
    for secret in (password, role, url, database.name, "postgres://"):
        assert secret not in said, secret
    names = [line.split(" ", 2)[1] for line in messages(log, "info") if line.startswith("env ")]
    assert "PYTHONHASHSEED" in names and not any(
        name.startswith(("QF_", "PG", "QUANTFORGE")) for name in names
    ), names
    assert "params [('label', 'x'), ('window', 3)]" in messages(log, "info")


def test_a_server_without_a_python_runs_no_bundle(tmp_path):
    place = with_minutes(stores.Place.sqlite(tmp_path), 2)
    with jobs.served(place, scratch(tmp_path), python=False) as server:
        assert server.said["job_runtime"] == "none"
        job = server.submit(server.upload(bundle(HOLD)), FIRST, FIRST + stores.MINUTE)
        assert server.ended(job)["error"] == {
            "code": "unavailable",
            "message": "this server runs no Python bundles: it was started without --job-python",
        }


def test_how_long_a_job_takes_to_reach_its_first_bar_is_measured(tmp_path, capsys):
    # Two ways a job starts on a pool of one worker: with that worker ready,
    # started ahead of the job, or submitted as soon as the job before it
    # ended, when it waits for the worker that replaces the last one to start
    # too. A pause of a second lets the replacement get ready.
    place = with_minutes(stores.Place.sqlite(tmp_path), 30)
    source = """
        import quantforge_py as qf


        class First(qf.Strategy):
            def __init__(self):
                self.seen = False

            def on_bar(self, ctx, bar):
                if not self.seen:
                    print("first bar")
                    self.seen = True
                return None
    """
    with jobs.served(place, scratch(tmp_path)) as server:
        offered = server.said["job_runtime"]
        cases = [("no package", bundle(source, "strategy.py:First"))]
        version = dict(each.split(" ", 1) for each in offered.split(", ")[2:]).get("numpy")
        if version is not None:
            # The same strategy, importing NumPy, which every worker has
            # loaded ahead of its job.
            imports = "\n        import numpy" + source
            numpy = bundle(imports, "strategy.py:First", deps=f'numpy = "{version}"')
            cases.append(("numpy", numpy))
        taken = {}
        for name, files in cases:
            stored = server.upload(files)
            for start in ("worker ready", "back to back"):
                for _ in range(3):
                    if start == "worker ready":
                        time.sleep(1.0)
                    submitted = time.time() * 1000
                    job = server.submit(stored, FIRST, THIRTIETH)
                    state = server.ended(job)
                    assert state["kind"] == "succeeded", state
                    (line,) = server.log(job)["lines"]
                    assert line["message"] == "first bar"
                    taken.setdefault((name, start), []).append(line["at_ms"] - submitted)
    with capsys.disabled():
        for (name, start), times in taken.items():
            print(
                f"\njob start, submit to first bar, {name}, {start}: median "
                f"{statistics.median(times):.0f} ms, fastest {min(times):.0f} ms, slowest "
                f"{max(times):.0f} ms, over {len(times)} jobs, on {sys.platform}, python {PYTHON}"
            )
    assert all(0 <= each < 30_000 for times in taken.values() for each in times), taken
