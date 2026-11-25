"""The client against a real QuantForge server: `quantforge serve run` on a
store of either backend, its workers this interpreter, with jobs in every
state for the client to list, get and cancel (docs/python-client.md).

A job is put there over the job routes, as `jobs.py` speaks them: a bundle
uploaded and a backtest of it submitted (`jobs.OneOfEach`). One that holds
nothing succeeds, one that raises fails, and one that waits as it starts
runs until it is cancelled, with a server that runs one job at a time
keeping the next one queued behind it. The client under test then says what
the server's own routes say, cancels what can be cancelled, and is refused
in the server's words.

What the client does with a server that refuses a token, says something
else or is not there is in `test_client.py`, against servers of the test's
own: no QuantForge server checks a token yet.
"""

import re
import time

import pytest

import jobs
import quantforge_py as qf
import stores

FIRST, THIRTIETH = jobs.HALF_HOUR

# The market every job here runs on, and the settings it is submitted with.
MARKET = qf.MarketId("binance_spot", "BTCUSDT", "1m")
SETTINGS = qf.BacktestConfig(initial_cash=10000, fee_bps=10, close_out_at_end=True)

FAILURE = (
    f'the strategy "fragile" failed in on_bar, at the bar that opened at {FIRST}: RuntimeError: '
    "no; its traceback is in the job's log"
)


@pytest.fixture
def fixture(place, tmp_path):
    """A server with a job in each state, on each backend in turn: for a
    test that cancels them."""
    with jobs.one_of_each(place, tmp_path) as fixture:
        yield fixture


@pytest.fixture(scope="module")
def standing(tmp_path_factory):
    """One such server for the tests of this module that change nothing, on
    SQLite: what a client reads does not depend on the store."""
    directory = tmp_path_factory.mktemp("client")
    (directory / "place").mkdir()
    with jobs.one_of_each(stores.Place.sqlite(directory / "place"), directory) as fixture:
        yield fixture


def as_the_route_says(status: qf.JobStatus, said: dict, bundle_hash: str):
    """Holds a status to what the server's own route says of the job, member
    for member."""
    state = said["state"]
    assert type(status) is qf.JobStatus
    assert (status.job_id, status.state) == (said["job_id"], state["kind"])
    assert status.submitted_at_ms == said["submitted_at_ms"]
    assert status.started_at_ms == state.get("started_at_ms")
    assert status.finished_at_ms == state.get("finished_at_ms")
    error = state.get("error", {})
    assert (status.error_code, status.error_message) == (error.get("code"), error.get("message"))
    assert status.ended == (state["kind"] in ("succeeded", "failed", "cancelled"))
    # What it runs is what was submitted, in the package's own types.
    assert status.bundle == bundle_hash == said["submission"]["bundle"]
    assert status.market == MARKET
    assert (status.start_open_ms, status.end_open_ms) == (FIRST, THIRTIETH)
    assert status.config == SETTINGS


def raised(kind, call):
    with pytest.raises(kind) as caught:
        call()
    assert type(caught.value) is kind
    return caught.value


def test_a_client_lists_gets_and_cancels_the_jobs_of_a_real_server(fixture):
    server = fixture.server
    client = qf.Client(fixture.url)

    # The list is the server's jobs, the last submitted first, each as the
    # server's own route gives it.
    listed = client.backtest.list()
    assert [job.id for job in listed] == fixture.newest_first
    assert [job.status.state for job in listed] == ["queued", "running", "failed", "succeeded"]
    for job in listed:
        as_the_route_says(job.status, server.status(job.id), fixture.hashes[job.id])
    queued, running, failed, succeeded = listed
    assert (failed.status.error_code, failed.status.error_message) == ("invalid_request", FAILURE)
    assert queued.status.started_at_ms is None and running.status.finished_at_ms is None
    assert succeeded.status.submitted_at_ms <= succeeded.status.started_at_ms
    assert succeeded.status.started_at_ms <= succeeded.status.finished_at_ms
    assert [job.id for job in client.backtest.list(limit=2)] == fixture.newest_first[:2]
    assert [job.id for job in client.backtest.list(limit=1000)] == fixture.newest_first

    # A job got by its ID is the job listed, as it stands.
    for job in listed:
        got = client.backtest.get(job.id)
        assert type(got) is qf.Job and got is not job
        assert (got.id, got.status) == (job.id, job.status)

    # Cancelling a queued job: it never starts.
    cancelled = queued.cancel()
    assert cancelled is queued.status and cancelled.state == "cancelled" and cancelled.ended
    assert cancelled.started_at_ms is None
    assert cancelled.finished_at_ms >= cancelled.submitted_at_ms
    as_the_route_says(cancelled, server.status(queued.id), fixture.hashes[queued.id])

    # Cancelling a running job: its worker is stopped. A handle got before
    # says what it was last given, until it asks again.
    before = client.backtest.get(running.id)
    cancelled = running.cancel()
    assert cancelled is running.status and cancelled.state == "cancelled"
    assert cancelled.started_at_ms == before.status.started_at_ms <= cancelled.finished_at_ms
    as_the_route_says(cancelled, server.status(running.id), fixture.hashes[running.id])
    assert (before.status.state, before.status.ended) == ("running", False)
    assert before.refresh() == cancelled and before.status == cancelled
    # A job that is cancelled already stays so, as it was.
    assert running.cancel() == cancelled and queued.cancel() == queued.status

    # A job that has ended otherwise is not cancelled, in the server's words,
    # and stays as it was.
    for job, ended in [(succeeded, "succeeded"), (failed, "failed")]:
        status = job.status
        error = raised(qf.Conflict, job.cancel)
        assert str(error) == (
            f"job {job.id} has already {ended}; only a queued or running job can be cancelled"
        )
        assert (error.code, error.status) == ("conflict", 409)
        assert job.status is status and job.refresh() == status

    # The worker a cancelled job ran in is gone, and the server runs the
    # next job: one whose handle is asked until the job has ended.
    job = client.backtest.get(fixture.submit(jobs.HOLDS))
    deadline = time.monotonic() + jobs.PATIENCE
    while not job.refresh().ended:
        assert time.monotonic() < deadline, job
        time.sleep(0.02)
    assert job.status.state == "succeeded"
    assert [each.status.state for each in client.backtest.list()] == [
        "succeeded",
        "cancelled",
        "cancelled",
        "failed",
        "succeeded",
    ]


def test_a_real_servers_refusals_are_raised_as_their_exceptions_in_its_words(standing):
    client = qf.Client(standing.url)
    # A validation failure: the server says which limits a list takes.
    for limit, said in [
        (0, "limit 0 is not between 1 and 1000"),
        (1001, "limit 1001 is not between 1 and 1000"),
        (
            -1,
            "the query string is not a /v1/backtests query: Failed to deserialize query string: "
            "limit: invalid digit found in string",
        ),
    ]:
        error = raised(qf.ValidationError, lambda: client.backtest.list(limit=limit))
        assert (str(error), error.code, error.status) == (said, "invalid_request", 400)
    # A job the server does not hold.
    for unknown in ("0" * 32, "no-such-job"):
        error = raised(qf.NotFound, lambda: client.backtest.get(unknown))
        assert str(error) == f"the server holds no job {unknown}"
        assert (error.code, error.status) == ("not_found", 404)
    # A job that has ended.
    error = raised(qf.Conflict, client.backtest.get(standing.failed).cancel)
    assert str(error) == (
        f"job {standing.failed} has already failed; only a queued or running job can be cancelled"
    )
    # Nothing of it changed a job.
    assert [job.status.state for job in client.backtest.list()] == [
        "queued",
        "running",
        "failed",
        "succeeded",
    ]


def test_a_client_agrees_with_a_real_server_on_the_version_it_speaks(standing):
    client = qf.Client(standing.url)
    assert (client.server_version, client.proto_version) == (None, None)
    client.handshake()
    # The server is the binary of the checkout, of the engine the package
    # was built with, and speaks the one version there is.
    assert (client.server_version, client.proto_version) == (qf.ENGINE_VERSION, 1)
    assert repr(client) == f"<Client {standing.url}/>"
    assert client.url == f"{standing.url}/"


def test_a_token_goes_to_a_real_server_which_checks_none_yet(standing, monkeypatch):
    # No QuantForge server checks a token yet: one that is sent is taken.
    client = qf.Client(standing.url, token="a-token-no-server-checks")
    assert [job.id for job in client.backtest.list()] == standing.newest_first
    monkeypatch.setenv("QF_REMOTE_TOKEN", "another-it-does-not-check")
    assert qf.Client(standing.url).backtest.get(standing.succeeded).status.state == "succeeded"


def test_a_server_that_stopped_is_unavailable_and_reached_again_once_it_is_back(tmp_path):
    place = stores.Place.sqlite(tmp_path)
    with jobs.one_of_each(place, tmp_path) as fixture:
        port = fixture.server.port
        job = qf.Client(fixture.url).backtest.get(fixture.queued)
        assert job.status.state == "queued"
    error = raised(qf.ServerUnavailable, job.refresh)
    assert str(error).startswith(f"could not reach the server at http://127.0.0.1:{port}/: ")
    assert (error.code, error.status) == (None, None)
    # What the job was last given stays.
    assert job.status.state == "queued"
    # A job outlives its server: the next one on its store holds it, runs
    # it, and a client of that one is given it.
    with jobs.served(place, tmp_path / "scratch") as server:
        again = qf.Client(f"http://127.0.0.1:{server.port}").backtest.get(job.id)
        assert again.status.submitted_at_ms == job.status.submitted_at_ms
        assert server.ended(job.id)["kind"] == "succeeded"
        assert again.refresh().state == "succeeded"


def test_the_readmes_example_of_the_client_runs_against_a_real_server(page, tmp_path):
    # What PyPI shows of the client is what a reader copies: the readme's
    # last example, with this server's address in the place of the one
    # `serve run` listens on unless it is told otherwise.
    readme = page("quantforge-py/README.md")
    example = readme.fences("python")[-1]
    shown = "http://127.0.0.1:9600"
    assert example.count(shown) == 1 and "QF_REMOTE_TOKEN" in example
    with jobs.one_of_each(stores.Place.sqlite(tmp_path), tmp_path) as fixture:
        names = {"qf": qf}
        exec(compile(example.replace(shown, fixture.url), readme.name, "exec"), names)
        # It listed the server's jobs, got the newest, which was queued, and
        # cancelled it, as it does one that has not ended.
        assert [job.id for job in names["jobs"]] == fixture.newest_first
        job = names["job"]
        assert (job.id, job.status.state) == (fixture.queued, "cancelled")
        assert fixture.server.status(fixture.queued)["state"]["kind"] == "cancelled"
        assert job.status.market == MARKET
        assert names["client"].url == f"{fixture.url}/"
    # Every state it names is one the protocol has, and it names them all.
    (named,) = re.findall(r"^job\.status\.state +# (.+)$", example, re.M)
    states = ["queued", "running", "succeeded", "failed", "cancelled"]
    assert re.findall(r"'(\w+)'", named) == states
    # And every exception it names is one a request is raised.
    raised_as = re.findall(r"`qf\.(\w+)`", readme.text.split(example)[1])
    assert raised_as == [
        "ValidationError",
        "NotFound",
        "Conflict",
        "AuthError",
        "ServerUnavailable",
        "VersionMismatch",
        "RemoteError",
    ]
    assert all(issubclass(getattr(qf, name), qf.RemoteError) for name in raised_as)
