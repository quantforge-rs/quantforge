"""`docs/python-jobs.md`, held to the package: the calls' signatures, what
each asks and gives, the fields of a line, of a result, of a result's file,
of a provenance and of a segment, the metrics as the result spec has them,
the two frames as the page of the DataFrames has them, what is raised, the
limits, every refusal it shows, and its examples, which run against a real
server that stores the day of candles the page says it does, from a
directory that holds the strategy it says it submits.

What the calls do beyond what the page shows is in `test_wait.py`,
`test_logs.py`, `test_results.py` and `test_provenance.py`, against servers
of the test's own, and against a real one on either backend in
`test_handle_jobs.py`."""

import builtins
import decimal
import doctest
import inspect
import linecache
import re
import shutil

import pytest

import jobs
import quantforge_py as qf
import stores
from conftest import REMOTE_TOKEN
from quantforge_py import _jobs, _logs, _results, _websocket, _wire

PAGE = "docs/python-jobs.md"
PANDAS_PAGE = "docs/python-pandas.md"
RESULT_SPEC = "docs/result-spec.md"
CLIENT_PAGE = "docs/python-client.md"
BUNDLES_PAGE = "docs/python-bundles.md"
PACKAGE_PAGE = "docs/python-package.md"

# Where the page's examples say their server is.
SHOWN = "127.0.0.1:9600"

# What of the page's examples needs pandas: the two frames.
FRAMES = ("trades_df(", "equity_df(")


@pytest.fixture(scope="module")
def session(page, tmp_path_factory, without_pandas):
    """The page's examples, run as one session from top to bottom against a
    real server that stores the goldens' day of candles, in a directory
    that holds `my_strategy`, with that server's address in the place of
    the one the page shows. Where there is no pandas, the examples of the
    frames are left out, which nothing after them needs. Gives what the
    run came to, what it said, how many examples it had, the names it left,
    and the server."""
    directory = tmp_path_factory.mktemp("jobs-page")
    for name in ("place", "scratch", "work"):
        (directory / name).mkdir()
    place = stores.Place.sqlite(directory / "place")
    with stores.Exchange({("BTCUSDT", "1m"): jobs.golden_klines()}) as exchange:
        place.sync(exchange)
    strategy = directory / "work" / "my_strategy"
    strategy.mkdir()
    shutil.copy(jobs.GOLDEN / "sma_cross.py", strategy / "sma_cross.py")
    examples = [example for example in page(PAGE).fences("python") if ">>> " in example]
    assert len(examples) >= 10
    of_the_frames = [example for example in examples if any(call in example for call in FRAMES)]
    assert len(of_the_frames) == 1
    if without_pandas:
        examples.remove(of_the_frames[0])
    with (
        jobs.served(place, directory / "scratch") as server,
        pytest.MonkeyPatch.context() as patched,
    ):
        patched.delenv(REMOTE_TOKEN, raising=False)
        patched.chdir(directory / "work")
        there = f"127.0.0.1:{server.port}"
        text = "\n".join(examples)
        assert text.count(SHOWN) == 1
        parsed = doctest.DocTestParser().get_doctest(text.replace(SHOWN, there), {}, PAGE, PAGE, 0)
        # Python keeps the text of what a session ran for as long as the
        # session lasts, which is how a class defined in it is found.
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


def test_every_example_on_the_page_runs_as_written(session, without_pandas):
    results, said, examples, names, server = session
    assert examples > (25 if without_pandas else 30)
    assert results.failed == 0, said
    assert results.attempted == examples
    # What the examples submitted is on the server of the test: the
    # backtest of the directory, the class's, and the directory's again.
    client = qf.Client(f"http://127.0.0.1:{server.port}")
    listed = client.backtest.list()
    assert [job.id for job in listed] == [names[name].id for name in ("again", "talks", "job")]
    assert all(job.status.state == "succeeded" for job in listed)
    # The first ran over the whole day the server stores, and made the
    # goldens' result: the strategy the page says it submits.
    expected = jobs.golden()["result"]
    metrics = names["result"].metrics
    assert {
        name: value if name == "trade_count" else format(value, "f")
        for name, value in metrics.items()
    } == {field: expected[field] for field in jobs.METRICS}
    whole = names["whole"]
    assert format(whole.trades[0].entry_price, "f") == expected["trades"][0]["entry_price"]
    # And the log the page shows is the log the server holds.
    assert [line.message for line in names["lines"]] == ["started", "held nothing"]
    assert server.log(names["talks"].id)["lines"][0]["message"] == "started"


def test_the_page_says_where_its_examples_run_and_what_they_submit(page):
    said = " ".join(page(PAGE).text.split())
    assert "stores a day of one-minute candles of `BTCUSDT`, 1,440 of them" in said
    assert "`my_strategy`, with one file, `sma_cross.py`: the SMA cross of" in said
    assert len(jobs.golden_klines()) == 1440


def called(function) -> str:
    """A method's signature as it is called, without `self`."""
    signature = inspect.signature(function)
    return str(signature.replace(parameters=list(signature.parameters.values())[1:]))


def public(kind) -> dict[str, str]:
    """A class's public members: each name with what it is."""
    return {
        name: type(member).__name__ for name, member in vars(kind).items() if not name[0] == "_"
    }


CALLS = [
    ("Job", "job", qf.Job, "wait"),
    ("Job", "job", qf.Job, "stream_logs"),
    ("Job", "job", qf.Job, "result"),
    ("Job", "job", qf.Job, "provenance"),
    ("JobResult", "result", qf.JobResult, "trades_df"),
    ("JobResult", "result", qf.JobResult, "equity_df"),
    ("JobResult", "result", qf.JobResult, "to_backtest_result"),
]


def test_the_page_gives_the_calls_as_python_does(page, quoted):
    lines = page(PAGE).section("signatures")
    assert (lines[0], lines[-1]) == ("```text", "```")
    assert lines[1:-1] == [
        f"{kind}.{name}{called(getattr(owner, name))}" for kind, _, owner, name in CALLS
    ]
    rows = page(PAGE).rows("calls")
    assert [quoted(call) for call, _, _ in rows] == [
        f"{held}.{name}{called(getattr(owner, name))}" for _, held, owner, name in CALLS
    ]
    # Every method of a job and of a result that asks a server is one of
    # them, beside the two the client's page is the contract of.
    methods = [name for name, kind in public(qf.Job).items() if kind == "function"]
    assert methods == ["refresh", "cancel", "wait", "stream_logs", "result", "provenance"]
    assert [name for name, kind in public(qf.JobResult).items() if kind == "function"] == [
        "trades_df",
        "equity_df",
        "to_backtest_result",
    ]
    for _, _, owner, name in CALLS:
        assert getattr(owner, name).__doc__, name


FIELDS = {
    "line fields": qf.LogLine,
    "file fields": qf.ResultFile,
    "provenance fields": qf.Provenance,
    "segment fields": qf.ProvenanceSegment,
}


@pytest.mark.parametrize("section", FIELDS)
def test_the_page_lists_every_field_of_a_record_in_the_order_the_record_holds_them(
    page, quoted, section
):
    kind = FIELDS[section]
    fields = [quoted(field) for field, *_ in page(PAGE).rows(section)]
    assert fields == list(public(kind))
    assert set(public(kind).values()) == {"property"}
    # Each is one the record keeps and is equal by.
    assert [name.removeprefix("_") for name in kind.__slots__] == fields
    # The class says of each what the page does, to `help()` as on the page.
    said = " ".join(kind.__doc__.split())
    for field in fields:
        assert f"`{field}`" in said, field
        assert getattr(kind, field).__doc__, field


def test_the_page_lists_a_results_members(page, quoted):
    members = [quoted(member) for member, _ in page(PAGE).rows("result members")]
    assert members == [
        name if kind == "property" else f"{name}{called(getattr(qf.JobResult, name))}"
        for name, kind in public(qf.JobResult).items()
    ]
    assert list(public(qf.JobResult)) == [
        "job_id",
        "metrics",
        "files",
        "trades_df",
        "equity_df",
        "to_backtest_result",
    ]
    said = " ".join(qf.JobResult.__doc__.split())
    for name in public(qf.JobResult):
        assert f"`{name}" in said, name
        assert getattr(qf.JobResult, name).__doc__, name


def test_the_page_gives_the_metrics_as_the_result_spec_writes_them(page, quoted, session):
    *_, names, _ = session
    rows = page(PAGE).rows("metrics fields")
    fields = [quoted(field) for field, _, _ in rows]
    # The result spec's own list, name for name and in its order.
    spec = [line.split("`")[1] for line in page(RESULT_SPEC).section("metrics fields")]
    assert fields == spec == list(names["result"].metrics)
    assert fields == [*_results._AMOUNTS, _results._TRADES]
    kinds = {"decimal.Decimal": decimal.Decimal, "int": int}
    for (_, kind, _), value in zip(rows, names["result"].metrics.values()):
        assert type(value) is kinds[quoted(kind)]
    # And a result's files as the spec names them.
    files = [(quoted(name), quoted(media)) for name, _, media in page(RESULT_SPEC).rows("files")]
    assert files == [(name, media) for name, _, media in _results._FILES]
    assert [quoted(file) for _, file, _ in page(RESULT_SPEC).rows("files")] == [
        file for _, file, _ in _results._FILES
    ]
    assert [(file.name, file.media_type) for file in names["result"].files] == files
    (number,) = page(RESULT_SPEC).section("result format")
    assert int(number) == _results._FORMAT == 1
    assert "reads provenance format 1 and result format 1" in " ".join(page(PAGE).text.split())


def test_the_pages_frames_are_the_frames_of_the_page_of_the_dataframes(
    page, quoted, session, without_pandas
):
    *_, names, _ = session
    for section in ("trade columns", "equity columns"):
        here = [(column, kind) for column, kind, _ in page(PAGE).rows(section)]
        there = [(column, kind) for column, kind, _ in page(PANDAS_PAGE).rows(section)]
        assert here == there, section
    if without_pandas:
        assert "trades" not in names and "equity" not in names
        return
    trades, equity = names["trades"], names["equity"]
    assert [(f"`{name}`", f"`{kind}`") for name, kind in trades.dtypes.astype(str).items()] == [
        (column, kind) for column, kind, _ in page(PAGE).rows("trade columns")
    ]
    (index, index_kind, _), (column, kind, _) = page(PAGE).rows("equity columns")
    assert index == f"`{equity.index.name}`, the index" and index_kind == f"`{equity.index.dtype}`"
    assert (column, kind) == ("`equity`", f"`{equity['equity'].dtype}`")
    # A job's frames are the functions' own, of the result it gives.
    import pandas as pd

    whole = names["whole"]
    pd.testing.assert_frame_equal(trades, qf.trades_to_dataframe(whole), check_exact=True)
    pd.testing.assert_frame_equal(equity, qf.equity_to_dataframe(whole), check_exact=True)
    # And each method says what its frame is, column by column.
    for method, section in [
        (qf.JobResult.trades_df, "trade columns"),
        (qf.JobResult.equity_df, "equity columns"),
    ]:
        said = " ".join(method.__doc__.split())
        for column, kind, _ in page(PAGE).rows(section):
            assert f"`{quoted(column.split(',')[0])}`" in said, column
            assert kind in said, (column, kind)


def test_the_page_says_where_an_amount_is_exact_and_where_a_float(
    page, quoted, session, without_pandas
):
    *_, names, _ = session
    rows = page(PAGE).rows("amounts")
    assert [quoted(where.split(",")[0]) for where, _, _ in rows] == [
        "result.metrics",
        "result.to_backtest_result()",
        "result.trades_df()",
    ]
    result, whole = names["result"], names["whole"]
    assert all("`decimal.Decimal`" in amount for _, amount, _ in rows[:2])
    assert "`float64`" in rows[2][1] and "`datetime64[ms, UTC]`" in rows[2][2]
    assert type(result.metrics["final_equity"]) is decimal.Decimal
    trade, point = whole.trades[0], whole.equity_curve[0]
    assert type(trade.qty) is type(point.equity) is decimal.Decimal
    assert type(trade.entry_time_ms) is type(point.open_time_ms) is int
    if not without_pandas:
        trades = names["trades"]
        assert str(trades["qty"].dtype) == "float64"
        assert str(trades["entry_time"].dtype) == "datetime64[ms, UTC]"
        # The float nearest to the decimal, and the same millisecond.
        assert trades["qty"][0] == float(trade.qty)
        assert trades["entry_time"][0].value // 1_000_000 == trade.entry_time_ms


def test_the_page_names_a_lines_levels_as_the_protocol_has_them(page, repository):
    source = (repository / "src" / "protocol" / "jobs" / "logs.rs").read_text(encoding="utf-8")
    (variants,) = re.findall(r"^pub enum LogLevel \{\n(.*?)^\}", source, re.S | re.M)
    levels = [name.lower() for name in re.findall(r"^    (\w+),$", variants, re.M)]
    assert levels[-1] == "unrecognized" and len(levels) == 5
    (level,) = [said for field, said in page(PAGE).rows("line fields") if field == "`level`"]
    assert re.findall(r"`(\w+)`", level) == levels[:-1] == ["debug", "info", "warning", "error"]
    said = " ".join(qf.LogLine.__doc__.split())
    assert all(f'`"{name}"`' in said for name in levels[:-1])


def test_the_page_says_what_each_call_raises(page, quoted):
    rows = page(PAGE).rows("raises")
    raised = []
    for call, raises, when in rows:
        assert when
        for name in re.findall(r"`(\w+)`", raises):
            kind = getattr(qf, name, None) or getattr(builtins, name)
            assert isinstance(kind, type) and issubclass(kind, Exception), name
            raised.append(name)
    assert raised == [
        "TimeoutError",
        "Conflict",
        "Conflict",
        "RemoteError",
        "RemoteError",
        "ImportError",
        "RemoteError",
    ]
    # Every call the page is the contract of is one a row names.
    named = " ".join(call for call, _, _ in rows)
    for _, held, _, name in CALLS:
        assert f"{held}.{name}(" in named, name
    assert not issubclass(TimeoutError, qf.RemoteError)
    assert not issubclass(ImportError, qf.RemoteError)


def test_the_page_states_the_limits_a_job_is_followed_within(page):
    limits = [limit for _, limit in page(PAGE).rows("limits")]
    assert limits == [
        f"{_jobs._WAIT_SECONDS} seconds",
        f"{_jobs._MOST_WAIT_SECONDS:g} seconds",
        f"{_wire._CONNECT_SECONDS} seconds",
        f"{_wire._READ_SECONDS} seconds",
        f"{_websocket._MOST_MESSAGE_BYTES // (1024 * 1024)} MiB",
        f"{_websocket._GOODBYE_SECONDS:g} second",
        f"{_logs._PAGE_SECONDS} seconds",
        f"{_results._MOST_FILE_BYTES // 1024**3} GiB",
    ]
    assert _websocket._MOST_MESSAGE_BYTES % (1024 * 1024) == 0
    assert _results._MOST_FILE_BYTES % 1024**3 == 0
    said = " ".join(page(PAGE).text.split())
    assert "after a tenth of a second, and then after twice as long each time" in said
    assert "after half a second where it did not" in said
    assert "A result file longer than 1 GiB is not downloaded" in said


def test_every_refusal_on_the_page_is_raised_as_written(page, session, quoted):
    *_, names, server = session
    before = len(qf.Client(f"http://127.0.0.1:{server.port}").backtest.list())
    rows = page(PAGE).rows("refusals")
    assert len(rows) >= 12
    shown = set()
    for call, raises in rows:
        call, raises = quoted(call), quoted(raises)
        kind, _, message = raises.partition(": ")
        with pytest.raises(Exception) as caught:
            eval(call, names)
        assert (type(caught.value).__name__, str(caught.value)) == (kind, message), call
        shown.add(kind)
    assert shown == {"TypeError", "ValueError"}
    # None of the five types is built: each has its row.
    for name in ("LogLine", "JobResult", "ResultFile", "Provenance", "ProvenanceSegment"):
        assert any(quoted(call) == f"qf.{name}()" for call, _ in rows), name
    assert len(qf.Client(f"http://127.0.0.1:{server.port}").backtest.list()) == before


def test_the_page_writes_a_line_and_a_result_as_python_writes_them(page, session):
    *_, names, _ = session
    text = page(PAGE).text
    (line,) = re.findall(r"`(<LogLine [^`]+>)`", text)
    assert repr(names["lines"][0]) == line
    (written,) = re.findall(r"`(<JobResult of job [0-9a-f]{32}: \d+ trades>)`", text)
    result = names["result"]
    assert repr(result).replace(result.job_id, written.split()[3].rstrip(":")) == written
    # The refusal it quotes is the server's wording for a job that runs.
    said = " ".join(text.split())
    (quoted_refusal,) = re.findall(r"`(job [0-9a-f]{32} is still running; [^`]+)`", said)
    assert quoted_refusal.endswith("only a job that has succeeded has a result")


def test_what_the_page_leaves_to_later_stories_and_what_the_other_pages_no_longer_do(page):
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
    assert sorted(here.values()) == ["QUA-345", "QUA-458", "QUA-471"]
    # What this page is the contract of is left by no page any more.
    for other in (CLIENT_PAGE, BUNDLES_PAGE, PACKAGE_PAGE):
        assert "QUA-468" not in left(other).values(), other
    # The server's own story is told in the client's words, and the stubs'
    # in the package's.
    assert {what for what, story in left(CLIENT_PAGE).items() if story == "QUA-345"} == {
        what for what, story in here.items() if story == "QUA-345"
    }
    package = left(PACKAGE_PAGE)
    assert "QUA-471" in package.values()
