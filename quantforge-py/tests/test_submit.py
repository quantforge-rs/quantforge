"""A backtest submitted from Python, against servers of the test's own
(docs/python-bundles.md): what a client checks before it asks anything,
what it asks and in which order, what it sends, what it never sends twice,
and what it refuses of an answer.

A stand-in here stores what it is uploaded as a QuantForge server does,
under the hash of the bytes, and queues what it is submitted, unless a test
tells it to answer otherwise; it records every request. So each test says
exactly what a server said, and can see that nothing was uploaded.

A real `serve run`, which runs what is submitted, is in
`test_submit_jobs.py`.
"""

import datetime
import decimal
import json
import types

import pytest

import jobs
import quantforge_py as qf
from quantforge_py import _wire
from standin import FIRST, JOB, LAST, MINUTE, PYTHON, Serving, refusal, stored, written

MARKET = qf.MarketId("binance_spot", "BTCUSDT", "1m")
CONFIG = qf.BacktestConfig(initial_cash=10000, fee_bps=10, close_out_at_end=True)

HOLD = b"import quantforge_py as qf\n\n\nclass Hold(qf.Strategy):\n    def on_bar(self, ctx, bar):\n        return None\n"
OWN = f'format = 1\nentry = "strategy.py:Hold"\nengine = ">=0.1.0"\npython = "{PYTHON}"\n'.encode()


@pytest.fixture
def server():
    with Serving() as serving:
        yield serving


@pytest.fixture
def strategy(tmp_path):
    """A strategy's directory, with no manifest of its own."""
    root = tmp_path / "my_strategy"
    root.mkdir()
    (root / "strategy.py").write_bytes(HOLD)
    return root


@pytest.fixture
def no_waiting(monkeypatch):
    waited = []
    monkeypatch.setattr(_wire, "time", types.SimpleNamespace(sleep=waited.append))
    return waited


def manifest(python=PYTHON, params="", deps="") -> bytes:
    text = (
        f'format = 1\nentry = "strategy.py:Hold"\nengine = "=={qf.ENGINE_VERSION}"\n'
        f'python = "{python}"\n'
    )
    if params:
        text += f"\n[params]\n{params}\n"
    if deps:
        text += f"\n[deps]\n{deps}\n"
    return text.encode()


def submit(server, strategy, **named):
    arguments = {"entry": "strategy.py:Hold", "market": MARKET, "config": CONFIG, **named}
    return qf.Client(server.url).backtest.submit(strategy, **arguments)


def raised(kind, call):
    with pytest.raises(kind) as caught:
        call()
    assert type(caught.value) is kind
    return caught.value


# What is asked, and what is sent.


def test_a_submission_asks_what_the_server_runs_and_stores_then_uploads_and_submits(
    server, strategy
):
    client = qf.Client(server.url, token="a-token-the-stand-in-records")
    job = client.backtest.submit(
        strategy,
        entry="strategy.py:Hold",
        params={"slow": 50, "fast": 20},
        market=MARKET,
        config=CONFIG,
    )
    # The bundle is the directory's file beside a manifest written for the
    # server's Python, as the test's own builder makes it.
    files = {"quantforge.toml": manifest(params="fast = 20\nslow = 50"), "strategy.py": HOLD}
    tar = jobs.canonical_tar(files)
    named = jobs.code_hash(tar)
    assert server.routes == [
        "GET /version",
        "GET /v1/runtime",
        "GET /v1/markets",
        f"PUT /v1/bundles/{named}",
        "POST /v1/backtests",
    ]
    ((_, headers, body),) = server.sent("PUT")
    assert body == tar and server.held == {named: tar}
    assert headers["Content-Type"] == "application/x-tar"
    assert int(headers["Content-Length"]) == len(tar)
    ((_, headers, body),) = server.sent("POST")
    assert headers["Content-Type"] == "application/json"
    assert json.loads(body) == {
        "bundle": named,
        "data": {
            "exchange": "binance",
            "category": "spot",
            "symbol": "BTCUSDT",
            "interval": "1m",
            # Neither bound was given: the first and the last bar stored.
            "start_open_ms": FIRST,
            "end_open_ms": LAST,
        },
        "backtest": {"initial_cash": "10000", "fee_bps": "10", "close_out_at_end": True},
    }
    # Every request says who asks, and carries the token.
    for _, _, headers, _ in server.asked:
        assert headers["Authorization"] == "Bearer a-token-the-stand-in-records"
        assert headers["User-Agent"] == f"quantforge-py/{qf.__version__}"
        assert headers["Connection"] == "close"
    # What comes back is the job the server queued, as a handle on it.
    assert type(job) is qf.Job and (job.id, job.status.state) == (JOB, "queued")
    assert (job.status.bundle, job.status.market, job.status.config) == (named, MARKET, CONFIG)
    assert (job.status.start_open_ms, job.status.end_open_ms) == (FIRST, LAST)
    assert repr(job) == f"<Job {JOB} queued>"
    # A second submission agrees on no version again, and a bundle the
    # server holds is uploaded all the same: the server says it held it.
    before = len(server.asked)
    again = client.backtest.submit(
        strategy,
        entry="strategy.py:Hold",
        params={"fast": 20, "slow": 50},
        market=MARKET,
        config=CONFIG,
    )
    assert server.routes[before:] == server.routes[1:5] and again.status.bundle == named


def test_a_single_file_and_a_directory_of_its_own_manifest_are_bundled_as_they_are(
    server, strategy, tmp_path
):
    # One file, under its own name.
    job = submit(server, strategy / "strategy.py", params={"fast": 2})
    tar = jobs.canonical_tar({"quantforge.toml": manifest(params="fast = 2"), "strategy.py": HOLD})
    assert job.status.bundle == jobs.code_hash(tar) and server.sent("PUT")[-1][2] == tar
    # A directory that says itself what it runs, sent as it is.
    (strategy / "quantforge.toml").write_bytes(OWN)
    job = qf.Client(server.url).backtest.submit(strategy, market=MARKET, config=CONFIG)
    tar = jobs.canonical_tar({"quantforge.toml": OWN, "strategy.py": HOLD})
    assert job.status.bundle == jobs.code_hash(tar) and server.sent("PUT")[-1][2] == tar
    # Nothing was written beside the strategy.
    assert sorted(path.name for path in strategy.iterdir()) == ["quantforge.toml", "strategy.py"]


def test_a_server_under_a_path_is_asked_and_sent_to_under_it(strategy):
    class Under(Serving):
        def answering(self, method, target, sent):
            return super().answering(method, target.removeprefix("/lab"), sent)

    with Under() as server:
        server.says("GET", "/lab/version", server._answers[("GET", "/version")][0])
        server.says("GET", "/lab/v1/runtime", server._answers[("GET", "/v1/runtime")][0])
        server.says("GET", "/lab/v1/markets", server._answers[("GET", "/v1/markets")][0])
        client = qf.Client(f"{server.url}/lab")
        job = client.backtest.submit(
            strategy, entry="strategy.py:Hold", market=MARKET, config=CONFIG
        )
        assert server.routes == [
            "GET /lab/version",
            "GET /lab/v1/runtime",
            "GET /lab/v1/markets",
            f"PUT /lab/v1/bundles/{job.status.bundle}",
            "POST /lab/v1/backtests",
        ]


def test_the_settings_and_the_market_are_sent_as_the_protocol_writes_them(strategy):
    markets = [
        stored(),
        stored(exchange="bybit", category="linear", symbol="ETHUSDT", interval="4h"),
        stored(exchange="binance", category="inverse", symbol="BTCUSD_PERP", interval="1d"),
    ]
    with Serving(markets=markets) as server:
        for market, named in [
            (qf.MarketId("bybit_linear", "ETHUSDT", "4h"), ("bybit", "linear", "ETHUSDT", "4h")),
            (
                qf.MarketId("binance_inverse", "BTCUSD_PERP", "1d"),
                ("binance", "inverse", "BTCUSD_PERP", "1d"),
            ),
        ]:
            config = qf.BacktestConfig(
                initial_cash=decimal.Decimal("2500.00"),
                fee_bps=decimal.Decimal("7.50"),
                close_out_at_end=False,
            )
            job = submit(server, strategy, market=market, config=config)
            sent = json.loads(server.sent("POST")[-1][2])
            data = sent["data"]
            assert (data["exchange"], data["category"], data["symbol"], data["interval"]) == named
            # An amount with every digit it was given, as exact text.
            assert sent["backtest"] == {
                "initial_cash": "2500.00",
                "fee_bps": "7.50",
                "close_out_at_end": False,
            }
            assert (job.status.market, job.status.config) == (market, config)


# The window.


def test_the_window_is_the_bars_the_server_stores_between_its_bounds(server, strategy):
    def window(**bounds):
        submit(server, strategy, **bounds)
        data = json.loads(server.sent("POST")[-1][2])["data"]
        return data["start_open_ms"], data["end_open_ms"]

    tenth, twentieth = FIRST + 10 * MINUTE, FIRST + 20 * MINUTE
    utc = datetime.timezone.utc
    assert window() == (FIRST, LAST)
    assert window(start=tenth) == (tenth, LAST)
    assert window(end=twentieth) == (FIRST, twentieth)
    assert window(start=tenth, end=twentieth) == (tenth, twentieth)
    assert window(start=tenth, end=tenth) == (tenth, tenth)
    # A bound beyond what the server stores reaches its first or last bar.
    assert window(start=FIRST - 86_400_000, end=LAST + 86_400_000) == (FIRST, LAST)
    assert window(start=0) == (FIRST, LAST)
    # Each of the three ways a time is written, read as a reader reads a
    # range's: a time between two milliseconds bounds the window where it
    # stands.
    assert window(start="2026-01-01T12:10:00Z", end="2026-01-01T13:20:00+01:00") == (
        tenth,
        twentieth,
    )
    at = datetime.datetime(2026, 1, 1, 12, 10, tzinfo=utc)
    assert window(start=at, end=at + datetime.timedelta(minutes=10)) == (tenth, twentieth)
    assert window(start="2026-01-01T12:10:00.0005Z") == (tenth + 1, LAST)
    assert window(end=at + datetime.timedelta(microseconds=1500)) == (FIRST, tenth + 1)


def test_a_market_or_a_window_the_server_stores_no_bar_of_is_refused_before_anything_is_uploaded(
    server, strategy
):
    url = f"{server.url}/"
    error = raised(
        qf.ValidationError,
        lambda: submit(server, strategy, market=qf.MarketId("binance_spot", "ETHUSDT", "1m")),
    )
    assert str(error) == f"the server at {url} stores no bars of binance spot ETHUSDT 1m"
    # The client's own refusal, of what the server would refuse: no code
    # and no status came with it.
    assert (error.code, error.status) == (None, None)
    for market in [
        qf.MarketId("binance_linear", "BTCUSDT", "1m"),
        qf.MarketId("bybit_spot", "BTCUSDT", "1m"),
        qf.MarketId("binance_spot", "BTCUSDT", "5m"),
    ]:
        raised(qf.ValidationError, lambda: submit(server, strategy, market=market))
    stores = "stores binance spot BTCUSDT 1m from 2026-01-01T12:00:00Z to 2026-01-01T12:59:00Z"
    for bounds, shown in [
        ({"start": LAST + MINUTE}, "2026-01-01T13:00:00Z to 2026-01-01T12:59:00Z"),
        ({"end": FIRST - 1}, "2026-01-01T12:00:00Z to 2026-01-01T11:59:59.999Z"),
        (
            {"start": "2027-01-01T00:00:00Z", "end": "2027-01-02T00:00:00Z"},
            "2027-01-01T00:00:00Z to 2027-01-02T00:00:00Z",
        ),
    ]:
        error = raised(qf.ValidationError, lambda: submit(server, strategy, **bounds))
        assert str(error) == f"the server at {url} {stores}, outside the window from {shown}"
    assert not server.sent("PUT") and not server.sent("POST") and server.held == {}
    assert set(server.routes) == {"GET /version", "GET /v1/runtime", "GET /v1/markets"}


def test_a_window_that_ends_before_it_starts_and_a_bound_that_is_no_time_are_refused_unasked(
    server, strategy, refused
):
    refused(
        ValueError,
        "the window ends before it starts: start is 2026-01-01T12:10:00Z and end "
        "2026-01-01T12:09:00Z, each the whole millisecond it bounds the window at",
        submit,
        server,
        strategy,
        start=FIRST + 10 * MINUTE,
        end=FIRST + 9 * MINUTE,
    )
    # Two bounds inside one millisecond bound a window that holds none.
    with pytest.raises(ValueError, match="^the window ends before it starts: start is "):
        submit(server, strategy, start="2026-01-01T12:10:00.0002Z", end="2026-01-01T12:10:00.0008Z")
    # A bound is refused as a reader refuses one.
    naive = datetime.datetime(2026, 1, 1, 12, 10)
    for bounds, kind, said in [
        (
            {"start": 1.5},
            TypeError,
            "start must be an int of milliseconds, a datetime.datetime with a time zone, a str or None, not float",
        ),
        (
            {"end": True},
            TypeError,
            "end must be an int of milliseconds, a datetime.datetime with a time zone, a str or None, not bool",
        ),
        (
            {"start": naive},
            TypeError,
            "start must be a datetime.datetime with a time zone, not one without: a time without a zone is never read as an instant; give it one, such as datetime.timezone.utc",
        ),
        ({"end": "noon"}, ValueError, None),
    ]:
        with pytest.raises(kind) as caught:
            submit(server, strategy, **bounds)
        assert said is None or str(caught.value) == said
    assert server.asked == []


# Before anything is asked.


def test_what_is_refused_of_the_arguments_is_refused_before_anything_is_asked(
    server, strategy, refused
):
    for arguments, kind, said in [
        ({"market": "binance_spot BTCUSDT 1m"}, TypeError, "market must be a MarketId, not str"),
        ({"market": None}, TypeError, "market must be a MarketId, not NoneType"),
        ({"config": None}, TypeError, "config must be a BacktestConfig, not NoneType"),
        (
            {"config": {"initial_cash": 10000}},
            TypeError,
            "config must be a BacktestConfig, not dict",
        ),
        ({"params": [1]}, TypeError, "params must be a dict or None, not list"),
        (
            {"deps": "numpy"},
            TypeError,
            "deps must be a dict of each package's version, a list or a tuple of package names, or None, not str",
        ),
        ({"entry": 7}, TypeError, "entry must be a str, not int"),
        (
            {"entry": "strategy:Hold"},
            ValueError,
            "entry 'strategy:Hold' names a module: name the strategy's file and its class, as a "
            "bundle's manifest does, such as 'strategy.py:Hold'",
        ),
    ]:
        refused(kind, said, submit, server, strategy, **arguments)
    # What the signature requires, Python itself asks for.
    client = qf.Client(server.url)
    with pytest.raises(
        TypeError, match="missing 2 required keyword-only arguments: 'market' and 'config'"
    ):
        client.backtest.submit(strategy, entry="strategy.py:Hold")
    with pytest.raises(TypeError, match="takes 2 positional arguments but 3 "):
        client.backtest.submit(strategy, "strategy.py:Hold", market=MARKET, config=CONFIG)
    assert server.asked == []


def test_a_bundle_the_spec_refuses_is_refused_before_anything_is_asked(server, strategy, tmp_path):
    # A file no bundle holds, a file too large, and an entry that names no
    # file of it: each is the spec's own problem, by its code.
    (strategy / "notes.md").write_bytes(b"x")
    error = raised(qf.BundleError, lambda: submit(server, strategy))
    assert [code for code, _ in error.problems] == ["extension_not_allowed"]
    (strategy / "notes.md").unlink()
    (strategy / "big.txt").write_bytes(b"x" * (1024 * 1024 + 1))
    error = raised(qf.BundleError, lambda: submit(server, strategy))
    assert [code for code, _ in error.problems] == ["file_too_large"]
    assert str(error) == (
        'the bundle is refused: "big.txt" is 1048577 bytes, over the limit of 1048576 bytes for '
        "one file"
    )
    (strategy / "big.txt").unlink()
    error = raised(qf.BundleError, lambda: submit(server, strategy, entry="other.py:Hold"))
    assert [code for code, _ in error.problems] == ["entry_not_found"]
    error = raised(qf.BundleError, lambda: submit(server, strategy, deps=["NumPy"]))
    assert [code for code, _ in error.problems] == ["dependency_invalid"]
    with pytest.raises(FileNotFoundError):
        submit(server, tmp_path / "nowhere")
    assert server.asked == []


# What the server runs.


def test_a_bundle_the_servers_runtime_cannot_run_is_refused_before_anything_is_uploaded(
    server, strategy
):
    url = f"{server.url}/"
    for arguments, expected, said in [
        (
            {"deps": ["scipy"]},
            ["dependency_not_allowed"],
            "the bundle imports scipy, which this server does not offer; it offers numpy",
        ),
        (
            {"deps": {"numpy": "2.1.3"}},
            ["dependency_version_unavailable"],
            "the bundle pins numpy 2.1.3, but this server offers numpy 2.4.6 only",
        ),
    ]:
        error = raised(qf.BundleError, lambda: submit(server, strategy, **arguments))
        assert [code for code, _ in error.problems] == expected
        assert str(error) == f"the server at {url} cannot run the bundle: {said}"
    # A directory's own manifest, written for another Python.
    (strategy / "quantforge.toml").write_bytes(OWN.replace(b'"3.12"', b'"3.9"'))
    error = raised(
        qf.BundleError,
        lambda: qf.Client(server.url).backtest.submit(strategy, market=MARKET, config=CONFIG),
    )
    assert error.problems == (
        (
            "python_mismatch",
            "the bundle was written for Python 3.9, but this server runs Python 3.12",
        ),
    )
    # The server was asked what it runs, and nothing was sent to it.
    assert set(server.routes) == {"GET /version", "GET /v1/runtime"} and server.held == {}


def test_a_package_named_alone_is_pinned_at_the_version_the_server_offers(strategy):
    packages = {"numpy": ["2.4.6"], "pandas": ["3.1.1"], "scipy": ["1.14.0", "1.15.2"]}
    with Serving(packages=packages) as server:
        job = submit(server, strategy, deps=["pandas", "numpy"])
        written_for = manifest(deps='numpy = "2.4.6"\npandas = "3.1.1"')
        tar = jobs.canonical_tar({"quantforge.toml": written_for, "strategy.py": HOLD})
        assert server.sent("PUT")[-1][2] == tar and job.status.bundle == jobs.code_hash(tar)
        # The same bundle as the pins written out.
        pinned = submit(server, strategy, deps={"numpy": "2.4.6", "pandas": "3.1.1"})
        assert pinned.status.bundle == job.status.bundle
        # More than one version on offer: the caller says which.
        before = len(server.sent("PUT"))
        error = raised(ValueError, lambda: submit(server, strategy, deps=["scipy"]))
        assert str(error) == (
            f"deps names scipy without a version, and the server at {server.url}/ offers 1.14.0 "
            "and 1.15.2 of it: pin one, as in deps={'scipy': '1.15.2'}"
        )
        assert len(server.sent("PUT")) == before
        assert submit(server, strategy, deps={"scipy": "1.14.0"}).status.state == "queued"


def test_a_server_that_runs_no_bundles_is_unavailable_and_sent_nothing(strategy):
    with Serving() as server:
        server.says("GET", "/v1/runtime", (200, {}))
        error = raised(qf.ServerUnavailable, lambda: submit(server, strategy))
        assert str(error) == (
            f"the server at {server.url}/ runs no Python bundles: it was started without "
            "--job-python"
        )
        assert (error.code, error.status) == (None, None)
        server.says("GET", "/v1/runtime", (200, {"runtime": None}))
        raised(qf.ServerUnavailable, lambda: submit(server, strategy))
        assert set(server.routes) == {"GET /version", "GET /v1/runtime"}


@pytest.mark.parametrize(
    "answer, why",
    [
        ([], "the server's runtime is not an object"),
        (
            {"runtime": ["0.2.0", "3.12", {}]},
            "the `runtime` of the server's runtime is not an object",
        ),
        (
            {"runtime": {"engine": "0.2", "python": "3.12", "packages": {}}},
            "it is no runtime as the protocol writes one (invalid engine version",
        ),
        (
            {"runtime": {"engine": "0.2.0", "python": "3", "packages": {}}},
            "it is no runtime as the protocol writes one (invalid python version",
        ),
        (
            {"runtime": {"engine": "0.2.0", "python": "3.12"}},
            "it is no runtime as the protocol writes one (missing field `packages`",
        ),
        (
            {"runtime": {"engine": "0.2.0", "python": "3.12", "packages": {"NumPy": ["2.1.3"]}}},
            "it is no runtime as the protocol writes one (invalid package name",
        ),
        ({"runtime": "3.12"}, "the `runtime` of the server's runtime is not an object"),
        (
            {"runtime": {"engine": "0.2.0", "python": "3.12", "packages": {"numpy": "2.1.3"}}},
            "it is no runtime as the protocol writes one (invalid type: string",
        ),
    ],
)
def test_a_runtime_the_protocol_does_not_write_is_refused_and_never_guessed_at(
    strategy, answer, why
):
    with Serving() as server:
        server.says("GET", "/v1/runtime", (200, answer))
        error = raised(qf.RemoteError, lambda: submit(server, strategy))
        assert str(error).startswith(
            f"the server at {server.url}/ answered GET /v1/runtime with a body the protocol does "
            f"not read: {why}"
        )
        assert str(error).endswith("; is it a QuantForge server?")
        assert set(server.routes) == {"GET /version", "GET /v1/runtime"}


@pytest.mark.parametrize(
    "answer, why",
    [
        ([], "the list of markets is not an object"),
        ({}, "the list of markets has no `markets`"),
        ({"markets": [7]}, "a market is not an object"),
        (
            {"markets": [{**stored(), "first_open_ms": "noon"}]},
            "the `first_open_ms` of a market is not an integer",
        ),
        (
            {"markets": [stored(first=LAST, last=FIRST)]},
            "the first bar of binance spot BTCUSDT 1m opens after its last",
        ),
        ({"markets": [stored(), {"exchange": "binance"}]}, "a market has no `category`"),
    ],
)
def test_a_list_of_markets_the_protocol_does_not_write_is_refused(strategy, answer, why):
    with Serving() as server:
        server.says("GET", "/v1/markets", (200, answer))
        error = raised(qf.RemoteError, lambda: submit(server, strategy))
        assert str(error) == (
            f"the server at {server.url}/ answered GET /v1/markets with a body the protocol does "
            f"not read: {why}; is it a QuantForge server?"
        )
        assert not server.sent("PUT")


# The upload.


def test_an_upload_is_sent_again_while_the_server_is_busy_and_a_submission_never(
    strategy, no_waiting
):
    busy = refusal(
        503,
        "unavailable",
        "the server's store could not answer in time; retry later",
        {"Retry-After": "1"},
    )
    with Serving() as server:
        # The upload of a bundle changes nothing the second time: a server
        # stores it once, under the hash of its bytes.
        named = jobs.code_hash(
            jobs.canonical_tar({"quantforge.toml": manifest(), "strategy.py": HOLD})
        )
        receipt = (201, {"code_hash": named, "size_bytes": 3072}, {})
        server.says("PUT", f"/v1/bundles/{named}", busy, busy, receipt)
        job = submit(server, strategy)
        assert len(server.sent("PUT")) == 3 and no_waiting == [1, 1]
        assert job.status.bundle == named and len(server.sent("POST")) == 1
        # A submission the server could not answer for may have been taken:
        # it is never sent a second time, whatever the server asks.
        no_waiting.clear()
        server.says("POST", "/v1/backtests", busy)
        error = raised(qf.ServerUnavailable, lambda: submit(server, strategy))
        assert str(error) == "the server's store could not answer in time; retry later"
        assert (error.code, error.status) == ("unavailable", 503)
        assert len(server.sent("POST")) == 2 and no_waiting == []
        # And an upload that stays busy is given up after its five tries.
        server.says("PUT", f"/v1/bundles/{named}", busy)
        before = len(server.sent("PUT"))
        raised(qf.ServerUnavailable, lambda: submit(server, strategy))
        assert len(server.sent("PUT")) - before == 5 and len(server.sent("POST")) == 2


def test_an_upload_the_server_refuses_is_raised_in_its_words_and_nothing_is_submitted(strategy):
    with Serving() as server:
        tar = jobs.canonical_tar({"quantforge.toml": manifest(), "strategy.py": HOLD})
        named = jobs.code_hash(tar)
        said = (
            f"the upload hashes to sha256:{'0' * 64}, not to {named}, the name it was sent under; "
            "send a bundle's canonical tar under its code hash"
        )
        server.says("PUT", f"/v1/bundles/{named}", refusal(400, "invalid_request", said))
        error = raised(qf.ValidationError, lambda: submit(server, strategy))
        assert (str(error), error.code, error.status) == (said, "invalid_request", 400)
        assert not server.sent("POST")


def test_a_receipt_for_another_bundle_is_no_receipt_and_nothing_is_submitted(strategy):
    tar = jobs.canonical_tar({"quantforge.toml": manifest(), "strategy.py": HOLD})
    named, other = jobs.code_hash(tar), "sha256:" + "ab" * 32
    route = f"/v1/bundles/{named}"
    for answer, why in [
        (
            {"code_hash": other, "size_bytes": len(tar)},
            f"it is a receipt for bundle {other}, and the bundle uploaded is {named}",
        ),
        (
            {"code_hash": named, "size_bytes": len(tar) + 512},
            f"it says that bundle {named} holds {len(tar) + 512} bytes, and {len(tar)} were uploaded",
        ),
        (
            {"code_hash": named.upper(), "size_bytes": len(tar)},
            "the `code_hash` of the receipt is no content hash",
        ),
        ({"code_hash": named}, "the receipt has no `size_bytes`"),
        ({"size_bytes": len(tar)}, "the receipt has no `code_hash`"),
        ([named], "the receipt is not an object"),
    ]:
        with Serving() as server:
            server.says("PUT", route, (201, answer))
            error = raised(qf.RemoteError, lambda: submit(server, strategy))
            assert str(error) == (
                f"the server at {server.url}/ answered PUT {route} with a body the protocol does "
                f"not read: {why}; is it a QuantForge server?"
            )
            assert not server.sent("POST")


# The submission.


def test_a_submission_the_server_refuses_is_raised_in_its_words(server, strategy):
    said = "the range spans 600 bars of 1m, more than the 60 this server runs one job on; ask for a shorter range"
    server.says("POST", "/v1/backtests", refusal(400, "invalid_request", said))
    error = raised(qf.ValidationError, lambda: submit(server, strategy))
    assert (str(error), error.code, error.status) == (said, "invalid_request", 400)
    assert len(server.sent("PUT")) == 1


def test_a_job_that_does_not_run_what_was_submitted_is_no_answer_to_a_submission(strategy):
    def answered(**changed):
        def answer():
            submitted = json.loads(server.sent("POST")[-1][2])
            for path, value in changed.items():
                target = submitted
                *parents, last = path.split("__")
                for parent in parents:
                    target = target[parent]
                target[last] = value
            return 201, written(submission=submitted), {}

        return answer

    for changed in [
        {"bundle": "sha256:" + "cd" * 32},
        {"data__symbol": "ETHUSDT"},
        {"data__start_open_ms": FIRST + MINUTE},
        {"data__end_open_ms": LAST - MINUTE},
        {"backtest__fee_bps": "11"},
        {"backtest__close_out_at_end": False},
    ]:
        with Serving() as server:
            server.says("POST", "/v1/backtests", answered(**changed))
            error = raised(qf.RemoteError, lambda: submit(server, strategy))
            assert str(error) == (
                f"the server at {server.url}/ answered POST /v1/backtests with a body the "
                f"protocol does not read: it is the status of job {JOB}, which does not run what "
                "was submitted; is it a QuantForge server?"
            ), changed
    # An amount written with other digits is the same amount.
    with Serving() as server:
        server.says("POST", "/v1/backtests", answered(backtest__fee_bps="10.0"))
        assert submit(server, strategy).status.config == CONFIG
    # And a job that already runs, on a server that started it at once, is
    # the job all the same.
    with Serving() as server:

        def running():
            submitted = json.loads(server.sent("POST")[-1][2])
            state = {"kind": "running", "started_at_ms": 1_792_454_400_001}
            return 201, written(submission=submitted, state=state), {}

        server.says("POST", "/v1/backtests", running)
        assert submit(server, strategy).status.state == "running"


def test_a_server_that_goes_away_as_a_job_is_submitted_is_unavailable_and_not_asked_again(strategy):
    with Serving() as server:
        server.says("POST", "/v1/backtests", (502, "bad gateway"))
        error = raised(qf.ServerUnavailable, lambda: submit(server, strategy))
        assert error.status == 502 and len(server.sent("POST")) == 1


# A class.


class Holds(qf.Strategy):
    """Holds nothing, whatever its windows."""

    def __init__(self, fast: int = 2):
        self.fast = fast

    def on_bar(self, ctx, bar):
        return None


def test_a_class_is_submitted_as_a_bundle_of_its_source_and_a_manifest_written_for_it(server):
    client = qf.Client(server.url)
    job = client.backtest.submit_class(
        Holds, params={"fast": 3}, market=MARKET, config=CONFIG, source_only=True
    )
    source = (
        "import quantforge_py as qf\n\n\n"
        "class Holds(qf.Strategy):\n"
        '    """Holds nothing, whatever its windows."""\n\n'
        "    def __init__(self, fast: int = 2):\n"
        "        self.fast = fast\n\n"
        "    def on_bar(self, ctx, bar):\n"
        "        return None\n"
    ).encode()
    written_for = manifest(params="fast = 3").replace(b"strategy.py:Hold", b"strategy.py:Holds")
    tar = jobs.canonical_tar({"quantforge.toml": written_for, "strategy.py": source})
    assert server.sent("PUT")[-1][2] == tar and job.status.bundle == jobs.code_hash(tar)
    assert server.routes == [
        "GET /version",
        "GET /v1/runtime",
        "GET /v1/markets",
        f"PUT /v1/bundles/{job.status.bundle}",
        "POST /v1/backtests",
    ]
    # With the imports it declares, and the packages, as `submit` takes them.
    job = client.backtest.submit_class(
        Holds,
        imports="import quantforge_py as qf\nimport numpy as np",
        deps=["numpy"],
        market=MARKET,
        start=FIRST + MINUTE,
        end=FIRST + 2 * MINUTE,
        config=CONFIG,
        source_only=True,
    )
    sent = server.sent("PUT")[-1][2]
    assert (
        b"import quantforge_py as qf\nimport numpy as np\n\n\nclass Holds(qf.Strategy):\n" in sent
    )
    assert b'[deps]\nnumpy = "2.4.6"\n' in sent
    assert (job.status.start_open_ms, job.status.end_open_ms) == (
        FIRST + MINUTE,
        FIRST + 2 * MINUTE,
    )


def test_a_class_is_not_submitted_unless_its_caller_says_that_its_source_is_all_that_is_sent(
    server, refused
):
    client = qf.Client(server.url)
    with pytest.raises(ValueError) as caught:
        client.backtest.submit_class(Holds, market=MARKET, config=CONFIG)
    assert str(caught.value).startswith(
        "submit_class sends the source text of the class as the one file of a bundle, and nothing "
        "else of where the class was written"
    )
    assert str(caught.value).endswith(
        "Pass source_only=True to say that this is what is meant; a strategy kept in files is "
        "submitted with submit()"
    )
    refused(
        TypeError,
        "source_only must be a bool, not int",
        client.backtest.submit_class,
        Holds,
        market=MARKET,
        config=CONFIG,
        source_only=1,
    )

    threshold = 3

    class Captures(qf.Strategy):
        def on_bar(self, ctx, bar):
            return None if threshold else None

    for strategy, kind, said in [
        (
            Captures,
            ValueError,
            "Captures is defined inside the function "
            "test_a_class_is_not_submitted_unless_its_caller_says_that_its_source_is_all_that_is_sent, "
            "and uses its threshold: a class is sent as its own source",
        ),
        (
            lambda ctx, bar: None,
            TypeError,
            "strategy must be a class derived from quantforge_py.Strategy, not function",
        ),
        (
            Holds(),
            TypeError,
            "strategy must be a class derived from quantforge_py.Strategy, not an object of one",
        ),
    ]:
        with pytest.raises(kind) as caught:
            client.backtest.submit_class(strategy, market=MARKET, config=CONFIG, source_only=True)
        assert str(caught.value).startswith(said)
    # Nothing of any of them reached the server.
    assert server.asked == []
