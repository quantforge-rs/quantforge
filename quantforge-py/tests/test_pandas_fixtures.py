"""The repository's Spot fixtures, taken through frames: the ones that were
there before a market got its product line, and the ones written since.

- `tests/fixtures/golden/`: the frozen day of Spot candles and what the
  engine's SMA cross made of them.
- `tests/fixtures/strategy_cases.json`: the candles and the results of the
  strategy boundary's shared cases.
- `tests/fixtures/storage_identity_cases.json`: every product line as a
  store names it, and decimal texts a store gives back unchanged, written by
  a model that is not the engine.
- `tests/fixtures/decimal_float_cases.csv`: the float policy's shared cases,
  computed by Python.
- `docs/result-spec.md`: the columns of a server job's result files.

A frame of candles is what Python itself reads from a fixture's text, a
batch that crosses to a frame and back is the same batch by value, a
backtest of it is the frozen result by value, and the frames of a result
hold Python's reading of the result's text, cell for cell. None of the
fixtures is changed: they are read, and what is expected of a frame is
worked out from their text without the package."""

import csv
import datetime
import importlib.util
import json
import pathlib
import struct
import sys
from decimal import Decimal

import pytest

import quantforge_py as qf

# The frames need pandas and NumPy, the package's two extras. Where the
# package is installed without them there is nothing of the frames to test,
# and the run says so: `test_pandas_optional.py` holds what the package says
# there.
np = pytest.importorskip("numpy")
pd = pytest.importorskip("pandas")

# What a frame is made with is to draw no warning from pandas or NumPy.
pytestmark = pytest.mark.usefixtures("no_warning_drawn_by_the_package")

REPOSITORY = pathlib.Path(__file__).resolve().parents[2]
FIXTURES = REPOSITORY / "tests" / "fixtures"
GOLDEN = FIXTURES / "golden"

IDENTITY = json.loads((FIXTURES / "storage_identity_cases.json").read_text("utf-8"))
EXPECTED = json.loads((GOLDEN / "sma_cross.json").read_text("utf-8"))
CASES = json.loads((FIXTURES / "strategy_cases.json").read_text("utf-8"))["cases"]

FLOATS = ["open", "high", "low", "close", "volume"]
AMOUNTS = ["entry_price", "exit_price", "qty", "gross_quote_pnl"]
TIMES = "datetime64[ms, UTC]"


def bits(value: float) -> str:
    return struct.pack(">d", value).hex()


def bits_of(column) -> list[str]:
    """A float64 column's cells, each as its bit pattern."""
    array = np.ascontiguousarray(column.to_numpy())
    assert array.dtype == np.float64
    return [format(pattern, "016x") for pattern in array.view(np.uint64).tolist()]


def read(text: str) -> str:
    """Python's own reading of a decimal's text as a float, as its bit
    pattern, with no part of the package in it. A zero is `+0.0` whatever
    sign its text has, as the policy has it."""
    return bits(float(text) + 0.0)


def stamps(times) -> list[int]:
    """The integers pandas keeps for a frame's times, which are the engine's
    milliseconds: `test_pandas.py` holds them to Python's own dates."""
    index = pd.DatetimeIndex(times)
    assert str(index.dtype) == TIMES
    return index.asi8.tolist()


def written(value: Decimal) -> str:
    """A decimal with every digit it has and no exponent, as a store and a
    fixture write one."""
    return format(value, "f")


def golden_rows() -> list[dict]:
    """The frozen dataset's lines, each field as the file writes it."""
    with open(GOLDEN / "candles.csv", newline="", encoding="utf-8") as file:
        return list(csv.DictReader(file))


def candle_of(row: dict) -> qf.Candle:
    return qf.Candle(
        open_time_ms=int(row["open_time_ms"]),
        close_time_ms=int(row["close_time_ms"]),
        # A trade count that was not reported is an empty field or a null.
        trades=int(row["trades"]) if row["trades"] not in ("", None) else None,
        **{field: Decimal(row[field]) for field in FLOATS},
    )


def result_of(given: dict) -> qf.BacktestResult:
    """A result as a fixture writes one, built from its text."""
    return qf.BacktestResult(
        **{
            field: Decimal(given[field])
            for field in ("initial_cash", "final_equity", "total_return_pct", "max_drawdown_pct")
        },
        trade_count=given["trade_count"],
        trades=[
            qf.ClosedTrade(
                symbol=trade["symbol"],
                entry_time_ms=trade["entry_time_ms"],
                exit_time_ms=trade["exit_time_ms"],
                entry_order_id=trade["entry_order_id"],
                exit_order_id=trade["exit_order_id"],
                **{field: Decimal(trade[field]) for field in AMOUNTS},
            )
            for trade in given["trades"]
        ],
        equity_curve=[
            qf.EquityPoint(open_time_ms=point["open_time_ms"], equity=Decimal(point["equity"]))
            for point in given["equity_curve"]
        ],
    )


def held_to_the_text(result: qf.BacktestResult, given: dict) -> None:
    """Holds the two frames of `result` to a fixture's own text of it."""
    trades = qf.trades_to_dataframe(result)
    assert len(trades) == len(given["trades"]) == given["trade_count"]
    assert trades["symbol"].tolist() == [trade["symbol"] for trade in given["trades"]]
    for column in ["entry_time", "exit_time"]:
        assert stamps(trades[column]) == [trade[f"{column}_ms"] for trade in given["trades"]]
    for column in AMOUNTS:
        assert bits_of(trades[column]) == [read(trade[column]) for trade in given["trades"]], column
    for column in ["entry_order_id", "exit_order_id"]:
        assert trades[column].tolist() == [trade[column] for trade in given["trades"]], column
    equity = qf.equity_to_dataframe(result)
    assert stamps(equity.index) == [point["open_time_ms"] for point in given["equity_curve"]]
    assert bits_of(equity["equity"]) == [read(point["equity"]) for point in given["equity_curve"]]


def sma_cross() -> type:
    """The SMA cross written in Python, from its file beside the goldens:
    imported by its path, and no bytecode written beside the fixture."""
    path = GOLDEN / "sma_cross.py"
    spec = importlib.util.spec_from_file_location(path.stem, path)
    module = importlib.util.module_from_spec(spec)
    wrote, sys.dont_write_bytecode = sys.dont_write_bytecode, True
    try:
        spec.loader.exec_module(module)
    finally:
        sys.dont_write_bytecode = wrote
    return module.SmaCross


@pytest.fixture(scope="module")
def golden_market() -> qf.MarketId:
    return qf.MarketId(**EXPECTED["dataset"]["market"])


@pytest.fixture(scope="module")
def golden_batch(golden_market) -> qf.CandleBatch:
    candles = [candle_of(row) for row in golden_rows()]
    return qf.CandleBatch.from_candles(candles, market=golden_market)


@pytest.fixture(scope="module")
def golden_config() -> qf.BacktestConfig:
    settings = EXPECTED["config"]
    return qf.BacktestConfig(
        initial_cash=Decimal(settings["initial_cash"]),
        fee_bps=Decimal(settings["fee_bps"]),
        close_out_at_end=settings["close_out_at_end"],
    )


# --- the frozen day of candles ------------------------------------------------


def test_the_frozen_days_frame_is_what_python_reads_from_the_file(golden_batch):
    rows = golden_rows()
    frame = qf.candles_to_dataframe(golden_batch)
    assert len(frame) == EXPECTED["dataset"]["candles"] == 1440
    assert stamps(frame.index) == [int(row["open_time_ms"]) for row in rows]
    assert stamps(frame["close_time"]) == [int(row["close_time_ms"]) for row in rows]
    for column in FLOATS:
        # Python's own reading of the text the file holds, with no part of
        # the package in it.
        assert bits_of(frame[column]) == [read(row[column]) for row in rows], column
    assert frame["trades"].tolist() == [int(row["trades"]) for row in rows]
    assert not frame["trades"].isna().any()
    # The day is the one its file says it is: the first bar opened at noon
    # UTC on the first of January 2026, and each a minute after the last.
    dataset = EXPECTED["dataset"]
    for time, said in [
        (frame.index[0], dataset["first_open"]),
        (frame.index[-1], dataset["last_open"]),
    ]:
        assert time == datetime.datetime.fromisoformat(said.replace("Z", "+00:00"))
    assert (frame.index[0].hour, frame.index[0].tzinfo) == (12, datetime.timezone.utc)
    assert frame.index.is_monotonic_increasing and frame.index.is_unique
    assert set(frame.index[1:] - frame.index[:-1]) == {pd.Timedelta(minutes=1)}
    assert set(frame["close_time"] - frame.index) == {pd.Timedelta(milliseconds=59_999)}


def test_the_frozen_day_comes_back_from_a_frame_the_same_batch_by_value(golden_batch):
    frame = qf.candles_to_dataframe(golden_batch)
    back = qf.dataframe_to_candles(frame, market=golden_batch.market)
    # A price with eight decimal places and five digits before them has
    # thirteen digits, which a float holds.
    assert back == golden_batch
    assert back.market == golden_batch.market
    assert back.to_candles() == golden_batch.to_candles()
    # Without its market it is the same candles and another batch.
    assert qf.dataframe_to_candles(frame) != golden_batch
    # It is the batch the day's own arrays build.
    assert back == qf.CandleBatch.from_numpy(**golden_batch.to_numpy(), market=golden_batch.market)
    # The file's own text is gone from what came back: the trailing zeros
    # an exchange writes are no part of a float.
    rows = golden_rows()
    assert [written(value) for value in golden_batch.open] == [row["open"] for row in rows]
    came = [written(value) for value in back.open]
    assert came != [row["open"] for row in rows]
    assert came == [written(Decimal(row["open"]).normalize()) for row in rows]


def test_the_frozen_day_read_by_pandas_itself_builds_the_frozen_batch(golden_batch):
    # A frame made elsewhere: pandas reads the file, and its reader says
    # what each column is. The times come in whatever unit this pandas
    # reads milliseconds in, and the counts as whole numbers that cannot be
    # missing, until they are said to be counts that can.
    read = pd.read_csv(GOLDEN / "candles.csv")
    assert str(read["trades"].dtype) == "int64" and str(read["close"].dtype) == "float64"
    frame = (
        read.assign(
            open_time=pd.to_datetime(read["open_time_ms"], unit="ms", utc=True),
            close_time=pd.to_datetime(read["close_time_ms"], unit="ms", utc=True),
            trades=read["trades"].astype("UInt64"),
        )
        .set_index("open_time")
        .drop(columns=["open_time_ms", "close_time_ms"])
    )
    # Nanoseconds before pandas 3.0, and milliseconds or microseconds from
    # it on: each is read exactly.
    assert frame.index.unit in ("ns", "ms", "us")
    assert frame.index.unit == frame["close_time"].dt.unit
    batch = qf.dataframe_to_candles(frame, market=golden_batch.market)
    assert batch == golden_batch
    assert batch.open_time_ms == golden_batch.open_time_ms
    # The same file through the package's own frame is the same frame, cell
    # for cell, but for the unit pandas chose for its times.
    own = qf.candles_to_dataframe(golden_batch)
    assert stamps(frame.index.as_unit("ms")) == stamps(own.index)
    assert stamps(frame["close_time"].dt.as_unit("ms")) == stamps(own["close_time"])
    for column in FLOATS:
        assert bits_of(frame[column]) == bits_of(own[column]), column
    assert frame["trades"].tolist() == own["trades"].tolist()
    assert frame["trades"].dtype == own["trades"].dtype
    # Left as pandas read them, the counts are refused: nothing is cast.
    with pytest.raises(TypeError) as caught:
        qf.dataframe_to_candles(frame.assign(trades=read["trades"].to_numpy()))
    assert str(caught.value) == "trades must be a column of UInt64 or of uint64, not of int64"


def test_a_backtest_of_the_day_that_came_back_from_a_frame_is_the_frozen_result_by_value(
    golden_batch, golden_config
):
    frame = qf.candles_to_dataframe(golden_batch)
    back = qf.dataframe_to_candles(frame, market=golden_batch.market)
    # The run is on the market the batch of the frame was told.
    result = qf.run_backtest(back, sma_cross()(**EXPECTED["strategy"]["params"]), golden_config)
    frozen = EXPECTED["result"]
    for field in ["initial_cash", "final_equity", "total_return_pct", "max_drawdown_pct"]:
        assert getattr(result, field) == Decimal(frozen[field]), field
    assert result.trade_count == frozen["trade_count"] == 15
    for trade, expected in zip(result.trades, frozen["trades"], strict=True):
        assert str(trade.symbol) == expected["symbol"]
        assert (trade.entry_time_ms, trade.exit_time_ms) == (
            expected["entry_time_ms"],
            expected["exit_time_ms"],
        )
        for field in AMOUNTS:
            assert getattr(trade, field) == Decimal(expected[field]), field
    assert len(result.equity_curve) == len(frozen["equity_curve"]) == 1440
    for point, expected in zip(result.equity_curve, frozen["equity_curve"], strict=True):
        assert point.open_time_ms == expected["open_time_ms"]
        assert point.equity == Decimal(expected["equity"])
    # By value, and not by the text: a price a trade copies from a candle
    # has lost the zeros the file wrote after it.
    assert frozen["trades"][0]["entry_price"] == "82659.21000000"
    assert str(result.trades[0].entry_price) == "82659.21"


def test_the_frames_of_the_frozen_run_hold_pythons_reading_of_the_frozen_result(
    golden_batch, golden_config, as_given
):
    # The run itself, on the day as its file writes it: the frozen result,
    # digit for digit.
    result = qf.run_backtest(
        golden_batch, sma_cross()(**EXPECTED["strategy"]["params"]), golden_config
    )
    frozen = EXPECTED["result"]
    assert as_given(result) == frozen
    held_to_the_text(result, frozen)
    trades = qf.trades_to_dataframe(result)
    assert len(trades) == 15 and set(trades["symbol"]) == {"BTCUSDT"}
    # No order of a backtest reached a venue.
    assert trades["entry_order_id"].isna().all() and trades["exit_order_id"].isna().all()
    # One row of equity for every bar of the day, under the bar's own time.
    equity = qf.equity_to_dataframe(result)
    candles = qf.candles_to_dataframe(golden_batch)
    assert len(equity) == 1440 and equity.index.equals(candles.index)
    assert candles.join(equity)["equity"].tolist() == equity["equity"].tolist()
    # Every trade was entered and left as a bar of the day opened.
    assert trades["entry_time"].isin(candles.index).all()
    assert trades["exit_time"].isin(candles.index)[:-1].all()
    # A frame is for computing with. The sum of its floats is near the
    # exact total, which the result itself gives: the goldens are held to
    # the text of their decimals, and never to a frame.
    exact = sum(trade.gross_quote_pnl for trade in result.trades)
    assert exact == sum(Decimal(trade["gross_quote_pnl"]) for trade in frozen["trades"])
    assert len(exact.as_tuple().digits) > 17
    assert abs(float(trades["gross_quote_pnl"].sum()) - float(exact)) < 1e-9
    assert written(result.final_equity) == frozen["final_equity"]
    assert bits(float(equity["equity"].iloc[-2])) == read(frozen["equity_curve"][-2]["equity"])


def test_the_frozen_result_as_its_file_writes_it_gives_the_same_frames(golden_batch, golden_config):
    result = qf.run_backtest(
        golden_batch, sma_cross()(**EXPECTED["strategy"]["params"]), golden_config
    )
    of_the_file = result_of(EXPECTED["result"])
    assert of_the_file == result
    assert qf.trades_to_dataframe(of_the_file).equals(qf.trades_to_dataframe(result))
    assert qf.equity_to_dataframe(of_the_file).equals(qf.equity_to_dataframe(result))


# --- the strategy boundary's cases ----------------------------------------------


@pytest.mark.parametrize("case", CASES, ids=lambda case: case["id"])
def test_the_candles_of_every_shared_strategy_case_come_back_from_a_frame(case):
    market = qf.MarketId(**case["market"])
    assert market.category == qf.Category.SPOT
    candles = [candle_of(row) for row in case["candles"]]
    batch = qf.CandleBatch.from_candles(candles, market=market)
    frame = qf.candles_to_dataframe(batch)
    assert stamps(frame.index) == [row["open_time_ms"] for row in case["candles"]]
    assert stamps(frame["close_time"]) == [row["close_time_ms"] for row in case["candles"]]
    for column in FLOATS:
        assert bits_of(frame[column]) == [read(row[column]) for row in case["candles"]], column
    assert frame["trades"].tolist() == [
        pd.NA if row["trades"] is None else row["trades"] for row in case["candles"]
    ]
    back = qf.dataframe_to_candles(frame, market=market)
    assert back == batch
    assert back.to_candles() == candles


@pytest.mark.parametrize(
    "case", [case for case in CASES if case["result"]], ids=lambda case: case["id"]
)
def test_the_result_of_every_shared_strategy_case_gives_frames_of_its_text(case):
    given = case["result"]
    held_to_the_text(result_of(given), given)
    # One row of equity for every bar of the case, under the bar's time.
    equity = qf.equity_to_dataframe(result_of(given))
    assert stamps(equity.index) == [row["open_time_ms"] for row in case["candles"]]


def test_the_shared_cases_show_times_before_1970_and_amounts_a_float_does_not_hold():
    by_id = {case["id"]: case for case in CASES}
    assert len(by_id) == len(CASES) > 15
    # A bar that opened before the epoch has a time below zero, and its
    # frame a date before 1970.
    early = by_id["bars_before_1970"]
    assert early["candles"][0]["open_time_ms"] < 0
    frame = qf.candles_to_dataframe(
        qf.CandleBatch.from_candles([candle_of(row) for row in early["candles"]])
    )
    assert frame.index[0].year == 1969 and frame.index[0].tzinfo is datetime.timezone.utc
    # And some amount of some result has more digits than a float holds:
    # its frame has the float nearest to it, and the result the amount.
    longest = max(
        (
            trade[field]
            for case in CASES
            if case["result"]
            for trade in case["result"]["trades"]
            for field in AMOUNTS
        ),
        key=lambda text: len(Decimal(text).as_tuple().digits),
    )
    assert len(Decimal(longest).as_tuple().digits) > 17
    assert Decimal(repr(float(longest))) != Decimal(longest)


# --- a market with its product line ---------------------------------------------


def test_the_same_candles_of_every_stored_product_line_are_one_frame_and_six_batches():
    assert IDENTITY["format"] == 1
    markets = [qf.MarketId(**market["storage"]) for market in IDENTITY["markets"]]
    assert sorted(str(market.exchange) for market in markets) == sorted(
        str(exchange) for exchange in qf.ExchangeId.ALL
    )
    candles = [candle_of(row) for row in golden_rows()[:5]]
    batches = [qf.CandleBatch.from_candles(candles, market=market) for market in markets]
    frames = [qf.candles_to_dataframe(batch) for batch in batches]
    # A frame holds values, and nothing of what they are of.
    for frame in frames[1:]:
        assert frame.equals(frames[0])
    assert all(frame.attrs == {} for frame in frames)
    # So a batch of a frame is of the market it is told, and of none where
    # it is told none: it is never taken for a line's batch by its values.
    unnamed = qf.dataframe_to_candles(frames[0])
    assert unnamed.market is None and all(unnamed != batch for batch in batches)
    for market in markets:
        named = qf.dataframe_to_candles(frames[0], market=market)
        assert named.market == market
        assert named.market.category == market.category
        assert [named == batch for batch in batches] == [market == other for other in markets]


def test_a_frame_of_a_futures_series_is_never_run_as_its_spot_namesake(refused):
    candles = [candle_of(row) for row in golden_rows()[:5]]
    frame = qf.candles_to_dataframe(qf.CandleBatch.from_candles(candles))
    config = qf.BacktestConfig(initial_cash=1000, fee_bps=0, close_out_at_end=True)

    class Holds(qf.Strategy):
        calls = 0

        def on_bar(self, ctx, bar):
            type(self).calls += 1
            return qf.TargetPosition.LONG_ALL_IN

    spot = qf.MarketId("binance_spot", "BTCUSDT", "1m")
    for family in ["linear", "inverse"]:
        futures = qf.MarketId(f"binance_{family}", "BTCUSDT", "1m")
        of_futures = qf.dataframe_to_candles(frame, market=futures)
        refused(
            ValueError,
            f"the candles are of binance_{family} BTCUSDT 1m, and market names binance_spot BTCUSDT 1m",
            qf.run_backtest,
            of_futures,
            Holds(),
            config,
            market=spot,
        )
        refused(
            ValueError,
            f"candle strategies run on spot markets only, and binance_{family} trades {family} futures",
            qf.run_backtest,
            of_futures,
            Holds(),
            config,
        )
    # A batch of a frame that was told no market names none, and is not run
    # until one is named.
    refused(
        TypeError,
        "market is required: the candles name no market",
        qf.run_backtest,
        qf.dataframe_to_candles(frame),
        Holds(),
        config,
    )
    assert Holds.calls == 0
    result = qf.run_backtest(qf.dataframe_to_candles(frame, market=spot), Holds(), config)
    assert Holds.calls == 5 and qf.trades_to_dataframe(result)["symbol"].tolist() == ["BTCUSDT"]


# --- a store's decimals, and the policy's cases ---------------------------------


def test_the_decimals_a_store_keeps_cross_a_frame_as_floats_and_not_as_they_are_written():
    texts = IDENTITY["decimals"]
    assert "1.10" in texts and "1.1" in texts and "67012.34000000" in texts
    decimals = [Decimal(text) for text in texts]
    batch = qf.CandleBatch.from_columns(
        open_time_ms=range(len(texts)),
        close_time_ms=range(len(texts)),
        **dict.fromkeys(FLOATS, decimals),
        trades=[None] * len(texts),
    )
    # A batch itself keeps every text as the store wrote it.
    assert [written(value) for value in batch.close] == texts
    # The float is the one Python reads from the same text.
    frame = qf.candles_to_dataframe(batch)
    assert bits_of(frame["close"]) == [read(text) for text in texts]
    back = qf.dataframe_to_candles(frame).close
    for text, went, came in zip(texts, decimals, back, strict=True):
        if len(went.as_tuple().digits) <= 15:
            # The same number, written with the places a float's shortest
            # text has: `1.10` and `1.1` both come back `1.1`.
            assert came == went, text
            assert written(came) == written(went.normalize()), text
        else:
            # More digits than a float holds: its nearest float's own text.
            assert came != went, text
            assert came == Decimal(repr(float(text))), text
    # What a store keeps and a segment hashes is the text with its places,
    # which a float has lost.
    assert [written(came) for came in back] != texts


def test_the_float_policys_shared_cases_hold_of_a_frames_columns(repository):
    with open(FIXTURES / "decimal_float_cases.csv", encoding="utf-8", newline="") as file:
        header, *cases = list(csv.reader(file))
    assert header == ["direction", "input", "expected", "note"]
    to_f64 = [(Decimal(given), expected) for way, given, expected, _ in cases if way == "to_f64"]
    assert len(to_f64) == 17
    rows = len(to_f64)
    batch = qf.CandleBatch.from_columns(
        open_time_ms=range(rows),
        close_time_ms=range(rows),
        **dict.fromkeys(FLOATS, [given for given, _ in to_f64]),
        trades=[None] * rows,
    )
    assert bits_of(qf.candles_to_dataframe(batch)["volume"]) == [expected for _, expected in to_f64]
    # And of a result's amounts, which are made the same way.
    result = qf.BacktestResult(
        initial_cash=1,
        final_equity=1,
        total_return_pct=0,
        trade_count=rows,
        max_drawdown_pct=0,
        trades=[
            qf.ClosedTrade(
                symbol="BTCUSDT",
                entry_time_ms=0,
                exit_time_ms=1,
                entry_price=given,
                exit_price=given,
                qty=given,
                gross_quote_pnl=given,
            )
            for given, _ in to_f64
        ],
        equity_curve=[
            qf.EquityPoint(open_time_ms=row, equity=given) for row, (given, _) in enumerate(to_f64)
        ],
    )
    for column in AMOUNTS:
        assert bits_of(qf.trades_to_dataframe(result)[column]) == [bits for _, bits in to_f64], (
            column
        )
    assert bits_of(qf.equity_to_dataframe(result)["equity"]) == [bits for _, bits in to_f64]

    from_f64 = [(given, expected) for way, given, expected, _ in cases if way == "from_f64"]
    assert len(from_f64) == 28

    def frame_of(patterns: list[str]):
        floats = [struct.unpack(">d", bytes.fromhex(pattern))[0] for pattern in patterns]
        made = qf.candles_to_dataframe(
            qf.CandleBatch.from_columns(
                open_time_ms=range(len(floats)),
                close_time_ms=range(len(floats)),
                **dict.fromkeys(FLOATS, [1] * len(floats)),
                trades=[None] * len(floats),
            )
        )
        made["volume"] = floats
        assert bits_of(made["volume"]) == patterns
        return made

    converts = [(given, expected) for given, expected in from_f64 if "error:" not in expected]
    back = qf.dataframe_to_candles(frame_of([given for given, _ in converts])).volume
    for decimal, (given, expected) in zip(back, converts, strict=True):
        assert type(decimal) is Decimal
        # Digit for digit: the engine writes no exponent.
        assert written(decimal) == expected, given
    # A float the policy refuses is refused by its row, for the same reason.
    refuses = [(given, expected) for given, expected in from_f64 if "error:" in expected]
    assert len(refuses) == 8
    for given, expected in refuses:
        with pytest.raises(ValueError) as caught:
            qf.dataframe_to_candles(frame_of([bits(1.5)] * 3 + [given] + [bits(1.5)]))
        message = str(caught.value)
        assert message.startswith("volume[3]: cannot convert "), given
        assert message.endswith(" to a decimal: " + expected.removeprefix("error:")), given


# --- a server job's result files --------------------------------------------------


def test_the_frames_of_a_result_have_the_columns_of_a_jobs_result_files(page, quoted):
    # A server job gives its result as files of trades and of equity
    # (docs/result-spec.md). Their columns are the ones a frame of the same
    # result has, by name and in order, so that a job's files read into
    # frames are the frames of the local run: a time under its name without
    # the unit, and the two order IDs, which a file has no column for, last.
    spec = page("docs/result-spec.md")

    def listed(name: str) -> list[str]:
        columns = []
        for line in spec.section(name):
            column, _, kind = line.removeprefix("- ").partition(": ")
            columns.append((quoted(column), quoted(kind)))
        return columns

    result = result_of(EXPECTED["result"])
    trades = qf.trades_to_dataframe(result)
    of_the_file = listed("trades columns")
    assert [column.removesuffix("_ms") for column, _ in of_the_file] == list(trades.columns)[:7]
    assert list(trades.columns)[7:] == ["entry_order_id", "exit_order_id"]
    kinds = {"object": "Utf8", TIMES: "Int64", "float64": "Utf8"}
    for column, kind in of_the_file:
        # Text is text, a time the file's integer of milliseconds, and an
        # amount the float nearest to the file's exact text.
        assert kinds[str(trades[column.removesuffix("_ms")].dtype)] == kind, column
    equity = qf.equity_to_dataframe(result)
    assert listed("equity columns") == [("open_time_ms", "Int64"), ("equity", "Utf8")]
    assert (equity.index.name, list(equity.columns)) == ("open_time", ["equity"])
