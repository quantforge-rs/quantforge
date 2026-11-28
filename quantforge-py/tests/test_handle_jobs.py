"""A job followed from Python on a real QuantForge server: `quantforge serve
run` on a store of either backend, its workers this interpreter
(docs/python-jobs.md).

- **A job's log is followed as the job writes it,** over the stream the
  server offers: a line arrives while its job still runs, a log followed
  late is the whole log, and the same lines come a page at a time where the
  stream cannot be had. A server that stops ends the stream, and the loop
  with it or with what is raised of a server that is gone, as a server
  that is killed does.
- **A job is waited for,** to its end or to a timeout.
- **A job's result is a local run's:** the goldens' SMA cross submitted as
  a bundle gives, as a `BacktestResult`, the result `run_backtest` gives of
  the same class over the same candles, read from the same store, and its
  two frames are that result's, column for column and cell for cell.
- **A job's provenance is what its server recorded,** each field in the
  package's own types, with the identity the server gave, which two
  submissions of the same thing share.

What the client does with a server that answers otherwise is in
`test_logs.py`, `test_results.py`, `test_provenance.py` and `test_wait.py`,
against servers of the test's own.
"""

import importlib.util
import os
import re
import shutil
import sys
import threading
import time
from decimal import Decimal

import pytest

import jobs
import quantforge_py as qf
import stores
from quantforge_py import _logs, _wire

# The goldens' frozen day: 1,440 minutes from noon, and the last of its
# first 30.
FIRST, LAST = stores.NOON, stores.NOON + 1439 * stores.MINUTE
THIRTIETH = FIRST + 29 * stores.MINUTE

PYTHON = "%d.%d" % sys.version_info[:2]
MARKET = qf.MarketId("binance_spot", "BTCUSDT", "1m")
SETTINGS = qf.BacktestConfig(initial_cash=10000, fee_bps=10, close_out_at_end=True)

# A strategy that says two things as it starts and then waits, so that its
# job runs until it is cancelled or its server stops: what it wrote can
# reach a follower only while the job still runs.
SPEAKS_THEN_WAITS = """
    import sys
    import time

    import quantforge_py as qf


    class Strategy(qf.Strategy):
        def on_start(self, ctx):
            print("the first line")
            print("the second, to standard error", file=sys.stderr)
            sys.stdout.flush()
            sys.stderr.flush()
            time.sleep(3600)

        def on_bar(self, ctx, bar):
            return None
"""

# One that writes as it goes and runs to its end.
SPEAKS = """
    import sys

    import quantforge_py as qf


    class Strategy(qf.Strategy):
        def __init__(self):
            self.bars = 0

        def on_start(self, ctx):
            print("started")

        def on_bar(self, ctx, bar):
            self.bars += 1
            if self.bars in (1, 15):
                print("bar", self.bars)
            if self.bars == 20:
                print("nearly there", file=sys.stderr)
            return None

        def on_finish(self, ctx):
            print("finished after", self.bars, "bars")
"""

SPOKEN = [
    ("info", "started"),
    ("info", "bar 1"),
    ("info", "bar 15"),
    ("warning", "nearly there"),
    ("info", "finished after 30 bars"),
]


def entry(path, name):
    """A class of a file, as a bundle's entry names one."""
    spec = importlib.util.spec_from_file_location(path.stem, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return getattr(module, name)


SmaCross = entry(jobs.GOLDEN / "sma_cross.py", "SmaCross")


def scratch(directory):
    made = directory / "scratch"
    made.mkdir(exist_ok=True)
    return made


def with_minutes(place, count=30):
    """The place, holding the first `count` minutes of the goldens' day."""
    with stores.Exchange({("BTCUSDT", "1m"): jobs.golden_klines()[:count]}) as exchange:
        place.sync(exchange)
    return place


def submitted(server, source, end=THIRTIETH) -> qf.Job:
    """A job of a strategy over the place's minutes, as a client holds it."""
    files = {
        "quantforge.toml": jobs.manifest("strategy.py:Strategy"),
        "strategy.py": jobs.strategy_file(source),
    }
    job_id = server.submit(server.upload(files), FIRST, end)
    return qf.Client(f"http://127.0.0.1:{server.port}").backtest.get(job_id)


def said(lines) -> list[tuple[str, str]]:
    return [(each.level, each.message) for each in lines]


def raised(kind, call):
    with pytest.raises(kind) as caught:
        call()
    assert type(caught.value) is kind
    return caught.value


@pytest.fixture
def never_paged(monkeypatch):
    """Fails a test whose log is read a page at a time: one that says its
    lines came over the stream."""

    def paged(*arguments):
        raise AssertionError("the log was read a page at a time, and not over its stream")
        yield

    monkeypatch.setattr(_logs, "_paged", paged)


@pytest.fixture
def never_streamed(monkeypatch):
    """Gives a client no stream, as a proxy that passes no WebSocket does:
    every log is read a page at a time."""
    from quantforge_py import _websocket

    def stream(self, route):
        raise _websocket._Broken("this test has no stream")

    monkeypatch.setattr(_wire._Wire, "stream", stream)


# A log.


def test_a_line_reaches_a_follower_while_its_job_still_runs_and_a_late_one_the_whole_log(
    place, tmp_path, never_paged
):
    with jobs.served(with_minutes(place), scratch(tmp_path)) as server:
        job = submitted(server, SPEAKS_THEN_WAITS)
        logs = job.stream_logs()
        # The job waits for an hour once it has written its two lines: they
        # come to a follower that is following, or never.
        first, second = next(logs), next(logs)
        assert said([first, second]) == [
            ("info", "the first line"),
            ("warning", "the second, to standard error"),
        ]
        assert (first.seq, second.seq) == (1, 2)
        assert first.at_ms <= second.at_ms and (first.dropped_bytes, second.dropped_bytes) == (
            None,
            None,
        )
        assert job.refresh().state == "running"
        # And a second follower, come late to the job that still runs, is
        # given the same two, and then waits with the first.
        late = job.stream_logs()
        assert [next(late), next(late)] == [first, second]
        # The log ends with the job, for both.
        assert job.cancel().state == "cancelled"
        assert list(logs) == [] and list(late) == []
        # Followed once the job has ended, it is the whole log again: what
        # the store holds of it, line for line as it came.
        assert list(job.stream_logs()) == [first, second]
        assert list(job.stream_logs(after=1)) == [second]
        assert list(job.stream_logs(after=2)) == []
        # As the server's own route gives it.
        assert server.log(job.id)["lines"] == [
            {"seq": each.seq, "at_ms": each.at_ms, "level": each.level, "message": each.message}
            for each in (first, second)
        ]


def test_a_job_that_runs_to_its_end_is_followed_to_its_last_line_and_told_how_far_it_came(
    place, tmp_path, never_paged
):
    with jobs.served(with_minutes(place), scratch(tmp_path)) as server:
        job = submitted(server, SPEAKS)
        told = []
        lines = list(job.stream_logs(on_progress=lambda *progress: told.append(progress)))
        assert said(lines) == SPOKEN
        assert [each.seq for each in lines] == [1, 2, 3, 4, 5]
        # How far it came: never back, and to the last of its thirty bars.
        assert told == sorted(told) and told[-1] == (30, 30)
        assert all(total == 30 and 0 <= done <= 30 for done, total in told)
        # The log ended because the job had: nothing is left to wait for.
        assert job.wait(jobs.PATIENCE).state == "succeeded"
        # A follower that comes after is given the same lines and is told
        # where the job ended.
        again = []
        assert list(job.stream_logs(on_progress=lambda *progress: again.append(progress))) == lines
        assert again == [(30, 30)]


def test_the_same_lines_come_a_page_at_a_time_where_no_stream_can_be_had(
    place, tmp_path, never_streamed
):
    with jobs.served(with_minutes(place), scratch(tmp_path)) as server:
        job = submitted(server, SPEAKS)
        told = []
        paged = list(job.stream_logs(on_progress=lambda *progress: told.append(progress)))
        assert said(paged) == SPOKEN and told[-1] == (30, 30)
        assert job.wait(jobs.PATIENCE).state == "succeeded"
        # And a running job's lines come while it runs, there too.
        waiting = submitted(server, SPEAKS_THEN_WAITS)
        logs = waiting.stream_logs()
        assert said([next(logs), next(logs)]) == [
            ("info", "the first line"),
            ("warning", "the second, to standard error"),
        ]
        assert waiting.refresh().state == "running"
        waiting.cancel()
        assert list(logs) == []


def test_the_stream_and_the_pages_give_the_same_log(tmp_path, monkeypatch):
    place = with_minutes(stores.Place.sqlite(tmp_path))
    with jobs.served(place, scratch(tmp_path)) as server:
        job = submitted(server, SPEAKS)
        assert job.wait(jobs.PATIENCE).state == "succeeded"
        streamed = list(job.stream_logs())
        from quantforge_py import _websocket

        def stream(self, route):
            raise _websocket._Broken("no stream")

        monkeypatch.setattr(_wire._Wire, "stream", stream)
        assert list(job.stream_logs()) == streamed and said(streamed) == SPOKEN


def test_the_log_of_a_queued_job_begins_with_the_job(tmp_path, never_paged):
    place = stores.Place.sqlite(tmp_path)
    with jobs.one_of_each(place, tmp_path) as fixture:
        client = qf.Client(fixture.url)
        running = client.backtest.get(fixture.running)
        # A job behind the one that runs: it writes as it starts, which it
        # does not while the server's one worker is taken.
        queued = submitted(fixture.server, SPEAKS)
        fixture.reached(queued.id, "queued")
        lines = []
        follower = threading.Thread(target=lambda: lines.extend(queued.stream_logs()))
        follower.start()
        follower.join(1.0)
        assert follower.is_alive() and lines == []
        assert queued.refresh().state == "queued"
        # The two before it out of the way, it starts, and its log with it.
        running.cancel()
        client.backtest.get(fixture.queued).cancel()
        follower.join(jobs.PATIENCE)
        assert not follower.is_alive()
        assert said(lines) == SPOKEN
        assert queued.wait(jobs.PATIENCE).state == "succeeded"


@pytest.mark.skipif(os.name != "posix", reason="a server is asked to stop by a signal, POSIX's")
def test_a_server_that_stops_ends_the_log_of_the_job_it_ran_which_the_next_server_holds(tmp_path):
    place = with_minutes(stores.Place.sqlite(tmp_path))
    with jobs.served(place, scratch(tmp_path)) as server:
        job = submitted(server, SPEAKS_THEN_WAITS)
        logs = job.stream_logs()
        assert next(logs).message == "the first line"
        assert next(logs).message == "the second, to standard error"
        # A server that stops fails the job it runs, and so ends its log:
        # the loop ends, as with any job that has ended.
        server.stop()
        assert list(logs) == []
    # The next server on the store holds the job as it was left: failed, by
    # a server that stopped while it ran, with the log it had written.
    with jobs.served(place, scratch(tmp_path)) as server:
        again = qf.Client(f"http://127.0.0.1:{server.port}").backtest.get(job.id)
        assert again.wait(jobs.PATIENCE).state == "failed"
        assert again.status.error_code == "internal"
        assert said(again.stream_logs()) == [
            ("info", "the first line"),
            ("warning", "the second, to standard error"),
        ]


def test_a_server_that_is_killed_leaves_the_follower_of_the_job_it_ran_with_no_server(tmp_path):
    place = with_minutes(stores.Place.sqlite(tmp_path))
    with jobs.served(place, scratch(tmp_path)) as server:
        port = server.port
        job = submitted(server, SPEAKS_THEN_WAITS)
        logs = job.stream_logs()
        assert next(logs).message == "the first line"
        assert next(logs).message == "the second, to standard error"
        # A server that is killed finishes nothing and tells nobody: the
        # stream is cut off without an end, and the page that is asked for
        # in its place finds no server.
        server.kill()
        error = raised(qf.ServerUnavailable, lambda: next(logs))
        assert str(error).startswith(f"could not reach the server at http://127.0.0.1:{port}/: ")
    # The next server on the store finds the job running under no server,
    # and fails it. What the job had written was the killed server's to
    # keep, and went with it.
    with jobs.served(place, scratch(tmp_path)) as server:
        again = qf.Client(f"http://127.0.0.1:{server.port}").backtest.get(job.id)
        assert again.wait(jobs.PATIENCE).state == "failed"
        assert again.status.error_code == "internal"
        assert said(again.stream_logs()) == []


def test_a_server_that_stops_leaves_the_follower_of_a_job_that_has_not_ended_with_no_server(
    tmp_path,
):
    with jobs.one_of_each(stores.Place.sqlite(tmp_path), tmp_path) as fixture:
        port = fixture.server.port
        queued = qf.Client(fixture.url).backtest.get(fixture.queued)
        followed = []

        def follow():
            try:
                followed.extend(queued.stream_logs())
            except Exception as error:
                followed.append(error)

        follower = threading.Thread(target=follow)
        follower.start()
        follower.join(1.0)
        assert follower.is_alive() and followed == []
        # The job never started: its stream is closed without `end`, and
        # the page that is asked for in its place finds nobody.
        fixture.server.stop()
        follower.join(jobs.PATIENCE)
        assert not follower.is_alive()
    (error,) = followed
    assert type(error) is qf.ServerUnavailable
    assert str(error).startswith(f"could not reach the server at http://127.0.0.1:{port}/: ")


# A wait.


def test_a_job_is_waited_for_to_its_end_or_to_a_timeout(place, tmp_path):
    with jobs.served(with_minutes(place), scratch(tmp_path)) as server:
        job = submitted(server, jobs.HOLDS)
        status = job.wait(jobs.PATIENCE)
        assert status is job.status and (status.state, status.ended) == ("succeeded", True)
        assert status.started_at_ms <= status.finished_at_ms
        # One that fails is given as it ended.
        failed = submitted(server, jobs.RAISES).wait(jobs.PATIENCE)
        assert (failed.state, failed.error_code) == ("failed", "invalid_request")
        assert failed.error_message.startswith('the strategy "fragile" failed in on_bar')
        # One that does not end in time goes on, and is cancelled.
        waiting = submitted(server, jobs.WAITS)
        began = time.monotonic()
        error = raised(TimeoutError, lambda: waiting.wait(0.5))
        assert 0.5 <= time.monotonic() - began < jobs.PATIENCE
        assert str(error).startswith(f"job {waiting.id} had not ended after 0.5 seconds: it was ")
        assert not waiting.status.ended and waiting.refresh().state in ("queued", "running")
        assert waiting.cancel().state == "cancelled"
        assert waiting.wait().state == "cancelled"


# A result.


def test_a_jobs_result_is_the_result_of_the_same_run_made_here(place, tmp_path, as_given):
    with stores.Exchange({("BTCUSDT", "1m"): jobs.golden_klines()}) as exchange:
        place.sync(exchange)
    expected = jobs.golden()["result"]
    with jobs.served(place, scratch(tmp_path)) as server:
        client = qf.Client(f"http://127.0.0.1:{server.port}")
        job = client.backtest.submit(
            jobs.GOLDEN / "sma_cross.py",
            entry="sma_cross.py:SmaCross",
            params={"fast": 20, "slow": 50},
            market=MARKET,
            config=SETTINGS,
        )
        assert job.wait(jobs.PATIENCE).state == "succeeded"
        result = job.result()
        remote = result.to_backtest_result()
        # The server's own routes say the same of it.
        listed = server.json("GET", f"/v1/backtests/{job.id}/result")
        assert [
            (each.name, each.media_type, each.hash, each.size_bytes) for each in result.files
        ] == [
            (each["name"], each["media_type"], each["hash"], each["size_bytes"])
            for each in listed["results"]
        ]
    # The goldens' summary, digit for digit, as the metrics and as the
    # result's own fields.
    summary = {field: expected[field] for field in jobs.METRICS}
    metrics = result.metrics
    assert {
        name: value if name == "trade_count" else format(value, "f")
        for name, value in metrics.items()
    } == summary
    assert all(type(metrics[name]) is Decimal for name in jobs.METRICS[:-1])
    given = as_given(remote)
    assert {field: given[field] for field in jobs.METRICS} == summary
    # Their trades and their curve, every amount with its digits.
    assert given["trades"] == expected["trades"]
    assert given["equity_curve"] == expected["equity_curve"]
    assert (len(remote.trades), len(remote.equity_curve)) == (15, 1440)

    # The same class, run here over the same candles, read from the same
    # store: the same result.
    with qf.CandleReader(**place.arguments) as reader:
        candles = reader.load(MARKET)
    assert len(candles) == 1440 and candles.market == MARKET
    local = qf.run_backtest(candles, SmaCross(fast=20, slow=50), SETTINGS)
    assert type(remote) is type(local) is qf.BacktestResult
    assert remote == local
    assert as_given(remote) == as_given(local)


def test_a_jobs_frames_are_the_frames_of_the_same_run_made_here_column_for_column(place, tmp_path):
    pd = pytest.importorskip("pandas")
    pytest.importorskip("numpy")
    with stores.Exchange({("BTCUSDT", "1m"): jobs.golden_klines()}) as exchange:
        place.sync(exchange)
    with jobs.served(place, scratch(tmp_path)) as server:
        client = qf.Client(f"http://127.0.0.1:{server.port}")
        job = client.backtest.submit(
            jobs.GOLDEN / "sma_cross.py",
            entry="sma_cross.py:SmaCross",
            params={"fast": 20, "slow": 50},
            market=MARKET,
            config=SETTINGS,
        )
        assert job.wait(jobs.PATIENCE).state == "succeeded"
        result = job.result()
        trades, equity = result.trades_df(), result.equity_df()
    with qf.CandleReader(**place.arguments) as reader:
        local = qf.run_backtest(reader.load(MARKET), SmaCross(fast=20, slow=50), SETTINGS)
    for remote, own in [
        (trades, qf.trades_to_dataframe(local)),
        (equity, qf.equity_to_dataframe(local)),
    ]:
        # The schema: the names of the columns and their order, the type of
        # each, the type of the names themselves, and the index.
        assert list(remote.columns) == list(own.columns)
        assert remote.dtypes.equals(own.dtypes), (remote.dtypes, own.dtypes)
        assert remote.columns.dtype == own.columns.dtype == object
        assert type(remote.index) is type(own.index) and remote.index.dtype == own.index.dtype
        assert remote.index.name == own.index.name and remote.index.equals(own.index)
        assert remote.attrs == own.attrs == {}
        # And every cell, as it is: no tolerance for a float.
        pd.testing.assert_frame_equal(
            remote, own, check_exact=True, check_column_type=True, check_index_type=True
        )
    assert (len(trades), len(equity)) == (15, 1440)
    assert list(trades.columns) == [
        "symbol",
        "entry_time",
        "exit_time",
        "entry_price",
        "exit_price",
        "qty",
        "gross_quote_pnl",
        "entry_order_id",
        "exit_order_id",
    ]
    assert trades["entry_order_id"].tolist() == [None] * 15
    assert trades["exit_order_id"].tolist() == [None] * 15
    assert (equity.index.name, list(equity.columns)) == ("open_time", ["equity"])
    # The frames hold the floats nearest to the goldens' decimals.
    expected = jobs.golden()["result"]
    assert trades["gross_quote_pnl"].tolist() == [
        float(trade["gross_quote_pnl"]) for trade in expected["trades"]
    ]
    assert equity["equity"].tolist() == [
        float(point["equity"]) for point in expected["equity_curve"]
    ]


def test_only_a_job_that_has_succeeded_has_a_result_in_the_servers_words(tmp_path):
    with jobs.one_of_each(stores.Place.sqlite(tmp_path), tmp_path) as fixture:
        client = qf.Client(fixture.url)
        for job_id, stands in [
            (fixture.queued, "is still queued"),
            (fixture.running, "is still running"),
            (fixture.failed, "failed"),
        ]:
            error = raised(qf.Conflict, client.backtest.get(job_id).result)
            assert (
                str(error) == f"job {job_id} {stands}; only a job that has succeeded has a result"
            )
            assert (error.code, error.status) == ("conflict", 409)
        cancelled = client.backtest.get(fixture.queued)
        cancelled.cancel()
        error = raised(qf.Conflict, cancelled.result)
        assert str(error) == (
            f"job {cancelled.id} was cancelled; only a job that has succeeded has a result"
        )
        # The one that succeeded held nothing, and its result says so.
        result = client.backtest.get(fixture.succeeded).result()
        assert result.metrics == {
            "initial_cash": Decimal("10000"),
            "final_equity": Decimal("10000"),
            "total_return_pct": Decimal("0"),
            "max_drawdown_pct": Decimal("0"),
            "trade_count": 0,
        }
        whole = result.to_backtest_result()
        assert (whole.trades, len(whole.equity_curve)) == ((), 30)
        assert repr(result) == f"<JobResult of job {fixture.succeeded}: 0 trades>"


# A provenance.


def test_a_jobs_provenance_is_what_its_server_recorded_and_two_of_one_submission_share_an_identity(
    place, tmp_path
):
    with stores.Exchange({("BTCUSDT", "1m"): jobs.golden_klines()}) as exchange:
        place.sync(exchange)
    files = jobs.golden_bundle()
    with jobs.served(place, scratch(tmp_path)) as server:
        client = qf.Client(f"http://127.0.0.1:{server.port}")
        stored = server.upload(files)
        first = client.backtest.get(server.submit(stored, FIRST, LAST))
        second = client.backtest.get(server.submit(stored, FIRST, LAST))
        assert first.wait(jobs.PATIENCE).state == second.wait(jobs.PATIENCE).state == "succeeded"
        provenance, other = first.provenance(), second.provenance()
        answered = server.json("GET", f"/v1/backtests/{first.id}/provenance")
        result = first.result()
    assert type(provenance) is qf.Provenance and provenance.job_id == first.id
    inputs = answered["inputs"]
    # What it ran on: the bundle, the bars and the segments that hold them.
    assert provenance.code_hash == stored == first.status.bundle == inputs["code_hash"]
    assert provenance.code_hash == jobs.code_hash(jobs.canonical_tar(files))
    assert provenance.market == MARKET == first.status.market
    assert (provenance.start_open_ms, provenance.end_open_ms) == (FIRST, LAST)
    assert provenance.bar_count == 1440
    assert [type(each) for each in provenance.segments] == [qf.ProvenanceSegment] * 2
    assert [
        (each.hash, each.first_open_ms, each.last_open_ms, each.bar_count)
        for each in provenance.segments
    ] == [tuple(each.values()) for each in inputs["data"]["segments"]]
    assert [each.bar_count for each in provenance.segments] == [720, 720]
    assert all(
        each.hash.startswith("sha256:") and len(each.hash) == 71 for each in provenance.segments
    )
    # How, and on what.
    assert provenance.config == SETTINGS == first.status.config
    assert (provenance.engine, provenance.python) == (qf.ENGINE_VERSION, PYTHON)
    assert provenance.deps == {} and provenance.proto_version == client.proto_version == 1
    assert (provenance.entry, provenance.params) == (
        "sma_cross.py:SmaCross",
        {"fast": 20, "slow": 50},
    )
    # Where and when, which is the store's.
    assert provenance.backend == place.backend
    assert provenance.schema_version == answered["recorded"]["schema_version"] >= 11
    assert first.status.submitted_at_ms <= provenance.recorded_at_ms <= first.status.finished_at_ms
    # What it produced, as its result lists it.
    assert provenance.results == result.files
    # The identity is the server's, of those inputs; and a second job of
    # the same bundle over the same bars shares it, with everything the
    # identity covers, though it is another job, recorded at another time.
    assert provenance.identity == answered["identity"]
    assert other.identity == provenance.identity and other.job_id == second.id != first.id
    assert other.code_hash == provenance.code_hash and other.segments == provenance.segments
    assert other.results == provenance.results
    assert other != provenance


def test_a_job_that_has_not_started_has_no_provenance_in_the_servers_words(tmp_path):
    with jobs.one_of_each(stores.Place.sqlite(tmp_path), tmp_path) as fixture:
        client = qf.Client(fixture.url)
        queued = client.backtest.get(fixture.queued)
        error = raised(qf.Conflict, queued.provenance)
        assert str(error) == (
            f"job {queued.id} is still queued; its provenance is recorded as it starts"
        )
        assert (error.code, error.status) == ("conflict", 409)
        # A job has it once it has started: one that runs, one that failed
        # as it ran, and one that succeeded.
        for job_id, produced in [
            (fixture.running, None),
            (fixture.failed, None),
            (fixture.succeeded, 3),
        ]:
            provenance = client.backtest.get(job_id).provenance()
            assert provenance.job_id == job_id and provenance.bar_count == 30
            assert provenance.code_hash == fixture.hashes[job_id]
            assert (provenance.results is None) == (produced is None)
            assert produced is None or len(provenance.results) == produced
        # A job cancelled before it started never has one.
        queued.cancel()
        error = raised(qf.Conflict, queued.provenance)
        assert str(error) == (
            f"job {queued.id} has no provenance: it ended before the server read its inputs, or a "
            "build that recorded none ran it"
        )


# The readme.


def test_the_readmes_example_of_a_job_followed_runs_against_a_real_server(
    page, tmp_path, monkeypatch, capsys, without_pandas
):
    # What PyPI shows of a job that is followed is what a reader copies: the
    # readme's last example, which goes on from the job the example before
    # it submitted.
    readme = page("quantforge-py/README.md")
    *_, of_a_submission, example = readme.fences("python")
    assert "job = client.backtest.submit(" in of_a_submission
    place = stores.Place.sqlite(tmp_path)
    with stores.Exchange({("BTCUSDT", "1m"): jobs.golden_klines()}) as exchange:
        place.sync(exchange)
    (tmp_path / "my_strategy").mkdir()
    shutil.copy(jobs.GOLDEN / "sma_cross.py", tmp_path / "my_strategy" / "sma_cross.py")
    monkeypatch.chdir(tmp_path)
    with jobs.served(place, scratch(tmp_path)) as server:
        names = {
            "qf": qf,
            "client": qf.Client(f"http://127.0.0.1:{server.port}"),
            "market": qf.MarketId("binance_spot", "btcusdt", "1m"),
            "config": qf.BacktestConfig(initial_cash=1000, fee_bps=0, close_out_at_end=True),
        }
        exec(compile(of_a_submission, readme.name, "exec"), names)
        # Its last two lines are the frames, which need pandas: without it
        # they say what to install, and everything before them has run.
        if without_pandas:
            with pytest.raises(ModuleNotFoundError, match="quantforge-py\\[pandas\\]"):
                exec(compile(example, readme.name, "exec"), names)
        else:
            exec(compile(example, readme.name, "exec"), names)
        job, remote, provenance = names["job"], names["remote"], names["provenance"]
        # The golden strategy writes nothing: its log has no line to print.
        assert capsys.readouterr().out == "" and "line" not in names
        assert job.status.state == "succeeded"
        assert type(remote) is qf.JobResult and type(provenance) is qf.Provenance
        # Each line that shows a value shows the value.
        (count,) = re.findall(r'^remote\.metrics\["trade_count"\] +# (\d+)$', example, re.M)
        assert remote.metrics["trade_count"] == int(count) == 15
        (state,) = re.findall(r"^job\.wait\(\)\.state +# '(\w+)'", example, re.M)
        assert state == job.status.state
        assert type(remote.metrics["final_equity"]) is Decimal
        assert provenance.code_hash == job.status.bundle
        whole = remote.to_backtest_result()
        assert type(whole) is qf.BacktestResult and len(whole.trades) == 15
        if not without_pandas:
            trades, curve = names["trades"], names["curve"]
            assert trades.equals(qf.trades_to_dataframe(whole))
            assert curve.equals(qf.equity_to_dataframe(whole))
    # Every exception the readme names of it is one the package or Python
    # has, and the page it points to is the page of a job.
    said_of_it = readme.text.split(example)[1].split("The package installs as")[0]
    assert re.findall(r"`qf\.(\w+)`", said_of_it) == ["Conflict"]
    assert "`TimeoutError`" in said_of_it and "docs/python-jobs.md" in said_of_it
