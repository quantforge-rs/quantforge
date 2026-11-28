"""A job's result read from Python, against servers of the test's own
(docs/python-jobs.md, docs/result-spec.md): what `Job.result()` asks and
holds its answer to, when each table is downloaded and what its file is
held to before it is read, the result as the engine's own type and as the
frames a local run gives, and every answer that is refused.

A stand-in here answers a job's result routes with the result format's
shared cases, `tests/fixtures/result_cases.json`, which no code of the
package or of the engine wrote, or with whatever a test would have a server
send in their place. What the client reads of a real `serve run`'s job is
in `test_handle_jobs.py`.
"""

import copy
import hashlib
import json
import pickle
import threading
from decimal import Decimal

import pytest

import quantforge_py as qf
from conftest import REPOSITORY
from quantforge_py import _native, _results
from standin import JOB, StandIn, refusal, written

RESULT = f"/v1/backtests/{JOB}/result"
ARROW = "application/vnd.apache.arrow.file"
SUCCEEDED = {
    "kind": "succeeded",
    "started_at_ms": 1_792_454_401_500,
    "finished_at_ms": 1_792_454_402_000,
    "results": [],
}

with open(REPOSITORY / "tests" / "fixtures" / "result_cases.json", encoding="utf-8") as shared:
    CASES = {case["name"]: case for case in json.load(shared)["cases"]}

# The files a result has, as the result format names them.
FILES = {"metrics": "metrics.json", "trades": "trades.arrow", "equity": "equity.arrow"}
MEDIA = {"metrics": "application/json", "trades": ARROW, "equity": ARROW}


def file(case: str) -> bytes:
    """A shared case's file, as its bytes."""
    return bytes.fromhex(CASES[case]["file_hex"])


def named(raw: bytes) -> str:
    return "sha256:" + hashlib.sha256(raw).hexdigest()


def link(name: str, raw: bytes, **changed) -> dict:
    """One file of a result as a server lists it."""
    listed = {
        "name": name,
        "media_type": MEDIA[name],
        "hash": named(raw),
        "size_bytes": len(raw),
        "path": f"{RESULT}/{FILES[name]}",
    }
    listed.update(changed)
    return listed


def listing(trades="trades.sma_cross", equity="equity.sma_cross", metrics="metrics.sma_cross"):
    """A job's result as its route answers it: the pinned SMA-cross run's,
    unless a case is given in the place of one of its files."""
    return {
        "job_id": JOB,
        "format": 1,
        "metrics": dict(CASES[metrics]["metrics"]),
        "results": [
            link("metrics", file(metrics)),
            link("trades", file(trades)),
            link("equity", file(equity)),
        ],
    }


class Serving(StandIn):
    """A stand-in that holds one succeeded job and its result: the listing
    a test gives it, and the files of the shared cases the listing names,
    unless a test says what a file's route answers."""

    def __init__(self, listed=None, prefix="", **cases):
        super().__init__()
        self.prefix = prefix
        if prefix:
            self.says("GET", f"{prefix}/version", self._answers[("GET", "/version")][0])
        self.says("GET", f"{prefix}/v1/backtests/{JOB}", (200, written(state=SUCCEEDED)))
        self.says("GET", f"{prefix}{RESULT}", (200, listing(**cases) if listed is None else listed))
        for name, case in {
            "trades": cases.get("trades", "trades.sma_cross"),
            "equity": cases.get("equity", "equity.sma_cross"),
            "metrics": cases.get("metrics", "metrics.sma_cross"),
        }.items():
            self.holds(name, file(case))

    def holds(self, name, raw, **headers):
        """From now on a file's route answers these bytes."""
        self.says("GET", f"{self.prefix}{RESULT}/{FILES[name]}", (200, raw, headers))
        return self

    @property
    def job(self):
        return qf.Client(f"{self.url}{self.prefix}").backtest.get(JOB)

    @property
    def fetched(self) -> list[str]:
        """Which of the job's result routes were asked, in order."""
        start = f"GET {self.prefix}{RESULT}"
        return [
            route.removeprefix(start) or "/" for route in self.routes if route.startswith(start)
        ]


@pytest.fixture
def server():
    with Serving() as stand_in:
        yield stand_in


def raised(kind, call):
    with pytest.raises(kind) as caught:
        call()
    assert type(caught.value) is kind
    return caught.value


class Held:
    """A file's route that answers nothing until the test lets it: `first`
    is set once a request for the file has come and `second` once another
    has, and each waits for `free` before it is answered, with `answers`
    in turn and the last of them from then on."""

    def __init__(self, server, name, *answers):
        self.first, self.second, self.free = (threading.Event() for _ in range(3))
        answers = list(answers)
        counted = threading.Lock()

        def answer():
            with counted:
                raw = answers.pop(0) if len(answers) > 1 else answers[0]
                came = self.second if self.first.is_set() else self.first
                came.set()
            assert self.free.wait(30)
            return 200, raw, {}

        server.says("GET", f"{server.prefix}{RESULT}/{FILES[name]}", answer)


def together(call, held, count=2):
    """Calls `call` from `count` callers at once, each once the one before
    it has had the time to ask its server, and gives what each was given
    or raised, in no order: the first is asking for the file that is held
    back when the next begins, and the file is let go of once all have
    begun."""
    outcomes = []

    def caller():
        try:
            outcomes.append(call())
        except qf.RemoteError as error:
            outcomes.append(error)

    callers = [threading.Thread(target=caller, daemon=True) for _ in range(count)]
    callers[0].start()
    assert held.first.wait(10)
    for each in callers[1:]:
        each.start()
    # A caller that asked its server itself would have by now: its request
    # would come while the first one's is held back.
    asked_again = held.second.wait(0.5)
    held.free.set()
    for each in callers:
        each.join(10)
        assert not each.is_alive()
    return outcomes, asked_again


def unread(server, why: str, target=RESULT) -> str:
    """What the client says of an answer the protocol does not write."""
    return (
        f"the server at {server.url}/ answered GET {target} with a body the protocol does not "
        f"read: {why}; is it a QuantForge server?"
    )


# The result, and what it asks.


def test_a_result_is_asked_for_once_and_gives_its_metrics_without_another_request(server):
    job = server.job
    result = job.result()
    assert type(result) is qf.JobResult and result.job_id == JOB
    assert server.fetched == ["/"]
    # The metrics, as `metrics.json` holds them and in its order: each
    # amount an exact decimal with the digits the server wrote.
    metrics = result.metrics
    assert list(metrics) == [
        "initial_cash",
        "final_equity",
        "total_return_pct",
        "max_drawdown_pct",
        "trade_count",
    ]
    written = CASES["metrics.sma_cross"]["metrics"]
    assert metrics == {
        "initial_cash": Decimal("10010"),
        "final_equity": Decimal("10999.989"),
        "total_return_pct": Decimal("9.89"),
        "max_drawdown_pct": Decimal("10"),
        "trade_count": 2,
    }
    for name, value in metrics.items():
        if name == "trade_count":
            assert type(value) is int and value == written[name]
        else:
            assert type(value) is Decimal and format(value, "f") == written[name]
    # The dict is the caller's own: changing it changes nothing in the
    # result, and the next read gives a new one.
    metrics["trade_count"] = 0
    del metrics["initial_cash"]
    assert result.metrics["trade_count"] == 2 and result.metrics is not result.metrics
    assert repr(result) == f"<JobResult of job {JOB}: 2 trades>"
    # Reading any of it asked the server nothing more, and nothing of a
    # status.
    assert result.files and result.job_id and server.fetched == ["/"]
    assert job.status.state == "succeeded"
    # Each call asks the server: a result is kept by its caller.
    assert job.result() is not result and server.fetched == ["/", "/"]


def test_the_digits_a_server_wrote_are_the_digits_a_results_metrics_hold():
    with Serving(metrics="metrics.scales", trades="trades.none") as server:
        written = CASES["metrics.scales"]["metrics"]
        metrics = server.job.result().metrics
        for name in list(metrics)[:-1]:
            assert format(metrics[name], "f") == written[name], name
        # Trailing zeros, a negative amount and a zero with places among
        # them: none is a float's digits.
        assert any(
            text.endswith("0") and "." in text for text in written.values() if isinstance(text, str)
        )
        assert (
            repr(server.job.result())
            == f"<JobResult of job {JOB}: {written['trade_count']} trades>"
        )


def test_a_result_lists_its_three_files_each_by_its_hash_and_its_size(server, refused):
    result = server.job.result()
    files = result.files
    assert type(files) is tuple and files is result.files
    assert [type(each) for each in files] == [qf.ResultFile] * 3
    assert [(each.name, each.media_type) for each in files] == list(MEDIA.items())
    for each, case in zip(files, ("metrics.sma_cross", "trades.sma_cross", "equity.sma_cross")):
        assert (each.hash, each.size_bytes) == (CASES[case]["hash"], CASES[case]["size_bytes"])
        assert each.hash == named(file(case))
    metrics, trades, _ = files
    assert repr(trades) == f"<ResultFile trades {CASES['trades.sma_cross']['hash']} of 1882 bytes>"
    # A value: equal by its fields, never built, changed, copied or a key.
    again = server.job.result().files
    assert again == files and again[0] is not metrics and metrics != trades
    said = (
        "a ResultFile cannot be built: a JobResult and a Provenance give one for each file a "
        "job produced"
    )
    refused(TypeError, said, qf.ResultFile)
    refused(TypeError, said, lambda: qf.ResultFile(name="trades", hash=trades.hash))
    refused(
        AttributeError,
        "attribute 'hash' of 'quantforge_py.ResultFile' objects is not writable",
        setattr,
        trades,
        "hash",
        metrics.hash,
    )
    with pytest.raises(TypeError, match="unhashable type"):
        hash(trades)
    for copied in (copy.copy, copy.deepcopy, pickle.dumps):
        refused(TypeError, "cannot pickle 'quantforge_py.ResultFile' object", copied, trades)
    with pytest.raises(TypeError, match="not an acceptable base type"):
        type("Derived", (qf.ResultFile,), {})


def test_a_result_is_no_value_and_is_never_built_changed_copied_or_derived_from(server, refused):
    result = server.job.result()
    assert result == result and result != server.job.result()
    assert hash(result) == hash(result)
    refused(
        TypeError,
        "a JobResult cannot be built: Job.result() gives the result of a job that has succeeded",
        qf.JobResult,
    )
    refused(
        AttributeError,
        "attribute 'metrics' of 'quantforge_py.JobResult' objects is not writable",
        setattr,
        result,
        "metrics",
        {},
    )
    refused(
        AttributeError,
        "'quantforge_py.JobResult' object has no attribute 'trades'",
        setattr,
        result,
        "trades",
        (),
    )
    refused(
        AttributeError,
        "attribute 'files' of 'quantforge_py.JobResult' objects is not writable",
        delattr,
        result,
        "files",
    )
    assert not hasattr(result, "__dict__")
    for copied in (copy.copy, copy.deepcopy, pickle.dumps):
        refused(TypeError, "cannot pickle 'quantforge_py.JobResult' object", copied, result)
    with pytest.raises(TypeError, match="not an acceptable base type"):
        type("Derived", (qf.JobResult,), {})


# The tables.


def test_each_table_is_downloaded_when_it_is_first_asked_for_and_kept(server):
    result = server.job.result()
    assert server.fetched == ["/"]
    whole = result.to_backtest_result()
    # Both tables, each once, by the route the client builds for it.
    assert server.fetched == ["/", "/trades.arrow", "/equity.arrow"]
    assert type(whole) is qf.BacktestResult
    assert result.to_backtest_result() == whole and result.to_backtest_result() is not whole
    assert server.fetched == ["/", "/trades.arrow", "/equity.arrow"]
    # Another result of the job holds nothing yet, and downloads only what
    # it is asked for.
    other = server.job.result()
    assert other.metrics == result.metrics
    assert server.fetched == ["/", "/trades.arrow", "/equity.arrow", "/"]


def test_a_table_two_callers_ask_for_at_once_is_downloaded_once_and_given_to_both(server):
    held = Held(server, "equity", file("equity.sma_cross"))
    result = server.job.result()
    (one, other), asked_again = together(result.to_backtest_result, held)
    # The second found the table being downloaded, waited for it, and was
    # given the rows the first one kept.
    assert not asked_again
    assert server.fetched == ["/", "/trades.arrow", "/equity.arrow"]
    assert type(one) is type(other) is qf.BacktestResult and one == other
    assert len(one.equity_curve) == 15
    # As were three of them.
    held = Held(server, "equity", file("equity.sma_cross"))
    result = server.job.result()
    results, asked_again = together(result.to_backtest_result, held, count=3)
    assert not asked_again and results[0] == results[1] == results[2]
    assert server.fetched[3:] == ["/", "/trades.arrow", "/equity.arrow"]


def test_a_table_whose_download_failed_is_asked_for_again_by_the_caller_that_waited(server):
    equity = file("equity.sma_cross")
    altered = bytes([equity[0] ^ 1]) + equity[1:]
    held = Held(server, "equity", altered, equity)
    result = server.job.result()
    outcomes, asked_again = together(result.to_backtest_result, held)
    # The one that downloaded the file was refused it, and kept nothing:
    # the one that waited for it asked the server itself, and was answered
    # the file the job lists.
    assert not asked_again
    assert server.fetched == ["/", "/trades.arrow", "/equity.arrow", "/equity.arrow"]
    (refused,) = [each for each in outcomes if type(each) is qf.RemoteError]
    (read,) = [each for each in outcomes if type(each) is qf.BacktestResult]
    assert f"it is {len(altered)} bytes hashing to {named(altered)}" in str(refused)
    assert len(read.equity_curve) == 15
    # And it is kept from then on.
    assert result.to_backtest_result() == read
    assert server.fetched.count("/equity.arrow") == 2


def test_a_table_that_is_being_downloaded_holds_up_no_caller_of_the_other(server):
    held = Held(server, "equity", file("equity.sma_cross"))
    result = server.job.result()
    given = {}
    curve = threading.Thread(
        target=lambda: given.update(equity=result._rows("equity")), daemon=True
    )
    curve.start()
    assert held.first.wait(10)
    # The trades are another file, with a turn of their own.
    trades = threading.Thread(
        target=lambda: given.update(trades=result._rows("trades")), daemon=True
    )
    trades.start()
    trades.join(10)
    assert list(given) == ["trades"] and len(given["trades"]) == 2
    held.free.set()
    curve.join(10)
    assert list(given) == ["trades", "equity"] and len(given["equity"]) == 15
    assert server.fetched == ["/", "/equity.arrow", "/trades.arrow"]


def test_a_file_is_asked_for_by_its_own_type_under_the_clients_own_url():
    # A server behind a proxy that serves it under a path lists each file
    # by the path on the server itself, which the proxy does not serve: the
    # client asks for a file where it asks for everything else.
    with Serving(prefix="/lab") as server:
        result = server.job.result()
        assert result.files[1].name == "trades"
        whole = result.to_backtest_result()
        assert len(whole.trades) == 2 and len(whole.equity_curve) == 15
        assert server.routes == [
            "GET /lab/version",
            f"GET /lab/v1/backtests/{JOB}",
            f"GET /lab{RESULT}",
            f"GET /lab{RESULT}/trades.arrow",
            f"GET /lab{RESULT}/equity.arrow",
        ]
        for _, target, headers, sent in server.asked[-2:]:
            assert headers["Accept"] == ARROW and sent == b""
            assert headers["Connection"] == "close"
            assert headers["User-Agent"] == f"quantforge-py/{qf.__version__}"
            # Asked for whole, each time: the client keeps no copy of its
            # own to ask after.
            assert not {"If-None-Match", "Range", "Authorization"} & set(headers)
        assert server.asked[2][2]["Accept"] == "application/json"


def test_a_token_goes_with_a_file_as_with_every_request(server):
    client = qf.Client(server.url, token="a-token-no-line-holds")
    client.backtest.get(JOB).result().to_backtest_result()
    assert [headers.get("Authorization") for *_, headers, _ in server.asked] == [
        "Bearer a-token-no-line-holds"
    ] * 5


def test_a_result_is_the_engines_own_with_every_amount_exact(server, as_given):
    whole = server.job.result().to_backtest_result()
    given = as_given(whole)
    # Its fields are the metrics, digit for digit.
    assert {name: given[name] for name in CASES["metrics.sma_cross"]["metrics"]} == CASES[
        "metrics.sma_cross"
    ]["metrics"]
    # Its trades are the rows of the file, each amount with the digits the
    # file holds, and no order of a backtest reached a venue.
    rows = CASES["trades.sma_cross"]["rows"]
    assert given["trades"] == [
        {**row, "entry_order_id": None, "exit_order_id": None} for row in rows
    ]
    assert [type(trade) for trade in whole.trades] == [qf.ClosedTrade] * 2
    assert whole.trades[0].symbol == qf.Symbol("BTCUSDT")
    # And its curve the rows of the other.
    assert given["equity_curve"] == CASES["equity.sma_cross"]["rows"]
    assert [type(point) for point in whole.equity_curve] == [qf.EquityPoint] * 15
    assert whole.trade_count == len(whole.trades) == 2


def test_a_result_without_a_trade_and_one_of_a_job_that_ran_no_bar_are_read(as_given):
    with Serving(trades="trades.none", equity="equity.none", metrics="metrics.scales") as server:
        listed = listing(trades="trades.none", equity="equity.none", metrics="metrics.scales")
        # The shared metrics count a trade; these count none.
        listed["metrics"]["trade_count"] = 0
        zero = _native._result_metrics(*list(listed["metrics"].values())[:4], 0)
        listed["results"][0] = link("metrics", zero)
        server.says("GET", RESULT, (200, listed))
        result = server.job.result()
        whole = result.to_backtest_result()
        assert (whole.trades, whole.equity_curve, whole.trade_count) == ((), (), 0)
        assert repr(result) == f"<JobResult of job {JOB}: 0 trades>"
        # Tables that hold no row are kept as any other: asked for once.
        result.to_backtest_result()
        assert server.fetched == ["/", "/trades.arrow", "/equity.arrow"]


def test_one_trade_is_written_as_one(server):
    listed = listing()
    listed["metrics"]["trade_count"] = 1
    one = _native._result_metrics(*list(listed["metrics"].values())[:4], 1)
    listed["results"][0] = link("metrics", one)
    server.says("GET", RESULT, (200, listed))
    assert repr(server.job.result()) == f"<JobResult of job {JOB}: 1 trade>"


def test_the_frames_of_a_result_are_the_frames_of_the_engines_own(server, without_pandas):
    result = server.job.result()
    whole = result.to_backtest_result()
    if without_pandas:
        # What needs pandas says what to install, in the words the local
        # functions say it, and before anything is downloaded.
        fresh = server.job.result()
        before = list(server.fetched)
        for remote, local in [
            (fresh.trades_df, qf.trades_to_dataframe),
            (fresh.equity_df, qf.equity_to_dataframe),
        ]:
            expected = raised(ModuleNotFoundError, lambda: local(whole))
            error = raised(ModuleNotFoundError, remote)
            assert str(error) == str(expected) and "quantforge-py[pandas]" in str(error)
        assert server.fetched == before
        return
    import pandas as pd

    trades, equity = result.trades_df(), result.equity_df()
    pd.testing.assert_frame_equal(trades, qf.trades_to_dataframe(whole))
    pd.testing.assert_frame_equal(equity, qf.equity_to_dataframe(whole))
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
    assert trades["entry_order_id"].tolist() == [None, None]
    assert trades["gross_quote_pnl"].tolist() == [-10.01, 999.999]
    assert (equity.index.name, list(equity.columns)) == ("open_time", ["equity"])
    # A frame is new each time, and the caller's own.
    trades.loc[0, "qty"] = 0.0
    assert result.trades_df() is not trades and result.trades_df()["qty"].tolist() == [100.0, 100.0]


def test_a_frame_downloads_its_own_table_and_no_other(server, without_pandas):
    if without_pandas:
        pytest.skip("a frame needs pandas")
    result = server.job.result()
    result.equity_df()
    assert server.fetched == ["/", "/equity.arrow"]
    result.equity_df()
    result.trades_df()
    result.trades_df()
    assert server.fetched == ["/", "/equity.arrow", "/trades.arrow"]
    result.to_backtest_result()
    assert server.fetched == ["/", "/equity.arrow", "/trades.arrow"]
    # And the trades alone, where they are asked for first.
    other = server.job.result()
    other.trades_df()
    assert server.fetched[3:] == ["/", "/trades.arrow"]


def test_a_frame_says_what_it_needs_before_anything_is_downloaded(server, monkeypatch):
    # Whatever is installed where the tests run: each of the two is found
    # or missing as the test says, pandas first and then NumPy, as the
    # package asks for them.
    result = server.job.result()
    for missing in ("_pandas", "_numpy"):

        def none(name=missing):
            raise ModuleNotFoundError(f"this test has no {name}")

        with monkeypatch.context() as patched:
            patched.setattr(_native, "_pandas", lambda: object())
            patched.setattr(_native, "_numpy", lambda: object())
            patched.setattr(_native, missing, none)
            for frame in (result.trades_df, result.equity_df):
                error = raised(ModuleNotFoundError, frame)
                assert str(error) == f"this test has no {missing}"
    assert server.fetched == ["/"]
    # The result itself needs neither.
    assert len(result.to_backtest_result().trades) == 2


# The shared cases, through the package's own readers.


@pytest.mark.parametrize("case", [name for name, case in CASES.items() if "file_hex" in case])
def test_the_result_formats_shared_cases_are_read_by_the_package_as_the_engine_reads_them(case):
    shared = CASES[case]
    raw = file(case)
    assert (named(raw), len(raw)) == (shared["hash"], shared["size_bytes"])
    if shared["result"] == "metrics":
        metrics = shared["metrics"]
        written = _native._result_metrics(*list(metrics.values())[:4], metrics["trade_count"])
        assert type(written) is bytes and written == raw
        return
    if shared["result"] == "equity":
        curve = _native._result_equity(raw)
        assert type(curve) is tuple
        assert [
            {"open_time_ms": point.open_time_ms, "equity": format(point.equity, "f")}
            for point in curve
        ] == shared["rows"]
        return
    if case == "trades.extremes":
        # The result format writes any text as a symbol, the empty one
        # among it. The engine has no trade of no symbol: a row of one is
        # not a row of a backtest's result.
        with pytest.raises(ValueError) as refused:
            _native._result_trades(raw)
        assert str(refused.value) == "the symbol of its row 1: invalid symbol: empty"
        return
    trades = _native._result_trades(raw)
    assert type(trades) is tuple
    assert [
        {
            "symbol": str(trade.symbol),
            "entry_time_ms": trade.entry_time_ms,
            "exit_time_ms": trade.exit_time_ms,
            **{
                name: format(getattr(trade, name), "f")
                for name in ("entry_price", "exit_price", "qty", "gross_quote_pnl")
            },
        }
        for trade in trades
    ] == shared["rows"]
    assert all((trade.entry_order_id, trade.exit_order_id) == (None, None) for trade in trades)


def test_what_is_no_file_of_the_result_format_is_refused_in_the_engines_words():
    trades, equity = file("trades.sma_cross"), file("equity.sma_cross")
    for read, raw, said in [
        (_native._result_trades, b"", "it does not start with the trades schema"),
        (_native._result_trades, equity, "it does not start with the trades schema"),
        (_native._result_equity, trades, "it does not start with the equity schema"),
        (_native._result_equity, equity[:-1], "it is not the file its contents are written as"),
        (_native._result_trades, trades + b"\0", "it is not the file its contents are written as"),
    ]:
        with pytest.raises(ValueError) as refused:
            read(raw)
        kind = "trades" if read is _native._result_trades else "equity"
        assert str(refused.value) == f"not a format 1 {kind} file: {said}"
    for given in ("text", bytearray(trades), memoryview(trades), None):
        with pytest.raises(TypeError):
            _native._result_trades(given)
    # An amount the engine's decimal does not hold, and one in no exact
    # form, is refused by its name.
    for amount in ("1e5", "1.", "-0", "0.00000000000000000000000000001", "7" * 30, "NaN", ""):
        with pytest.raises(ValueError, match="^final_equity: invalid decimal "):
            _native._result_metrics("1", amount, "1", "1", 0)
    for count in (-1, 2**64):
        with pytest.raises(OverflowError):
            _native._result_metrics("1", "1", "1", "1", count)


# Every result that is refused.


def changed(**members) -> dict:
    listed = listing()
    listed.update(members)
    return listed


def with_file(index: int, **members) -> dict:
    listed = listing()
    listed["results"][index].update(members)
    return listed


def without(index: int, member: str) -> dict:
    listed = listing()
    del listed["results"][index][member]
    return listed


def with_metric(name: str, value) -> dict:
    listed = listing()
    listed["metrics"][name] = value
    return listed


LONG = "7" * 30
REWRITTEN = _native._result_metrics("10010", "10999.98900", "9.89", "10", 2)
UNREADABLE = (
    [
        ([], "a job's result is not an object"),
        (changed(job_id="0" * 32), f"it is the result of job {'0' * 32}, not of job {JOB}"),
        (changed(job_id="no such job"), "the `job_id` of a job's result is no job ID"),
        (changed(job_id=None), "the `job_id` of a job's result is not a string"),
        (
            changed(format=2),
            "it is a result of format 2, and this version of the package reads format 1",
        ),
        (
            changed(format=0),
            "it is a result of format 0, and this version of the package reads format 1",
        ),
        (changed(format="1"), f"the `format` of job {JOB}'s result is not an integer"),
        (changed(metrics=[]), f"the `metrics` of job {JOB}'s result is not an object"),
        (changed(results={}), f"the `results` of job {JOB}'s result is not a list"),
        (changed(results=[]), f"job {JOB}'s result lists no metrics file"),
        (
            changed(results=listing()["results"][:2]),
            f"job {JOB}'s result lists no equity file",
        ),
        (
            changed(results=listing()["results"] + listing()["results"][1:2]),
            f"job {JOB}'s result lists its 'trades' file twice",
        ),
        (changed(results=[7]), f"a file of job {JOB}'s result is not an object"),
        (
            with_metric("final_equity", 10999.989),
            f"the `final_equity` of job {JOB}'s metrics is not a string",
        ),
        (
            with_metric("trade_count", "2"),
            f"the `trade_count` of job {JOB}'s metrics is not an integer",
        ),
        (
            with_metric("trade_count", 2.0),
            f"the `trade_count` of job {JOB}'s metrics is not an integer",
        ),
        (with_metric("trade_count", -1), f"the `trade_count` of job {JOB}'s metrics is negative"),
        (
            with_metric("initial_cash", "1e4"),
            f"job {JOB}'s metrics hold an amount the engine does not (initial_cash: invalid decimal "
            '"1e4": expected digits with an optional fraction, such as "67012.34", and no sign on '
            "zero)",
        ),
        (
            with_metric("max_drawdown_pct", LONG),
            f"job {JOB}'s metrics hold an amount the engine does not (max_drawdown_pct: invalid "
            f'decimal "{LONG}": a decimal cannot hold it without rounding)',
        ),
        (
            with_file(1, hash="sha256:" + "A" * 64),
            f"the `hash` of a file of job {JOB}'s result is no content hash",
        ),
        (
            with_file(1, hash="md5:" + "a" * 64),
            f"the `hash` of a file of job {JOB}'s result is no content hash",
        ),
        (
            with_file(2, size_bytes=-1),
            f"the `size_bytes` of a file of job {JOB}'s result is negative",
        ),
        (
            with_file(2, size_bytes="1178"),
            f"the `size_bytes` of a file of job {JOB}'s result is not an integer",
        ),
        (with_file(0, name=7), f"the `name` of a file of job {JOB}'s result is not a string"),
        (
            with_file(1, media_type="application/octet-stream"),
            f"job {JOB}'s result lists its trades file as 'application/octet-stream', and the result "
            f"format writes it as {ARROW}",
        ),
        (
            with_file(0, media_type=ARROW),
            f"job {JOB}'s result lists its metrics file as '{ARROW}', and the result format writes it "
            "as application/json",
        ),
        # The metrics a result gives are the ones its `metrics.json` holds: a
        # digit more or less is another file.
        (
            with_metric("final_equity", "10999.98900"),
            f"job {JOB}'s metrics are not the ones its metrics.json holds: written as that file they "
            f"are {len(REWRITTEN)} bytes hashing to {named(REWRITTEN)}, and the file is listed as 117 "
            f"bytes hashing to {CASES['metrics.sma_cross']['hash']}",
        ),
    ]
    + [
        (without(index, member), f"a file of job {JOB}'s result has no `{member}`")
        for index, member in [(0, "name"), (1, "media_type"), (2, "hash"), (1, "size_bytes")]
    ]
    + [
        (
            {key: value for key, value in listing().items() if key != member},
            f"job {JOB}'s result has no `{member}`"
            if member != "job_id"
            else "a job's result has no `job_id`",
        )
        for member in ("job_id", "format", "metrics", "results")
    ]
    + [
        (
            {
                **listing(),
                "metrics": {k: v for k, v in listing()["metrics"].items() if k != member},
            },
            f"job {JOB}'s metrics has no `{member}`",
        )
        for member in (
            "initial_cash",
            "final_equity",
            "total_return_pct",
            "max_drawdown_pct",
            "trade_count",
        )
    ]
)


@pytest.mark.parametrize(("answered", "why"), UNREADABLE)
def test_a_result_the_protocol_does_not_write_is_refused_and_never_guessed_at(
    server, answered, why
):
    server.says("GET", RESULT, (200, answered))
    error = raised(qf.RemoteError, server.job.result)
    assert str(error) == unread(server, why)
    assert (error.code, error.status) == (None, None)
    assert server.fetched == ["/"]


def test_a_result_is_read_past_what_a_later_revision_adds(server):
    wider = listing()
    wider["checked_at_ms"] = 5
    wider["metrics"]["sharpe"] = "1.2"
    wider["results"].append(
        {**link("trades", b"positions"), "name": "positions", "path": f"{RESULT}/positions.arrow"}
    )
    wider["results"][1]["encoding"] = "identity"
    server.says("GET", RESULT, (200, wider))
    result = server.job.result()
    # The three files of this format, and the five fields of its metrics.
    assert [each.name for each in result.files] == ["metrics", "trades", "equity"]
    assert len(result.metrics) == 5 and len(result.to_backtest_result().trades) == 2
    # Nor is the path a server lists read: the client asks by its own.
    listed = listing()
    for each in listed["results"]:
        del each["path"]
    server.says("GET", RESULT, (200, listed))
    assert len(server.job.result().to_backtest_result().equity_curve) == 15


def test_a_job_that_has_not_succeeded_has_no_result_in_the_servers_words(server):
    for stands in ("is still queued", "is still running", "failed", "was cancelled"):
        message = f"job {JOB} {stands}; only a job that has succeeded has a result"
        server.says("GET", RESULT, refusal(409, "conflict", message))
        error = raised(qf.Conflict, server.job.result)
        assert (str(error), error.code, error.status) == (message, "conflict", 409)
    server.says("GET", RESULT, refusal(404, "not_found", f"the server holds no job {JOB}"))
    assert str(raised(qf.NotFound, server.job.result)) == f"the server holds no job {JOB}"


# Every file that is refused.


def chunked(raw: bytes) -> bytes:
    return b"%x\r\n%b\r\n0\r\n\r\n" % (len(raw), raw)


def test_a_file_is_held_to_the_size_and_the_hash_its_job_lists_before_it_is_read(server):
    trades = file("trades.sma_cross")
    listed = CASES["trades.sma_cross"]
    target = f"{RESULT}/trades.arrow"
    lists = f"job {JOB}'s result lists its trades file as 1882 bytes hashing to {listed['hash']}"
    altered = bytes([trades[0] ^ 1]) + trades[1:]
    for raw, why in [
        # As long, and other bytes.
        (altered, f"it is 1882 bytes hashing to {named(altered)}, and {lists}"),
        # Shorter, by one byte and by all of them.
        (trades[:-1], f"it is 1881 bytes hashing to {named(trades[:-1])}, and {lists}"),
        (b"", f"it is 0 bytes hashing to {named(b'')}, and {lists}"),
        # Another table of the job, which is a file of the result format.
        (
            file("equity.sma_cross"),
            f"it is 1178 bytes hashing to {CASES['equity.sma_cross']['hash']}, and {lists}",
        ),
    ]:
        server.holds("trades", raw)
        result = server.job.result()
        error = raised(qf.RemoteError, result.to_backtest_result)
        assert str(error) == unread(server, why, target)
        assert (error.code, error.status) == (None, None)
        # Nothing of a file that was refused is kept: asked again, it is
        # downloaded again, and read where it is the file the job lists.
        server.holds("trades", trades)
        assert len(result.to_backtest_result().trades) == 2
    # The equity curve was never asked for behind a table of trades that
    # was refused.
    assert server.fetched.count("/equity.arrow") == 4


def test_a_file_longer_than_its_job_lists_is_not_read(server):
    trades = file("trades.sma_cross")
    target = f"{RESULT}/trades.arrow"
    said = (
        f"the server at {server.url}/ answered GET {target} with more than the 1882 bytes job "
        f"{JOB}'s result lists for its trades file"
    )
    result = server.job.result()
    head = b"HTTP/1.1 200 OK\r\nContent-Type: application/vnd.apache.arrow.file\r\n"
    in_chunks = head + b"Transfer-Encoding: chunked\r\n\r\n"
    for longer in [
        # By the length it states, of which nothing is then read.
        (200, trades + b"\0", {}),
        (200, trades * 3, {}),
        # By what came, where it states none: read one byte past, and no
        # further.
        head + b"\r\n" + trades + b"\0",
        head + b"\r\n" + trades * 50,
        # And by its chunks: by one that would make it longer, which is not
        # read, and by one more chunk after all it lists.
        in_chunks + chunked(trades + b"\0"),
        in_chunks + chunked(trades)[: -len(b"0\r\n\r\n")] + chunked(b"\0"),
    ]:
        server.says("GET", target, longer)
        error = raised(qf.RemoteError, result.to_backtest_result)
        assert str(error) == said, longer[:80]
        assert (error.code, error.status) == (None, None)
    # One that is exactly as long is taken, whichever way it says where it
    # ends.
    for exact in [
        (200, trades, {}),
        head + b"\r\n" + trades,
        in_chunks + chunked(trades),
        in_chunks + chunked(trades[:700])[: -len(b"0\r\n\r\n")] + chunked(trades[700:]),
    ]:
        server.says("GET", target, exact)
        assert len(server.job.result().to_backtest_result().trades) == 2


def test_a_file_that_was_cut_short_did_not_come_whole_and_is_asked_for_again(server):
    trades = file("trades.sma_cross")
    target = f"{RESULT}/trades.arrow"
    head = b"HTTP/1.1 200 OK\r\nContent-Length: 1882\r\n\r\n"
    result = server.job.result()
    server.says("GET", target, head + trades[:1000])
    error = raised(qf.ServerUnavailable, result.to_backtest_result)
    assert str(error).startswith(f"could not reach the server at {server.url}/: IncompleteRead")
    # A refusal's body is the protocol's error envelope whatever was asked
    # for, and is held to the limit every answer is, not to the file's.
    long = refusal(500, "internal", "the server's store failed to answer " + "x" * 4000)
    server.says("GET", target, long)
    error = raised(qf.RemoteError, result.to_backtest_result)
    assert (error.code, error.status, len(str(error))) == ("internal", 500, 4036)
    server.says("GET", target, (200, trades, {}))
    assert len(result.to_backtest_result().trades) == 2


def test_a_file_that_is_not_what_the_result_format_writes_is_refused_though_its_hash_is_listed(
    server,
):
    trades = file("trades.sma_cross")
    # Bytes after the file's end, under the hash of all of them: what a
    # reader of Arrow would take, and the result format does not write.
    padded = trades + b"\0" * 8
    for raw, name, why in [
        (
            padded,
            "trades",
            "it is not what the result format writes (not a format 1 trades file: it is not the "
            "file its contents are written as)",
        ),
        (
            file("equity.sma_cross"),
            "trades",
            "it is not what the result format writes (not a format 1 trades file: it does not "
            "start with the trades schema)",
        ),
        (
            b"not arrow at all",
            "equity",
            "it is not what the result format writes (not a format 1 equity file: it does not "
            "start with the equity schema)",
        ),
        # Fewer rows, or more, than the job's metrics count.
        (file("trades.none"), "trades", f"it holds 0 trades, and job {JOB}'s metrics count 2"),
        # A symbol the engine does not hold.
        (
            file("trades.extremes"),
            "trades",
            "it is not what the result format writes (the symbol of its row 1: invalid symbol: "
            "empty)",
        ),
    ]:
        listed = listing()
        index = 1 if name == "trades" else 2
        listed["results"][index] = link(name, raw)
        server.says("GET", RESULT, (200, listed))
        server.holds(name, raw)
        result = server.job.result()
        error = raised(qf.RemoteError, result.to_backtest_result)
        assert str(error) == unread(server, why, f"{RESULT}/{FILES[name]}"), name
        server.holds(name, file(f"{name}.sma_cross"))


def test_a_symbol_that_is_not_in_its_canonical_form_is_no_symbol_of_a_result():
    # The shared cases' own table, with the seven bytes of a trade's symbol
    # written otherwise: in lower case, and with a space before them. Each
    # is a table the result format writes as readily, byte for byte, and no
    # symbol the engine writes.
    pinned = file("trades.sma_cross")
    assert pinned.count(b"BTCUSDTBTCUSDT") == 1 and pinned.count(b"BTCUSDT") == 2
    for symbols, row, symbol in [
        (b"btcusdtBTCUSDT", 0, "btcusdt"),
        (b"BTCUSDTBtcUsdt", 1, "BtcUsdt"),
        (b" TCUSDTBTCUSDT", 0, " TCUSDT"),
        (b"BTCUSDTBTCUSD\t", 1, "BTCUSD\t"),
    ]:
        other = pinned.replace(b"BTCUSDTBTCUSDT", symbols)
        with pytest.raises(ValueError) as refused:
            _native._result_trades(other)
        assert str(refused.value) == (
            f"the symbol of its row {row}, {json.dumps(symbol)}, is not in its canonical form"
        )


def test_a_file_longer_than_the_client_downloads_is_not_asked_for(server, monkeypatch):
    monkeypatch.setattr(_results, "_MOST_FILE_BYTES", 1881)
    result = server.job.result()
    error = raised(qf.RemoteError, result.to_backtest_result)
    assert str(error) == (
        f"job {JOB}'s result lists its trades file as 1882 bytes, and the client downloads a "
        "result file of at most 1881; read it with a tool that reads Arrow files from the "
        "server's own route"
    )
    assert server.fetched == ["/"]
    # One of exactly that length is.
    monkeypatch.setattr(_results, "_MOST_FILE_BYTES", 1882)
    assert len(result.to_backtest_result().trades) == 2
    assert _results._MOST_FILE_BYTES == 1882 and 1024**3 > 1882


def test_what_a_server_refuses_a_file_with_is_raised_in_its_words_and_asked_again(server):
    damaged = f"the server's copy of job {JOB}'s trades result is damaged"
    server.says("GET", f"{RESULT}/trades.arrow", refusal(500, "internal", damaged))
    result = server.job.result()
    error = raised(qf.RemoteError, result.to_backtest_result)
    assert (str(error), error.code, error.status) == (damaged, "internal", 500)
    # A server whose store could not answer in time is asked again, as for
    # any read, and the file taken once it comes.
    busy = refusal(
        503,
        "unavailable",
        "the server's store could not answer in time; retry later",
        {"Retry-After": "0"},
    )
    server.says("GET", f"{RESULT}/trades.arrow", busy, busy, (200, file("trades.sma_cross"), {}))
    assert len(result.to_backtest_result().trades) == 2
    assert server.fetched.count("/trades.arrow") == 4
