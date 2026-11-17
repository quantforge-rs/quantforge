"""The repository's fixtures, taken through a batch that names its market and
through NumPy: the ones written when a market got its product line, and the
ones that were there before.

- `tests/fixtures/storage_identity_cases.json`: every product line as a
  store names it, and decimal texts a store gives back unchanged, written
  by a model that is not the engine.
- `tests/fixtures/golden/`: the frozen day of Spot candles and what the
  engine's SMA cross made of them.
- `tests/fixtures/strategy_cases.json`: the candles of the strategy
  boundary's shared cases.

A batch keeps the market it is given, whichever product line it names, and
the same candles under two lines are two batches. What crosses to NumPy and
back is the same batch by value, and a backtest of it is the frozen result
by value. Its decimals are no longer written as the files write them, which
is why nothing that is stored or hashed crosses to a float."""

import csv
import importlib.util
import itertools
import json
import pathlib
import struct
import sys
from decimal import Decimal

import pytest

import quantforge_py as qf

# The arrays need NumPy, the package's one extra. Where the package is
# installed without it there is nothing of them to test, and the run says so:
# `test_numpy_optional.py` holds what the package says there.
np = pytest.importorskip("numpy")

REPOSITORY = pathlib.Path(__file__).resolve().parents[2]
FIXTURES = REPOSITORY / "tests" / "fixtures"
GOLDEN = FIXTURES / "golden"

IDENTITY = json.loads((FIXTURES / "storage_identity_cases.json").read_text("utf-8"))
EXPECTED = json.loads((GOLDEN / "sma_cross.json").read_text("utf-8"))
CASES = json.loads((FIXTURES / "strategy_cases.json").read_text("utf-8"))["cases"]

COLUMNS = ["open_time_ms", "close_time_ms", "open", "high", "low", "close", "volume", "trades"]
DECIMALS = ["open", "high", "low", "close", "volume"]


def bits_of(array) -> list[str]:
    return [format(pattern, "016x") for pattern in array.view(np.uint64).tolist()]


def bits(value: float) -> str:
    return struct.pack(">d", value).hex()


def written(value: Decimal) -> str:
    """A decimal with every digit it has and no exponent, as a store and a
    fixture write one."""
    return format(value, "f")


def minutes(count: int) -> list[qf.Candle]:
    """A few one-minute candles, each with a price of its own."""
    return [
        qf.Candle(
            open_time_ms=index * 60_000,
            close_time_ms=index * 60_000 + 59_999,
            open=Decimal("67012.34000000") + index,
            high=Decimal("67020.5") + index,
            low=Decimal("67000") + index,
            close=Decimal("67015.25") + index,
            volume=Decimal("12.04510000"),
            trades=None if index % 2 else 40 + index,
        )
        for index in range(count)
    ]


def stored(market: dict) -> qf.MarketId:
    """A market of the identity fixture, by the names a store keeps it
    under."""
    return qf.MarketId(**market["storage"])


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
        **{field: Decimal(row[field]) for field in DECIMALS},
    )


def sma_cross() -> type:
    """The SMA cross written in Python, from its file beside the goldens:
    imported by its path, and no bytecode written beside the fixture."""
    path = GOLDEN / "sma_cross.py"
    spec = importlib.util.spec_from_file_location(path.stem, path)
    module = importlib.util.module_from_spec(spec)
    written, sys.dont_write_bytecode = sys.dont_write_bytecode, True
    try:
        spec.loader.exec_module(module)
    finally:
        sys.dont_write_bytecode = written
    return module.SmaCross


@pytest.fixture(scope="module")
def golden_market() -> qf.MarketId:
    return qf.MarketId(**EXPECTED["dataset"]["market"])


@pytest.fixture(scope="module")
def golden_batch(golden_market) -> qf.CandleBatch:
    candles = [candle_of(row) for row in golden_rows()]
    return qf.CandleBatch.from_candles(candles, market=golden_market)


# --- a market with its product line ----------------------------------------


def test_the_identity_fixture_names_every_product_line_the_package_has():
    assert IDENTITY["format"] == 1
    listed = [market["storage"]["exchange"] for market in IDENTITY["markets"]]
    assert sorted(listed) == sorted(str(exchange) for exchange in qf.ExchangeId.ALL)
    assert len(listed) == 6
    for market in IDENTITY["markets"]:
        named = stored(market)
        # A product line is a venue and a family together, and the family
        # is read from the line: nothing assumes it.
        assert str(named.exchange) == f"{market['venue']}_{market['category']}"
        assert named.category == qf.Category(market["category"])
        # The symbol as it was typed is the symbol as it is stored.
        typed = qf.MarketId(market["storage"]["exchange"], market["symbol"], market["interval"])
        assert typed == named
        assert str(named.symbol) == market["storage"]["symbol"]
        assert str(named.interval) == market["storage"]["interval"]


@pytest.mark.parametrize(
    "market", IDENTITY["markets"], ids=lambda market: market["storage"]["exchange"]
)
def test_a_batch_keeps_the_product_line_it_is_given_through_every_way_of_building_it(market):
    named = stored(market)
    candles = minutes(4)
    of_candles = qf.CandleBatch.from_candles(candles, market=named)
    of_columns = qf.CandleBatch.from_columns(
        **{column: getattr(of_candles, column) for column in COLUMNS}, market=named
    )
    of_arrays = qf.CandleBatch.from_numpy(**of_candles.to_numpy(), market=named)
    for batch in (of_candles, of_columns, of_arrays):
        assert type(batch.market) is qf.MarketId
        assert batch.market == named
        assert batch.market.category == qf.Category(market["category"])
        assert str(batch.market.exchange) == market["storage"]["exchange"]
        assert batch == of_candles
    # The market is a value of its own each time, equal to the one given.
    assert of_candles.market is not named


def test_the_same_candles_under_two_product_lines_are_two_batches():
    candles = minutes(4)
    # One symbol and one interval on all six lines: Spot, linear and inverse
    # of two venues.
    markets = [qf.MarketId(exchange, "BTCUSDT", "1m") for exchange in qf.ExchangeId.ALL]
    assert len({market.category for market in markets}) == 3
    batches = [qf.CandleBatch.from_candles(candles, market=market) for market in markets]
    for one, other in itertools.combinations(batches, 2):
        assert one != other, (one.market, other.market)
    for batch, market in zip(batches, markets, strict=True):
        assert batch == qf.CandleBatch.from_candles(candles, market=market)
    # Their arrays are the same cell for cell: an array holds values, and
    # nothing of what they are of.
    arrays = [batch.to_numpy() for batch in batches]
    for other in arrays[1:]:
        for column in COLUMNS:
            assert arrays[0][column].tolist() == other[column].tolist(), column
    # So a batch of arrays is of the market it is told, and of none where it
    # is told none: it is never taken for a line's batch by its values.
    unnamed = qf.CandleBatch.from_numpy(**arrays[0])
    assert unnamed.market is None
    assert all(unnamed != batch for batch in batches)
    for market in markets:
        named = qf.CandleBatch.from_numpy(**arrays[0], market=market)
        assert [named == batch for batch in batches] == [market == other for other in markets]


def test_a_spot_batch_is_never_run_as_its_futures_namesake_nor_the_other_way(refused):
    candles = minutes(4)
    config = qf.BacktestConfig(initial_cash=1000, fee_bps=0, close_out_at_end=True)

    class Holds(qf.Strategy):
        calls = 0

        def on_bar(self, ctx, bar):
            type(self).calls += 1
            return qf.TargetPosition.LONG_ALL_IN

    spot = qf.MarketId("binance_spot", "BTCUSDT", "1m")
    for family in ["linear", "inverse"]:
        futures = qf.MarketId(f"binance_{family}", "BTCUSDT", "1m")
        of_spot = qf.CandleBatch.from_candles(candles, market=spot)
        of_futures = qf.CandleBatch.from_candles(candles, market=futures)
        refused(
            ValueError,
            f"the candles are of binance_spot BTCUSDT 1m, and market names binance_{family} BTCUSDT 1m",
            qf.run_backtest,
            of_spot,
            Holds(),
            config,
            market=futures,
        )
        refused(
            ValueError,
            f"the candles are of binance_{family} BTCUSDT 1m, and market names binance_spot BTCUSDT 1m",
            qf.run_backtest,
            of_futures,
            Holds(),
            config,
            market=spot,
        )
        # Left to itself, the futures batch is refused by the engine, which
        # runs no candle strategy on futures.
        refused(
            ValueError,
            f"candle strategies run on spot markets only, and binance_{family} trades {family} futures",
            qf.run_backtest,
            of_futures,
            Holds(),
            config,
        )
    assert Holds.calls == 0
    # And the Spot batch runs on the market it names.
    result = qf.run_backtest(qf.CandleBatch.from_candles(candles, market=spot), Holds(), config)
    assert Holds.calls == 4 and str(result.trades[0].symbol) == "BTCUSDT"


# --- a store's decimals -------------------------------------------------------


def test_the_decimals_a_store_keeps_cross_as_floats_and_not_as_they_are_written():
    texts = IDENTITY["decimals"]
    assert "1.10" in texts and "1.1" in texts and "67012.34000000" in texts
    decimals = [Decimal(text) for text in texts]
    batch = qf.CandleBatch.from_columns(
        open_time_ms=range(len(texts)),
        close_time_ms=range(len(texts)),
        **dict.fromkeys(DECIMALS, decimals),
        trades=[None] * len(texts),
    )
    # A batch itself keeps every text as the store wrote it.
    assert [written(value) for value in batch.close] == texts
    # The float is the one Python reads from the same text.
    array = batch.to_numpy()["close"]
    assert bits_of(array) == [bits(float(text)) for text in texts]
    back = qf.CandleBatch.from_numpy(**batch.to_numpy()).close
    for text, went, came in zip(texts, decimals, back, strict=True):
        digits = len(went.as_tuple().digits)
        if digits <= 15:
            # The same number, written with the places a float's shortest
            # text has: `1.10` and `1.1` both come back `1.1`.
            assert came == went, text
            assert written(came) == written(went.normalize()), text
        else:
            # More digits than a float holds: its nearest float's own text.
            assert came != went, text
            assert came == Decimal(repr(float(text))), text
    assert [written(came) for came in back][:7] == [
        "0",
        "1.1",
        "1.1",
        "0.0000001",
        "67012.34",
        "12.0451",
        "-1.5",
    ]
    # What a store keeps and a segment hashes is the text with its places,
    # which a float has lost.
    assert [written(came) for came in back] != texts


# --- the frozen day of candles ------------------------------------------------


def test_the_frozen_day_is_a_spot_batch_of_the_market_its_file_names(golden_batch, golden_market):
    assert EXPECTED["dataset"]["market"] == dict(
        exchange="binance_spot", symbol="BTCUSDT", interval="1m"
    )
    assert golden_market.category == qf.Category.SPOT
    assert golden_batch.market == golden_market
    assert len(golden_batch) == EXPECTED["dataset"]["candles"] == 1440


def test_the_frozen_days_arrays_are_what_python_reads_from_the_file(golden_batch):
    rows = golden_rows()
    arrays = golden_batch.to_numpy()
    for column in ["open_time_ms", "close_time_ms"]:
        assert arrays[column].tolist() == [int(row[column]) for row in rows], column
    for column in DECIMALS:
        # Python's own reading of the text the file holds, with no part of
        # the package in it.
        assert bits_of(arrays[column]) == [bits(float(row[column])) for row in rows], column
    assert arrays["trades"].tolist() == [int(row["trades"]) for row in rows]
    assert not arrays["trades"].mask.any()


def test_the_frozen_day_comes_back_from_numpy_the_same_batch_by_value(golden_batch):
    back = qf.CandleBatch.from_numpy(**golden_batch.to_numpy(), market=golden_batch.market)
    # A price with eight decimal places and five digits before them has
    # thirteen digits, which a float holds.
    assert back == golden_batch
    assert back.market == golden_batch.market
    assert back.to_candles() == golden_batch.to_candles()
    # Without its market it is the same candles and another batch.
    assert qf.CandleBatch.from_numpy(**golden_batch.to_numpy()) != golden_batch
    # The file's own text is gone from what came back: the trailing zeros
    # an exchange writes are no part of a float.
    rows = golden_rows()
    assert [written(value) for value in golden_batch.open] == [row["open"] for row in rows]
    came = [written(value) for value in back.open]
    assert came != [row["open"] for row in rows]
    assert came == [written(Decimal(row["open"]).normalize()) for row in rows]


def test_a_backtest_of_the_day_that_came_back_from_numpy_is_the_frozen_result_by_value(
    golden_batch,
):
    back = qf.CandleBatch.from_numpy(**golden_batch.to_numpy(), market=golden_batch.market)
    settings = EXPECTED["config"]
    config = qf.BacktestConfig(
        initial_cash=Decimal(settings["initial_cash"]),
        fee_bps=Decimal(settings["fee_bps"]),
        close_out_at_end=settings["close_out_at_end"],
    )
    # The run is on the market the batch names: none is given beside it.
    result = qf.run_backtest(back, sma_cross()(**EXPECTED["strategy"]["params"]), config)
    frozen = EXPECTED["result"]
    for field in ["initial_cash", "final_equity", "total_return_pct", "max_drawdown_pct"]:
        assert getattr(result, field) == Decimal(frozen[field]), field
    assert result.trade_count == frozen["trade_count"] == 15
    assert len(result.trades) == len(frozen["trades"])
    for trade, expected in zip(result.trades, frozen["trades"], strict=True):
        assert str(trade.symbol) == expected["symbol"]
        assert (trade.entry_time_ms, trade.exit_time_ms) == (
            expected["entry_time_ms"],
            expected["exit_time_ms"],
        )
        for field in ["entry_price", "exit_price", "qty", "gross_quote_pnl"]:
            assert getattr(trade, field) == Decimal(expected[field]), field
    assert len(result.equity_curve) == len(frozen["equity_curve"]) == 1440
    for point, expected in zip(result.equity_curve, frozen["equity_curve"], strict=True):
        assert point.open_time_ms == expected["open_time_ms"]
        assert point.equity == Decimal(expected["equity"])
    # By value, and not by the text: a price a trade copies from a candle
    # has lost the zeros the file wrote after it.
    assert frozen["trades"][0]["entry_price"] == "82659.21000000"
    assert str(result.trades[0].entry_price) == "82659.21"


# --- the strategy boundary's candles ------------------------------------------


@pytest.mark.parametrize("case", CASES, ids=lambda case: case["id"])
def test_the_candles_of_every_shared_strategy_case_come_back_from_numpy(case):
    market = qf.MarketId(**case["market"])
    assert market.category == qf.Category.SPOT
    candles = [candle_of(row) for row in case["candles"]]
    batch = qf.CandleBatch.from_candles(candles, market=market)
    arrays = batch.to_numpy()
    for column in DECIMALS:
        expected = [bits(float(row[column])) for row in case["candles"]]
        assert bits_of(arrays[column]) == expected, column
    assert arrays["trades"].tolist() == [row["trades"] for row in case["candles"]]
    back = qf.CandleBatch.from_numpy(**arrays, market=market)
    assert back == batch
    assert back.to_candles() == candles
