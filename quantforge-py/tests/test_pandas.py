"""`candles_to_dataframe`, `dataframe_to_candles`, `trades_to_dataframe` and
`equity_to_dataframe`: candles and a backtest's result as pandas DataFrames,
and a frame of candles as a batch.

The contract is `docs/python-pandas.md`: a frame of candles is indexed by
the UTC time each bar opened; a time is the engine's own milliseconds,
exactly; a price, a volume and an amount are float64, by the engine's own
conversions (`docs/decimal-float-policy.md`); a trade count the exchange did
not report is missing, and never a number; and a frame holds no market.

What a value should come to is never asked of the package itself. The float
nearest to a decimal is Python's own `float(str(d))`, the decimal of a float
is `Decimal(repr(x))`, and a time is Python's own `datetime`."""

import datetime
import inspect
import random
import struct
import warnings
import zoneinfo
from decimal import Decimal

import pytest

import quantforge_py as qf
from quantforge_py import _frames

# The frames need pandas and the arrays they are made of NumPy, the
# package's two extras. Where the package is installed without them there is
# nothing of the frames to test, and the run says so: `test_pandas_optional.py`
# holds what the package says there.
np = pytest.importorskip("numpy")
pd = pytest.importorskip("pandas")

I64_MIN, I64_MAX = -(2**63), 2**63 - 1
U64_MAX = 2**64 - 1

UTC = datetime.timezone.utc
EPOCH = datetime.datetime(1970, 1, 1, tzinfo=UTC)

# A frame of candles: its index, its columns in order, and the type of each
# as pandas names it.
OPEN_TIME = "open_time"
CANDLE_COLUMNS = {
    "close_time": "datetime64[ms, UTC]",
    "open": "float64",
    "high": "float64",
    "low": "float64",
    "close": "float64",
    "volume": "float64",
    "trades": "UInt64",
}
FLOATS = ["open", "high", "low", "close", "volume"]
TIMES = "datetime64[ms, UTC]"

# A frame of a result's trades, and of its equity curve.
TRADE_COLUMNS = {
    "symbol": "object",
    "entry_time": TIMES,
    "exit_time": TIMES,
    "entry_price": "float64",
    "exit_price": "float64",
    "qty": "float64",
    "gross_quote_pnl": "float64",
    "entry_order_id": "object",
    "exit_order_id": "object",
}
EQUITY_COLUMNS = {"equity": "float64"}

MARKET = qf.MarketId("binance_spot", "BTCUSDT", "1m")

NO_ZONE = "a time without a zone is never read as an instant; name its zone with tz_localize"


def batch_of(rows: int = 3, market=None, **columns) -> qf.CandleBatch:
    """A batch of `rows` one-minute candles, flat at one, with the columns
    named put in place of its own."""
    made = dict(
        open_time_ms=[row * 60_000 for row in range(rows)],
        close_time_ms=[row * 60_000 + 59_999 for row in range(rows)],
        **dict.fromkeys(FLOATS, [Decimal(1)] * rows),
        trades=[None] * rows,
    )
    made.update(columns)
    return qf.CandleBatch.from_columns(**made, market=market)


def frame_of(rows: int = 3, **columns):
    """The frame of `batch_of`."""
    return qf.candles_to_dataframe(batch_of(rows, **columns))


def bits(value: float) -> str:
    """A float as the sixteen hex digits of its IEEE 754 bit pattern."""
    return struct.pack(">d", value).hex()


def bits_of(column) -> list[str]:
    """A float64 column's cells, each as its bit pattern."""
    array = np.ascontiguousarray(column.to_numpy())
    assert array.dtype == np.float64
    return [format(pattern, "016x") for pattern in array.view(np.uint64).tolist()]


def nearest(value: Decimal) -> float:
    """The float nearest to a decimal, as the policy states it without the
    engine: Python's `float(str(d))`, and `+0.0` for a zero of either sign."""
    return float(str(value)) + 0.0


def shortest(value: float) -> Decimal:
    """The decimal of a float's shortest text, as the policy states it
    without the engine: Python's `Decimal(repr(x))`."""
    return Decimal(repr(value))


def milliseconds(times) -> list[int]:
    """The times of a pandas index or column as whole milliseconds since the
    Unix epoch, worked out from each time's own fields as Python has them:
    nothing of the package, and no integer pandas keeps."""
    counted = []
    for time in times:
        assert type(time) is pd.Timestamp and time.tzinfo is not None
        days = time.toordinal() - EPOCH.toordinal()
        seconds = (days * 24 + time.hour) * 3600 + time.minute * 60 + time.second
        seconds -= int(time.utcoffset().total_seconds())
        assert time.nanosecond == 0 and time.microsecond % 1000 == 0
        counted.append(seconds * 1000 + time.microsecond // 1000)
    return counted


def some_decimals(count: int, seed: int) -> list[Decimal]:
    """Decimals the engine holds, of every size and every number of places:
    the same on every run. Each is built from its digits, and not worked
    out: arithmetic on a decimal rounds it to the 28 digits of Python's
    default context, and the engine's has 29."""
    drawn = random.Random(seed)
    made = []
    for _ in range(count):
        whole = drawn.getrandbits(drawn.randint(1, 96))
        places = drawn.randint(0, 28)
        digits = tuple(int(digit) for digit in str(whole))
        made.append(Decimal((drawn.randint(0, 1), digits, -places)))
    return made


def dtypes_of(frame) -> dict[str, str]:
    return {name: str(dtype) for name, dtype in frame.dtypes.items()}


# Making a frame and reading one draw no warning, from pandas or from NumPy:
# a warning is how either says that what was asked of it will change. Every
# call of the four functions below is held to that.
pytestmark = pytest.mark.usefixtures("no_warning_drawn_by_the_package")


# --- a frame of candles ---------------------------------------------------


def test_a_frame_of_candles_has_the_index_and_the_columns_the_contract_states():
    batch = batch_of(trades=[5, None, 0], market=MARKET)
    frame = qf.candles_to_dataframe(batch)
    assert type(frame) is pd.DataFrame
    assert frame.shape == (3, 7)
    # The index is the time each bar opened, in UTC.
    assert type(frame.index) is pd.DatetimeIndex
    assert frame.index.name == OPEN_TIME
    assert str(frame.index.dtype) == TIMES
    assert frame.index.tz is UTC
    # The columns, in their order, each of its type and with no name of
    # their own.
    assert dtypes_of(frame) == CANDLE_COLUMNS
    assert list(frame.columns) == list(CANDLE_COLUMNS)
    assert frame.columns.name is None
    assert frame["close_time"].dt.tz is UTC
    # One row for each candle, in the batch's order.
    assert len(frame) == len(batch) == 3
    assert milliseconds(frame.index) == [0, 60_000, 120_000]


def frames_of_every_kind() -> dict:
    """A frame of candles, of trades and of equity, each with text or a
    count that is missing in it."""
    result = result_of(
        [trade(0), trade(1, entry_order_id="8389765519", exit_order_id="b1c2-d3")],
        curve_of(Decimal("1000"), Decimal("1100.50")),
    )
    return {
        "candles": qf.candles_to_dataframe(batch_of(trades=[5, None, 0])),
        "trades": qf.trades_to_dataframe(result),
        "equity": qf.equity_to_dataframe(result),
    }


def test_the_names_of_a_frames_columns_are_pythons_own_text_on_every_pandas():
    expected = {"candles": CANDLE_COLUMNS, "trades": TRADE_COLUMNS, "equity": EQUITY_COLUMNS}
    for kind, frame in frames_of_every_kind().items():
        names = frame.columns
        # An index of the type `object`, holding `str`: what pandas makes of
        # text by default is `object` before pandas 3 and `str` from it on.
        assert type(names) is pd.Index and names.dtype == np.dtype(object), kind
        assert [type(name) for name in names] == [str] * len(names), kind
        assert list(names) == list(expected[kind]) and names.name is None, kind
        assert dtypes_of(frame) == expected[kind], kind


@pytest.mark.parametrize("told", [True, False], ids=["to infer it", "not to infer it"])
def test_a_frame_is_the_same_whatever_pandas_is_told_to_make_of_text(told):
    # pandas has an option that says what it makes of text: off before
    # pandas 3 unless it is asked for, and on from it on. Each way, a frame
    # is what it is with the option as it comes.
    option = "future.infer_string"
    made = frames_of_every_kind()
    with warnings.catch_warnings():
        # The option is on its way out of pandas, which says so each time it
        # is touched. That is this test's own doing: every call of the four
        # functions below is still held to drawing no warning.
        warnings.simplefilter("ignore")
        with pd.option_context(option, told):
            assert pd.get_option(option) is told
            # pandas itself follows it, for the names of a frame's columns:
            # where it is told to infer and cannot, for want of the library
            # it infers with before pandas 3, it makes no such frame at all.
            # The refusal is pandas' own and names that library, whether
            # Python finds none or pandas cannot use the one it finds.
            try:
                left_to_pandas = pd.DataFrame({"close": [1.0]}).columns.dtype
            except ImportError as unable:
                assert told and int(pd.__version__.split(".")[0]) < 3
                assert "pyarrow" in str(unable).lower()
            else:
                assert (left_to_pandas == np.dtype(object)) == (not told)
            other = frames_of_every_kind()
            for kind, frame in other.items():
                assert frame.columns.dtype == np.dtype(object), kind
                assert [type(name) for name in frame.columns] == [str] * len(frame.columns), kind
                assert dtypes_of(frame) == dtypes_of(made[kind]), kind
                assert frame.equals(made[kind]), kind
                assert frame.index.equals(made[kind].index), kind
            # Text in a frame is Python's own either way, and a missing
            # order ID is `None`.
            assert other["trades"]["symbol"].tolist() == ["BTCUSDT", "BTCUSDT"]
            assert other["trades"]["entry_order_id"].tolist() == [None, "8389765519"]
            assert other["trades"]["entry_order_id"].iloc[0] is None
            back = qf.dataframe_to_candles(other["candles"])
    assert back == qf.dataframe_to_candles(made["candles"]) == batch_of(trades=[5, None, 0])


def test_a_frame_equals_the_one_pandas_makes_of_its_cells_but_for_the_type_of_its_names():
    for kind, ours in frames_of_every_kind().items():
        own = pd.DataFrame({name: ours[name] for name in ours.columns})
        # The same cells under the same names: pandas' own comparison of
        # two frames says so, each way.
        assert ours.equals(own) and own.equals(ours), kind
        assert ours.columns.equals(own.columns) and list(ours.columns) == list(own.columns), kind
        assert dtypes_of(own) == dtypes_of(ours), kind
        # pandas' testing helper compares the type of the names as well,
        # which in the frame pandas made is pandas' own choice: `object`
        # before pandas 3 and its own text from it on, unless it is told
        # otherwise. Told not to compare it, the helper takes the two for
        # equal on every pandas.
        pd.testing.assert_frame_equal(ours, own, check_column_type=False)
        if own.columns.dtype == np.dtype(object):
            pd.testing.assert_frame_equal(ours, own)
        else:
            with pytest.raises(AssertionError, match="DataFrame.columns are different"):
                pd.testing.assert_frame_equal(ours, own)
        # A step of pandas' that makes the names anew gives them back as
        # pandas' own.
        assert ours.rename(columns=str.upper).columns.dtype == own.columns.dtype, kind


def test_the_names_of_a_frame_are_its_own_and_naming_them_names_no_other_frames():
    for kind, first in frames_of_every_kind().items():
        # A reader may name the names of a frame's columns, in place.
        first.columns.name = "field"
        assert first.columns.name == "field", kind
        second = frames_of_every_kind()[kind]
        assert second.columns is not first.columns, kind
        assert second.columns.name is None, kind


def test_the_rows_of_a_frame_are_the_batchs_candles_in_order():
    batch = batch_of(
        open_time_ms=[120_000, 0, 60_000],
        close_time_ms=[179_999, 59_999, 119_999],
        close=[Decimal("99.5"), Decimal("100.10"), Decimal("100.00")],
        trades=[7, None, 9],
    )
    frame = qf.candles_to_dataframe(batch)
    # Nothing is sorted: a batch out of order is a frame out of order.
    assert milliseconds(frame.index) == [120_000, 0, 60_000]
    assert milliseconds(frame["close_time"]) == [179_999, 59_999, 119_999]
    assert frame["close"].tolist() == [99.5, 100.1, 100.0]
    assert frame["trades"].tolist() == [7, pd.NA, 9]


def test_a_time_is_the_engines_own_millisecond_shown_as_a_utc_time():
    times = [-86_400_001, -1, 0, 1, 999, 1_000, 1_767_268_800_000, 1_767_268_859_999, 2**53 + 1]
    frame = frame_of(len(times), open_time_ms=times, close_time_ms=list(reversed(times)))
    for column, expected in [(frame.index, times), (frame["close_time"], list(reversed(times)))]:
        # Each is Python's own datetime of that many milliseconds, in UTC,
        # for as far as Python's dates go: the last is in the year 287396.
        dated = [
            (shown, time) for shown, time in zip(column, expected, strict=True) if time < 2**53
        ]
        assert len(dated) == len(times) - 1
        for shown, time in dated:
            assert shown == EPOCH + datetime.timedelta(milliseconds=time)
            assert shown.tzinfo is UTC and shown.unit == "ms"
        assert milliseconds([shown for shown, _ in dated]) == [time for _, time in dated]
        # And the integer pandas keeps for it is the engine's own, in
        # milliseconds, whichever pandas this is.
        assert pd.DatetimeIndex(column).unit == "ms"
        assert pd.DatetimeIndex(column).asi8.tolist() == expected
    # A time is a time and no float: 2^53 + 1 milliseconds is not a number a
    # float holds.
    assert float(2**53 + 1) != 2**53 + 1
    assert frame.index.asi8[-1] == 2**53 + 1


def test_a_time_of_any_64_bits_crosses_but_the_one_pandas_reads_as_no_time(refused):
    times = [I64_MIN + 1, -(2**62), 2**62, I64_MAX - 1, I64_MAX]
    batch = batch_of(len(times), open_time_ms=times, close_time_ms=times[::-1])
    frame = qf.candles_to_dataframe(batch)
    assert frame.index.asi8.tolist() == times
    assert pd.DatetimeIndex(frame["close_time"]).asi8.tolist() == times[::-1]
    assert not frame.index.isna().any() and not frame["close_time"].isna().any()
    assert qf.dataframe_to_candles(frame) == batch
    # The lowest integer of 64 bits is how pandas keeps a time that is
    # missing. It is refused by its column and its row, and never read as
    # no time.
    assert pd.DatetimeIndex(np.array([I64_MIN]).view("datetime64[ms]")).isna().tolist() == [True]
    refused(
        ValueError,
        f"invalid open_time_ms[2] {I64_MIN}: pandas reads it as no time at all (NaT)",
        frame_of,
        open_time_ms=[0, 1, I64_MIN],
    )
    refused(
        ValueError,
        f"invalid close_time_ms[0] {I64_MIN}: pandas reads it as no time at all (NaT)",
        frame_of,
        close_time_ms=[I64_MIN, 1, I64_MIN],
    )
    # The times of the bars' opening are looked at first.
    refused(
        ValueError,
        f"invalid open_time_ms[1] {I64_MIN}: pandas reads it as no time at all (NaT)",
        frame_of,
        open_time_ms=[0, I64_MIN, 2],
        close_time_ms=[I64_MIN, 1, 2],
    )


@pytest.mark.parametrize("column", FLOATS)
def test_a_price_and_a_volume_are_the_floats_nearest_to_their_decimals(column):
    decimals = some_decimals(400, seed=473) + [
        Decimal("81940.01000000"),
        Decimal("0.17235000"),
        Decimal("0"),
        Decimal("-0.00"),
        Decimal("0.0000000000000000000000000001"),
        Decimal("66633140290.84541427"),
        Decimal(2**96 - 1),
    ]
    batch = batch_of(len(decimals), **{column: decimals})
    frame = qf.candles_to_dataframe(batch)
    assert frame[column].dtype == np.float64
    # Python's own reading of each decimal's text, bit for bit.
    assert bits_of(frame[column]) == [bits(nearest(value)) for value in decimals]
    # Which is what the batch's own array holds, and what the conversion of
    # one value gives.
    assert bits_of(frame[column]) == [bits(value) for value in batch.to_numpy()[column].tolist()]
    assert frame[column].tolist()[:50] == [qf.decimal_to_f64(value) for value in decimals[:50]]
    assert not frame[column].isna().any()
    # The other columns are untouched by it.
    for other in set(FLOATS) - {column}:
        assert frame[other].tolist() == [1.0] * len(decimals)


def test_a_trade_count_is_exact_and_missing_where_the_exchange_reported_none():
    counts = [412, None, 0, U64_MAX, 2**53 + 1, None]
    frame = frame_of(len(counts), trades=counts)
    trades = frame["trades"]
    assert trades.dtype == pd.UInt64Dtype()
    assert trades.isna().tolist() == [count is None for count in counts]
    assert trades.tolist() == [pd.NA if count is None else count for count in counts]
    # Each is Python's own int, and never a float on the way: 2^64 - 1 and
    # 2^53 + 1 are not numbers a float holds.
    assert all(type(count) is int for count in trades.dropna().tolist())
    # No number stands for a count that is missing: it is left out of a sum,
    # and a zero is a count.
    assert int(trades.iloc[[0, 2]].sum()) == 412
    assert int(trades.count()) == 4
    assert trades.iloc[2] == 0 and trades.iloc[1] is pd.NA
    # Where every count is known the column is of the same type, with
    # nothing missing; and so it is where none is.
    assert frame_of(trades=[1, 2, 3])["trades"].dtype == pd.UInt64Dtype()
    assert not frame_of(trades=[1, 2, 3])["trades"].isna().any()
    assert frame_of()["trades"].dtype == pd.UInt64Dtype()
    assert frame_of()["trades"].isna().all()


def test_a_frame_is_new_each_time_and_changing_it_changes_nothing_in_the_batch():
    batch = batch_of(trades=[1, None, 3], close=[Decimal("1.5"), Decimal("2.5"), Decimal("3.5")])
    first, second = qf.candles_to_dataframe(batch), qf.candles_to_dataframe(batch)
    assert first is not second
    assert first.equals(second)
    for column in FLOATS:
        assert not np.shares_memory(first[column].to_numpy(), second[column].to_numpy())
    first.loc[first.index[0], "close"] = 99.0
    first.loc[first.index[1], "trades"] = 7
    first.index = first.index + pd.Timedelta(1, "ms")
    assert not first.equals(second)
    assert qf.candles_to_dataframe(batch).equals(second)
    assert batch.close == (Decimal("1.5"), Decimal("2.5"), Decimal("3.5"))
    assert batch.trades == (1, None, 3) and batch.open_time_ms == (0, 60_000, 120_000)


def test_an_empty_batch_gives_a_frame_of_no_rows_and_the_same_types():
    empty = qf.CandleBatch.from_candles([], market=MARKET)
    frame = qf.candles_to_dataframe(empty)
    assert frame.shape == (0, 7)
    assert dtypes_of(frame) == CANDLE_COLUMNS
    assert type(frame.index) is pd.DatetimeIndex
    assert (frame.index.name, str(frame.index.dtype)) == (OPEN_TIME, TIMES)
    assert qf.dataframe_to_candles(frame, market=MARKET) == empty


def test_a_frame_holds_no_market_and_the_same_candles_of_two_markets_are_one_frame():
    candles = batch_of(4).to_candles()
    frames = []
    for exchange in qf.ExchangeId.ALL:
        batch = qf.CandleBatch.from_candles(candles, market=qf.MarketId(exchange, "BTCUSDT", "1m"))
        frames.append(qf.candles_to_dataframe(batch))
    unnamed = qf.candles_to_dataframe(qf.CandleBatch.from_candles(candles))
    for frame in frames:
        # Nothing of a market is in it: not in what pandas keeps about a
        # frame, and not as a column.
        assert frame.attrs == {}
        assert list(frame.columns) == list(CANDLE_COLUMNS)
        assert frame.equals(unnamed)
    # So a batch of a frame is of the market it is told, and of none where
    # it is told none.
    assert qf.dataframe_to_candles(frames[0]).market is None
    linear = qf.MarketId("binance_linear", "BTCUSDT", "1m")
    assert qf.dataframe_to_candles(frames[0], market=linear).market == linear


@pytest.mark.parametrize(
    "candles, named",
    [
        ([], "list"),
        ((), "tuple"),
        (None, "NoneType"),
        (5, "int"),
        ("BTCUSDT", "str"),
        (qf.CandleBatch, "type"),
    ],
)
def test_the_candles_of_a_frame_are_a_batch_and_nothing_else(candles, named, refused):
    refused(
        TypeError, f"candles must be a CandleBatch, not {named}", qf.candles_to_dataframe, candles
    )


def test_a_list_of_candles_is_refused_and_a_batch_of_them_is_taken(refused):
    candles = batch_of().to_candles()
    refused(TypeError, "candles must be a CandleBatch, not list", qf.candles_to_dataframe, candles)
    assert qf.candles_to_dataframe(qf.CandleBatch.from_candles(candles)).equals(frame_of())
    # And the arrays of a batch are no batch either.
    refused(
        TypeError,
        "candles must be a CandleBatch, not dict",
        qf.candles_to_dataframe,
        batch_of().to_numpy(),
    )


# --- a batch of a frame ----------------------------------------------------


def test_a_batch_comes_back_from_its_frame_with_the_market_it_is_told():
    batch = batch_of(
        5,
        market=MARKET,
        open=[Decimal("81940.01"), Decimal("81955.25"), 81900, Decimal("0.1"), Decimal("1E-28")],
        trades=[412, None, 0, U64_MAX, None],
    )
    frame = qf.candles_to_dataframe(batch)
    back = qf.dataframe_to_candles(frame, market=MARKET)
    assert type(back) is qf.CandleBatch
    assert back == batch
    assert back.market == MARKET and back.market is not MARKET
    assert back.to_candles() == batch.to_candles()
    # Without the market it is the same candles, and another batch.
    unnamed = qf.dataframe_to_candles(frame)
    assert unnamed.market is None
    assert unnamed != batch
    assert unnamed.to_candles() == batch.to_candles()
    assert qf.dataframe_to_candles(frame, market=None) == unnamed
    # And it is the batch the frame's own arrays build.
    assert back == qf.CandleBatch.from_numpy(**batch.to_numpy(), market=MARKET)


def test_columns_are_found_by_name_in_any_order_and_any_other_is_left_unread():
    batch = batch_of(trades=[1, None, 3], close=[Decimal("1.5"), Decimal("2.5"), Decimal("3.5")])
    frame = qf.candles_to_dataframe(batch)
    backwards = frame[list(reversed(frame.columns))]
    assert list(backwards.columns) != list(frame.columns)
    assert qf.dataframe_to_candles(backwards) == batch
    # A frame that gained columns of its reader's own still gives its
    # candles back, whatever those columns hold.
    grown = frame.assign(
        sma=frame["close"].rolling(2).mean(),
        note=["a", None, "c"],
        when=frame.index,
        Close=[9.0, 9.0, 9.0],
    )
    grown.insert(0, "first", [object(), object(), object()])
    grown[5] = [float("nan")] * 3
    assert len(grown.columns) == 13
    assert qf.dataframe_to_candles(grown) == batch


@pytest.mark.parametrize(
    "zone",
    [
        "Europe/Berlin",
        "America/New_York",
        "Asia/Kolkata",
        zoneinfo.ZoneInfo("Australia/Lord_Howe"),
        datetime.timezone(datetime.timedelta(hours=-9, minutes=-30)),
        "UTC",
        zoneinfo.ZoneInfo("Etc/UTC"),
    ],
    ids=str,
)
def test_a_time_in_another_zone_is_the_same_instant_and_is_read_as_it(zone):
    # One bar an hour over a day and a half around a change of clocks: the
    # last Sunday of March 2026.
    opened = [1_774_648_800_000 + hour * 3_600_000 for hour in range(36)]
    batch = batch_of(
        len(opened), open_time_ms=opened, close_time_ms=[time + 3_599_999 for time in opened]
    )
    frame = qf.candles_to_dataframe(batch)
    elsewhere = frame.tz_convert(zone)
    elsewhere["close_time"] = elsewhere["close_time"].dt.tz_convert(zone)
    assert str(elsewhere.index.dtype) != TIMES or str(zone) == "UTC"
    assert qf.dataframe_to_candles(elsewhere) == batch
    # Each on its own, too: the index in one zone and the column in another.
    half = frame.tz_convert(zone)
    assert qf.dataframe_to_candles(half) == batch


@pytest.mark.parametrize("unit", ["s", "ms", "us", "ns"])
def test_a_time_in_any_of_pandas_units_is_read_exactly(unit):
    # Whole seconds, so that every unit holds them.
    opened = [-86_400_000, -1_000, 0, 1_000, 1_767_268_800_000, 4_102_444_800_000]
    closed = [time + 59_000 for time in opened]
    batch = batch_of(len(opened), open_time_ms=opened, close_time_ms=closed)
    frame = qf.candles_to_dataframe(batch)
    frame.index = frame.index.as_unit(unit)
    frame["close_time"] = frame["close_time"].dt.as_unit(unit)
    assert str(frame.index.dtype) == f"datetime64[{unit}, UTC]"
    assert str(frame["close_time"].dtype) == f"datetime64[{unit}, UTC]"
    assert qf.dataframe_to_candles(frame) == batch
    # The integers pandas keeps are in that unit, and the batch's in
    # milliseconds.
    per = {"s": 1_000, "ms": 1, "us": 1_000, "ns": 1_000_000}[unit]
    kept = frame.index.asi8.tolist()
    assert kept == [time // per if unit == "s" else time * per for time in opened]


@pytest.mark.parametrize("unit", ["us", "ns"])
def test_whole_milliseconds_in_a_finer_unit_are_read_as_integers_and_never_through_a_float(unit):
    per = {"us": 1_000, "ns": 1_000_000}[unit]
    most = I64_MAX // per
    # Times a float does not hold once they are written in the finer unit:
    # milliseconds one after another in 2026 and as long before 1970, and
    # the last the unit holds at each end, with those next to them.
    run = [1_767_268_800_000 + step for step in range(16)]
    opened = run + [-time for time in run] + [most, most - 1, most - 2, -most, 1 - most, 2 - most]
    closed = opened[::-1]

    def held(times):
        integers = [time * per for time in times]
        assert all(I64_MIN < integer <= I64_MAX for integer in integers)
        # None of them is a float's to hold: each is beyond 2**53, or the
        # ends of the unit are.
        assert max(integers) > 2**53 and float(max(integers)) != max(integers)
        array = np.array(integers, dtype=np.int64)
        return pd.DatetimeIndex(array.view(f"datetime64[{unit}]")).tz_localize("UTC")

    frame = frame_of(len(opened))
    frame.index = held(opened).rename(OPEN_TIME)
    frame["close_time"] = held(closed)
    assert str(frame.index.dtype) == str(frame["close_time"].dtype) == f"datetime64[{unit}, UTC]"
    batch = qf.dataframe_to_candles(frame)
    assert batch.open_time_ms == tuple(opened)
    assert batch.close_time_ms == tuple(closed)


@pytest.mark.parametrize(
    "unit, part, written",
    [
        ("us", 1, "1970-01-01T00:02:00.000001"),
        ("us", 999, "1970-01-01T00:02:00.000999"),
        ("us", -1, "1970-01-01T00:01:59.999999"),
        ("ns", 1, "1970-01-01T00:02:00.000000001"),
        ("ns", 999_999, "1970-01-01T00:02:00.000999999"),
        ("ns", -1, "1970-01-01T00:01:59.999999999"),
    ],
)
def test_a_time_that_is_no_whole_millisecond_is_refused_and_never_rounded(
    unit, part, written, refused
):
    frame = frame_of(4)
    step = pd.Timedelta(part, unit)
    # The third row of the index, and then the third and the fourth of the
    # column: the first that is no whole millisecond is the one named.
    opened = frame.copy()
    opened.index = opened.index.as_unit(unit).where(np.arange(4) != 2, opened.index[2] + step)
    refused(
        ValueError,
        f"invalid open_time[2] {written} UTC: must be a whole number of milliseconds",
        qf.dataframe_to_candles,
        opened,
    )
    closed = frame.copy()
    closed["close_time"] = frame.index.as_unit(unit).where(np.arange(4) < 2, frame.index[2] + step)
    refused(
        ValueError,
        f"invalid close_time[2] {written} UTC: must be a whole number of milliseconds",
        qf.dataframe_to_candles,
        closed,
    )
    # The same times on a whole millisecond are read.
    whole = frame.copy()
    whole.index = whole.index.as_unit(unit)
    assert qf.dataframe_to_candles(whole) == batch_of(4)


@pytest.mark.parametrize(
    "unit, kept, written",
    [
        ("us", -1, "1969-12-31T23:59:59.999999"),
        ("us", -999, "1969-12-31T23:59:59.999001"),
        ("us", -1_001, "1969-12-31T23:59:59.998999"),
        ("ns", -1, "1969-12-31T23:59:59.999999999"),
        ("ns", -999_999, "1969-12-31T23:59:59.999000001"),
        ("ns", -1_000_001, "1969-12-31T23:59:59.998999999"),
    ],
)
def test_a_time_before_1970_that_is_no_whole_millisecond_is_refused_too(
    unit, kept, written, refused
):
    # Below zero a whole number is cut the other way: such a time is neither
    # the millisecond before it nor the one after, and is refused as any
    # other is.
    per = {"us": 1_000, "ns": 1_000_000}[unit]
    times = np.array([-2_000 * per, kept, 0], dtype=np.int64)
    index = pd.DatetimeIndex(times.view(f"datetime64[{unit}]"), name=OPEN_TIME).tz_localize("UTC")
    frame = frame_of().set_axis(index, axis=0)
    refused(
        ValueError,
        f"invalid open_time[1] {written} UTC: must be a whole number of milliseconds",
        qf.dataframe_to_candles,
        frame,
    )
    refused(
        ValueError,
        f"invalid close_time[1] {written} UTC: must be a whole number of milliseconds",
        qf.dataframe_to_candles,
        frame_of().assign(close_time=index),
    )
    # The whole milliseconds about it are read, each as itself.
    for whole in (kept // per, kept // per + 1):
        times[1] = whole * per
        index = pd.DatetimeIndex(times.view(f"datetime64[{unit}]"), name=OPEN_TIME)
        frame = frame_of().set_axis(index.tz_localize("UTC"), axis=0)
        assert qf.dataframe_to_candles(frame).open_time_ms == (-2_000, whole, 0)


def test_seconds_whose_milliseconds_do_not_fit_64_bits_are_refused(refused):
    most = I64_MAX // 1000
    seconds = np.array([0, most, -most, most + 1, -most - 1], dtype=np.int64)
    times = pd.DatetimeIndex(seconds.view("datetime64[s]")).tz_localize("UTC")
    frame = frame_of(5)
    fits = frame.iloc[:3].copy()
    fits.index = times[:3].rename(OPEN_TIME)
    assert qf.dataframe_to_candles(fits).open_time_ms == (0, most * 1000, -most * 1000)
    for row, written in [(3, "292278994-08-17T07:12:56"), (4, "-292275055-05-16T16:47:04")]:
        beyond = frame.iloc[[0, row]].copy()
        beyond.index = times[[0, row]].rename(OPEN_TIME)
        refused(
            ValueError,
            f"invalid open_time[1] {written} UTC: its milliseconds must fit 64 bits",
            qf.dataframe_to_candles,
            beyond,
        )
        beyond = frame.iloc[[0, 1]].copy()
        beyond["close_time"] = times[[row, 0]]
        refused(
            ValueError,
            f"invalid close_time[0] {written} UTC: its milliseconds must fit 64 bits",
            qf.dataframe_to_candles,
            beyond,
        )


@pytest.mark.parametrize("unit", ["s", "ms", "us", "ns"])
def test_a_time_that_is_missing_is_refused_by_its_row(unit, refused):
    frame = frame_of(4)
    frame.index = frame.index.as_unit(unit)
    frame["close_time"] = frame["close_time"].dt.as_unit(unit)
    opened = frame.copy()
    opened.index = opened.index.where(np.arange(4) != 3)
    assert opened.index.isna().tolist() == [False, False, False, True]
    refused(
        ValueError,
        "invalid open_time[3] NaT: a candle's time is never missing",
        qf.dataframe_to_candles,
        opened,
    )
    closed = frame.copy()
    closed["close_time"] = closed["close_time"].where(np.arange(4) > 1)
    refused(
        ValueError,
        "invalid close_time[0] NaT: a candle's time is never missing",
        qf.dataframe_to_candles,
        closed,
    )
    # The index is read before the column.
    both = closed.copy()
    both.index = opened.index
    refused(
        ValueError,
        "invalid open_time[3] NaT: a candle's time is never missing",
        qf.dataframe_to_candles,
        both,
    )


WHOLE = "must be a whole number of milliseconds"
FITS = "its milliseconds must fit 64 bits"


@pytest.mark.parametrize(
    "unit, times, row, said",
    [
        # A time off the millisecond before a missing one, and after one:
        # whichever stands first is the one named.
        ("us", [1, I64_MIN, 0, 0], 0, f"1970-01-01T00:00:00.000001 UTC: {WHOLE}"),
        ("us", [I64_MIN, 1, 0, 0], 0, "NaT: a candle's time is never missing"),
        ("us", [0, 1_000, 2_001, I64_MIN], 2, f"1970-01-01T00:00:00.002001 UTC: {WHOLE}"),
        ("us", [0, I64_MIN, 2_001, I64_MIN], 1, "NaT: a candle's time is never missing"),
        ("ns", [0, 1, I64_MIN, 0], 1, f"1970-01-01T00:00:00.000000001 UTC: {WHOLE}"),
        ("ns", [0, I64_MIN, 1, 0], 1, "NaT: a candle's time is never missing"),
        # Seconds beyond 64 bits of milliseconds before a missing time, and
        # after one.
        ("s", [0, I64_MAX // 1000 + 1, I64_MIN], 1, f"292278994-08-17T07:12:56 UTC: {FITS}"),
        ("s", [0, I64_MIN, I64_MAX // 1000 + 1], 1, "NaT: a candle's time is never missing"),
        ("s", [I64_MAX, I64_MIN, 0], 0, f"292277026596-12-04T15:30:07 UTC: {FITS}"),
        ("ms", [0, 1, I64_MIN, I64_MIN], 2, "NaT: a candle's time is never missing"),
    ],
)
def test_the_first_time_that_is_refused_is_the_one_named_whatever_it_is_refused_for(
    unit, times, row, said, refused
):
    kept = np.array(times, dtype=np.int64).view(f"datetime64[{unit}]")
    index = pd.DatetimeIndex(kept, name=OPEN_TIME).tz_localize("UTC")
    frame = frame_of(len(times))
    refused(
        ValueError,
        f"invalid open_time[{row}] {said}",
        qf.dataframe_to_candles,
        frame.set_axis(index, axis=0),
    )
    refused(
        ValueError,
        f"invalid close_time[{row}] {said}",
        qf.dataframe_to_candles,
        frame.assign(close_time=index),
    )
    # In another zone the same times are refused the same way, and written
    # in UTC.
    refused(
        ValueError,
        f"invalid open_time[{row}] {said}",
        qf.dataframe_to_candles,
        frame.set_axis(index.tz_convert("Asia/Kolkata"), axis=0),
    )


@pytest.mark.parametrize("unit", ["s", "ms", "us", "ns"])
def test_a_time_without_a_zone_is_refused_and_no_zone_is_assumed(unit, refused):
    frame = frame_of()
    frame.index = frame.index.as_unit(unit)
    frame["close_time"] = frame["close_time"].dt.as_unit(unit)
    naive = frame.tz_localize(None)
    assert naive.index.tz is None
    refused(
        TypeError,
        f"open_time must be an index of datetime64 with a time zone, not of datetime64[{unit}]: "
        f"{NO_ZONE}",
        qf.dataframe_to_candles,
        naive,
    )
    naive = frame.assign(close_time=frame["close_time"].dt.tz_localize(None))
    refused(
        TypeError,
        f"close_time must be a column of datetime64 with a time zone, "
        f"not of datetime64[{unit}]: {NO_ZONE}",
        qf.dataframe_to_candles,
        naive,
    )
    # Named, they are read: the one call the refusal names.
    named = frame.tz_localize(None).tz_localize("UTC")
    assert qf.dataframe_to_candles(named) == qf.dataframe_to_candles(frame)


def test_the_index_is_the_time_each_bar_opened_by_its_type_and_by_its_name(refused):
    frame = frame_of()
    for index, named in [
        (pd.RangeIndex(3), "RangeIndex"),
        (pd.Index([0, 60_000, 120_000], dtype="int64"), "Index"),
        (pd.Index(["a", "b", "c"]), "Index"),
        (frame.index.tz_localize(None).to_period("min"), "PeriodIndex"),
        (frame.index - frame.index[0], "TimedeltaIndex"),
        (pd.MultiIndex.from_arrays([frame.index, [1, 2, 3]], names=[OPEN_TIME, "n"]), "MultiIndex"),
    ]:
        refused(
            TypeError,
            f"the index must be a pandas.DatetimeIndex named open_time, not {named}",
            qf.dataframe_to_candles,
            frame.set_axis(index, axis=0),
        )
    # Its times as a column beside another index are not the index.
    refused(
        TypeError,
        "the index must be a pandas.DatetimeIndex named open_time, not RangeIndex",
        qf.dataframe_to_candles,
        frame.reset_index(),
    )
    for name, shown in [
        (None, "None"),
        ("close_time", "'close_time'"),
        ("open_time_ms", "'open_time_ms'"),
        ("Open_Time", "'Open_Time'"),
        (0, "0"),
        (b"open_time", "b'open_time'"),
        # Names that answer no comparison, or answer every one alike.
        (pd.NA, "<NA>"),
        (float("nan"), "nan"),
        (pd.NaT, "NaT"),
    ]:
        refused(
            ValueError,
            f"the index must be named open_time, not {shown}",
            qf.dataframe_to_candles,
            frame.rename_axis(name),
        )
    assert qf.dataframe_to_candles(frame.rename_axis(None).rename_axis(OPEN_TIME)) == batch_of()
    # NumPy's own text is text, in the name of the index as in a column's.
    named = frame.rename_axis(np.str_(OPEN_TIME))
    assert type(named.index.name) is np.str_
    assert qf.dataframe_to_candles(named) == batch_of()


def test_an_index_of_a_type_derived_from_pandas_own_is_refused(refused):
    class Shifted(pd.DatetimeIndex):
        """An index whose times mean an hour later than its integers say: a
        type derived from pandas' own may mean something its cells do not."""

        def tz_convert(self, tz):
            return pd.DatetimeIndex(self).tz_convert(tz) + pd.Timedelta(1, "h")

    frame = frame_of()
    shifted = frame.set_axis(Shifted(frame.index), axis=0)
    assert type(shifted.index) is Shifted and isinstance(shifted.index, pd.DatetimeIndex)
    assert shifted.index.name == OPEN_TIME and shifted.index.tz is UTC
    # It is what the package would read the times through, and is no hour
    # the batch has.
    assert shifted.index.tz_convert(None)[0] == frame.index.tz_convert(None)[0] + pd.Timedelta(
        1, "h"
    )
    refused(
        TypeError,
        "the index must be a pandas.DatetimeIndex named open_time, not Shifted",
        qf.dataframe_to_candles,
        shifted,
    )
    # The plain index of one is read, as the integers it holds.
    plain = shifted.set_axis(pd.DatetimeIndex(shifted.index), axis=0)
    assert type(plain.index) is pd.DatetimeIndex
    assert qf.dataframe_to_candles(plain) == batch_of()


@pytest.mark.parametrize("column", list(CANDLE_COLUMNS))
def test_a_column_that_is_missing_or_there_twice_is_refused(column, refused):
    frame = frame_of()
    refused(
        ValueError,
        f"frame has no column named {column}",
        qf.dataframe_to_candles,
        frame.drop(columns=column),
    )
    # A name that only looks like it is another column.
    refused(
        ValueError,
        f"frame has no column named {column}",
        qf.dataframe_to_candles,
        frame.rename(columns={column: column.capitalize()}),
    )
    twice = pd.concat([frame, frame[[column]]], axis=1)
    assert list(twice.columns).count(column) == 2
    refused(ValueError, f"frame has 2 columns named {column}", qf.dataframe_to_candles, twice)
    thrice = pd.concat([twice, frame[[column]]], axis=1)
    refused(ValueError, f"frame has 3 columns named {column}", qf.dataframe_to_candles, thrice)


def test_a_column_is_found_under_any_text_that_is_its_name():
    # NumPy's own text is text: a frame whose names were made by NumPy is
    # read as any other.
    frame = frame_of(trades=[1, None, 3])
    # Kept as the objects they are: handed a list of them, pandas makes
    # them text of its own where it infers text.
    names = pd.Index([np.str_(name) for name in frame.columns], dtype=object)
    named = frame.set_axis(names, axis=1)
    assert {type(name) for name in named.columns} == {np.str_}
    assert qf.dataframe_to_candles(named) == batch_of(trades=[1, None, 3])
    # Names that are no text name no column of a candle, whatever they hold.
    beside = frame.copy()
    beside[("close",)] = 9.0
    beside[0] = 9.0
    assert qf.dataframe_to_candles(beside) == batch_of(trades=[1, None, 3])


def test_a_column_is_taken_by_its_place_and_not_looked_up_by_its_name():
    class Label:
        """A name that is no text, and says it equals the text `close`."""

        def __eq__(self, other):
            return other == "close"

        def __hash__(self):
            return hash("close")

    frame = frame_of(trades=[1, None, 3], close=[Decimal("1.5"), Decimal("2.5"), Decimal("3.5")])
    beside = frame.assign(other=9.0)
    beside.columns = [*frame.columns, Label()]
    assert len(beside.columns) == 8 and type(beside.columns[-1]) is Label
    # Asked for the column by its name, pandas gives both: the name that is
    # no text answers to it as well.
    assert beside["close"].shape == (3, 2)
    # The one column whose name is that text is read, where it stands,
    # wherever that is.
    assert qf.dataframe_to_candles(beside) == qf.dataframe_to_candles(frame)
    assert qf.dataframe_to_candles(beside).close == (
        Decimal("1.5"),
        Decimal("2.5"),
        Decimal("3.5"),
    )
    assert qf.dataframe_to_candles(beside.iloc[:, ::-1]) == qf.dataframe_to_candles(frame)


def test_columns_under_names_that_are_no_text_are_no_columns_of_candles(refused):
    frame = frame_of()
    # Columns with two levels of names: none of them is named `close_time`.
    levels = frame.set_axis(pd.MultiIndex.from_product([list(frame.columns), ["a"]]), axis=1)
    refused(ValueError, "frame has no column named close_time", qf.dataframe_to_candles, levels)
    numbered = frame.set_axis(range(7), axis=1)
    refused(ValueError, "frame has no column named close_time", qf.dataframe_to_candles, numbered)
    refused(
        ValueError,
        "frame has no column named close_time",
        qf.dataframe_to_candles,
        pd.DataFrame(index=frame.index),
    )


def refusals_of_a_type():
    """A column of a type it does not take, and how the refusal names it."""
    floats = [
        ("int64", "int64"),
        ("float32", "float32"),
        ("Float64", "Float64"),
        ("object", "object"),
        ("bool", "bool"),
        ("uint64", "uint64"),
        ("complex128", "complex128"),
    ]
    for column in FLOATS:
        for dtype, named in floats:
            yield (
                column,
                lambda frame, c=column, d=dtype: frame.astype({c: d}),
                (f"{column} must be a column of float64, not of {named}"),
            )
        yield (
            column,
            lambda frame, c=column: frame.assign(**{c: frame.index}),
            (f"{column} must be a column of float64, not of datetime64[ms, UTC]"),
        )
        yield (
            column,
            lambda frame, c=column: frame.assign(**{c: ["1.5", "2.5", "3.5"]}),
            (f"{column} must be a column of float64, not of {pd.Series(['1.5']).dtype}"),
        )
    # Floats in the other byte order, floats kept apart from their zeros,
    # floats as categories, and exact decimals as Python's objects.
    yield (
        "close",
        lambda frame: frame.assign(close=np.array([1.0, 2.0, 3.0], dtype=">f8")),
        "close must be a column of float64, not of >f8",
    )
    yield (
        "close",
        lambda frame: frame.astype({"close": pd.SparseDtype("float64")}),
        "close must be a column of float64, not of Sparse[float64, nan]",
    )
    yield (
        "close",
        lambda frame: frame.astype({"close": "category"}),
        "close must be a column of float64, not of category",
    )
    yield (
        "close",
        lambda frame: frame.assign(close=[Decimal("1.5")] * 3),
        "close must be a column of float64, not of object",
    )
    for dtype, named in [
        ("int64", "int64"),
        ("Int64", "Int64"),
        ("UInt32", "UInt32"),
        ("uint32", "uint32"),
        ("float64", "float64"),
        ("Float64", "Float64"),
        ("object", "object"),
    ]:
        yield (
            "trades",
            lambda frame, d=dtype: frame.astype({"trades": d}),
            (f"trades must be a column of UInt64 or of uint64, not of {named}"),
        )
    for dtype, named in [("int64", "int64"), ("object", "object"), ("float64", "float64")]:
        yield (
            "close_time",
            lambda frame, d=dtype: frame.assign(
                close_time=pd.Series([59_999, 119_999, 179_999], index=frame.index, dtype=d)
            ),
            f"close_time must be a column of datetime64 with a time zone, not of {named}",
        )
    yield (
        "close_time",
        lambda frame: frame.assign(close_time=frame["close_time"] - frame["close_time"].iloc[0]),
        "close_time must be a column of datetime64 with a time zone, not of timedelta64[ms]",
    )


@pytest.mark.parametrize(
    "column, changed, message",
    list(refusals_of_a_type()),
    ids=[
        f"{column}: {message.rsplit(' of ', 1)[1]}" for column, _, message in refusals_of_a_type()
    ],
)
def test_nothing_is_cast_and_a_column_of_another_type_is_refused(column, changed, message, refused):
    frame = frame_of(trades=[1, 2, 3])
    refused(TypeError, message, qf.dataframe_to_candles, changed(frame))


def test_a_count_kept_as_a_float_with_nan_for_the_unknown_is_no_count(refused):
    frame = frame_of(trades=[1, None, 3])
    # What pandas makes of a missing count where it is asked for floats.
    floats = frame["trades"].astype("float64")
    assert floats.isna().tolist() == [False, True, False]
    refused(
        TypeError,
        "trades must be a column of UInt64 or of uint64, not of float64",
        qf.dataframe_to_candles,
        frame.assign(trades=floats),
    )
    refused(
        TypeError,
        "trades must be a column of UInt64 or of uint64, not of int64",
        qf.dataframe_to_candles,
        frame.assign(trades=frame["trades"].astype("Int64").fillna(-1).astype("int64")),
    )


def test_trade_counts_are_read_from_a_nullable_column_or_a_plain_one():
    counts = [412, None, 0, U64_MAX, 2**53 + 1]
    batch = batch_of(len(counts), trades=counts)
    frame = qf.candles_to_dataframe(batch)
    assert qf.dataframe_to_candles(frame).trades == tuple(counts)
    # Whatever lies under a count that is missing is never read as one.
    assert frame["trades"].array._data.tolist()[1] == 0
    hidden = frame.copy()
    hidden["trades"] = pd.arrays.IntegerArray(
        np.array([412, 99, 0, U64_MAX, 2**53 + 1], dtype=np.uint64),
        np.array([False, True, False, False, False]),
    )
    assert qf.dataframe_to_candles(hidden).trades == tuple(counts)
    # A plain column of uint64 says that every count is known.
    plain = frame.assign(trades=np.array([1, 0, U64_MAX, 2**53 + 1, 5], dtype=np.uint64))
    assert str(plain["trades"].dtype) == "uint64"
    assert qf.dataframe_to_candles(plain).trades == (1, 0, U64_MAX, 2**53 + 1, 5)
    # And a nullable one with nothing missing says the same.
    known = frame.assign(trades=pd.array([1, 0, U64_MAX, 2**53 + 1, 5], dtype="UInt64"))
    assert qf.dataframe_to_candles(known) == qf.dataframe_to_candles(plain)
    # All of them missing is a batch with no count at all.
    none = frame.assign(trades=pd.array([None] * 5, dtype="UInt64"))
    assert qf.dataframe_to_candles(none).trades == (None,) * 5


@pytest.mark.parametrize(
    "value, shown, reason",
    [
        (float("nan"), "NaN", "not finite"),
        (float("inf"), "inf", "not finite"),
        (float("-inf"), "-inf", "not finite"),
        (1e29, "1e29", "beyond the decimal range"),
        (-7.922816251426434e28, "-7.922816251426434e28", "beyond the decimal range"),
        (1e-29, "1e-29", "more than 28 decimal places"),
        (5e-324, "5e-324", "more than 28 decimal places"),
    ],
)
@pytest.mark.parametrize("column", FLOATS)
def test_a_float_that_has_no_decimal_is_refused_by_its_row_in_the_engines_words(
    column, value, shown, reason, refused
):
    frame = frame_of(4)
    frame.loc[frame.index[2], column] = value
    refused(
        ValueError,
        f"{column}[2]: cannot convert {shown} to a decimal: {reason}",
        qf.dataframe_to_candles,
        frame,
    )
    # The words are those of the conversion of one value.
    with pytest.raises(ValueError) as one:
        qf.decimal_from_f64(value)
    assert str(one.value) == f"cannot convert {shown} to a decimal: {reason}"


def test_a_float_is_read_as_the_decimal_of_its_shortest_text():
    drawn = random.Random(473)
    floats = [0.1, 100.0, 81940.01, -0.0, 5e-28, 1e28, 2.0**53, 790101343014070.25]
    while len(floats) < 400:
        value = struct.unpack(">d", struct.pack(">Q", drawn.getrandbits(64)))[0]
        if abs(value) < 7e28 and (value == 0 or abs(value) > 1e-12):
            floats.append(value)
    frame = frame_of(len(floats))
    frame["close"] = floats
    back = qf.dataframe_to_candles(frame).close
    for value, came in zip(floats, back, strict=True):
        # Python's own `Decimal(repr(x))`, by value and place for place but
        # for a whole float, which has no decimal place, and a zero.
        assert came == shortest(value), value
        assert came.as_tuple() == qf.decimal_from_f64(value).as_tuple(), value
    assert back[1] == Decimal("100") and str(back[1]) == "100"
    assert str(back[3]) == "0"
    # Halfway between two shortest texts, the one whose last digit is even.
    assert back[7] == Decimal("790101343014070.2")
    # And a float comes back the same float, bit for bit.
    again = qf.candles_to_dataframe(qf.dataframe_to_candles(frame))
    assert bits_of(again["close"]) == [bits(value + 0.0) for value in floats]


def test_a_batch_comes_back_the_same_by_value_where_floats_hold_its_decimals():
    # At most fifteen significant digits: every one comes back the same
    # value, written with the places its float's shortest text has.
    held = [
        Decimal("81940.01000000"),
        Decimal("0.17235000"),
        Decimal("0.00000000"),
        Decimal("100"),
        Decimal("999999999999999"),
        Decimal("0.000000000000001"),
        Decimal("1234567.12345678"),
    ]
    batch = batch_of(len(held), close=held)
    back = qf.dataframe_to_candles(qf.candles_to_dataframe(batch))
    assert back == batch
    assert [format(value, "f") for value in back.close] == [
        "81940.01",
        "0.17235",
        "0",
        "100",
        "999999999999999",
        "0.000000000000001",
        "1234567.12345678",
    ]
    # More digits than a float holds come back as the nearest float's own
    # text, which is another decimal.
    longer = [Decimal("66633140290.84541427"), Decimal("9007199254740993"), Decimal("0.1") + 0]
    batch = batch_of(len(longer), close=longer)
    back = qf.dataframe_to_candles(qf.candles_to_dataframe(batch))
    assert back != batch
    assert back.close == (
        Decimal("66633140290.84541"),
        Decimal("9007199254740992"),
        Decimal("0.1"),
    )
    assert back.close == tuple(shortest(nearest(value)) for value in longer)


def test_a_frame_of_a_type_derived_from_pandas_own_is_refused(refused):
    class Tagged(pd.DataFrame):
        @property
        def _constructor(self):
            return Tagged

    frame = frame_of()
    tagged = Tagged(frame)
    assert isinstance(tagged, pd.DataFrame) and tagged.equals(frame)
    refused(
        TypeError, "frame must be a pandas.DataFrame, not Tagged", qf.dataframe_to_candles, tagged
    )
    # The plain frame of one is read.
    assert qf.dataframe_to_candles(pd.DataFrame(tagged)) == batch_of()


@pytest.mark.parametrize(
    "frame, named",
    [
        (None, "NoneType"),
        ({"close": [1.0]}, "dict"),
        ([[1.0]], "list"),
        ("frame.csv", "str"),
        (5, "int"),
    ],
)
def test_a_frame_is_a_pandas_dataframe_and_nothing_that_could_be_made_one(frame, named, refused):
    refused(
        TypeError, f"frame must be a pandas.DataFrame, not {named}", qf.dataframe_to_candles, frame
    )


def test_what_is_no_frame_of_pandas_is_refused_whatever_it_holds(refused):
    frame = frame_of()
    for value, named in [
        (frame["close"], "Series"),
        (frame.index, "DatetimeIndex"),
        (frame.to_numpy(), "ndarray"),
        (batch_of(), "CandleBatch"),
        (batch_of().to_numpy(), "dict"),
        (pd.DataFrame, "type"),
    ]:
        refused(
            TypeError,
            f"frame must be a pandas.DataFrame, not {named}",
            qf.dataframe_to_candles,
            value,
        )


@pytest.mark.parametrize(
    "market, named",
    [("binance_spot", "str"), (("binance_spot", "BTCUSDT", "1m"), "tuple"), (0, "int")],
)
def test_the_market_of_a_frame_is_a_market_or_none(market, named, refused):
    message = f"market must be a MarketId or None, not {named}"
    refused(TypeError, message, qf.dataframe_to_candles, frame_of(), market=market)
    # In the words a batch of arrays refuses it with.
    refused(TypeError, message, qf.CandleBatch.from_numpy, **batch_of().to_numpy(), market=market)


def test_the_market_is_given_by_name_alone(refused):
    with pytest.raises(TypeError, match="takes 1 positional argument but 2 were given"):
        qf.dataframe_to_candles(frame_of(), MARKET)
    with pytest.raises(TypeError, match="positional-only"):
        qf.dataframe_to_candles(frame=frame_of())
    with pytest.raises(TypeError, match="positional-only"):
        qf.candles_to_dataframe(candles=batch_of())


def test_every_type_is_held_in_the_columns_order_before_any_value_is_read(refused):
    frame = frame_of(4)
    # A frame with everything wrong at once: each refusal gives way to the
    # next as it is mended, in this order.
    broken = frame.reset_index(drop=True).drop(columns="close_time")
    broken["open"] = [1, 2, 3, 4]
    broken["volume"] = np.float32(1.0)
    broken["trades"] = [1.0, float("nan"), 3.0, 4.0]
    broken["close"] = [1.0, float("nan"), float("inf"), 1.0]
    mended = [
        (
            TypeError,
            "the index must be a pandas.DatetimeIndex named open_time, not RangeIndex",
            lambda at: at.set_axis(frame.index.tz_localize(None).rename("when"), axis=0),
        ),
        (
            ValueError,
            "the index must be named open_time, not 'when'",
            lambda at: at.rename_axis(OPEN_TIME),
        ),
        (
            TypeError,
            f"open_time must be an index of datetime64 with a time zone, "
            f"not of datetime64[ms]: {NO_ZONE}",
            lambda at: at.tz_localize("UTC").set_axis(
                frame.index.as_unit("ns") + pd.Timedelta(1, "ns"), axis=0
            ),
        ),
        (
            ValueError,
            "frame has no column named close_time",
            lambda at: at.assign(close_time=frame["close_time"].dt.tz_localize(None).to_numpy()),
        ),
        (
            TypeError,
            f"close_time must be a column of datetime64 with a time zone, "
            f"not of datetime64[ms]: {NO_ZONE}",
            lambda at: at.assign(
                close_time=at["close_time"].dt.tz_localize("UTC").where(np.arange(4) != 1)
            ),
        ),
        (
            TypeError,
            "open must be a column of float64, not of int64",
            lambda at: at.astype({"open": "float64"}),
        ),
        (
            TypeError,
            "volume must be a column of float64, not of float32",
            lambda at: at.astype({"volume": "float64"}),
        ),
        (
            TypeError,
            "trades must be a column of UInt64 or of uint64, not of float64",
            lambda at: at.assign(trades=pd.array([1, None, 3, 4], dtype="UInt64")),
        ),
        # Every type is held now. The market is held to its own before a
        # value is read, and then the values are, the times first.
        (
            ValueError,
            "invalid open_time[0] 1970-01-01T00:00:00.000000001 UTC: "
            "must be a whole number of milliseconds",
            lambda at: at.set_axis(frame.index, axis=0),
        ),
        (
            ValueError,
            "invalid close_time[1] NaT: a candle's time is never missing",
            lambda at: at.assign(close_time=frame["close_time"]),
        ),
        (
            ValueError,
            "close[1]: cannot convert NaN to a decimal: not finite",
            lambda at: at.assign(close=[1.0, 1.0, float("inf"), 1.0]),
        ),
        (
            ValueError,
            "close[2]: cannot convert inf to a decimal: not finite",
            lambda at: at.assign(close=1.0),
        ),
    ]
    at = broken
    for kind, message, mend in mended:
        refused(kind, message, qf.dataframe_to_candles, at)
        if kind is ValueError and message.startswith(("invalid ", "close[")):
            # A market of the wrong type is refused before any value.
            refused(
                TypeError,
                "market must be a MarketId or None, not str",
                qf.dataframe_to_candles,
                at,
                market="binance_spot",
            )
        elif "the index" in message or "open_time must be" in message:
            # And after everything of the frame's own that is of a wrong type.
            refused(kind, message, qf.dataframe_to_candles, at, market="binance_spot")
        at = mend(at)
    assert qf.dataframe_to_candles(at).trades == (1, None, 3, 4)


def test_a_frame_that_is_no_frame_is_refused_before_its_market_is_looked_at(refused):
    refused(
        TypeError,
        "frame must be a pandas.DataFrame, not list",
        qf.dataframe_to_candles,
        [],
        market="binance_spot",
    )


def test_the_frame_is_read_and_not_kept():
    frame = frame_of(trades=[1, None, 3])
    before = frame.copy(deep=True)
    batch = qf.dataframe_to_candles(frame)
    # Reading it changed nothing in it.
    assert frame.equals(before)
    assert dtypes_of(frame) == CANDLE_COLUMNS
    frame.loc[frame.index[0], "close"] = 99.0
    frame.loc[frame.index[0], "trades"] = 7
    frame.index = frame.index + pd.Timedelta(1, "ms")
    assert batch == qf.dataframe_to_candles(before)
    assert batch.close == (Decimal("1"),) * 3 and batch.trades == (1, None, 3)


def test_a_frame_is_read_wherever_its_rows_lie():
    batch = batch_of(
        6,
        close=[Decimal(f"{row}.5") for row in range(6)],
        trades=[row if row % 2 else None for row in range(6)],
    )
    frame = qf.candles_to_dataframe(batch)
    candles = batch.to_candles()
    # Every other row, backwards, a slice, rows picked by a flag, and rows
    # put in another order.
    for picked, rows in [
        (frame.iloc[::2], candles[::2]),
        (frame.iloc[::-1], candles[::-1]),
        (frame.iloc[1:4], candles[1:4]),
        (frame[frame["close"] > 2], candles[2:]),
        (frame.sort_values("close", ascending=False), candles[::-1]),
        (frame.iloc[[5, 0, 3]], [candles[5], candles[0], candles[3]]),
        (frame.iloc[0:0], []),
    ]:
        assert qf.dataframe_to_candles(picked).to_candles() == rows
    # And a frame put together from frames.
    joined = pd.concat([frame.iloc[3:], frame.iloc[:3]])
    assert qf.dataframe_to_candles(joined).to_candles() == candles[3:] + candles[:3]


def test_nothing_checks_a_frames_candles_as_nothing_checks_a_batchs():
    frame = frame_of(3)
    # Twice the same time, out of order, and a low above the high: a batch
    # holds what it is given, and so does one built of a frame.
    odd = frame.set_axis(frame.index[[2, 2, 0]].rename(OPEN_TIME), axis=0)
    odd["low"] = 5.0
    back = qf.dataframe_to_candles(odd)
    assert back.open_time_ms == (120_000, 120_000, 0)
    assert back.low == (Decimal("5"),) * 3 and back.high == (Decimal("1"),) * 3


# --- a result's trades -------------------------------------------------------


def trade(index: int = 0, **fields) -> qf.ClosedTrade:
    made = dict(
        symbol="BTCUSDT",
        entry_time_ms=60_000 + index * 600_000,
        exit_time_ms=180_000 + index * 600_000,
        entry_price=Decimal("100.10"),
        exit_price=Decimal("125.00000000"),
        qty=Decimal("9.99000999"),
        gross_quote_pnl=Decimal("248.5512475125"),
    )
    made.update(fields)
    return qf.ClosedTrade(**made)


def result_of(trades=(), curve=()) -> qf.BacktestResult:
    return qf.BacktestResult(
        initial_cash=1000,
        final_equity=1000,
        total_return_pct=0,
        trade_count=len(trades),
        max_drawdown_pct=0,
        trades=trades,
        equity_curve=curve,
    )


def test_a_frame_of_trades_has_every_field_of_a_trade_as_a_column():
    result = result_of([trade(0), trade(1), trade(2)])
    frame = qf.trades_to_dataframe(result)
    assert type(frame) is pd.DataFrame
    assert dtypes_of(frame) == TRADE_COLUMNS
    assert list(frame.columns) == list(TRADE_COLUMNS)
    # One row for each trade, numbered from zero in the result's order.
    assert type(frame.index) is pd.RangeIndex
    assert (frame.index.start, frame.index.stop, frame.index.step) == (0, 3, 1)
    assert frame.index.name is None and frame.columns.name is None
    assert frame.attrs == {}
    # The columns are a trade's fields, by their names and in their order,
    # but for the two times, which are times and no integers.
    fields = list(inspect.signature(qf.ClosedTrade).parameters)
    assert list(frame.columns) == [field.removesuffix("_ms") for field in fields]
    assert [field for field in fields if field.endswith("_ms")] == ["entry_time_ms", "exit_time_ms"]


def test_each_row_of_a_frame_of_trades_is_the_trade_of_that_number():
    trades = [
        trade(0),
        trade(
            1,
            symbol="ethusdt",
            entry_price=Decimal("66633140290.84541427"),
            qty=Decimal("0.0000000000000000000000000001"),
            gross_quote_pnl=Decimal("-12.50"),
            entry_order_id="8389765519",
            exit_order_id="b1c2-d3",
        ),
        trade(2, gross_quote_pnl=Decimal("0"), exit_order_id="77"),
    ]
    frame = qf.trades_to_dataframe(result_of(trades))
    for row, made in enumerate(trades):
        shown = frame.loc[row]
        assert shown["symbol"] == str(made.symbol)
        assert milliseconds([shown["entry_time"], shown["exit_time"]]) == [
            made.entry_time_ms,
            made.exit_time_ms,
        ]
        for field in ["entry_price", "exit_price", "qty", "gross_quote_pnl"]:
            # Python's own reading of the decimal's text, bit for bit.
            assert bits(shown[field]) == bits(nearest(getattr(made, field))), (row, field)
        assert shown["entry_order_id"] == made.entry_order_id
        assert shown["exit_order_id"] == made.exit_order_id
    assert frame["symbol"].tolist() == ["BTCUSDT", "ETHUSDT", "BTCUSDT"]
    assert frame["gross_quote_pnl"].tolist() == [248.5512475125, -12.5, 0.0]
    assert bits(frame["gross_quote_pnl"].iloc[2]) == bits(0.0)


def test_text_in_a_frame_of_trades_is_pythons_own_on_every_pandas():
    frame = qf.trades_to_dataframe(
        result_of([trade(0), trade(1, entry_order_id="8389765519", exit_order_id="b1c2-d3")])
    )
    # A symbol is a str, and an order ID a str or None: no order of a
    # backtest reached a venue. Neither is NaN, nor pandas' own missing value.
    assert [type(symbol) for symbol in frame["symbol"]] == [str, str]
    assert frame["entry_order_id"].tolist() == [None, "8389765519"]
    assert frame["exit_order_id"].tolist() == [None, "b1c2-d3"]
    assert frame["entry_order_id"].iloc[0] is None
    assert frame["entry_order_id"].isna().tolist() == [True, False]
    for column in ["symbol", "entry_order_id", "exit_order_id"]:
        assert frame[column].dtype == np.dtype(object), column


def test_a_result_of_no_trades_gives_a_frame_of_no_rows_and_the_same_types():
    frame = qf.trades_to_dataframe(result_of())
    assert frame.shape == (0, 9)
    assert dtypes_of(frame) == TRADE_COLUMNS
    assert type(frame.index) is pd.RangeIndex and len(frame.index) == 0


def test_a_frame_of_trades_is_new_each_time_and_changes_nothing_in_the_result():
    result = result_of([trade(0), trade(1)])
    first, second = qf.trades_to_dataframe(result), qf.trades_to_dataframe(result)
    assert first is not second and first.equals(second)
    first.loc[0, "gross_quote_pnl"] = -1.0
    first.loc[1, "symbol"] = "ETHUSDT"
    assert not first.equals(second)
    assert qf.trades_to_dataframe(result).equals(second)
    assert result.trades[0].gross_quote_pnl == Decimal("248.5512475125")
    assert str(result.trades[1].symbol) == "BTCUSDT"


def test_a_trade_time_pandas_reads_as_no_time_is_refused(refused):
    frame = qf.trades_to_dataframe(
        result_of([trade(0, entry_time_ms=I64_MIN + 1, exit_time_ms=I64_MAX)])
    )
    assert pd.DatetimeIndex(frame["entry_time"]).asi8.tolist() == [I64_MIN + 1]
    assert pd.DatetimeIndex(frame["exit_time"]).asi8.tolist() == [I64_MAX]
    refused(
        ValueError,
        f"invalid entry_time_ms[1] {I64_MIN}: pandas reads it as no time at all (NaT)",
        qf.trades_to_dataframe,
        result_of([trade(0), trade(1, entry_time_ms=I64_MIN)]),
    )
    refused(
        ValueError,
        f"invalid exit_time_ms[0] {I64_MIN}: pandas reads it as no time at all (NaT)",
        qf.trades_to_dataframe,
        result_of([trade(0, exit_time_ms=I64_MIN)]),
    )


# --- a result's equity curve -------------------------------------------------


def curve_of(*equities, step=60_000) -> list[qf.EquityPoint]:
    return [
        qf.EquityPoint(open_time_ms=index * step, equity=equity)
        for index, equity in enumerate(equities)
    ]


def test_a_frame_of_equity_is_indexed_as_a_frame_of_candles_is():
    curve = curve_of(Decimal("1000"), Decimal("1100.50"), Decimal("1200.123456789012345678"))
    frame = qf.equity_to_dataframe(result_of(curve=curve))
    assert type(frame) is pd.DataFrame
    assert dtypes_of(frame) == EQUITY_COLUMNS
    assert type(frame.index) is pd.DatetimeIndex
    assert (frame.index.name, str(frame.index.dtype)) == (OPEN_TIME, TIMES)
    assert frame.index.tz is UTC
    assert frame.attrs == {} and frame.columns.name is None
    assert milliseconds(frame.index) == [0, 60_000, 120_000]
    assert bits_of(frame["equity"]) == [bits(nearest(point.equity)) for point in curve]
    # The bars of the run it is of open at these times: the two frames have
    # one index.
    candles = frame_of(3)
    assert frame.index.equals(candles.index)
    assert frame.index.dtype == candles.index.dtype and frame.index.name == candles.index.name
    assert list(candles.join(frame).columns) == [*CANDLE_COLUMNS, "equity"]


def test_a_result_of_no_curve_gives_a_frame_of_no_rows_and_the_same_types():
    frame = qf.equity_to_dataframe(result_of())
    assert frame.shape == (0, 1)
    assert dtypes_of(frame) == EQUITY_COLUMNS
    assert (frame.index.name, str(frame.index.dtype)) == (OPEN_TIME, TIMES)


def test_an_equity_time_pandas_reads_as_no_time_is_refused(refused):
    points = [
        qf.EquityPoint(open_time_ms=time, equity=1) for time in (I64_MIN + 1, 0, I64_MAX, I64_MIN)
    ]
    frame = qf.equity_to_dataframe(result_of(curve=points[:3]))
    assert frame.index.asi8.tolist() == [I64_MIN + 1, 0, I64_MAX]
    refused(
        ValueError,
        f"invalid open_time_ms[3] {I64_MIN}: pandas reads it as no time at all (NaT)",
        qf.equity_to_dataframe,
        result_of(curve=points),
    )


@pytest.mark.parametrize("view", ["trades_to_dataframe", "equity_to_dataframe"])
@pytest.mark.parametrize(
    "result, named",
    [
        (None, "NoneType"),
        ([], "list"),
        ((), "tuple"),
        ({}, "dict"),
        ("result", "str"),
        (qf.BacktestResult, "type"),
    ],
)
def test_the_result_of_a_frame_is_a_backtest_result_and_nothing_else(view, result, named, refused):
    # Each function is looked up by its name as the test runs, as everywhere
    # here: what the package publishes then is what is held to no warning.
    view = getattr(qf, view)
    refused(TypeError, f"result must be a BacktestResult, not {named}", view, result)


@pytest.mark.parametrize("view", ["trades_to_dataframe", "equity_to_dataframe"])
def test_what_a_result_holds_is_no_result(view, refused):
    view = getattr(qf, view)
    result = result_of([trade(0)], curve_of(1000))
    for value, named in [
        (result.trades, "tuple"),
        (result.trades[0], "ClosedTrade"),
        (result.equity_curve[0], "EquityPoint"),
        (batch_of(), "CandleBatch"),
    ]:
        refused(TypeError, f"result must be a BacktestResult, not {named}", view, value)
    with pytest.raises(TypeError, match="positional-only"):
        view(result=result)


# --- a run, end to end ---------------------------------------------------------


class EnterThenExit(qf.Strategy):
    """Goes long on the first bar and flat on the third."""

    def __init__(self):
        self.bars = 0

    def on_bar(self, ctx, bar):
        self.bars += 1
        if self.bars == 1:
            return qf.TargetPosition.LONG_ALL_IN
        if self.bars == 3:
            return qf.TargetPosition.FLAT
        return None


def test_a_run_of_candles_that_came_from_a_frame_gives_frames_of_its_result(minutes):
    candles = minutes((100, 100), (100, 110), (110, 120), (125, 125))
    batch = qf.CandleBatch.from_candles(candles, market=MARKET)
    config = qf.BacktestConfig(initial_cash=1000, fee_bps=0, close_out_at_end=True)
    expected = qf.run_backtest(batch, EnterThenExit(), config)
    # The candles through a frame, and the run on the market the batch of
    # the frame was told.
    frame = qf.candles_to_dataframe(batch)
    back = qf.dataframe_to_candles(frame, market=MARKET)
    result = qf.run_backtest(back, EnterThenExit(), config)
    assert result == expected
    trades = qf.trades_to_dataframe(result)
    assert trades["entry_price"].tolist() == [100.0] and trades["exit_price"].tolist() == [125.0]
    assert trades["gross_quote_pnl"].tolist() == [250.0]
    assert milliseconds(trades["entry_time"]) == [60_000]
    assert milliseconds(trades["exit_time"]) == [180_000]
    equity = qf.equity_to_dataframe(result)
    assert equity["equity"].tolist() == [1000.0, 1100.0, 1200.0, 1250.0]
    # One row of equity for every bar, under the bar's own time.
    assert equity.index.equals(frame.index)
    # And a batch of a frame that names no market needs one named to run.
    with pytest.raises(TypeError, match="market is required: the candles name no market"):
        qf.run_backtest(qf.dataframe_to_candles(frame), EnterThenExit(), config)


# --- what the docstrings state --------------------------------------------------


def stated(function, name: str, dtype: str) -> bool:
    """Whether a function's docstring names a column or an index with its
    type: both written as code in the first sentence of one item, the name
    before the type."""
    doc = " ".join(inspect.getdoc(function).split())
    for item in doc.replace("Index: ", "- ").split("- ")[1:]:
        sentence = item.split(". ")[0]
        named, typed = sentence.find(f"`{name}`"), sentence.find(f"`{dtype}`")
        if 0 <= named < typed:
            return True
    return False


def test_each_docstring_states_the_index_and_every_column_of_its_frame():
    batch = batch_of(trades=[1, None, 3])
    result = result_of([trade(0)], curve_of(1000))
    for function, frame in [
        (qf.candles_to_dataframe, qf.candles_to_dataframe(batch)),
        (qf.trades_to_dataframe, qf.trades_to_dataframe(result)),
        (qf.equity_to_dataframe, qf.equity_to_dataframe(result)),
    ]:
        doc = inspect.getdoc(function)
        assert doc, function.__name__
        for column, dtype in dtypes_of(frame).items():
            assert stated(function, column, dtype), (function.__name__, column, dtype)
        # And the index: its name and its type where it is a time, and what
        # it is where it is a number.
        if type(frame.index) is pd.DatetimeIndex:
            assert stated(function, frame.index.name, str(frame.index.dtype)), function.__name__
            assert "Index: `open_time`, a `DatetimeIndex`" in " ".join(doc.split())
        else:
            assert "Index: a `RangeIndex` from 0" in " ".join(doc.split())
        # Every time is UTC, and every docstring says so.
        assert "UTC" in doc and "milliseconds since the Unix epoch" in " ".join(doc.split())
    # What it takes back is stated the same way: by name, with the types
    # that are taken.
    taken = " ".join(inspect.getdoc(qf.dataframe_to_candles).split())
    assert "Index: `open_time`, a `DatetimeIndex` with a time zone" in taken
    for column in CANDLE_COLUMNS:
        assert f"`{column}`" in taken, column
    for dtype in ["float64", "UInt64", "uint64", "datetime64"]:
        assert f"`{dtype}`" in taken, dtype
    assert "`market`" in taken and "`MarketId`" in taken


def test_every_docstring_says_what_to_install_and_none_calls_a_spot_pnl_a_futures_one():
    for name in [
        "candles_to_dataframe",
        "dataframe_to_candles",
        "trades_to_dataframe",
        "equity_to_dataframe",
    ]:
        doc = " ".join(inspect.getdoc(getattr(qf, name)).split())
        assert '`pip install "quantforge-py[pandas]"`' in doc, name
    trades = " ".join(inspect.getdoc(qf.trades_to_dataframe).split())
    assert "`gross_quote_pnl`" in trades
    assert "It is Spot's PnL field" in trades and "It is not a futures PnL." in trades


def test_the_four_functions_take_what_the_contract_states():
    assert {name: str(inspect.signature(getattr(qf, name))) for name in qf.__all__[-4:]} == {
        "candles_to_dataframe": "(candles, /)",
        "dataframe_to_candles": "(frame, /, *, market=None)",
        "trades_to_dataframe": "(result, /)",
        "equity_to_dataframe": "(result, /)",
    }


def test_a_warning_drawn_while_a_frame_is_made_fails_a_test_here(monkeypatch):
    batch, result = batch_of(), result_of([trade(0)], curve_of(1000))
    frame = qf.candles_to_dataframe(batch)

    def warns_first(function, kind):
        """`function`, with a warning of `kind` drawn before it, named for
        whichever code called it."""

        def called(*args, **kwargs):
            warnings.warn("going away", kind, stacklevel=2)
            return function(*args, **kwargs)

        return called

    # Whatever pandas or NumPy is asked on the way, by whichever code: the
    # frame itself as pandas makes it, a time as pandas reads it, and the
    # array NumPy allocates for the extension.
    for kind in (FutureWarning, DeprecationWarning, UserWarning):
        with monkeypatch.context() as patched:
            patched.setattr(pd, "DataFrame", warns_first(pd.DataFrame, kind))
            for call in [
                lambda: qf.candles_to_dataframe(batch),
                lambda: qf.trades_to_dataframe(result),
                lambda: qf.equity_to_dataframe(result),
            ]:
                with pytest.raises(kind, match="going away"):
                    call()
        with monkeypatch.context() as patched:
            patched.setattr(np, "zeros", warns_first(np.zeros, kind))
            with pytest.raises(kind, match="going away"):
                qf.candles_to_dataframe(batch)
        with monkeypatch.context() as patched:
            patched.setattr(np, "datetime_data", warns_first(np.datetime_data, kind))
            with pytest.raises(kind, match="going away"):
                qf.dataframe_to_candles(frame)
    # A warning a test's own code draws, outside the four, is no error of
    # this run.
    with pytest.warns(FutureWarning, match="going away"):
        warns_first(len, FutureWarning)([])
    # And what is held so is the package's own functions, each as it is.
    for name in ["candles_to_dataframe", "dataframe_to_candles"]:
        held = getattr(qf, name)
        assert held.__wrapped__ is getattr(_frames, name)
        assert (held.__module__, held.__name__) == ("quantforge_py._frames", name)


# --- pandas itself ----------------------------------------------------------------


def test_the_pandas_of_the_run_is_one_the_package_takes(pyproject, oldest_pandas):
    from packaging.requirements import Requirement
    from packaging.version import Version

    # The package's `pandas` extra requires pandas, and the NumPy the arrays
    # a frame is made of need: the `numpy` extra's own requirement.
    extras = pyproject["project"]["optional-dependencies"]
    numpy, required = map(Requirement, extras["pandas"])
    assert [numpy.name, required.name] == ["numpy", "pandas"]
    assert extras["pandas"][0] == extras["numpy"][0]
    assert required.specifier.contains(pd.__version__), pd.__version__
    assert numpy.specifier.contains(np.__version__), np.__version__
    # The tests run once on the oldest it takes, which a group pins: that
    # version is the extra's own lower bound. Its other bound is the first
    # pandas it does not take.
    (pinned,) = map(Requirement, pyproject["dependency-groups"]["oldest-pandas"])
    bound, following = sorted(required.specifier, key=lambda bound: Version(bound.version))
    (pin,) = pinned.specifier
    assert (pinned.name, pin.operator) == ("pandas", "==")
    assert (bound.operator, following.operator) == (">=", "<")
    assert Version(pin.version) == Version(bound.version)
    if oldest_pandas:
        assert Version(pd.__version__) == Version(pin.version)


def test_what_pandas_itself_makes_of_milliseconds_depends_on_its_version():
    # Why a frame's times are made from the engine's integers themselves:
    # asked to read them, pandas gives nanoseconds before 3.0 and another
    # unit from it on. The frame's are milliseconds on every pandas.
    made = pd.to_datetime([0, 60_000, 120_000], unit="ms", utc=True)
    major = int(pd.__version__.split(".")[0])
    assert (made.unit == "ns") == (major < 3)
    assert frame_of().index.unit == "ms"
    assert frame_of().index.equals(made.as_unit("ms"))
