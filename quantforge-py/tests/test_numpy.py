"""`CandleBatch.to_numpy` and `CandleBatch.from_numpy`: a batch's columns as
NumPy arrays, and arrays as a batch.

The contract is `docs/python-numpy.md`: each column is one array of one
dimension; a time and a trade count cross exactly, a count the exchange did
not report as a masked entry; and a price or a volume crosses as a float64,
by the engine's own two conversions (`docs/decimal-float-policy.md`).

What a value should come to is never asked of the package itself. The float
nearest to a decimal is Python's own `float(str(d))`, the decimal of a float
is `Decimal(repr(x))`, and the policy's shared cases,
`tests/fixtures/decimal_float_cases.csv`, were computed by Python too."""

import csv
import gc
import inspect
import random
import struct
import subprocess
import sys
import textwrap
import threading
import time
import tracemalloc
import warnings
import weakref
from decimal import Decimal

import pytest

import quantforge_py as qf

# The arrays need NumPy, the package's one extra. Where the package is
# installed without it there is nothing of them to test, and the run says so:
# `test_numpy_optional.py` holds what the package says there.
np = pytest.importorskip("numpy")

I64_MIN, I64_MAX = -(2**63), 2**63 - 1
U64_MAX = 2**64 - 1

# The largest decimal the engine holds: 2^96 - 1, with no decimal place.
DECIMAL_MAX = 2**96 - 1

COLUMNS = ["open_time_ms", "close_time_ms", "open", "high", "low", "close", "volume", "trades"]
TIMES = ["open_time_ms", "close_time_ms"]
DECIMALS = ["open", "high", "low", "close", "volume"]

# What each column's cells are, as NumPy names the type.
DTYPES = {
    "open_time_ms": "int64",
    "close_time_ms": "int64",
    "open": "float64",
    "high": "float64",
    "low": "float64",
    "close": "float64",
    "volume": "float64",
    "trades": "uint64",
}


def inspect_source(function) -> str:
    """A function of this module as its source, for an interpreter of its
    own to run."""
    return textwrap.dedent(inspect.getsource(function))


def batch_of(rows: int = 3, market=None, **columns) -> qf.CandleBatch:
    """A batch of `rows` one-minute candles, flat at one, with the columns
    named put in place of its own."""
    made = dict(
        open_time_ms=[row * 60_000 for row in range(rows)],
        close_time_ms=[row * 60_000 + 59_999 for row in range(rows)],
        **dict.fromkeys(DECIMALS, [Decimal(1)] * rows),
        trades=[None] * rows,
    )
    made.update(columns)
    return qf.CandleBatch.from_columns(**made, market=market)


def arrays_of(rows: int = 4, **columns) -> dict:
    """Eight arrays of `rows` one-minute candles as `from_numpy` takes them,
    with the arrays named put in place of its own."""
    opened = np.arange(rows, dtype=np.int64) * 60_000
    made = dict(
        open_time_ms=opened,
        close_time_ms=opened + 59_999,
        **{name: np.full(rows, 1.5) for name in DECIMALS},
        trades=np.arange(rows, dtype=np.uint64),
    )
    made.update(columns)
    return made


def bits(value: float) -> str:
    """A float as the sixteen hex digits of its IEEE 754 bit pattern."""
    return struct.pack(">d", value).hex()


def bits_of(array) -> list[str]:
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


def reason_refused(value: float) -> str | None:
    """Why the policy refuses a float, or `None` where it has a decimal:
    worked out from Python's own shortest text of it."""
    if value != value or value in (float("inf"), float("-inf")):
        return "not finite"
    sign, digits, exponent = shortest(value).as_tuple()
    if -exponent > 28:
        return "more than 28 decimal places"
    whole = int("".join(map(str, digits))) * 10 ** max(exponent, 0)
    return "beyond the decimal range" if whole > DECIMAL_MAX else None


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


def float_cases(repository) -> list[list[str]]:
    """The float policy's shared cases: direction, input, expected, note."""
    path = repository / "tests" / "fixtures" / "decimal_float_cases.csv"
    with open(path, encoding="utf-8", newline="") as file:
        rows = list(csv.reader(file))
    assert rows[0] == ["direction", "input", "expected", "note"]
    return rows[1:]


def float_of(pattern: str) -> float:
    return struct.unpack(">d", bytes.fromhex(pattern))[0]


# --- what a batch gives ---------------------------------------------------


def test_a_batch_gives_eight_arrays_by_column_name_in_the_columns_order():
    batch = batch_of(trades=[5, None, 0])
    arrays = batch.to_numpy()
    assert type(arrays) is dict
    assert list(arrays) == COLUMNS
    for name, array in arrays.items():
        # One array for each column, of one dimension and as long as the batch.
        assert array.dtype == np.dtype(DTYPES[name]), name
        assert array.dtype.isnative, name
        assert array.shape == (3,), name
        assert array.ndim == 1, name
        assert array.flags.c_contiguous and array.flags.aligned and array.flags.writeable, name
    # Seven are arrays and nothing more, each owning its memory.
    for name in TIMES + DECIMALS:
        assert type(arrays[name]) is np.ndarray, name
        assert arrays[name].flags.owndata and arrays[name].base is None, name
    # The trade counts are a masked array, with one flag for every row.
    trades = arrays["trades"]
    assert type(trades) is np.ma.MaskedArray
    assert type(trades.mask) is np.ndarray
    assert trades.mask.dtype == np.dtype("bool") and trades.mask.shape == (3,)


def test_the_arrays_hold_the_batchs_rows_in_order():
    batch = batch_of(
        open_time_ms=[120_000, 0, 60_000],
        close=[Decimal("99.5"), Decimal("100.10"), Decimal("100.00")],
        trades=[7, None, 9],
    )
    arrays = batch.to_numpy()
    assert arrays["open_time_ms"].tolist() == [120_000, 0, 60_000]
    assert arrays["close"].tolist() == [99.5, 100.1, 100.0]
    assert arrays["trades"].tolist() == [7, None, 9]


@pytest.mark.parametrize("column", TIMES)
def test_a_time_crosses_exactly(column):
    times = [I64_MIN, I64_MIN + 1, -1, 0, 1, 1_767_268_800_000, 2**53 + 1, I64_MAX - 1, I64_MAX]
    array = batch_of(len(times), **{column: times}).to_numpy()[column]
    assert array.dtype == np.int64
    assert array.tolist() == times
    back = qf.CandleBatch.from_numpy(**arrays_of(len(times), **{column: array}))
    assert getattr(back, column) == tuple(times)


def test_a_trade_count_crosses_exactly_and_an_unknown_one_is_masked():
    counts = [0, 1, None, 2**53 + 1, U64_MAX, None, 412]
    trades = batch_of(len(counts), trades=counts).to_numpy()["trades"]
    assert trades.dtype == np.uint64
    # A masked entry is NumPy's own way of saying that a value is missing:
    # it reads as `None`, and no number stands for it.
    assert trades.tolist() == counts
    assert trades.mask.tolist() == [count is None for count in counts]
    assert trades.count() == 5
    assert [trades[row] is np.ma.masked for row in range(len(counts))] == [
        count is None for count in counts
    ]
    # Zero is a count, and is not masked.
    assert trades[0] == 0 and not trades.mask[0]
    back = qf.CandleBatch.from_numpy(**arrays_of(len(counts), trades=trades))
    assert back.trades == tuple(counts)


def test_the_cell_under_a_mask_is_zero_and_is_no_count():
    trades = batch_of(trades=[5, None, 7]).to_numpy()["trades"]
    # A cell has to hold something. What it holds is the same on every run,
    # and the mask is what says that it is no count.
    assert np.ma.getdata(trades).tolist() == [5, 0, 7]
    assert trades.mask.tolist() == [False, True, False]
    # Read without the mask, as `data` and `numpy.asarray` read it, the zero
    # looks like a count: the mask is to be read with the cells.
    assert trades.data.tolist() == np.asarray(trades).tolist() == [5, 0, 7]
    assert type(np.asarray(trades)) is np.ndarray
    # Summed, the unknown count is left out, where a zero would be counted.
    assert trades.sum() == 12 and trades.count() == 2
    assert trades.mean() == 6


def test_the_mask_has_a_flag_for_every_row_even_where_every_count_is_known():
    known = batch_of(trades=[1, 2, 3]).to_numpy()["trades"]
    assert type(known) is np.ma.MaskedArray
    assert known.mask.tolist() == [False, False, False]
    unknown = batch_of(trades=[None] * 3).to_numpy()["trades"]
    assert unknown.mask.tolist() == [True, True, True]
    assert unknown.tolist() == [None, None, None]


@pytest.mark.parametrize("column", DECIMALS)
def test_a_decimal_becomes_the_float_nearest_to_it(column):
    decimals = some_decimals(1_500, seed=475) + [
        Decimal("81940.01000000"),
        Decimal("0.17235000"),
        Decimal("0.1"),
        Decimal("0"),
        Decimal("0.00"),
        Decimal(DECIMAL_MAX),
        Decimal(-DECIMAL_MAX),
        Decimal("0.0000000000000000000000000001"),
        # 2^53 + 1 lies halfway between two floats, and goes to the even one.
        Decimal(2**53 + 1),
        Decimal(2**53 + 3),
    ]
    # Among them are decimals of all 29 digits the engine holds.
    assert sum(len(value.as_tuple().digits) == 29 for value in decimals) > 20
    array = batch_of(len(decimals), **{column: decimals}).to_numpy()[column]
    assert bits_of(array) == [bits(nearest(value)) for value in decimals]
    # No float is NaN or infinite, and no zero is negative.
    assert np.isfinite(array).all()
    assert not np.signbit(array[array == 0]).any()
    # It is the conversion the package publishes, cell for cell.
    assert array.tolist() == [qf.decimal_to_f64(value) for value in decimals]


def test_every_shared_case_of_the_float_policy_holds_for_a_whole_column(repository):
    cases = float_cases(repository)
    to_f64 = [(Decimal(given), expected) for way, given, expected, _ in cases if way == "to_f64"]
    assert len(to_f64) == 17
    array = batch_of(len(to_f64), volume=[given for given, _ in to_f64]).to_numpy()["volume"]
    assert bits_of(array) == [expected for _, expected in to_f64]

    from_f64 = [(given, expected) for way, given, expected, _ in cases if way == "from_f64"]
    assert len(from_f64) == 28
    converts = [(given, expected) for given, expected in from_f64 if "error:" not in expected]
    floats = np.array([float_of(given) for given, _ in converts], dtype=np.float64)
    assert bits_of(floats) == [given for given, _ in converts]
    back = qf.CandleBatch.from_numpy(**arrays_of(len(converts), volume=floats)).volume
    for decimal, (given, expected) in zip(back, converts, strict=True):
        assert type(decimal) is Decimal
        assert decimal == Decimal(expected), given
        # Digit for digit as well: the engine writes no exponent.
        assert format(decimal, "f") == expected, given
    # A float the policy refuses is refused by its row, for the same reason.
    refused = [(given, expected) for given, expected in from_f64 if "error:" in expected]
    assert len(refused) == 8
    for given, expected in refused:
        floats = np.full(5, 1.5)
        floats[3] = float_of(given)
        with pytest.raises(ValueError) as caught:
            qf.CandleBatch.from_numpy(**arrays_of(5, volume=floats))
        message = str(caught.value)
        assert message.startswith("volume[3]: cannot convert "), given
        assert message.endswith(" to a decimal: " + expected.removeprefix("error:")), given


def test_an_empty_batch_gives_eight_empty_arrays_of_their_types():
    arrays = batch_of(0).to_numpy()
    assert list(arrays) == COLUMNS
    for name, array in arrays.items():
        assert array.dtype == np.dtype(DTYPES[name]), name
        assert array.shape == (0,), name
    assert type(arrays["trades"]) is np.ma.MaskedArray
    assert arrays["trades"].mask.shape == (0,)
    back = qf.CandleBatch.from_numpy(**arrays)
    assert len(back) == 0 and back == batch_of(0)


def test_the_arrays_are_new_each_time_and_changing_one_changes_nothing_else():
    batch = batch_of(close=[Decimal("1.5"), Decimal("2.5"), Decimal("3.5")], trades=[1, None, 3])
    first, second = batch.to_numpy(), batch.to_numpy()
    for name in COLUMNS:
        assert first[name] is not second[name], name
        assert not np.shares_memory(first[name], second[name]), name
    assert not np.shares_memory(first["trades"].mask, second["trades"].mask)
    # The arrays are the caller's own, to change as they like.
    first["close"][:] = -1.0
    first["open_time_ms"][0] = 99
    first["trades"][0] = np.ma.masked
    first["trades"][1] = 8
    assert second["close"].tolist() == [1.5, 2.5, 3.5]
    assert second["trades"].tolist() == [1, None, 3]
    assert batch.close == (Decimal("1.5"), Decimal("2.5"), Decimal("3.5"))
    assert batch.open_time_ms == (0, 60_000, 120_000)
    assert batch.trades == (1, None, 3)
    assert batch.to_numpy()["close"].tolist() == [1.5, 2.5, 3.5]


def test_to_numpy_takes_nothing_and_from_numpy_takes_each_column_by_name():
    batch = batch_of()
    with pytest.raises(TypeError):
        batch.to_numpy(True)
    with pytest.raises(TypeError):
        batch.to_numpy(copy=False)
    given = arrays_of()
    with pytest.raises(TypeError, match="takes 0 positional arguments"):
        qf.CandleBatch.from_numpy(*given.values())
    with pytest.raises(TypeError, match="takes 0 positional arguments"):
        qf.CandleBatch.from_numpy(given)
    for column in COLUMNS:
        without = {name: array for name, array in given.items() if name != column}
        with pytest.raises(TypeError, match=f"missing 1 required keyword argument: '{column}'"):
            qf.CandleBatch.from_numpy(**without)
    with pytest.raises(TypeError, match="unexpected keyword argument"):
        qf.CandleBatch.from_numpy(**given, quote_volume=np.full(4, 1.5))
    with pytest.raises(TypeError, match="unexpected keyword argument"):
        qf.CandleBatch.from_numpy(**given, trades_known=np.ones(4, dtype=bool))


# --- what a batch is built from ---------------------------------------------


@pytest.mark.parametrize("column", DECIMALS)
def test_a_float_becomes_the_decimal_of_its_shortest_text(column):
    drawn = random.Random(475)
    floats = [0.1, 0.5, 81940.01, 0.17235, 1e-7, 1.5e-12, 123456789.125, 1e21, 2.0**53, -2.5]
    floats += [round(drawn.uniform(-1e6, 1e6), drawn.randint(0, 8)) for _ in range(400)]
    floats += [drawn.random() * 10.0 ** drawn.randint(-12, 22) for _ in range(400)]
    floats = [value for value in floats if reason_refused(value) is None]
    assert len(floats) > 700
    array = np.array(floats, dtype=np.float64)
    back = getattr(qf.CandleBatch.from_numpy(**arrays_of(len(floats), **{column: array})), column)
    for decimal, value in zip(back, floats, strict=True):
        assert type(decimal) is Decimal
        assert decimal == shortest(value), value
        # And the decimal reads back as the float it came from, bit for bit.
        assert bits(nearest(decimal)) == bits(value), value
        # It is the conversion the package publishes, cell for cell.
        assert decimal.as_tuple() == qf.decimal_from_f64(value).as_tuple(), value


def test_a_negative_zero_becomes_zero():
    array = np.array([-0.0, 0.0, -0.0])
    assert np.signbit(array).tolist() == [True, False, True]
    back = qf.CandleBatch.from_numpy(**arrays_of(3, volume=array)).volume
    assert [(value == 0, value.is_signed()) for value in back] == [(True, False)] * 3
    # Which is what comes back, and never the negative zero that went in.
    again = qf.CandleBatch.from_numpy(**arrays_of(3, volume=array)).to_numpy()["volume"]
    assert bits_of(again) == ["0000000000000000"] * 3


def test_a_float_that_has_no_decimal_is_refused_by_its_row_in_the_engines_words(refused):
    for value, said in [
        (float("nan"), "cannot convert NaN to a decimal: not finite"),
        (float("inf"), "cannot convert inf to a decimal: not finite"),
        (float("-inf"), "cannot convert -inf to a decimal: not finite"),
        (5e-324, "cannot convert 5e-324 to a decimal: more than 28 decimal places"),
        (1e-29, "cannot convert 1e-29 to a decimal: more than 28 decimal places"),
        (1e29, "cannot convert 1e29 to a decimal: beyond the decimal range"),
        (2.0**96, "cannot convert 7.922816251426434e28 to a decimal: beyond the decimal range"),
        (
            1.7976931348623157e308,
            "cannot convert 1.7976931348623157e308 to a decimal: beyond the decimal range",
        ),
    ]:
        for column in DECIMALS:
            for row in (0, 2, 4):
                floats = np.full(5, 1.5)
                floats[row] = value
                message = f"{column}[{row}]: {said}"
                given = arrays_of(5, **{column: floats})
                refused(ValueError, message, qf.CandleBatch.from_numpy, **given)
            # The same words as the conversion of one float has for it.
            refused(ValueError, said, qf.decimal_from_f64, value)


def test_what_a_float_is_refused_for_is_what_the_policy_says_of_it():
    drawn = random.Random(4750)
    # Floats of every size, most of them beyond what a decimal holds.
    floats = [struct.unpack(">d", drawn.randbytes(8))[0] for _ in range(600)]
    floats += [drawn.random() * 10.0 ** drawn.randint(-40, 40) for _ in range(600)]
    floats += [float("nan"), float("inf"), float("-inf"), -float("nan")]
    seen = set()
    for value in floats:
        reason = reason_refused(value)
        seen.add(reason)
        given = arrays_of(1, low=np.array([value]))
        if reason is None:
            assert qf.CandleBatch.from_numpy(**given).low == (shortest(value),), value
            continue
        with pytest.raises(ValueError) as caught:
            qf.CandleBatch.from_numpy(**given)
        assert type(caught.value) is ValueError
        message = str(caught.value)
        assert message.startswith("low[0]: cannot convert "), value
        assert message.endswith(" to a decimal: " + reason), value
    assert seen == {None, "not finite", "more than 28 decimal places", "beyond the decimal range"}


def test_the_first_float_refused_is_the_first_column_in_order_and_then_the_first_row(refused):
    bad = np.array([1.5, float("nan"), float("inf"), 1.5])
    worse = np.array([float("-inf"), 1.5, 1.5, 1.5])
    built = qf.CandleBatch.from_numpy
    message = "open[1]: cannot convert NaN to a decimal: not finite"
    refused(ValueError, message, built, **arrays_of(open=bad))
    refused(ValueError, message, built, **arrays_of(open=bad, high=worse, volume=worse))
    message = "high[0]: cannot convert -inf to a decimal: not finite"
    refused(ValueError, message, built, **arrays_of(high=worse, low=bad, close=bad))
    message = "low[1]: cannot convert NaN to a decimal: not finite"
    refused(ValueError, message, built, **arrays_of(low=bad, close=worse, volume=worse))
    message = "close[0]: cannot convert -inf to a decimal: not finite"
    refused(ValueError, message, built, **arrays_of(close=worse, volume=bad))
    message = "volume[1]: cannot convert NaN to a decimal: not finite"
    refused(ValueError, message, built, **arrays_of(volume=bad))


def test_trade_counts_are_an_array_where_all_are_known_or_a_masked_one():
    known = np.array([0, 5, U64_MAX, 7], dtype=np.uint64)
    assert qf.CandleBatch.from_numpy(**arrays_of(trades=known)).trades == (0, 5, U64_MAX, 7)
    masked = np.ma.MaskedArray(known, mask=[False, True, False, True])
    assert qf.CandleBatch.from_numpy(**arrays_of(trades=masked)).trades == (0, None, U64_MAX, None)
    # A masked array whose mask was never set masks nothing.
    unset = np.ma.MaskedArray(known)
    assert unset.mask is np.ma.nomask
    assert qf.CandleBatch.from_numpy(**arrays_of(trades=unset)).trades == (0, 5, U64_MAX, 7)
    # And one that masks everything holds no count at all.
    nothing = np.ma.masked_all(4, dtype=np.uint64)
    assert qf.CandleBatch.from_numpy(**arrays_of(trades=nothing)).trades == (None,) * 4


def test_whatever_lies_under_a_mask_is_never_read_as_a_count():
    under = np.array([11, 22, 33, 44], dtype=np.uint64)
    masked = np.ma.MaskedArray(under, mask=[True, False, True, False])
    batch = qf.CandleBatch.from_numpy(**arrays_of(trades=masked))
    assert batch.trades == (None, 22, None, 44)
    # What comes back is masked where it was, with nothing of 11 or 33.
    again = batch.to_numpy()["trades"]
    assert again.tolist() == [None, 22, None, 44]
    assert np.ma.getdata(again).tolist() == [0, 22, 0, 44]


def test_a_mask_is_read_as_numpy_reads_one_whatever_its_bytes():
    # A mask is a byte for each row, and NumPy takes any byte that is not
    # zero for a mask that is set.
    flags = np.array([0, 2, 0, 255], dtype=np.uint8).view(np.bool_)
    masked = np.ma.MaskedArray(np.array([1, 2, 3, 4], dtype=np.uint64), mask=flags)
    assert masked.tolist() == [1, None, 3, None]
    assert qf.CandleBatch.from_numpy(**arrays_of(trades=masked)).trades == (1, None, 3, None)


def test_a_masked_array_that_is_not_one_flag_for_each_count_is_refused(refused):
    built = qf.CandleBatch.from_numpy
    counts = np.array([1, 2, 3, 4], dtype=np.uint64)
    # NumPy keeps a masked array's mask as long as its values. One put out
    # of step by hand is refused, and never read past its end.
    short = np.ma.MaskedArray(counts, mask=[False, True, False, False])
    short._mask = np.zeros(2, dtype=np.bool_)
    message = "the mask of trades has 2 rows, and trades has 4"
    refused(ValueError, message, built, **arrays_of(trades=short))
    # It is refused in the turn of `trades`, as part of what the argument
    # is: before a market of another type, a column of another length and a
    # float that has no decimal, each of which comes after that turn.
    nan = np.array([float("nan")] * 4)
    amiss = arrays_of(open=nan, close=np.zeros(3), trades=short)
    refused(ValueError, message, built, **amiss, market="binance_spot")
    other = np.ma.MaskedArray(counts, mask=[False, True, False, False])
    other._mask = np.zeros(4, dtype=np.float64)
    message = "the mask of trades must be a numpy.ndarray of bool, not of float64"
    refused(TypeError, message, built, **arrays_of(trades=other))


COUNTS = "a numpy.ndarray or a numpy.ma.MaskedArray of uint64"


def test_trade_counts_are_of_uint64_and_nothing_cast_to_it(refused):
    built = qf.CandleBatch.from_numpy
    for dtype in ["int64", "uint32", "float64", "bool", "object"]:
        counts = np.array([1, 2, 3, 4]).astype(dtype)
        message = f"trades must be {COUNTS}, not of {dtype}"
        refused(TypeError, message, built, **arrays_of(trades=counts))
        masked = np.ma.MaskedArray(counts, mask=[False, True, False, False])
        refused(TypeError, message, built, **arrays_of(trades=masked))
    # A float column with NaN for a count that is unknown is no trade count.
    floats = np.array([1.0, float("nan"), 3.0, 4.0])
    refused(
        TypeError, f"trades must be {COUNTS}, not of float64", built, **arrays_of(trades=floats)
    )
    for value, named in [(None, "NoneType"), ([1, None, 3, 4], "list"), (5, "int")]:
        message = f"trades must be {COUNTS}, not {named}"
        refused(TypeError, message, built, **arrays_of(trades=value))


@pytest.mark.parametrize("column", TIMES + DECIMALS)
def test_a_column_is_a_numpy_array_of_its_type_and_nothing_cast_to_it(column, refused):
    built = qf.CandleBatch.from_numpy
    takes = f"a numpy.ndarray of {DTYPES[column]}"
    for dtype in ["int64", "int32", "uint64", "float64", "float32", "float16", "bool", "object"]:
        if dtype == DTYPES[column]:
            continue
        array = np.array([1, 2, 3, 4]).astype(dtype)
        message = f"{column} must be {takes}, not of {dtype}"
        refused(TypeError, message, built, **arrays_of(**{column: array}))
    # Its cells are in this machine's byte order.
    swapped = np.array([1, 2, 3, 4]).astype(np.dtype(DTYPES[column]).newbyteorder())
    assert not swapped.dtype.isnative
    message = f"{column} must be {takes}, not of {swapped.dtype}"
    refused(TypeError, message, built, **arrays_of(**{column: swapped}))
    # A time is a count of milliseconds, not NumPy's own time.
    stamps = np.array([0, 60_000, 120_000, 180_000], dtype="datetime64[ms]")
    message = f"{column} must be {takes}, not of datetime64[ms]"
    refused(TypeError, message, built, **arrays_of(**{column: stamps}))
    # What is no array is not turned into one.
    for value, named in [
        ([1.5, 2.5, 3.5, 4.5], "list"),
        ((1, 2, 3, 4), "tuple"),
        (None, "NoneType"),
        (1.5, "float"),
        (np.float64(1.5), "float64"),
        (np.int64(1), "int64"),
        (range(4), "range"),
        (memoryview(np.zeros(4)), "memoryview"),
        ("1.5", "str"),
    ]:
        message = f"{column} must be {takes}, not {named}"
        refused(TypeError, message, built, **arrays_of(**{column: value}))


@pytest.mark.parametrize("column", COLUMNS)
def test_a_column_has_one_dimension(column, refused):
    built = qf.CandleBatch.from_numpy
    flat = np.arange(4).astype(DTYPES[column])
    for shaped, dimensions in [
        (flat.reshape(4, 1), 2),
        (flat.reshape(1, 4), 2),
        (flat.reshape(2, 2), 2),
        (flat.reshape(1, 4, 1), 3),
        (np.array(flat[0]), 0),
    ]:
        assert type(shaped) is np.ndarray and shaped.ndim == dimensions
        message = f"{column} must have one dimension, not {dimensions}"
        refused(ValueError, message, built, **arrays_of(**{column: shaped}))
    masked = np.ma.MaskedArray(flat.reshape(2, 2).astype(np.uint64))
    message = "trades must have one dimension, not 2"
    refused(ValueError, message, built, **arrays_of(trades=masked))


class Unit(np.ndarray):
    """An array whose values mean something its memory does not say."""


class Flagged(np.ma.MaskedArray):
    """A masked array of a type of its own."""


@pytest.mark.parametrize("column", TIMES + DECIMALS)
def test_an_array_of_a_type_derived_from_ndarray_is_not_read_as_one(column, tmp_path, refused):
    built = qf.CandleBatch.from_numpy
    takes = f"a numpy.ndarray of {DTYPES[column]}"
    plain = np.array([1, 2, 3, 4]).astype(DTYPES[column])
    # A masked value is no price and no time: reading the memory under it
    # would take it for one.
    masked = np.ma.MaskedArray(plain, mask=[False, True, False, False])
    message = f"{column} must be {takes}, not MaskedArray"
    refused(TypeError, message, built, **arrays_of(**{column: masked}))
    unmasked = np.ma.MaskedArray(plain)
    refused(TypeError, message, built, **arrays_of(**{column: unmasked}))
    message = f"{column} must be {takes}, not Unit"
    refused(TypeError, message, built, **arrays_of(**{column: plain.view(Unit)}))
    mapped = np.memmap(tmp_path / "column.bin", dtype=DTYPES[column], mode="w+", shape=(4,))
    mapped[:] = plain
    message = f"{column} must be {takes}, not memmap"
    refused(TypeError, message, built, **arrays_of(**{column: mapped}))
    # The array of one is given by whoever decides that it is one.
    for derived in [masked.filled(9), plain.view(Unit), mapped]:
        array = np.asarray(derived)
        assert type(array) is np.ndarray
        back = getattr(built(**arrays_of(**{column: array})), column)
        assert [float(value) for value in back] == [float(value) for value in array]


def test_trade_counts_of_a_type_derived_from_either_are_not_read_as_one(refused):
    built = qf.CandleBatch.from_numpy
    counts = np.array([1, 2, 3, 4], dtype=np.uint64)
    refused(
        TypeError,
        f"trades must be {COUNTS}, not Unit",
        built,
        **arrays_of(trades=counts.view(Unit)),
    )
    flagged = np.ma.MaskedArray(counts, mask=[False, True, False, False]).view(Flagged)
    assert isinstance(flagged, np.ma.MaskedArray)
    refused(TypeError, f"trades must be {COUNTS}, not Flagged", built, **arrays_of(trades=flagged))
    # A masked array made over an array of a derived type is of that type
    # underneath, and is refused as an array of that type is.
    over = np.ma.MaskedArray(counts.view(Unit), mask=[False, True, False, False])
    assert type(over) is np.ma.MaskedArray and type(over.data) is Unit
    assert over.baseclass is Unit
    message = f"trades must be {COUNTS}, not a MaskedArray over Unit"
    refused(TypeError, message, built, **arrays_of(trades=over))
    # The masked array of its plain counts is read.
    plain = np.ma.MaskedArray(np.asarray(over.data), mask=over.mask)
    assert plain.baseclass is np.ndarray
    assert built(**arrays_of(trades=plain)).trades == (1, None, 3, 4)


def test_a_masked_array_over_a_derived_type_is_refused_before_that_types_code_runs(refused):
    ran = []

    class Watches(np.ndarray):
        """An array whose type's own code runs whenever a view of it is
        made, which is how numpy.ma hands out a masked array's cells."""

        def __array_finalize__(self, viewed):
            ran.append(type(viewed).__name__)

    counts = np.array([1, 2, 3, 4], dtype=np.uint64)
    over = np.ma.MaskedArray(counts.view(Watches), mask=[False, True, False, False])
    assert type(over) is np.ma.MaskedArray and over.baseclass is Watches
    del ran[:]
    message = f"trades must be {COUNTS}, not a MaskedArray over Watches"
    refused(TypeError, message, qf.CandleBatch.from_numpy, **arrays_of(trades=over))
    # Nothing was asked of it: none of its type's code ran.
    assert ran == []
    # Asked for its cells, it runs that code, as it would have.
    np.ma.getdata(over)
    assert ran != []


@pytest.mark.parametrize(
    "column, rows, change, kind, refusal",
    [
        # Half the size: twice the cells in the same memory. The array has
        # half the rows of the others when it is taken, so that it has as
        # many as they when the lengths are compared: read as what it was
        # taken for, its cells would be read past their end.
        (
            "open",
            2,
            lambda array: setattr(array, "dtype", np.float32),
            TypeError,
            "open must be a numpy.ndarray of float64, not of float32",
        ),
        (
            "close_time_ms",
            2,
            lambda array: setattr(array, "dtype", np.int32),
            TypeError,
            "close_time_ms must be a numpy.ndarray of int64, not of int32",
        ),
        # The same size: read as what it was taken for, its cells would be
        # numbers they are not.
        (
            "close",
            4,
            lambda array: setattr(array, "dtype", np.int64),
            TypeError,
            "close must be a numpy.ndarray of float64, not of int64",
        ),
        (
            "open_time_ms",
            4,
            lambda array: setattr(array, "dtype", np.float64),
            TypeError,
            "open_time_ms must be a numpy.ndarray of int64, not of float64",
        ),
        (
            "volume",
            4,
            lambda array: setattr(array, "shape", (2, 2)),
            ValueError,
            "volume must have one dimension, not 2",
        ),
        # Twice the cells in an array that had all its rows: the lengths no
        # longer agree, and it is refused before any cell is read.
        (
            "open",
            4,
            lambda array: setattr(array, "dtype", np.float32),
            ValueError,
            "candle column `open` has 8 rows, expected 4",
        ),
    ],
    ids=[
        "smaller cells",
        "smaller times",
        "the same size",
        "a time as a float",
        "a shape",
        "another length",
    ],
)
@pytest.mark.filterwarnings("ignore::DeprecationWarning")
def test_an_array_changed_where_it_stands_after_it_was_taken_is_refused_for_what_it_is_now(
    column, rows, change, kind, refusal, monkeypatch, refused
):
    # NumPy lets an array's type and shape be changed where it stands, and
    # is giving that up. The arrays are held to their types as they are
    # taken, and Python's own code runs before their cells are read:
    # numpy.ma's, for masked counts. Here that code changes an array that
    # was already taken.
    memory = np.empty(8, dtype=DTYPES[column])
    memory[:rows] = 1
    # What lies past the end of the array, in memory this test owns: no
    # float there has a decimal, so reading it would be refused by a row the
    # array does not have.
    memory[rows:] = np.nan if DTYPES[column] == "float64" else -1
    taken = memory[:rows]
    counts = np.ma.MaskedArray(np.arange(4, dtype=np.uint64), mask=[False, True, False, False])
    given = arrays_of(**{column: taken}, trades=counts)
    was = []
    hands_out = np.ma.getdata

    def changes_first(masked, subok=True):
        was.append((str(taken.dtype), taken.shape))
        change(taken)
        return hands_out(masked, subok=subok)

    monkeypatch.setattr(np.ma, "getdata", changes_first)
    refused(kind, refusal, qf.CandleBatch.from_numpy, **given)
    # It was of its type when it was taken, and changed afterwards.
    assert was == [(DTYPES[column], (rows,))]


# Enough candles that a column of floats holds the interpreter for longer
# than the interpreter waits before a thread asks for it.
RACED_ROWS = 100_000


def raced(change, attempts: int = 12) -> list[str]:
    """Calls `from_numpy` while another thread changes one of its arrays,
    after every array was taken and before the changed one is read: gives
    what each attempt came to, up to the first that was refused.

    `change(volume, mask)` is handed the last column of floats, with what
    lies past its end in memory of its own, and the mask of the counts.

    Once a first call has set everything up, nothing here is left to how
    fast a thread happens to wake:

    - **Nothing is set up while the thread waits.** The package and the
      array bindings set a few things up when they are first used, and let
      other threads run as they do: a thread that was waiting could get in
      there, before the lengths are compared. So a small call with a mask
      comes first, with no thread about.
    - **The thread is let go from numpy.ma's own code,** which hands out the
      mask of the counts last of all. Every array has been taken by then.
    - **It cannot run until the call lets it.** The call then turns a whole
      column of floats into decimals, and holds the interpreter while it
      does, for longer than the interpreter's switch interval: the thread
      has asked for the interpreter by the end of it.
    - **NumPy lets it in.** The next column lies apart, so NumPy copies it,
      and lets other threads run meanwhile. An interpreter that has been
      asked for is handed over there, and the call waits until it was taken.
    - **The changed arrays are read after that.**"""
    built = qf.CandleBatch.from_numpy
    hands_out = np.ma.getmask
    few = np.arange(4, dtype=np.int64)
    built(
        open_time_ms=few,
        close_time_ms=few,
        **{name: np.full((4, 2), 1.5)[:, 0] for name in ("open", "high", "low", "close", "volume")},
        trades=np.ma.MaskedArray(few.astype(np.uint64), mask=[False, True, False, False]),
    )
    came_to = []
    for _ in range(attempts):
        opened = np.arange(RACED_ROWS, dtype=np.int64) * 60_000
        price = np.full(RACED_ROWS, 1.5)
        apart = np.full((RACED_ROWS, 2), 1.5)
        memory = np.empty(2 * RACED_ROWS)
        memory[:RACED_ROWS] = 1.5
        # Past the end of `volume`: read, it would be refused by a row the
        # array does not have.
        memory[RACED_ROWS:] = np.nan
        volume = memory[:RACED_ROWS]
        mask = np.zeros(RACED_ROWS, dtype=np.bool_)
        trades = np.ma.MaskedArray(np.arange(RACED_ROWS, dtype=np.uint64), mask=mask)
        # The masked array holds the mask itself, and not a copy of it.
        assert np.ma.getmask(trades) is mask
        taken = threading.Event()

        def says_so(masked):
            handed = hands_out(masked)
            taken.set()
            return handed

        def other():
            taken.wait()
            with warnings.catch_warnings():
                warnings.simplefilter("ignore", DeprecationWarning)
                change(volume, mask)

        thread = threading.Thread(target=other, daemon=True)
        thread.start()
        np.ma.getmask = says_so
        try:
            built(
                open_time_ms=opened,
                close_time_ms=opened + 59_999,
                open=price,
                high=apart[:, 0],
                low=price,
                close=apart[:, 1],
                volume=volume,
                trades=trades,
            )
            came_to.append("built")
        except (TypeError, ValueError) as error:
            came_to.append(f"{type(error).__name__}: {error}")
        finally:
            np.ma.getmask = hands_out
            taken.set()
            thread.join()
        if came_to[-1] != "built":
            break
    return came_to


@pytest.mark.parametrize(
    "change, refusal",
    [
        # Twice the cells in the same memory: read as what it was taken for,
        # the column would be read past its end.
        (
            lambda volume, mask: setattr(volume, "dtype", np.float32),
            "TypeError: volume must be a numpy.ndarray of float64, not of float32",
        ),
        # A mask that is no longer one of flags.
        (
            lambda volume, mask: setattr(mask, "dtype", np.int8),
            "TypeError: the mask of trades must be a numpy.ndarray of bool, not of int8",
        ),
    ],
    ids=["a column's cells halved", "a mask of another type"],
)
def test_an_array_another_thread_changes_after_it_was_taken_is_refused_for_what_it_is_now(
    change, refusal
):
    assert sys.getswitchinterval() < 0.05, "the test counts on a thread asking for the interpreter"
    came_to = raced(change)
    # An attempt whose thread came too late built its batch: nothing was
    # changed while it was read. No attempt came to anything else, and none
    # read a cell past the end of an array, which would be refused by a row
    # the array does not have.
    assert set(came_to) <= {"built", refusal}, came_to
    # And the thread did change the array between the two, or this held
    # nothing.
    assert came_to[-1] == refusal, came_to


# A mask resized where it stands gives its memory back. Whatever reads it
# where it was reads memory that is no longer its own, which may end the
# process: so this is asked of an interpreter of its own.
RESIZED = (
    """
    import sys
    import threading
    import warnings

    import numpy as np

    import quantforge_py as qf

    RACED_ROWS = %d
    """
    % RACED_ROWS
)


def test_a_mask_another_thread_resizes_where_it_stands_is_never_read_where_it_was():
    source = textwrap.dedent(RESIZED) + textwrap.dedent(
        """
        def resized(volume, mask):
            # NumPy refuses this of an array that is in use, unless it is
            # asked not to look.
            mask.resize(1, refcheck=False)
        """
    )
    source += inspect_source(raced) + "\nprint(*raced(resized), sep='\\n')\n"
    ran = subprocess.run(
        [sys.executable, "-c", source], capture_output=True, text=True, timeout=120
    )
    # The process was not ended, and nothing was read where the mask was.
    assert ran.returncode == 0, ran.stderr
    refusal = f"ValueError: the mask of trades has 1 rows, and trades has {RACED_ROWS}"
    came_to = ran.stdout.splitlines()
    assert set(came_to) <= {"built", refusal}, came_to
    assert came_to[-1] == refusal, came_to


# What the scenarios below start with, each in an interpreter of its own:
# where something of Python's runs while an array is borrowed and changes
# it, the array bindings end the process.
ON_ITS_OWN = """
    import gc
    import sys
    import threading
    import warnings

    import numpy as np

    import quantforge_py as qf

    warnings.simplefilter("ignore", DeprecationWarning)
    built = qf.CandleBatch.from_numpy
    PRICES = ("open", "high", "low", "close", "volume")


    def given(rows, trades, **columns):
        opened = np.arange(rows, dtype=np.int64) * 60_000
        made = dict(open_time_ms=opened, close_time_ms=opened + 59_999, trades=trades)
        made.update({name: np.full(rows, 1.5) for name in PRICES})
        made.update(columns)
        return made


    def counts(rows, mask=None):
        numbers = np.arange(rows, dtype=np.uint64)
        return np.ma.MaskedArray(numbers) if mask is None else np.ma.MaskedArray(numbers, mask=mask)


    def came_to(call):
        try:
            call()
        except Exception as error:
            return f"{type(error).__name__}: {error}"
        return "built"
"""


def on_its_own(*parts: str) -> subprocess.CompletedProcess:
    source = "\n".join(textwrap.dedent(part) for part in (ON_ITS_OWN, *parts))
    return subprocess.run(
        [sys.executable, "-c", source], capture_output=True, text=True, timeout=120
    )


def test_an_array_changed_again_as_its_refusal_is_worded_is_refused_and_the_process_goes_on():
    # An array that is no longer of its column's type is refused, and NumPy
    # words the type it has in Python. Code that runs then can change the
    # array once more: here a tracer does, at the first of NumPy's wording,
    # to cells of another size. The array is not borrowed while its refusal
    # is worded, so nothing is asked to give back a borrow of an array that
    # is no longer what was borrowed.
    ran = on_its_own(
        """
        rows = 4
        # Everything that is set up when the conversions are first used.
        built(**given(rows, counts(rows, [False, True, False, False])))
        memory = np.empty(2 * rows)
        memory[:rows] = 1.5
        memory[rows:] = np.nan
        volume = memory[:rows]
        changes = []
        hands_out = np.ma.getdata


        def changes_first(masked, subok=True):
            # After `volume` was taken: another type, of the same size.
            volume.dtype = np.int64
            changes.append("taken")
            return hands_out(masked, subok=subok)


        def as_it_is_worded(frame, event, arg):
            code = frame.f_code
            words = event == "call" and code.co_name == "__str__" and "numpy" in code.co_filename
            if words and changes == ["taken"]:
                volume.dtype = np.float32
                changes.append("worded")


        np.ma.getdata = changes_first
        sys.settrace(as_it_is_worded)
        try:
            said = came_to(lambda: built(**given(rows, counts(rows, [False] * rows), volume=volume)))
        finally:
            sys.settrace(None)
            np.ma.getdata = hands_out
        print(said)
        print(changes)
        # The process goes on, and the package with it.
        print(built(**given(rows, counts(rows))).volume[0])
        """
    )
    assert ran.returncode == 0, ran.stderr
    assert ran.stdout.splitlines() == [
        # For what it was when it was held to its type, and no cell read.
        "TypeError: volume must be a numpy.ndarray of float64, not of int64",
        "['taken', 'worded']",
        "1.5",
    ]


MASK_CHANGES = {
    "resized": (
        "mask.resize(1, refcheck=False)",
        "ValueError: the mask of trades has 1 rows, and trades has {rows}",
    ),
    "of another type": (
        "mask.dtype = np.int16",
        "TypeError: the mask of trades must be a numpy.ndarray of bool, not of int16",
    ),
}


@pytest.mark.parametrize("change", MASK_CHANGES)
def test_a_mask_a_thread_changes_as_the_view_of_it_is_made_is_refused_and_the_process_goes_on(
    change,
):
    # The flags of a mask are read as bytes, through a view that is made as
    # they are read. The first time a process makes one, the interpreter is
    # let go for a moment just before: a thread that has been waiting gets
    # in there, and changes the mask. Every column lies one after another,
    # so NumPy copies none and lets the thread in nowhere else. The mask is
    # not borrowed then, and is refused for what it has become.
    changed, refusal = MASK_CHANGES[change]
    rows = RACED_ROWS
    for _ in range(3):
        ran = on_its_own(
            f"""
            rows = {rows}
            # Everything that is set up when the conversions are first used,
            # but for what is set up as a mask is first read: these counts
            # have none.
            built(**given(4, counts(4)))
            mask = np.zeros(rows, dtype=np.bool_)
            trades = counts(rows, mask)
            assert np.ma.getmask(trades) is mask
            taken = threading.Event()
            hands_out = np.ma.getmask


            def says_so(masked):
                handed = hands_out(masked)
                taken.set()
                return handed


            def other():
                taken.wait()
                {changed}


            thread = threading.Thread(target=other, daemon=True)
            thread.start()
            np.ma.getmask = says_so
            try:
                print(came_to(lambda: built(**given(rows, trades))))
            finally:
                np.ma.getmask = hands_out
                taken.set()
                thread.join()
            print(built(**given(4, counts(4))).volume[0])
            """
        )
        # The process was not ended.
        assert ran.returncode == 0, ran.stderr
        said, goes_on = ran.stdout.splitlines()
        assert goes_on == "1.5"
        if said != "built":
            break
    # The thread did get in, and the mask was refused for what it had become.
    assert said == refusal.format(rows=rows)


@pytest.mark.parametrize("change", MASK_CHANGES)
def test_a_mask_the_collector_changes_as_the_view_of_it_is_made_is_refused(change):
    # Before Python 3.12 the collector runs wherever something it keeps
    # track of is made, and with it whatever code it was given to call:
    # here, as the view of the mask is asked for. From 3.12 on it runs only
    # between two steps of Python's own code: as numpy.ma hands out the
    # counts and the mask, or once the call has returned.
    changed, refusal = MASK_CHANGES[change]
    rows = 1_000
    ran = on_its_own(
        f"""
        rows = {rows}
        # Everything is set up first, the view of a mask among it.
        built(**given(rows, counts(rows, np.zeros(rows, dtype=np.bool_))))
        mask = np.zeros(rows, dtype=np.bool_)
        trades = counts(rows, mask)
        state = []
        kept = []
        hands_out = np.ma.getmask


        # Something the collector counts as it is made. A list or a tuple
        # will not do: Python hands out ones it has kept back, and does not
        # count those.
        class Counted:
            pass


        def collected(phase, info):
            # What is made here and kept leaves the collector above its
            # threshold as it ends: so it runs again at the very next thing
            # it counts, wherever that is made.
            kept.append(Counted())
            kept.append(Counted())
            kept.append(Counted())
            if state == ["armed"] and phase == "start":
                state.append("changed")
                {changed}


        def arms(masked):
            handed = hands_out(masked)
            state.append("armed")
            return handed


        arrays = given(rows, trades)
        gc.callbacks.append(collected)
        np.ma.getmask = arms
        gc.set_threshold(1)
        try:
            said = came_to(lambda: built(**arrays))
            state.append("left")
        finally:
            gc.set_threshold(700, 10, 10)
            gc.callbacks.remove(collected)
            np.ma.getmask = hands_out
        print(said)
        print(state.index("changed") < state.index("left") if "changed" in state else False)
        print(built(**given(4, counts(4))).volume[0])
        """
    )
    assert ran.returncode == 0, ran.stderr
    said, inside, goes_on = ran.stdout.splitlines()
    assert goes_on == "1.5"
    if sys.version_info < (3, 12):
        assert (said, inside) == (refusal.format(rows=rows), "True")
    else:
        assert said in ("built", refusal.format(rows=rows))


def layouts(values: list, dtype: str) -> dict:
    """One column's values as arrays that lay them out in memory each in a
    way of its own."""
    count = len(values)
    plain = np.array(values, dtype=dtype)
    wide = np.zeros((count, 3), dtype=dtype)
    wide[:, 1] = plain
    spaced = np.zeros(count * 2, dtype=dtype)
    spaced[::2] = plain
    # A packed record: each cell of `value` lies a byte after a flag, so
    # one of eight bytes is nine from the one before it, and off its
    # alignment.
    packed = np.zeros(count, dtype=[("flag", "u1"), ("value", dtype)])
    packed["value"] = plain
    assert packed["value"].strides == (plain.itemsize + 1,)
    assert packed["value"].flags.aligned == (plain.itemsize == 1)
    frozen = plain.copy()
    frozen.flags.writeable = False
    # One after another, and a byte off their alignment.
    shifted = np.frombuffer(b"\x00" + plain.tobytes(), dtype=dtype, offset=1)
    assert shifted.flags.c_contiguous and shifted.flags.aligned == (plain.itemsize == 1)
    return {
        "as it is": plain,
        "a column of a table": wide[:, 1],
        "every other cell": spaced[::2],
        "backwards": plain[::-1].copy()[::-1],
        "backwards over a table": np.flipud(wide)[::-1, 1],
        "a field of a packed record": packed["value"],
        "a byte off its alignment": shifted,
        "read-only": frozen,
        "a view of bytes": np.frombuffer(plain.tobytes(), dtype=dtype),
        "in Fortran order": np.asfortranarray(plain),
    }


@pytest.mark.parametrize("column", COLUMNS)
def test_an_array_is_read_wherever_its_cells_lie_in_memory(column):
    values = {
        "int64": [I64_MIN, -7, 0, 1_767_268_800_000, I64_MAX],
        "float64": [0.1, 81940.01, 2.5, 1e-7, 123456789.125],
        "uint64": [0, 1, 412, 2**53 + 1, U64_MAX],
    }[DTYPES[column]]
    expected = getattr(
        qf.CandleBatch.from_numpy(**arrays_of(5, **{column: np.array(values, DTYPES[column])})),
        column,
    )
    assert len(expected) == 5
    made = layouts(values, DTYPES[column])
    assert not all(array.flags.c_contiguous for array in made.values())
    for layout, array in made.items():
        assert type(array) is np.ndarray and array.tolist() == values, layout
        back = getattr(qf.CandleBatch.from_numpy(**arrays_of(5, **{column: array})), column)
        assert back == expected, layout


def test_masked_counts_are_read_wherever_their_cells_and_their_flags_lie():
    counts = [0, 1, 412, 2**53 + 1, U64_MAX]
    flags = [False, True, False, False, True]
    expected = (0, None, 412, 2**53 + 1, None)
    for layout, values in layouts(counts, "uint64").items():
        for mask_layout, mask in layouts(flags, "bool").items():
            masked = np.ma.MaskedArray(values, mask=mask)
            batch = qf.CandleBatch.from_numpy(**arrays_of(5, trades=masked))
            assert batch.trades == expected, (layout, mask_layout)


def test_a_column_of_another_length_is_refused_in_the_engines_words(refused):
    built = qf.CandleBatch.from_numpy
    # Every column is held to the open times' length, and the first that
    # differs is named, in the order the columns are given.
    for column in COLUMNS[1:]:
        for found in [3, 5, 0]:
            other = np.zeros(found, dtype=DTYPES[column])
            message = f"candle column `{column}` has {found} rows, expected 4"
            refused(ValueError, message, built, **arrays_of(**{column: other}))
    short = arrays_of(open_time_ms=np.zeros(2, dtype=np.int64))
    refused(ValueError, "candle column `close_time_ms` has 4 rows, expected 2", built, **short)
    both = arrays_of(volume=np.zeros(2), high=np.zeros(1))
    refused(ValueError, "candle column `high` has 1 rows, expected 4", built, **both)
    masked = np.ma.masked_all(3, dtype=np.uint64)
    message = "candle column `trades` has 3 rows, expected 4"
    refused(ValueError, message, built, **arrays_of(trades=masked))
    # The words are the ones a batch of columns is refused with.
    columns = {name: [1] * 4 for name in COLUMNS}
    with pytest.raises(ValueError) as caught:
        qf.CandleBatch.from_columns(**{**columns, "trades": [1] * 3})
    assert str(caught.value) == message


def test_every_argument_is_held_to_its_type_then_to_one_length_and_then_read(refused):
    built = qf.CandleBatch.from_numpy
    nan = np.array([float("nan")] * 4)
    wrong = dict(
        open_time_ms=[0, 1, 2, 3],
        close_time_ms=np.zeros(4),
        open=np.zeros(4, dtype=np.float32),
        high=np.zeros((2, 2)),
        low=np.ma.MaskedArray(np.zeros(4)),
        close=np.zeros(4, dtype=np.int64),
        volume=None,
        trades=np.zeros(4),
        market="binance_spot",
    )
    right = dict(arrays_of(open=nan, close=np.zeros(3)), market=None)
    # With every argument of the wrong type, each is refused in its turn as
    # the ones before it are put right.
    for name, kind, message in [
        ("open_time_ms", TypeError, "open_time_ms must be a numpy.ndarray of int64, not list"),
        (
            "close_time_ms",
            TypeError,
            "close_time_ms must be a numpy.ndarray of int64, not of float64",
        ),
        ("open", TypeError, "open must be a numpy.ndarray of float64, not of float32"),
        ("high", ValueError, "high must have one dimension, not 2"),
        ("low", TypeError, "low must be a numpy.ndarray of float64, not MaskedArray"),
        ("close", TypeError, "close must be a numpy.ndarray of float64, not of int64"),
        ("volume", TypeError, "volume must be a numpy.ndarray of float64, not NoneType"),
        ("trades", TypeError, f"trades must be {COUNTS}, not of float64"),
        ("market", TypeError, "market must be a MarketId or None, not str"),
    ]:
        refused(kind, message, built, **wrong)
        wrong[name] = right[name]
    # Every argument is of its type now. A length is compared before a value
    # is read, though the value comes first among the columns.
    refused(ValueError, "candle column `close` has 3 rows, expected 4", built, **wrong)
    wrong["close"] = np.zeros(4)
    message = "open[0]: cannot convert NaN to a decimal: not finite"
    refused(ValueError, message, built, **wrong)
    wrong["open"] = np.zeros(4)
    assert len(built(**wrong)) == 4


@pytest.mark.parametrize(
    "stands_in, kind, refusal",
    [
        (
            lambda zeros, rows, dtype: zeros(rows - 1, dtype),
            ValueError,
            "what numpy.zeros gave has 2 rows, and 3 were asked for",
        ),
        (
            lambda zeros, rows, dtype: zeros(rows + 1, dtype),
            ValueError,
            "what numpy.zeros gave has 4 rows, and 3 were asked for",
        ),
        (
            lambda zeros, rows, dtype: zeros((rows, 1), dtype),
            ValueError,
            "what numpy.zeros gave must have one dimension, not 2",
        ),
        (
            lambda zeros, rows, dtype: zeros(rows, np.float32),
            TypeError,
            "what numpy.zeros gave must be a numpy.ndarray of int64, not of float32",
        ),
        (
            lambda zeros, rows, dtype: zeros(rows, dtype).view(Unit),
            TypeError,
            "what numpy.zeros gave must be a numpy.ndarray of int64, not Unit",
        ),
        (
            lambda zeros, rows, dtype: [0] * rows,
            TypeError,
            "what numpy.zeros gave must be a numpy.ndarray of int64, not list",
        ),
    ],
    ids=["shorter", "longer", "two dimensions", "another type of cell", "a derived type", "a list"],
)
def test_an_array_to_numpy_gives_is_the_one_it_asked_numpy_for(
    stands_in, kind, refusal, monkeypatch, refused
):
    # Every array is as long as the batch and of its column's type, and that
    # does not rest on NumPy alone: what stands where `numpy.zeros` was is
    # held to what was asked of it, and an array of another length or type
    # is given to nobody.
    batch = batch_of(3)
    zeros = np.zeros
    asked = []

    def stood_in(rows, dtype):
        asked.append((rows, str(dtype)))
        return stands_in(zeros, rows, dtype)

    monkeypatch.setattr(np, "zeros", stood_in)
    refused(kind, refusal, batch.to_numpy)
    # It was asked for the first column's array, and for none after that.
    assert asked == [(3, "int64")]
    monkeypatch.undo()
    assert batch.to_numpy()["open_time_ms"].tolist() == [0, 60_000, 120_000]


def test_the_arrays_are_read_and_not_kept():
    given = arrays_of(
        close=np.array([1.5, 2.5, 3.5, 4.5]),
        trades=np.ma.MaskedArray(np.array([1, 2, 3, 4], dtype=np.uint64), mask=[0, 1, 0, 0]),
    )
    batch = qf.CandleBatch.from_numpy(**given)
    # Changing an array afterwards changes nothing in the batch.
    given["close"][:] = 99.0
    given["open_time_ms"][:] = -1
    given["trades"][:] = np.ma.masked
    assert batch.close == (Decimal("1.5"), Decimal("2.5"), Decimal("3.5"), Decimal("4.5"))
    assert batch.open_time_ms == (0, 60_000, 120_000, 180_000)
    assert batch.trades == (1, None, 3, 4)
    # And the batch holds on to none of them.
    watched = {name: weakref.ref(array) for name, array in given.items()}
    del given
    gc.collect()
    assert [name for name, array in watched.items() if array() is not None] == []
    assert batch.close[0] == Decimal("1.5")


def test_an_array_that_is_refused_is_let_go_of_too():
    given = arrays_of(close=np.array([1.5, float("nan"), 3.5, 4.5]))
    with pytest.raises(ValueError, match="close\\[1\\]"):
        qf.CandleBatch.from_numpy(**given)
    # It can be put right and given again, and nothing is left holding it.
    given["close"][1] = 2.5
    assert qf.CandleBatch.from_numpy(**given).close[1] == Decimal("2.5")
    watched = {name: weakref.ref(array) for name, array in given.items()}
    del given
    gc.collect()
    assert [name for name, array in watched.items() if array() is not None] == []


def traced(call) -> tuple[int, int]:
    """What Python's tracer saw allocated while `call` ran, in bytes: what
    was still held when it returned, and the most held at once. It sees
    NumPy's arrays, and none of the engine's own memory."""
    gc.collect()
    tracemalloc.start()
    try:
        before, _ = tracemalloc.get_traced_memory()
        tracemalloc.reset_peak()
        kept = call()
        held, most = tracemalloc.get_traced_memory()
    finally:
        tracemalloc.stop()
    del kept
    return held - before, most - before


def test_an_array_whose_cells_lie_one_after_another_is_read_where_it_lies():
    rows = 200_000
    # More than the tracer sees of a call that copies nothing, and far less
    # than one array.
    little = 64 * 1024
    table = np.zeros((rows, 2))
    wide = np.zeros((rows, 2), dtype=np.int64)
    given = arrays_of(rows)
    assert all(array.nbytes == rows * 8 for array in given.values())
    built = qf.CandleBatch.from_numpy
    # Once first: NumPy and the extension import what they need as they are
    # first used.
    built(**given)
    _, most = traced(lambda: built(**given))
    assert most < little
    masked = np.ma.MaskedArray(given["trades"], mask=np.zeros(rows, dtype=np.bool_))
    _, most = traced(lambda: built(**{**given, "trades": masked}))
    assert most < little
    # Counts whose mask was never set have none to read, and none is made.
    unset = np.ma.MaskedArray(given["trades"])
    assert unset.mask is np.ma.nomask
    _, most = traced(lambda: built(**{**given, "trades": unset}))
    assert most < little

    # Nor is a mask made for a masked array that is refused for its cells.
    def refused_for_its_cells():
        with pytest.raises(TypeError, match="trades must be .* not of float64"):
            built(**{**given, "trades": np.ma.MaskedArray(given["volume"])})

    _, most = traced(refused_for_its_cells)
    assert most < little
    # An array whose cells lie apart is copied by NumPy first, once, and
    # the copy is let go of with the call.
    for column, apart in [
        ("close", table[:, 1]),
        ("open_time_ms", wide[:, 0]),
        ("volume", given["volume"][::-1]),
        ("trades", wide.astype(np.uint64)[:, 1]),
    ]:
        assert not apart.flags.c_contiguous
        held, most = traced(lambda: built(**{**given, column: apart}))
        assert rows * 8 <= most < rows * 8 + little, column
        assert held < little, column

    # Each copy is let go of before the next column is read: five columns
    # that lie apart never have two copies at once.
    apart = {column: table[:, 1] for column in DECIMALS}
    _, most = traced(lambda: built(**{**given, **apart}))
    assert rows * 8 <= most < rows * 8 + little

    # And nothing is copied for a batch that is then refused for a length:
    # every argument is held to its type and its length before a cell is
    # read.
    def refused_for_a_length():
        with pytest.raises(ValueError, match="candle column `volume` has 3 rows"):
            built(**{**given, **apart, "volume": np.zeros(3)})

    _, most = traced(refused_for_a_length)
    assert most < little


def test_to_numpy_allocates_the_eight_arrays_and_the_mask_and_nothing_beside_them():
    rows = 200_000
    batch = qf.CandleBatch.from_numpy(**arrays_of(rows))
    batch.to_numpy()
    held, most = traced(batch.to_numpy)
    # Eight arrays of eight bytes a cell, and a mask of one.
    expected = rows * (8 * 8 + 1)
    assert sum(array.nbytes for array in batch.to_numpy().values()) == rows * 8 * 8
    assert expected <= held < expected + 64 * 1024
    # Nothing more was held on the way: no array was built and then copied.
    assert most < expected + 64 * 1024


OUT_OF_MEMORY = """
    import resource
    import sys

    import numpy as np

    import quantforge_py as qf

    rows = 2_000_000
    opened = np.arange(rows, dtype=np.int64)
    price = np.full(rows, 1.5)
    given = dict(
        open_time_ms=opened, close_time_ms=opened, open=price, high=price, low=price, close=price,
        volume=price, trades=np.zeros(rows, dtype=np.uint64),
    )
    small = {name: array[:4] for name, array in given.items()}
    # Everything NumPy and the extension import as they are first used.
    qf.CandleBatch.from_numpy(**small).to_numpy()
    batch = qf.CandleBatch.from_numpy(**given) if sys.argv[1] == "to_numpy" else None

    def address_space() -> int:
        with open("/proc/self/status", encoding="ascii") as status:
            (size,) = [line for line in status if line.startswith("VmSize:")]
        return int(size.split()[1]) * 1024

    # Room for what Python needs to raise an error, and for no column of
    # two million rows, which takes sixteen megabytes.
    limit = address_space() + 6 * 1024 * 1024
    resource.setrlimit(resource.RLIMIT_AS, (limit, limit))
    try:
        if batch is None:
            qf.CandleBatch.from_numpy(**given)
        else:
            batch.to_numpy()
    except BaseException as error:
        print(type(error).__name__, "|", isinstance(error, MemoryError), "|", error)
    else:
        print("nothing was raised")
    # The interpreter is as it was: the package still works.
    print(qf.CandleBatch.from_numpy(**small).to_numpy()["close"].tolist())
"""

# An address-space limit is how these tests run out of memory, and how they
# read the space in use is Linux's.
on_linux = pytest.mark.skipif(
    not sys.platform.startswith("linux"), reason="needs /proc and RLIMIT_AS"
)


@on_linux
def test_where_numpy_has_no_memory_for_an_array_to_numpy_raises_numpys_memory_error():
    ran = subprocess.run(
        [sys.executable, "-c", textwrap.dedent(OUT_OF_MEMORY), "to_numpy"],
        capture_output=True,
        text=True,
        timeout=120,
    )
    assert ran.returncode == 0, ran.stderr
    raised, still_works = ran.stdout.splitlines()
    # NumPy's own error, which is a MemoryError, and no panic of the
    # extension's.
    kind, is_memory_error, message = raised.split(" | ")
    assert (kind, is_memory_error) == ("MemoryError", "True"), raised
    assert message.startswith("Unable to allocate "), raised
    assert still_works == "[1.5, 1.5, 1.5, 1.5]"
    assert "panic" not in ran.stderr.lower()


@on_linux
def test_where_there_is_no_memory_for_a_batch_from_numpy_raises_a_memory_error():
    ran = subprocess.run(
        [sys.executable, "-c", textwrap.dedent(OUT_OF_MEMORY), "from_numpy"],
        capture_output=True,
        text=True,
        timeout=120,
    )
    # The process is not stopped: it is told, and goes on.
    assert ran.returncode == 0, ran.stderr
    raised, still_works = ran.stdout.splitlines()
    assert raised == "MemoryError | True | no memory for 2000000 rows of open_time_ms"
    assert still_works == "[1.5, 1.5, 1.5, 1.5]"
    assert "memory allocation of" not in ran.stderr


def test_one_array_may_be_given_for_several_columns():
    price = np.array([1.5, 2.5, 3.5, 4.5])
    batch = qf.CandleBatch.from_numpy(
        **arrays_of(open=price, high=price, low=price, close=price, volume=price)
    )
    for column in DECIMALS:
        assert getattr(batch, column) == tuple(map(Decimal, ["1.5", "2.5", "3.5", "4.5"]))


def test_a_batch_of_arrays_is_the_batch_of_their_values():
    given = arrays_of(
        open=np.array([100.1, 100.0, 99.5, 0.1]),
        volume=np.array([0.17235, 2.0, 0.0, 1e-8]),
        trades=np.ma.MaskedArray(np.array([5, 0, 7, 9], dtype=np.uint64), mask=[0, 1, 0, 0]),
    )
    batch = qf.CandleBatch.from_numpy(**given)
    assert type(batch) is qf.CandleBatch and len(batch) == 4
    assert batch.market is None
    expected = qf.CandleBatch.from_columns(
        open_time_ms=[0, 60_000, 120_000, 180_000],
        close_time_ms=[59_999, 119_999, 179_999, 239_999],
        open=[Decimal("100.1"), Decimal("100"), Decimal("99.5"), Decimal("0.1")],
        **dict.fromkeys(["high", "low", "close"], [Decimal("1.5")] * 4),
        volume=[Decimal("0.17235"), Decimal("2"), Decimal("0"), Decimal("0.00000001")],
        trades=[5, None, 7, 9],
    )
    assert batch == expected
    # Digit for digit: a float's shortest text has no decimal place it does
    # not need, and no exponent.
    assert [format(value, "f") for value in batch.volume] == ["0.17235", "2", "0", "0.00000001"]


# --- there and back ---------------------------------------------------------


def test_a_batch_comes_back_from_its_arrays_equal_where_its_decimals_have_fifteen_digits():
    drawn = random.Random(15)
    rows = 500

    def fifteen() -> Decimal:
        """A decimal of at most fifteen significant digits."""
        digits = drawn.randint(1, 15)
        whole = drawn.randrange(10**digits)
        return Decimal(whole).scaleb(-drawn.randint(0, 20)) * drawn.choice([1, -1])

    batch = qf.CandleBatch.from_columns(
        open_time_ms=[drawn.randint(I64_MIN, I64_MAX) for _ in range(rows)],
        close_time_ms=[drawn.randint(I64_MIN, I64_MAX) for _ in range(rows)],
        **{column: [fifteen() for _ in range(rows)] for column in DECIMALS},
        trades=[drawn.choice([None, drawn.randint(0, U64_MAX)]) for _ in range(rows)],
    )
    back = qf.CandleBatch.from_numpy(**batch.to_numpy())
    # Equal by value in every cell, which is how decimals are compared.
    assert back == batch
    for column in COLUMNS:
        assert getattr(back, column) == getattr(batch, column), column


def test_a_decimals_places_do_not_come_back_from_a_float():
    batch = batch_of(
        open=[Decimal("81940.01000000"), Decimal("100.00"), Decimal("0.17235000")],
        volume=[Decimal("0.00000000"), Decimal("1.50"), Decimal("2")],
    )
    back = qf.CandleBatch.from_numpy(**batch.to_numpy())
    # The same prices and the same volumes.
    assert back == batch
    # Written with the places a float's shortest text has, which are not
    # the ones the exchange wrote.
    assert [str(value) for value in batch.open] == ["81940.01000000", "100.00", "0.17235000"]
    assert [str(value) for value in back.open] == ["81940.01", "100", "0.17235"]
    assert [str(value) for value in back.volume] == ["0", "1.5", "2"]


def test_a_decimal_of_more_digits_than_a_float_holds_comes_back_as_its_nearest_float():
    long = [
        Decimal("66633140290.84541427"),
        Decimal("0.1234567890123456789"),
        Decimal("12345678901234567890"),
        Decimal(2**53 + 1),
    ]
    batch = batch_of(len(long), high=long)
    back = qf.CandleBatch.from_numpy(**batch.to_numpy())
    assert back != batch
    for came, went in zip(back.high, long, strict=True):
        assert came != went
        # It is the shortest text of the float nearest to what went in.
        assert came == shortest(nearest(went))
        assert abs(came - went) / abs(went) < Decimal("1e-15")
    # One more crossing changes nothing: a float comes back the same float.
    again = qf.CandleBatch.from_numpy(**back.to_numpy())
    assert again == back
    assert bits_of(again.to_numpy()["high"]) == bits_of(batch.to_numpy()["high"])


def test_a_float_comes_back_the_same_float_bit_for_bit():
    drawn = random.Random(64)
    floats = [0.1, 1 / 3, 81940.01, 5e-13, 1e21, 2.0**53 + 2, 4.9e-13, 7.9e28]
    floats += [drawn.random() * 10.0 ** drawn.randint(-12, 28) for _ in range(800)]
    floats += [-value for value in floats]
    floats = [value for value in floats if reason_refused(value) is None]
    assert len(floats) > 1_200
    array = np.array(floats)
    back = qf.CandleBatch.from_numpy(**arrays_of(len(floats), close=array)).to_numpy()["close"]
    assert bits_of(back) == bits_of(array)


def test_the_largest_decimals_cross_to_a_float_that_does_not_cross_back(refused):
    # 2^96 - 1 is nearest to the float 2^96, which is beyond a decimal.
    batch = batch_of(1, volume=[Decimal(DECIMAL_MAX)])
    array = batch.to_numpy()["volume"]
    assert array.tolist() == [2.0**96]
    message = (
        "volume[0]: cannot convert 7.922816251426434e28 to a decimal: beyond the decimal range"
    )
    refused(ValueError, message, qf.CandleBatch.from_numpy, **batch.to_numpy())
    # The largest that comes back is the float below it, as its shortest
    # text: sixteen digits and thirteen zeros.
    below = float(np.nextafter(2.0**96, 0.0))
    back = qf.CandleBatch.from_numpy(**arrays_of(1, volume=np.array([below])))
    assert back.volume == (Decimal("79228162514264330000000000000"),)
    assert back.volume == (shortest(below),)
    assert back.to_numpy()["volume"].tolist() == [below]


# --- numpy itself -------------------------------------------------------------


def test_the_numpy_of_the_run_is_one_the_package_takes(pyproject, oldest_numpy):
    from packaging.requirements import Requirement
    from packaging.version import Version

    # NumPy is the package's one extra, and the extra requires NumPy alone.
    (required,) = map(Requirement, pyproject["project"]["optional-dependencies"]["numpy"])
    assert required.name == "numpy"
    assert required.specifier.contains(np.__version__), np.__version__
    # The tests run once on the oldest it takes, which a group pins: that
    # version is the extra's own lower bound. Its other bound is the first
    # NumPy it does not take.
    (pinned,) = map(Requirement, pyproject["dependency-groups"]["oldest-numpy"])
    bound, following = sorted(required.specifier, key=lambda bound: Version(bound.version))
    (pin,) = pinned.specifier
    assert (pinned.name, pin.operator) == ("numpy", "==")
    assert (bound.operator, following.operator) == (">=", "<")
    assert Version(pin.version) == Version(bound.version)
    if oldest_numpy:
        assert Version(np.__version__) == Version(pin.version)
