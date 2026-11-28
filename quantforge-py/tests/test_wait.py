"""A job waited for from Python, against servers of the test's own
(docs/python-jobs.md): what `Job.wait` asks and when, what it gives, how
long it waits between two requests, and what a timeout is.

A stand-in here says where a job stands, one answer after another, and the
clock is the test's own, so that a test says exactly how long a wait waited.
What `wait` does with a real `serve run`'s job is in `test_handle_jobs.py`.
"""

import time

import pytest

import quantforge_py as qf
from quantforge_py import _jobs
from standin import JOB, StandIn, refusal, written

STATUS = f"/v1/backtests/{JOB}"
STARTED = 1_792_454_401_500
QUEUED = {"kind": "queued"}
RUNNING = {"kind": "running", "started_at_ms": STARTED}
SUCCEEDED = {
    "kind": "succeeded",
    "started_at_ms": STARTED,
    "finished_at_ms": STARTED + 500,
    "results": [],
}
FAILED = {
    "kind": "failed",
    "started_at_ms": STARTED,
    "finished_at_ms": STARTED + 500,
    "error": {"code": "invalid_request", "message": "the strategy failed"},
}
CANCELLED = {"kind": "cancelled", "finished_at_ms": STARTED}


class Clock:
    """A clock of the test's own: it moves when something sleeps, by as
    long as that sleeps, and records each sleep."""

    def __init__(self):
        self.now = 1000.0
        self.slept = []

    def monotonic(self):
        return self.now

    def sleep(self, seconds):
        assert seconds > 0
        self.slept.append(seconds)
        self.now += seconds


@pytest.fixture
def clock(monkeypatch):
    clock = Clock()
    # The module's own name for the clock, and no other module's.
    monkeypatch.setattr(_jobs, "time", clock)
    return clock


@pytest.fixture
def server():
    with StandIn() as stand_in:
        yield stand_in


def stands(server, *states):
    """A job of a server that says it stands so, one answer after another
    and the last from then on: the first as the job is got."""
    server.says("GET", STATUS, *[(200, written(state=state)) for state in states])
    return qf.Client(server.url).backtest.get(JOB)


def asked(server) -> int:
    """How many times a server was asked where the job stands, once the
    client had it."""
    assert server.routes[:2] == ["GET /version", f"GET {STATUS}"]
    assert set(server.routes[2:]) <= {f"GET {STATUS}"}
    return len(server.routes) - 2


def raised(kind, call):
    with pytest.raises(kind) as caught:
        call()
    assert type(caught.value) is kind
    return caught.value


def test_a_job_is_asked_after_until_it_has_ended_and_its_last_status_given(server, clock):
    job = stands(server, QUEUED, QUEUED, RUNNING, RUNNING, SUCCEEDED)
    status = job.wait()
    assert type(status) is qf.JobStatus and status is job.status
    assert (status.state, status.ended, status.finished_at_ms) == ("succeeded", True, STARTED + 500)
    assert asked(server) == 4
    # It waited a tenth of a second, and then twice as long each time.
    assert clock.slept == [0.1, 0.2, 0.4]
    # A job the handle holds as ended is given as it is: nothing is asked.
    assert job.wait() is status and job.wait(0) is status and job.wait(timeout=5) is status
    assert asked(server) == 4 and clock.slept == [0.1, 0.2, 0.4]


def test_a_job_that_failed_or_was_cancelled_is_given_like_any_other(server, clock):
    for ended, state in [(FAILED, "failed"), (CANCELLED, "cancelled")]:
        job = stands(server, RUNNING, ended)
        status = job.wait()
        assert (status.state, status.ended) == (state, True)
    assert stands(server, RUNNING, FAILED).wait().error_message == "the strategy failed"


def test_a_wait_asks_no_more_often_than_every_two_seconds(server, clock):
    job = stands(server, *[RUNNING] * 12, SUCCEEDED)
    assert job.wait().state == "succeeded"
    assert clock.slept == [0.1, 0.2, 0.4, 0.8, 1.6, 2.0, 2.0, 2.0, 2.0, 2.0, 2.0]
    assert (_jobs._WAIT_SECONDS, _jobs._MOST_WAIT_SECONDS) == (0.1, 2.0)


def test_a_job_that_has_not_ended_in_time_is_a_timeout_and_goes_on(server, clock):
    job = stands(server, QUEUED, QUEUED, RUNNING)
    error = raised(TimeoutError, lambda: job.wait(1.0))
    assert str(error) == (
        f"job {JOB} had not ended after 1 seconds: it was running when the server was last asked"
    )
    # It is Python's own, and no failure of a request.
    assert not isinstance(error, qf.RemoteError)
    # The last wait was cut to what was left of the time, and the server
    # was asked once more as it ran out.
    assert clock.slept == pytest.approx([0.1, 0.2, 0.4, 0.3])
    assert asked(server) == 5
    # The job is as the server last said, and may be waited for again.
    assert job.status.state == "running"
    server.says("GET", STATUS, (200, written(state=SUCCEEDED)))
    assert job.wait(1.0).state == "succeeded"


def test_a_job_that_ended_as_the_time_ran_out_is_given_and_is_no_timeout(server, clock):
    # The last wait ends as the time runs out, and the server is asked
    # then: what it says of a job that ended meanwhile is given.
    job = stands(server, QUEUED, QUEUED, RUNNING, RUNNING, RUNNING, SUCCEEDED)
    assert job.wait(1.0).state == "succeeded"
    assert clock.slept == pytest.approx([0.1, 0.2, 0.4, 0.3])
    assert asked(server) == 5


def test_a_timeout_of_no_time_asks_once(server, clock):
    job = stands(server, QUEUED, RUNNING)
    for timeout in (0, 0.0):
        error = raised(TimeoutError, lambda: job.wait(timeout))
        assert str(error) == (
            f"job {JOB} had not ended after 0 seconds: it was running when the server was last "
            "asked"
        )
    assert asked(server) == 2 and clock.slept == []
    # One that has ended by then is given.
    server.says("GET", STATUS, (200, written(state=CANCELLED)))
    assert job.wait(0).state == "cancelled"


def test_a_timeout_is_a_number_of_seconds_or_none(server, clock, refused):
    job = stands(server, RUNNING)
    said = "it is how many seconds to wait at the most, or None for as long as the job takes"
    for timeout, kind, message in [
        ("5", TypeError, "timeout must be a number of seconds or None, not str"),
        (True, TypeError, "timeout must be a number of seconds or None, not bool"),
        ([5], TypeError, "timeout must be a number of seconds or None, not list"),
        (5j, TypeError, "timeout must be a number of seconds or None, not complex"),
        (-1, ValueError, f"timeout is negative: {said}"),
        (-0.001, ValueError, f"timeout is negative: {said}"),
        (float("-inf"), ValueError, f"timeout is negative: {said}"),
        (-(10**5000), ValueError, f"timeout is negative: {said}"),
        (float("nan"), ValueError, f"timeout is no number: {said}"),
    ]:
        refused(kind, message, job.wait, timeout)
        refused(kind, message, job.wait, timeout=timeout)
    # Refused before anything is asked, though the job has not ended.
    assert asked(server) == 0 and clock.slept == []
    # A time written as a whole number or not, however long.
    server.says("GET", STATUS, *[(200, written(state=RUNNING))] * 3, (200, written(state=FAILED)))
    assert job.wait(3600).state == "failed"
    for forever in (float("inf"), 10**5000, None):
        waiting = stands(server, *[RUNNING] * 4, SUCCEEDED)
        assert waiting.wait(forever).state == "succeeded"
    # The time is told as it was given, to the digits a reader needs.
    job = stands(server, RUNNING)
    for timeout, written_as in [(0.25, "0.25"), (2, "2"), (1.5, "1.5"), (1e-3, "0.001")]:
        error = raised(TimeoutError, lambda: job.wait(timeout))
        assert f"after {written_as} seconds" in str(error)


def test_a_request_that_fails_while_waiting_is_raised_and_the_job_is_as_it_was_last_given(
    server, clock
):
    job = stands(server, QUEUED)
    gone = refusal(404, "not_found", f"the server holds no job {JOB}")
    server.says("GET", STATUS, (200, written(state=RUNNING)), gone)
    error = raised(qf.NotFound, job.wait)
    assert str(error) == f"the server holds no job {JOB}"
    assert job.status.state == "running" and clock.slept == [0.1]
    # Waited for again, it is asked after again.
    server.says("GET", STATUS, (200, written(state=RUNNING)), (200, written(state=SUCCEEDED)))
    assert job.wait().state == "succeeded"


def test_a_state_a_later_revision_adds_does_not_end_a_job(server, clock):
    job = stands(server, QUEUED, {"kind": "paused", "since_ms": 5})
    error = raised(TimeoutError, lambda: job.wait(5))
    assert str(error) == (
        f"job {JOB} had not ended after 5 seconds: it was paused when the server was last asked"
    )
    assert (job.status.state, job.status.ended) == ("paused", False)


def test_a_wait_waits_by_the_clock(server):
    # By the real one: two answers of a job that runs, a tenth of a second
    # and then two tenths apart, and then its end.
    job = stands(server, QUEUED, RUNNING, RUNNING, SUCCEEDED)
    began = time.monotonic()
    assert job.wait(30).state == "succeeded"
    # By a clock that may tick seldom, as Windows' does.
    assert 0.25 <= time.monotonic() - began < 10
    # And a timeout is one by it.
    job = stands(server, RUNNING)
    began = time.monotonic()
    raised(TimeoutError, lambda: job.wait(0.25))
    assert 0.24 <= time.monotonic() - began < 10
