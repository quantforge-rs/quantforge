"""`docs/python-client.md`, held to the package: the client's signatures and
members, a job's and a status's, the token's argument and variable, the
exception each code of the protocol is raised as, what is raised where no
QuantForge server answered, the limits a request is held to, every refusal
it shows, and its examples, which run against a real server that holds the
four jobs the page says it does.

What the client does beyond what the page shows is in `test_client.py`,
against servers of the test's own, and in `test_client_jobs.py`, against a
real one on either backend."""

import doctest
import inspect
import re
import shlex

import pytest

import jobs
import quantforge_py as qf
import stores
from conftest import REMOTE_TOKEN
from quantforge_py import _errors, _jobs, _wire

PAGE = "docs/python-client.md"
PACKAGE_PAGE = "docs/python-package.md"
CONFIGURATION = "docs/configuration.md"

# Where the page's examples say their server is: where `serve run` listens
# unless it is told otherwise.
SHOWN = "127.0.0.1:9600"

ERRORS = [
    "RemoteError",
    "AuthError",
    "ValidationError",
    "NotFound",
    "Conflict",
    "ServerUnavailable",
    "VersionMismatch",
]


def prompted(examples: list[str]) -> list[str]:
    """The fences that are a session at Python's prompt."""
    return [example for example in examples if ">>> " in example]


@pytest.fixture(scope="module")
def session(page, tmp_path_factory):
    """The page's examples, run as one session from top to bottom against a
    real server that holds a job in each state, with that server's address
    in the place of the one the page shows, and no token from outside.
    Gives what the run came to, what it said, how many examples it had, the
    names it left, the server's jobs, and each job's status as a client of
    the test's own read it before the session and after."""
    directory = tmp_path_factory.mktemp("page")
    (directory / "place").mkdir()
    examples = prompted(page(PAGE).fences("python"))
    assert len(examples) >= 7
    with (
        jobs.one_of_each(stores.Place.sqlite(directory / "place"), directory) as fixture,
        pytest.MonkeyPatch.context() as patched,
    ):
        patched.delenv(REMOTE_TOKEN, raising=False)
        there = f"127.0.0.1:{fixture.server.port}"
        text = "\n".join(examples)
        assert text.count(SHOWN) >= 3
        parsed = doctest.DocTestParser().get_doctest(text.replace(SHOWN, there), {}, PAGE, PAGE, 0)
        own = qf.Client(fixture.url)
        before = {job.id: job.status for job in own.backtest.list()}
        said = []
        results = doctest.DocTestRunner(verbose=False).run(
            parsed, out=said.append, clear_globs=False
        )
        after = {job.id: job.status for job in own.backtest.list()}
        yield results, "".join(said), len(parsed.examples), parsed.globs, fixture, before, after


def test_every_example_on_the_page_runs_as_written(session):
    results, said, examples, names, fixture, before, after = session
    assert examples > 25
    assert results.failed == 0, said
    assert results.attempted == examples
    # What the examples asked is the server of the test, and its four jobs.
    assert names["client"].url == f"{fixture.url}/"
    assert [job.id for job in names["jobs"]] == fixture.newest_first
    # They cancelled the two that could be, and left the two that had ended.
    assert [before[job].state for job in fixture.newest_first] == [
        "queued",
        "running",
        "failed",
        "succeeded",
    ]
    assert [after[job].state for job in fixture.newest_first] == [
        "cancelled",
        "cancelled",
        "failed",
        "succeeded",
    ]
    assert after[fixture.failed] == before[fixture.failed]
    assert after[fixture.succeeded] == before[fixture.succeeded]


def called(function) -> str:
    """A method's signature as it is called, without `self`."""
    signature = inspect.signature(function)
    return str(signature.replace(parameters=list(signature.parameters.values())[1:]))


def public(kind) -> dict[str, str]:
    """A class's public members: each name with what it is."""
    return {
        name: type(member).__name__ for name, member in vars(kind).items() if not name[0] == "_"
    }


def test_the_page_gives_the_client_and_a_job_as_python_does(page, quoted, session):
    *_, names, _, _, _ = session
    backtests = type(names["client"].backtest)
    lines = page(PAGE).section("signatures")
    assert (lines[0], lines[-1]) == ("```text", "```")
    assert lines[1:-1] == [
        f"Client{inspect.signature(qf.Client)}",
        f"Client.handshake{called(qf.Client.handshake)}",
        f"Client.backtest.list{called(backtests.list)}",
        f"Client.backtest.get{called(backtests.get)}",
        f"Job.refresh{called(qf.Job.refresh)}",
        f"Job.cancel{called(qf.Job.cancel)}",
    ]
    # A client's members, each once: the class as it is called, its method,
    # its properties, and what `backtest` gives.
    assert public(qf.Client) == {
        "url": "property",
        "backtest": "property",
        "server_version": "property",
        "proto_version": "property",
        "handshake": "function",
    }
    assert public(backtests) == {"list": "function", "get": "function"}
    assert [quoted(member) for member, _ in page(PAGE).rows("members")] == [
        f"Client{inspect.signature(qf.Client)}",
        f"handshake{called(qf.Client.handshake)}",
        "url",
        "backtest",
        f"backtest.list{called(backtests.list)}",
        f"backtest.get{called(backtests.get)}",
        "server_version",
        "proto_version",
    ]
    # And a job's.
    assert public(qf.Job) == {
        "id": "property",
        "status": "property",
        "refresh": "function",
        "cancel": "function",
    }
    assert [quoted(member) for member, _ in page(PAGE).rows("job members")] == [
        "id",
        "status",
        f"refresh{called(qf.Job.refresh)}",
        f"cancel{called(qf.Job.cancel)}",
    ]
    # Every one says what it is, to `help()` as on the page.
    for kind in (qf.Client, backtests, qf.Job, qf.JobStatus):
        assert kind.__doc__, kind
        for name in public(kind):
            assert getattr(kind, name).__doc__, (kind, name)


def test_the_page_lists_every_field_of_a_status_in_the_order_a_status_holds_them(page, quoted):
    fields = [quoted(field) for field, _ in page(PAGE).rows("status fields")]
    assert fields == list(public(qf.JobStatus))
    assert set(public(qf.JobStatus).values()) == {"property"}
    # Each field but `ended`, which is told from the state, is one a status
    # keeps and is equal by.
    assert [name.removeprefix("_") for name in qf.JobStatus.__slots__] == [
        field for field in fields if field != "ended"
    ]
    # The class says of each what the page does.
    said = " ".join(qf.JobStatus.__doc__.split())
    for field in fields:
        assert f"`{field}`" in said, field


def test_the_page_says_what_a_status_holds_in_each_state_of_the_protocol(
    page, quoted, session, protocol_states
):
    *_, fixture, before, after = session
    rows = page(PAGE).rows("states")
    assert [quoted(state) for state, *_ in rows] == protocol_states
    assert list(_jobs._STATES) == protocol_states
    ended = [quoted(state) for state, *_, ended in rows if quoted(ended) == "True"]
    assert ended == list(_jobs._ENDED) == ["succeeded", "failed", "cancelled"]
    assert {quoted(ended) for *_, ended in rows} == {"True", "False"}
    # Each row, by a job of the page's server that was in that state: one
    # that was cancelled while queued and one while running among them.
    held = {
        "queued": [before[fixture.queued]],
        "running": [before[fixture.running]],
        "succeeded": [before[fixture.succeeded]],
        "failed": [before[fixture.failed]],
        "cancelled": [after[fixture.queued], after[fixture.running]],
    }
    assert after[fixture.queued].started_at_ms is None
    assert after[fixture.running].started_at_ms is not None
    for state, started, finished, error, ended in rows:
        for status in held[quoted(state)]:
            assert status.state == quoted(state)
            assert str(status.ended) == quoted(ended)
            if started.startswith("A time, or `None`"):
                assert status.started_at_ms is None or status.started_at_ms > 0
            else:
                assert (status.started_at_ms is None) == (started == "`None`"), state
            assert (status.finished_at_ms is None) == (finished == "`None`"), state
            assert (status.error_code is None) == (error == "`None`"), state
            assert (status.error_message is None) == (error == "`None`"), state


def test_the_page_lists_the_arguments_and_the_variable_the_command_line_has(page, quoted):
    rows = page(PAGE).rows("settings")
    assert [quoted(argument) for argument, *_ in rows] == list(
        inspect.signature(qf.Client).parameters
    )
    (_, no_variable, required, _), (_, variable, default, _) = rows
    assert (no_variable, quoted(required)) == ("—", "required")
    assert (quoted(variable), default) == (REMOTE_TOKEN, "none") == (_wire._TOKEN_VARIABLE, "none")
    # The token is the command line's `--remote-token`, with that flag's
    # variable and that flag's default, as the configuration matrix has it;
    # and `--remote` itself has no variable there either.
    matrix = {}
    for line in page(CONFIGURATION).lines:
        cells = [cell.strip() for cell in line.strip().strip("|").split("|")]
        if len(cells) == 5 and cells[1].startswith("`--remote"):
            matrix.setdefault(cells[1].strip("`").split(" ")[0], (cells[2], cells[3]))
    assert matrix["--remote-token"] == (variable, default)
    assert matrix["--remote"][0] == no_variable


def test_the_page_shows_how_a_client_is_given_its_token(page, monkeypatch, session):
    (fence,) = page(PAGE).fences("bash")
    name, _, value = fence.strip().removeprefix("export ").partition("=")
    assert fence.startswith("export ") and name == REMOTE_TOKEN
    (example,) = [text for text in page(PAGE).fences("python") if ">>> " not in text]
    monkeypatch.setenv(name, shlex.split(value)[0])
    made = {"qf": qf}
    exec(compile(example, PAGE, "exec"), made)
    # Making them asked no server anything: none of the three is there.
    assert repr(made["exported"]) == "<Client https://qf.example/, with a token>"
    assert repr(made["other"]) == "<Client https://qf.example/, with a token>"
    assert repr(made["plain"]) == "<Client http://lab.example:9600/>"
    assert f"`{made['exported']!r}`" in page(PAGE).text
    # A reader who runs the page from its top still has the client of the
    # examples around it: this one takes no name of theirs.
    *_, names, _, _, _ = session
    theirs = {name for name in names if not name.startswith("__")}
    assert {"client", "jobs", "job", "status"} <= theirs
    assert set(made) & theirs == {"qf"}
    # Without the argument that says it sends none, the last is refused.
    with pytest.raises(ValueError, match="The token is QF_REMOTE_TOKEN's"):
        qf.Client("http://lab.example:9600")


def test_the_page_names_every_exception_and_the_one_each_code_is_raised_as(
    page, quoted, protocol_codes
):
    assert [quoted(name) for name, *_ in page(PAGE).rows("exceptions")] == ERRORS
    first = qf.__all__.index(ERRORS[0])
    assert qf.__all__[first : first + len(ERRORS)] == ERRORS
    # No other name the package publishes is an exception.
    assert ERRORS == [
        name
        for name in qf.__all__
        if isinstance(getattr(qf, name), type) and issubclass(getattr(qf, name), BaseException)
    ]
    rows = [
        (quoted(code), int(status), quoted(kind)) for code, status, kind in page(PAGE).rows("codes")
    ]
    # Every code the protocol has, in its order, with its HTTP status.
    assert {code: status for code, status, _ in rows} == protocol_codes
    assert [code for code, *_ in rows] == list(protocol_codes)
    # And the exception the client raises for it.
    for code, _, kind in rows:
        assert _errors._RAISED.get(code, qf.RemoteError) is getattr(qf, kind), code
    assert {kind for *_, kind in rows} == set(ERRORS)


def test_the_page_says_what_is_raised_where_no_quantforge_server_answered(page, quoted):
    rows = page(PAGE).rows("without an envelope")
    raised = {}
    for what, kind in rows:
        for status in re.findall(r"`(\d{3})`", what):
            raised[int(status)] = getattr(qf, quoted(kind))
    assert raised == _errors._RAISED_BY_STATUS
    assert [quoted(kind) for _, kind in rows] == [
        "ServerUnavailable",
        "AuthError",
        "ServerUnavailable",
        "RemoteError",
        "RemoteError",
        "RemoteError",
        "RemoteError",
    ]


def test_the_page_states_the_limits_a_request_is_held_to(page):
    limits = [limit for _, limit in page(PAGE).rows("limits")]
    assert limits == [
        f"{_wire._CONNECT_SECONDS} seconds",
        f"{_wire._READ_SECONDS} seconds",
        str(_wire._ATTEMPTS),
        f"The seconds `Retry-After` names, {_wire._MOST_RETRY_SECONDS} at most",
        f"{_wire._MOST_ANSWER_BYTES // (1024 * 1024)} MiB",
    ]
    assert _wire._MOST_ANSWER_BYTES % (1024 * 1024) == 0
    # And the one protocol version the package speaks.
    assert _wire._MIN_SUPPORTED == _wire._PROTO_VERSION
    assert f"This package speaks version {_wire._PROTO_VERSION}." in page(PAGE).text


def test_every_refusal_on_the_page_is_raised_as_written(page, session, quoted, refused):
    *_, names, fixture, _, _ = session
    rows = page(PAGE).rows("refusals")
    assert len(rows) >= 15
    shown = set()
    for call, raises in rows:
        call, raises = quoted(call), quoted(raises)
        kind, _, message = raises.partition(": ")
        with pytest.raises(Exception) as caught:
            eval(call, names)
        assert (type(caught.value).__name__, str(caught.value)) == (kind, message), call
        shown.add(kind)
    assert shown == {"TypeError", "ValueError"}
    # The page's two sentences of a token the variable holds.
    said = " ".join(page(PAGE).text.split())
    with pytest.MonkeyPatch.context() as patched:
        for token, url in [("two words", fixture.url), ("TOKEN", "http://qf.example:9600")]:
            patched.setenv(REMOTE_TOKEN, token)
            with pytest.raises(ValueError) as caught:
                qf.Client(url)
            assert f"`{caught.value}`" in said, caught.value
            assert REMOTE_TOKEN in str(caught.value)


def test_the_page_writes_a_job_and_a_client_as_python_writes_them(page, session):
    *_, fixture, before, _ = session
    (written,) = re.findall(r"`(<Job [0-9a-f]{32} running>)`", page(PAGE).text)
    job = qf.Client(fixture.url).backtest.get(fixture.failed)
    assert repr(job).replace(job.id, written.split()[1]) == written.replace("running", "failed")
    assert f"<Client http://{SHOWN}/>" in page(PAGE).text
    assert repr(qf.Client(f"http://{SHOWN}")) == f"<Client http://{SHOWN}/>"


def test_what_the_page_leaves_to_later_stories_is_what_the_packages_page_does(page, quoted):
    left = dict(row for row in table(page(PAGE), "## What is left"))
    assert sorted(left.values()) == ["QUA-345", "QUA-462", "QUA-468"]
    package = dict(row for row in table(page(PACKAGE_PAGE), "## What is left"))
    # A client's own stories, as the package's page lists them among what is
    # left of the package; the server's is the server's.
    assert {story for story in left.values() if story in package.values()} == {
        "QUA-462",
        "QUA-468",
    }


def table(read, heading: str) -> list[list[str]]:
    """The rows of the first table under a heading of a page."""
    at = read.lines.index(heading)
    rows = []
    for line in read.lines[at + 1 :]:
        if line.startswith("## "):
            break
        if line.startswith("|"):
            rows.append([cell.strip() for cell in line.strip().strip("|").split("|")])
    assert len(rows) > 2, heading
    return rows[2:]
