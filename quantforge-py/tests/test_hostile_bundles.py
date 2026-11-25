"""Bundles that mean harm, run by a QuantForge server's workers: `quantforge
serve run --job-python` this interpreter, each bundle uploaded and a backtest
of it submitted over the job routes, as any bundle is
(docs/security/remote-execution.md).

Each bundle tries one thing a job has no business doing: to start processes
without end, to take more memory than it may, to reach the network, to read
or write a file outside its scratch directory, to read what its server
holds, to signal its server, to run past its CPU time or its wall clock, to
say in its worker's place how the job ended, or to make its failure or its
log as long as it likes. Where the server's workers say they enforce the
guard that holds it, the attempt fails and the job's state or its log says
so; where they say they enforce none, the attempt goes through, as the
page says it does there, and that is held to as well. After every one of
them the server runs an honest job.

Nothing here harms the machine the tests run on: a bundle starts sixteen
processes that wait and exit, takes memory it never writes to, connects to
a listener of the test's own, and writes into directories the test made.

`QUANTFORGE_TEST_SANDBOX_REQUIRED=1` says that the sandbox has to hold where
the tests run, as on CI's Linux legs (docs/ci.md): a guard the workers do
not enforce then fails its test, so that no run passes for a sandbox that
was never there."""

import json
import os
import secrets
import signal
import socket
import sys
import time

import pytest

import jobs
import stores

PAGE = "docs/security/remote-execution.md"

# Set to 1 where the workers' sandbox has to hold.
SANDBOX_REQUIRED = "QUANTFORGE_TEST_SANDBOX_REQUIRED"

FIRST = stores.NOON
THIRTIETH = FIRST + 29 * stores.MINUTE

# The memory every server here allows a job: far above what a worker and an
# honest job of thirty bars take, and little enough to be taken in a moment.
MEMORY_MIB = 1024

# How a worker that was sent what no worker sends fails its job.
UNREADABLE = {
    "code": "internal",
    "message": "the job's worker sent the server something it cannot read",
}

# Every hostile bundle of the suite, by the name the page gives it, with
# what holds it: a guard, as `worker_isolation:` names one, or `server`,
# for what the server itself holds a job to on every system.
SUITE = {}


def hostile(name, held_by):
    """Lists a test's bundle in the suite, as the page lists it."""

    def listed(test):
        assert name not in SUITE, name
        SUITE[name] = held_by
        return test

    return listed


# What every bundle here starts with: a line a test reads back from the
# job's log, and a strategy that holds nothing.
PRELUDE = """
import json
import os
import sys
import time

import quantforge_py as qf


def found(**what):
    print("found " + json.dumps(what, sort_keys=True))


def tried(attempts):
    answers = {}
    for name, attempt in attempts.items():
        try:
            attempt()
            answers[name] = "done"
        except OSError as error:
            answers[name] = "refused: " + type(error).__name__
    return answers


class Quiet(qf.Strategy):
    def __init__(self, **given):
        self.given = given

    def on_bar(self, ctx, bar):
        return None
"""

HONEST = PRELUDE + """

class Hostile(Quiet):
    pass
"""


class Ran:
    """A job that has ended: its state, its log, and the line its bundle
    left for the test."""

    def __init__(self, server, job):
        self.job = job
        self.state = server.ended(job)
        self.log = server.log(job)
        self.lines = [line["message"] for line in self.log["lines"]]

    @property
    def found(self):
        (line,) = [line for line in self.lines if line.startswith("found ")]
        return json.loads(line.removeprefix("found "))

    @property
    def error(self):
        assert self.state["kind"] == "failed", (self.state, self.lines)
        return self.state["error"]

    def succeeded(self):
        assert self.state["kind"] == "succeeded", (self.state, self.lines)
        return self.found

    def refused(self, error):
        """Holds the job to having failed as its bundle's own fault, of the
        exception the system refused its attempt with, its traceback in the
        job's log."""
        assert self.error["code"] == "invalid_request", (self.error, self.lines)
        assert f": {error}" in self.error["message"], self.error
        assert self.error["message"].endswith("; its traceback is in the job's log")
        traceback = [line["message"] for line in self.log["lines"] if line["level"] == "error"]
        assert traceback[0] == "Traceback (most recent call last):", self.lines
        assert any(line.startswith(error) for line in traceback), traceback


class Served:
    """A running server, what stands around it for a hostile job to reach
    for, and what its workers say they enforce."""

    def __init__(self, server, directory, cpu_secs, wall_secs):
        self.server = server
        self.cpu_secs, self.wall_secs = cpu_secs, wall_secs
        self.scratch = directory / "scratch"
        # Outside every scratch directory, with a file to read or change.
        self.outside = directory / "outside"
        self.outside.mkdir()
        self.canary = self.outside / "canary.txt"
        self.canary.write_text("kept")
        self.guards = dict(
            guard.split(" ", 1) for guard in server.said["worker_isolation"].split(", ")
        )

    def holds(self, guard):
        """Whether the workers enforce `guard`. Where the run was told that
        the sandbox has to hold, one they do not fails the test."""
        held = self.guards[guard] != "none"
        if not held and os.environ.get(SANDBOX_REQUIRED) == "1":
            pytest.fail(
                f"the workers enforce no {guard} guard here ({self.server.said['worker_isolation']}),"
                f" and {SANDBOX_REQUIRED} says the sandbox has to hold"
            )
        return held

    def run(self, source, **params):
        """Runs a bundle of `source`, its parameters `params`, to its end;
        and after it an honest job, which the server runs as if nothing had
        been tried."""
        ran = self.only(source, **params)
        honest = self.only(HONEST)
        assert honest.state["kind"] == "succeeded", (honest.state, honest.lines)
        return ran

    def only(self, source, **params):
        written = "\n".join(f"{name} = {json.dumps(value)}" for name, value in params.items())
        files = {
            "quantforge.toml": jobs.manifest("strategy.py:Hostile", params=written),
            "strategy.py": source.encode(),
        }
        job = self.server.submit(self.server.upload(files), FIRST, THIRTIETH)
        return Ran(self.server, job)


def serving(backend, request, tmp_path_factory, *, cpu_secs, wall_secs):
    """A server on a store of `backend` that holds thirty minutes of
    candles, its workers held to `cpu_secs`, `MEMORY_MIB` and `wall_secs`,
    and holding two secrets in its environment, as a server started from a
    shell that exports an exchange's credentials would."""
    directory = tmp_path_factory.mktemp(f"hostile-{backend}")
    (directory / "scratch").mkdir()
    (directory / "place").mkdir()
    held = {"QF_API_KEY": secrets.token_hex(16), "QF_API_SECRET": secrets.token_hex(16)}
    limits = (
        "--job-cpu-secs",
        str(cpu_secs),
        "--job-memory-mib",
        str(MEMORY_MIB),
        "--job-wall-secs",
        str(wall_secs),
    )

    def started(place):
        with stores.Exchange({("BTCUSDT", "1m"): jobs.golden_klines()[:30]}) as exchange:
            place.sync(exchange)
        with jobs.served(place, directory / "scratch", *limits, variables=held) as server:
            served = Served(server, directory, cpu_secs, wall_secs)
            served.secret = held["QF_API_SECRET"]
            yield served

    if backend == "sqlite":
        yield from started(stores.Place.sqlite(directory / "place"))
        return
    with stores.Database(request.getfixturevalue("postgres_server")) as database:
        yield from started(stores.Place.postgres(database, directory / "place"))


@pytest.fixture(scope="module", params=["sqlite", "postgres"])
def served(request, tmp_path_factory):
    """One server for the module's tests on each backend in turn, with
    limits no bundle that is held by something else runs into."""
    yield from serving(request.param, request, tmp_path_factory, cpu_secs=20, wall_secs=30)


# The bundles that run until a limit ends them take seconds each, and no
# limit depends on the store: each limit has one server, on SQLite, on
# which it is the one a job runs into, however slowly the machine runs it.


@pytest.fixture(scope="module")
def out_of_cpu(request, tmp_path_factory):
    """A server whose jobs run out of CPU time long before their wall
    clock."""
    yield from serving("sqlite", request, tmp_path_factory, cpu_secs=2, wall_secs=45)


@pytest.fixture(scope="module")
def out_of_time(request, tmp_path_factory):
    """A server whose jobs run out of wall-clock time long before their
    CPU time."""
    yield from serving("sqlite", request, tmp_path_factory, cpu_secs=600, wall_secs=3)


def test_the_variable_that_says_the_sandbox_has_to_hold_is_unset_or_1():
    assert os.environ.get(SANDBOX_REQUIRED, "") in ("", "1"), (
        f"{SANDBOX_REQUIRED} is {os.environ[SANDBOX_REQUIRED]!r}: set it to 1 where the workers' "
        "sandbox has to hold, or leave it unset"
    )


def test_on_linux_the_workers_enforce_every_guard_where_the_sandbox_has_to_hold(out_of_time):
    # Each guard, where it does not hold, fails the test that needs it. This
    # one says so for all four at once, and for a kernel without Landlock
    # in the words of the page.
    said = out_of_time.server.said["worker_isolation"]
    if sys.platform.startswith("linux"):
        assert out_of_time.guards["cpu"] == "rlimit", said
        assert out_of_time.guards["memory"] == "rlimit", said
        assert out_of_time.guards["network"] == "seccomp", said
        assert out_of_time.guards["files"] in ("landlock", "none"), said
    for guard in out_of_time.guards:
        if os.environ.get(SANDBOX_REQUIRED) == "1":
            assert out_of_time.holds(guard), said


FORK_BOMB = PRELUDE + """
import subprocess


class Hostile(Quiet):
    def on_start(self, ctx):
        # A bomb's processes each start more. These wait and exit, so that
        # nothing is left where nothing stops them.
        waits = self.given["seconds"]
        started = []
        for _ in range(16):
            if hasattr(os, "fork"):
                child = os.fork()
                if child == 0:
                    try:
                        time.sleep(waits)
                    finally:
                        os._exit(0)
            else:
                waiting = f"import time; time.sleep({waits})"
                child = subprocess.Popen([sys.executable, "-c", waiting]).pid
            started.append(child)
        found(started=started)
"""


@hostile("fork-bomb", "network")
def test_a_bundle_that_starts_processes_starts_none_or_none_that_outlives_its_job(served):
    if served.holds("network"):
        # The filter that refuses a socket refuses a process too.
        served.run(FORK_BOMB, seconds=20).refused("PermissionError")
        return
    posix = os.name == "posix"
    started = served.run(FORK_BOMB, seconds=20 if posix else 5).succeeded()["started"]
    assert len(started) == 16
    if not posix:
        # Windows kills a worker alone: what it started goes on, and ends by
        # itself here.
        return
    # Killed with the worker's process group as the job ended.
    deadline = time.monotonic() + jobs.PATIENCE
    left = set(started)
    while left:
        for child in sorted(left):
            try:
                os.kill(child, 0)
            except ProcessLookupError:
                left.discard(child)
        assert time.monotonic() < deadline, f"processes {sorted(left)} outlived their job"
        time.sleep(0.05)


MEMORY_BOMB = PRELUDE + """

class Hostile(Quiet):
    def on_start(self, ctx):
        # Address space, which the limit counts, and never written to.
        taken = []
        while len(taken) * 64 < self.given["mib"]:
            taken.append(bytearray(64 << 20))
        found(taken_mib=len(taken) * 64)
"""


@hostile("memory-bomb", "memory")
def test_a_bundle_that_takes_more_memory_than_it_may_fails_at_the_limit(served):
    beyond = MEMORY_MIB + 256
    ran = served.run(MEMORY_BOMB, mib=beyond)
    if not served.holds("memory"):
        assert ran.succeeded() == {"taken_mib": beyond}
        return
    # As the allocation Python was refused, or as the worker's own.
    assert ran.error["code"] == "invalid_request", ran.error
    message = ran.error["message"]
    limit = f"the job needed more than the {MEMORY_MIB} MiB of memory it may use, and was stopped"
    assert ": MemoryError" in message or message == limit, message


SOCKET = PRELUDE + """
import socket


class Hostile(Quiet):
    def on_start(self, ctx):
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as connection:
            connection.settimeout(5)
            connection.connect(("127.0.0.1", self.given["port"]))
            connection.sendall(b"out")
        found(connected=True)
"""


SOCKET_PAIR = PRELUDE + """
import socket


class Hostile(Quiet):
    def on_start(self, ctx):
        ours, theirs = socket.socketpair(socket.AF_UNIX, socket.SOCK_DGRAM)
        ours.close()
        theirs.close()
        found(paired=True)
"""


@hostile("socket", "network")
def test_a_bundle_that_opens_a_connection_reaches_no_one(served):
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        listener.listen(4)
        listener.settimeout(0.5)
        ran = served.run(SOCKET, port=listener.getsockname()[1])
        if not served.holds("network"):
            assert ran.succeeded() == {"connected": True}
            connection, _ = listener.accept()
            connection.close()
            return
        ran.refused("PermissionError")
        with pytest.raises(TimeoutError):
            listener.accept()
    # Nor a pair of sockets of its own: one of a pair of datagram sockets
    # can be sent to the address of any Unix socket on the machine.
    served.run(SOCKET_PAIR).refused("PermissionError")


OUTSIDE_READ = PRELUDE + """

class Hostile(Quiet):
    def on_start(self, ctx):
        with open(self.given["path"], "rb") as file:
            found(read_bytes=len(file.read()))
"""


@hostile("outside-read", "files")
def test_a_bundle_that_reads_a_file_outside_its_scratch_directory_reads_none(served):
    served.canary.write_text("kept")
    paths = [str(served.canary)]
    if os.name == "posix":
        paths.append("/etc/passwd")
    held = served.holds("files")
    for path in paths:
        ran = served.run(OUTSIDE_READ, path=path)
        if held:
            ran.refused("PermissionError")
        else:
            assert ran.succeeded() == {"read_bytes": os.path.getsize(path)}
    # Its own files it reads: the bundle's root is its working directory.
    assert served.run(OUTSIDE_READ, path="quantforge.toml").succeeded()["read_bytes"] > 0


OUTSIDE_WRITE = PRELUDE + """

def write(path):
    with open(path, "w") as file:
        file.write("escaped")


class Hostile(Quiet):
    def on_start(self, ctx):
        outside = self.given["outside"]
        canary = os.path.join(outside, "canary.txt")

        def through_a_link():
            os.symlink(outside, "way-out")
            write(os.path.join("way-out", "escaped-linked.txt"))

        def by_another_name():
            os.link(canary, "canary-too")
            write("canary-too")

        attempts = {
            "a path that climbs out": lambda: write(
                os.path.join("..", "..", "escaped-climbing.txt")
            ),
            "a path from the root": lambda: write(os.path.join(outside, "escaped-rooted.txt")),
            "the canary, by its path": lambda: write(canary),
        }
        if os.name == "posix":
            attempts["a link to a directory outside"] = through_a_link
            attempts["another name for the canary"] = by_another_name
        found(attempts=tried(attempts))
"""


@hostile("outside-write", "files")
def test_a_bundle_that_writes_outside_its_scratch_directory_leaves_nothing_there(served):
    served.canary.write_text("kept")
    attempts = served.run(OUTSIDE_WRITE, outside=str(served.outside)).succeeded()["attempts"]
    climbed = list(served.scratch.rglob("escaped-climbing.txt"))
    try:
        if not served.holds("files"):
            for attempt in list(attempts)[:3]:
                assert attempts[attempt] == "done", attempts
            assert served.canary.read_text() == "escaped"
            return
        assert len(attempts) >= 3, attempts
        for attempt, answer in attempts.items():
            assert answer.startswith("refused: "), (attempt, attempts)
        assert served.canary.read_text() == "kept"
        assert sorted(path.name for path in served.outside.iterdir()) == ["canary.txt"]
        assert climbed == []
    finally:
        for left in served.outside.iterdir():
            left.unlink()
        served.canary.write_text("kept")


SERVER_SECRETS = PRELUDE + """
import subprocess


class Hostile(Quiet):
    def on_start(self, ctx):
        server, secret = os.getppid(), self.given["secret"].encode()
        seen = {}
        if sys.platform.startswith("linux"):
            for name in ("environ", "cmdline"):
                try:
                    with open(f"/proc/{server}/{name}", "rb") as file:
                        seen[name] = secret in file.read()
                except OSError as error:
                    seen[name] = "refused: " + type(error).__name__
        elif sys.platform == "darwin":
            asked = ["/bin/ps", "eww", "-o", "command=", "-p", str(server)]
            seen["ps"] = secret in subprocess.run(asked, capture_output=True, timeout=30).stdout
        # Its own environment and arguments, which hold nothing of it.
        own = repr(sorted(os.environ.items())) + repr(sys.argv)
        seen["own"] = self.given["secret"] in own
        found(**seen)
"""


@hostile("server-secrets", "server")
def test_a_bundle_that_reads_its_servers_environment_finds_no_secret_where_linux_closes_it(served):
    # Where the page says a job can read it, the test says so too: on
    # macOS the environment of every process of the server's user is one
    # `ps` away, and no guard of the process profile stands in between.
    seen = served.run(SERVER_SECRETS, secret=served.secret).succeeded()
    assert seen.pop("own") is False
    if sys.platform.startswith("linux"):
        # Not dumpable, whatever else holds: its environment is root's to read.
        assert seen["environ"] == "refused: PermissionError", seen
        assert seen["cmdline"] in (False, "refused: PermissionError"), seen
        if served.holds("files"):
            assert seen["cmdline"] == "refused: PermissionError", seen
    elif sys.platform == "darwin":
        assert seen == {"ps": True}, seen
    else:
        assert seen == {}, seen


SIGNAL_SERVER = PRELUDE + """

class Hostile(Quiet):
    def on_start(self, ctx):
        server = os.getppid()
        # The signal that only asks whether one may be sent, and the limits
        # read, not set: neither changes anything where it is allowed.
        attempts = {
            "a signal for the server": lambda: os.kill(server, 0),
            "a signal for itself": lambda: os.kill(os.getpid(), 0),
        }
        import resource

        if hasattr(resource, "prlimit"):
            attempts["the server's limits"] = lambda: resource.prlimit(
                server, resource.RLIMIT_CORE
            )
        if sys.platform.startswith("linux"):
            # The server named as the owner of a pipe, whom the kernel
            # would signal once the pipe was told to say when it has
            # something to read. It is never told to.
            import fcntl

            read, write = os.pipe()
            attempts["a descriptor's signals for the server"] = lambda: fcntl.fcntl(
                read, fcntl.F_SETOWN, server
            )
            attempts["a descriptor's signals for itself"] = lambda: fcntl.fcntl(
                write, fcntl.F_SETOWN, os.getpid()
            )
        found(attempts=tried(attempts))
"""


@hostile("signal-server", "network")
@pytest.mark.skipif(os.name != "posix", reason="a signal that only asks is POSIX's")
def test_a_bundle_that_signals_its_server_or_takes_its_limits_is_refused(served):
    attempts = served.run(SIGNAL_SERVER).succeeded()["attempts"]
    assert attempts.pop("a signal for itself") == "done"
    assert attempts.pop("a descriptor's signals for itself", "done") == "done"
    if not served.holds("network"):
        assert attempts.pop("a signal for the server") == "done"
        return
    # The same filter: no signal, and no limit, for any process but itself.
    assert attempts == {
        "a signal for the server": "refused: PermissionError",
        "the server's limits": "refused: PermissionError",
        "a descriptor's signals for the server": "refused: PermissionError",
    }


CPU_SPIN = PRELUDE + """
import signal


class Hostile(Quiet):
    def on_start(self, ctx):
        if self.given["ignoring"]:
            signal.signal(signal.SIGXCPU, signal.SIG_IGN)

    def on_bar(self, ctx, bar):
        while True:
            pass
"""

def stopped_for_cpu(server):
    """How a job that used up its CPU time on `server` fails."""
    return {
        "code": "invalid_request",
        "message": f"the job used the {server.cpu_secs} s of CPU time it may use, and was stopped",
    }


def stopped_for_time(server):
    """How a job that ran past its wall clock on `server` fails."""
    return {
        "code": "invalid_request",
        "message": f"the job ran for the {server.wall_secs} s of wall-clock time it may run, and "
        "was stopped",
    }


@hostile("cpu-spin", "cpu")
def test_a_bundle_that_spins_is_stopped_at_its_cpu_time_or_at_its_wall_clock(
    out_of_cpu, out_of_time
):
    if out_of_cpu.holds("cpu"):
        assert out_of_cpu.run(CPU_SPIN, ignoring=False).error == stopped_for_cpu(out_of_cpu)
    else:
        ran = out_of_time.run(CPU_SPIN, ignoring=False)
        assert ran.error == stopped_for_time(out_of_time)


@hostile("cpu-spin-ignoring-the-signal", "cpu")
@pytest.mark.skipif(not hasattr(signal, "SIGXCPU"), reason="no signal for CPU time here")
def test_a_bundle_that_ignores_the_signal_for_its_cpu_time_is_stopped_all_the_same(
    out_of_cpu, out_of_time
):
    if sys.platform.startswith("linux") and out_of_cpu.holds("cpu"):
        # The kernel kills it a second past the limit, and says no more.
        assert out_of_cpu.run(CPU_SPIN, ignoring=True).error == {
            "code": "internal",
            "message": "the job's worker stopped before the job finished: it was stopped by "
            "signal 9 (SIGKILL)",
        }
    else:
        # macOS signals once and lets a job that ignores it run on.
        ran = out_of_time.run(CPU_SPIN, ignoring=True)
        assert ran.error == stopped_for_time(out_of_time)


WALL_SLEEP = PRELUDE + """

class Hostile(Quiet):
    def on_bar(self, ctx, bar):
        time.sleep(3600)
"""


@hostile("wall-sleep", "server")
def test_a_bundle_that_waits_is_stopped_at_its_wall_clock(out_of_time):
    began = time.monotonic()
    assert out_of_time.run(WALL_SLEEP).error == stopped_for_time(out_of_time)
    assert time.monotonic() - began >= out_of_time.wall_secs


FORGED_FRAME = PRELUDE + """

class Hostile(Quiet):
    def on_start(self, ctx):
        # The worker's frames go out on a descriptor of its own, which the
        # bundle's code holds as the worker does.
        frame = self.given["frame"].encode()
        if self.given["framed"]:
            frame = len(frame).to_bytes(4, "big") + frame
        for descriptor in range(3, 64):
            try:
                os.write(descriptor, frame)
            except OSError:
                pass
        time.sleep(60)
"""

FORGED = {
    "a result of its own naming": {
        "type": "finished",
        "outcome": {
            "kind": "succeeded",
            "results": [{"name": "metrics", "media_type": "application/json", "text": "{}"}],
        },
    },
    "a trade of another symbol": {
        "type": "trades",
        "rows": [
            {
                "symbol": "ETHUSDT",
                "entry_time_ms": 0,
                "exit_time_ms": 1,
                "entry_price": "1",
                "exit_price": "2",
                "qty": "1",
                "gross_quote_pnl": "1",
            }
        ],
    },
    "more equity rows than the job has bars": {
        "type": "equity",
        "rows": [{"open_time_ms": n, "equity": "1000000"} for n in range(31)],
    },
    "a failure no job fails as": {
        "type": "finished",
        "outcome": {"kind": "failed", "error": {"code": "not_found", "message": "forged"}},
    },
    "a frame of no kind a worker sends": {"type": "nothing a worker says"},
}


@hostile("forged-frames", "server")
def test_a_bundle_that_speaks_in_its_workers_place_fails_its_job(served):
    for what, frame in FORGED.items():
        ran = served.run(FORGED_FRAME, frame=json.dumps(frame), framed=True)
        assert ran.error == UNREADABLE, what
        with pytest.raises(jobs.Refused) as refused:
            served.server.json("GET", f"/v1/backtests/{ran.job}/result")
        assert (refused.value.status, refused.value.code) == (409, "conflict"), what
    # And bytes that are no frame at all.
    assert served.run(FORGED_FRAME, frame="no frame", framed=False).error == UNREADABLE


LONG_FAILURE = PRELUDE + """

class Hostile(Quiet):
    name = "n" * 200_000

    def on_start(self, ctx):
        raise ValueError("x" * self.given["length"])
"""


@hostile("long-failure", "server")
def test_a_bundle_that_fails_at_great_length_fails_in_a_few_kilobytes(served):
    ran = served.run(LONG_FAILURE, length=4_000_000)
    assert ran.error["code"] == "invalid_request"
    message = ran.error["message"]
    assert message.startswith('the strategy "nnnn')
    assert message.endswith(" more bytes left out]") and "n… [" in message, message[-80:]
    assert len(message.encode()) < 8192 + 64, len(message.encode())
    # What every answer that shows the job carries.
    status, _, body = served.server.request("GET", f"/v1/backtests/{ran.job}")
    assert status == 200 and len(body) < 16_384, len(body)


LOG_FLOOD = PRELUDE + """

class Hostile(Quiet):
    def on_start(self, ctx):
        for n in range(self.given["lines"]):
            print(f"line {n} " + "x" * 1000)
        found(done=True)
"""


@hostile("log-flood", "server")
def test_a_bundle_that_floods_its_log_keeps_its_newest_megabyte(served):
    ran = served.run(LOG_FLOOD, lines=5_000)
    assert ran.succeeded() == {"done": True}
    kept = [line for line in ran.lines if line.startswith("line ")]
    assert kept[-1].startswith("line 4999 "), kept[-1][:40]
    assert sum(len(line) for line in ran.lines) <= 1 << 20
    assert len(kept) < 5_000


EXIT = PRELUDE + """

class Hostile(Quiet):
    def on_bar(self, ctx, bar):
        os._exit(7)
"""


@hostile("exit", "server")
def test_a_bundle_that_ends_its_worker_fails_its_own_job_alone(served):
    assert served.run(EXIT).error == {
        "code": "internal",
        "message": "the job's worker stopped before the job finished: it exited with status 7",
    }


def test_ci_says_that_the_sandbox_has_to_hold_wherever_it_tests_on_linux(repository, page):
    workflow = (repository / ".github" / "workflows" / "ci.yml").read_text(encoding="utf-8")
    always = f'          {SANDBOX_REQUIRED}: "1"\n'
    on_linux = f"          {SANDBOX_REQUIRED}: " + "${{ runner.os == 'Linux' && '1' || '' }}\n"
    # The jobs on Linux alone say so outright: `postgres` and `python`. The
    # jobs with a leg for each platform say so for the Linux one: both of
    # `checks`' test steps, and `wheel`'s.
    assert workflow.count(always) == 2, workflow.count(always)
    assert workflow.count(on_linux) == 3, workflow.count(on_linux)
    assert workflow.count(SANDBOX_REQUIRED) == 5
    # Every job that runs tests is in the page's table, and no other.
    listed = [job for job, _ in page("docs/ci.md").rows("sandbox required")]
    assert listed == ["`checks`", "`postgres 16`, `postgres 17`, `postgres 18`", "`python`", "`wheel`"]


def test_the_page_lists_the_suite_and_what_holds_each_bundle(page, quoted):
    listed = {quoted(name): held_by for name, _, held_by in page(PAGE).rows("hostile bundles")}
    assert list(listed.items()) == list(SUITE.items())
    # Each a guard as `worker_isolation:` names one, or the server itself.
    assert set(SUITE.values()) == {"cpu", "memory", "network", "files", "server"}
