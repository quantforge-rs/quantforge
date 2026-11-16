"""`Candle`, one bar, and `CandleBatch`, bars held column by column.

A candle holds what it is given: its times as integers, its prices and its
volume as exact decimals, and a trade count that may be unknown. A batch
splits candles into columns and gives them back without converting a
value."""

from decimal import Decimal

import pytest

import quantforge_py as qf

I64_MIN, I64_MAX = -(2**63), 2**63 - 1
U64_MAX = 2**64 - 1

# One candle's fields, as the tests build on them.
FIELDS = dict(
    open_time_ms=1_700_000_000_000,
    close_time_ms=1_700_000_059_999,
    open=Decimal("81940.01000000"),
    high=Decimal("81990.5"),
    low=Decimal("81900"),
    close=Decimal("81955.25"),
    volume=Decimal("0.17235000"),
    trades=412,
)

# For each field, a value that is not the one above.
OTHER = dict(
    open_time_ms=1_700_000_060_000,
    close_time_ms=1_700_000_119_999,
    open=Decimal("81940.02"),
    high=Decimal("81990.6"),
    low=Decimal("81899"),
    close=Decimal("81955.26"),
    volume=Decimal("0.17235001"),
    trades=413,
)

COLUMNS = ["open_time_ms", "close_time_ms", "open", "high", "low", "close", "volume", "trades"]
DECIMALS = ["open", "high", "low", "close", "volume"]


def candle(**changed) -> qf.Candle:
    return qf.Candle(**{**FIELDS, **changed})


def minute(index: int, close: str, trades=None) -> qf.Candle:
    """The `index`th one-minute bar of a small series, flat at `close`."""
    return qf.Candle(
        open_time_ms=index * 60_000,
        close_time_ms=index * 60_000 + 59_999,
        open=Decimal(close),
        high=Decimal(close),
        low=Decimal(close),
        close=Decimal(close),
        volume=Decimal("1.50"),
        trades=trades,
    )


def exact(value):
    """A value as it is, a decimal down to its digits and decimal places."""
    return value.as_tuple() if isinstance(value, Decimal) else value


def test_a_candle_gives_back_every_field_as_it_was_given():
    made = candle()
    for field, value in FIELDS.items():
        back = getattr(made, field)
        assert type(back) is type(value), field
        assert exact(back) == exact(value), field


def test_a_trade_count_the_exchange_did_not_report_stays_unknown():
    unknown = candle(trades=None)
    assert unknown.trades is None
    # Unknown is what a candle is built with when no count is given.
    without = {field: value for field, value in FIELDS.items() if field != "trades"}
    assert qf.Candle(**without).trades is None
    assert qf.Candle(**without) == unknown
    # It is not zero, which is a count.
    assert candle(trades=0).trades == 0
    assert unknown != candle(trades=0)


def test_a_candle_is_kept_as_given_and_never_checked_for_making_a_bar():
    # No bar looks like this: low above high, a negative price and volume,
    # and a close before the open. Checking candles is the engine's own
    # separate pass, and building one does not run it.
    odd = qf.Candle(
        open_time_ms=60_000,
        close_time_ms=0,
        open=Decimal("-1"),
        high=Decimal("1"),
        low=Decimal("2"),
        close=Decimal("0"),
        volume=Decimal("-5"),
    )
    assert (odd.open_time_ms, odd.close_time_ms) == (60_000, 0)
    assert (odd.open, odd.high, odd.low, odd.close, odd.volume) == (-1, 1, 2, 0, -5)


def test_candles_are_equal_when_every_field_is_decimals_by_value():
    assert candle() == candle()
    # 81940.01000000 and 81940.01 are one price.
    assert candle() == candle(open=Decimal("81940.01"), volume=Decimal("0.17235"))
    for field, other in OTHER.items():
        assert candle() != candle(**{field: other}), field
    assert candle() != FIELDS
    assert candle() != tuple(FIELDS.values())


def test_a_candle_is_not_a_key(evaluated):
    with pytest.raises(TypeError, match="unhashable"):
        hash(candle())
    with pytest.raises(TypeError, match="unhashable"):
        {candle()}
    assert evaluated(repr(candle())) == candle()


def test_a_candles_repr_evaluates_back_to_it_digit_for_digit(evaluated):
    assert repr(candle(trades=None)) == (
        "Candle(open_time_ms=1700000000000, close_time_ms=1700000059999, "
        "open=Decimal('81940.01000000'), high=Decimal('81990.5'), low=Decimal('81900'), "
        "close=Decimal('81955.25'), volume=Decimal('0.17235000'), trades=None)"
    )
    for made in [candle(), candle(trades=None), candle(trades=0), minute(3, "0.00000001")]:
        back = evaluated(repr(made))
        assert back == made
        for field in COLUMNS:
            assert exact(getattr(back, field)) == exact(getattr(made, field)), field


def test_a_candle_cannot_be_changed_or_extended():
    made = candle()
    for field in COLUMNS:
        with pytest.raises(AttributeError):
            setattr(made, field, FIELDS[field])
    with pytest.raises(AttributeError):
        made.note = "mine"


def test_a_candles_fields_are_named_and_all_but_the_trade_count_are_required():
    with pytest.raises(TypeError, match="takes 0 positional arguments"):
        qf.Candle(*FIELDS.values())
    for field in COLUMNS:
        without = {name: value for name, value in FIELDS.items() if name != field}
        if field == "trades":
            assert qf.Candle(**without).trades is None
        else:
            with pytest.raises(TypeError, match=f"missing 1 required keyword argument: '{field}'"):
                qf.Candle(**without)
    with pytest.raises(TypeError, match="unexpected keyword argument"):
        candle(quote_volume=Decimal("1"))


def test_a_time_is_an_int_of_64_bits(refused):
    for field in ["open_time_ms", "close_time_ms"]:
        for whole in [I64_MIN, -1, 0, I64_MAX]:
            assert getattr(candle(**{field: whole}), field) == whole
        for whole in [I64_MAX + 1, I64_MIN - 1, 2**200]:
            message = f"invalid {field} {whole}: must be between {I64_MIN} and {I64_MAX}"
            refused(ValueError, message, candle, **{field: whole})
        for value, named in [
            (1_700_000_000_000.0, "float"),
            ("1700000000000", "str"),
            (Decimal("1700000000000"), "Decimal"),
            (None, "NoneType"),
            (True, "bool"),
        ]:
            refused(TypeError, f"{field} must be an int, not {named}", candle, **{field: value})


def test_a_trade_count_is_an_int_of_64_bits_that_is_not_negative_or_none(refused):
    for whole in [0, 1, U64_MAX]:
        assert candle(trades=whole).trades == whole
    for whole in [-1, U64_MAX + 1]:
        message = f"invalid trades {whole}: must be between 0 and {U64_MAX}"
        refused(ValueError, message, candle, trades=whole)
    for value, named in [
        (412.0, "float"),
        ("412", "str"),
        (Decimal("412"), "Decimal"),
        (False, "bool"),
    ]:
        refused(TypeError, f"trades must be an int or None, not {named}", candle, trades=value)


def test_a_number_that_says_it_is_whole_is_taken_for_an_int():
    class Count:
        def __index__(self):
            return 7

    made = candle(open_time_ms=Count(), trades=Count())
    assert (made.open_time_ms, made.trades) == (7, 7)


def test_a_batch_splits_candles_into_columns_and_gives_them_back():
    candles = [minute(0, "100.10", trades=5), minute(1, "100.00"), minute(2, "99.5", trades=0)]
    batch = qf.CandleBatch.from_candles(candles)
    assert len(batch) == 3
    assert batch.open_time_ms == (0, 60_000, 120_000)
    assert batch.close_time_ms == (59_999, 119_999, 179_999)
    assert batch.trades == (5, None, 0)
    for column in ["open", "high", "low", "close"]:
        assert getattr(batch, column) == (Decimal("100.10"), Decimal("100.00"), Decimal("99.5"))
    assert batch.volume == (Decimal("1.50"),) * 3
    back = batch.to_candles()
    assert type(back) is list
    assert back == candles


def test_a_batch_converts_no_value_on_the_way_in_or_out():
    candles = [candle(), candle(trades=None), minute(7, "0.00000001"), minute(8, "100.00")]
    batch = qf.CandleBatch.from_candles(candles)
    for column in COLUMNS:
        values = getattr(batch, column)
        assert type(values) is tuple
        for value, made in zip(values, candles, strict=True):
            assert type(value) is type(getattr(made, column)), column
            assert exact(value) == exact(getattr(made, column)), column
    for back, made in zip(batch.to_candles(), candles, strict=True):
        assert type(back) is qf.Candle
        for column in COLUMNS:
            assert exact(getattr(back, column)) == exact(getattr(made, column)), column


def test_a_batch_of_columns_is_the_batch_of_their_candles():
    candles = [minute(0, "100.10", trades=5), minute(1, "100.00"), minute(2, "99.5", trades=0)]
    columns = {column: [getattr(made, column) for made in candles] for column in COLUMNS}
    batch = qf.CandleBatch.from_columns(**columns)
    assert batch == qf.CandleBatch.from_candles(candles)
    assert batch.to_candles() == candles
    for column in COLUMNS:
        assert getattr(batch, column) == tuple(columns[column])
    # And a batch's own columns make the batch again.
    again = qf.CandleBatch.from_columns(**{column: getattr(batch, column) for column in COLUMNS})
    assert again == batch


def test_a_batch_takes_anything_that_iterates_its_candles_or_a_columns_values():
    candles = [minute(0, "100"), minute(1, "101")]
    batch = qf.CandleBatch.from_candles(candles)
    assert qf.CandleBatch.from_candles(tuple(candles)) == batch
    assert qf.CandleBatch.from_candles(made for made in candles) == batch
    assert qf.CandleBatch.from_candles(iter(candles)) == batch

    def values(column):
        return (getattr(made, column) for made in candles)

    assert qf.CandleBatch.from_columns(**{column: values(column) for column in COLUMNS}) == batch


def test_a_batch_keeps_the_order_it_is_given_and_checks_nothing_of_it():
    # Out of order and with one bar twice: a batch holds candles, and
    # checking them is the engine's own separate pass.
    candles = [minute(2, "99"), minute(0, "100"), minute(0, "100")]
    batch = qf.CandleBatch.from_candles(candles)
    assert batch.open_time_ms == (120_000, 0, 0)
    assert batch.to_candles() == candles


def test_an_empty_batch_holds_no_candles():
    empty = qf.CandleBatch.from_candles([])
    assert len(empty) == 0
    assert not empty
    assert empty.to_candles() == []
    for column in COLUMNS:
        assert getattr(empty, column) == ()
    assert empty == qf.CandleBatch.from_columns(**dict.fromkeys(COLUMNS, ()))
    assert qf.CandleBatch.from_candles([minute(0, "100")])


def test_batches_are_equal_when_their_candles_are_and_are_not_keys():
    one = qf.CandleBatch.from_candles([minute(0, "100.00"), minute(1, "101")])
    assert one == qf.CandleBatch.from_candles([minute(0, "100"), minute(1, "101.0")])
    assert one != qf.CandleBatch.from_candles([minute(1, "101"), minute(0, "100")])
    assert one != qf.CandleBatch.from_candles([minute(0, "100")])
    assert one != qf.CandleBatch.from_candles([minute(0, "100"), minute(1, "101", trades=0)])
    assert one != [minute(0, "100.00"), minute(1, "101")]
    with pytest.raises(TypeError, match="unhashable"):
        hash(one)


def test_a_batchs_repr_counts_its_candles():
    assert repr(qf.CandleBatch.from_candles([])) == "CandleBatch(<0 candles>)"
    assert repr(qf.CandleBatch.from_candles([minute(0, "1")])) == "CandleBatch(<1 candle>)"
    two = qf.CandleBatch.from_candles([minute(0, "1"), minute(1, "1")])
    assert repr(two) == "CandleBatch(<2 candles>)"


def test_a_column_of_another_length_is_refused_in_the_engines_words(refused):
    full = {column: [FIELDS[column]] * 3 for column in COLUMNS}
    # Every column is held to the open times' length, and the first that
    # differs is named, in the order the columns are given.
    for column in COLUMNS[1:]:
        for found in [2, 4, 0]:
            columns = {**full, column: [FIELDS[column]] * found}
            message = f"candle column `{column}` has {found} rows, expected 3"
            refused(ValueError, message, qf.CandleBatch.from_columns, **columns)
    short = {**full, "open_time_ms": [FIELDS["open_time_ms"]] * 2}
    message = "candle column `close_time_ms` has 3 rows, expected 2"
    refused(ValueError, message, qf.CandleBatch.from_columns, **short)
    both = {**full, "volume": [FIELDS["volume"]] * 2, "high": [FIELDS["high"]] * 1}
    message = "candle column `high` has 1 rows, expected 3"
    refused(ValueError, message, qf.CandleBatch.from_columns, **both)


def test_a_value_a_column_cannot_hold_is_refused_by_its_row(refused):
    full = {column: [FIELDS[column]] * 3 for column in COLUMNS}

    def with_value(column, value):
        return {**full, column: [FIELDS[column], value, FIELDS[column]]}

    built = qf.CandleBatch.from_columns
    for column in DECIMALS:
        refused(
            TypeError,
            f"{column}[1] must be a decimal.Decimal or an int, not float: a float is never read "
            "as a decimal; convert it with decimal_from_f64",
            built,
            **with_value(column, 81940.01),
        )
        refused(
            ValueError,
            f"invalid {column}[1] NaN: must be a finite number",
            built,
            **with_value(column, Decimal("NaN")),
        )
        refused(
            TypeError,
            f"{column}[1] must be a decimal.Decimal or an int, not NoneType",
            built,
            **with_value(column, None),
        )
    for column in ["open_time_ms", "close_time_ms"]:
        refused(
            TypeError,
            f"{column}[1] must be an int, not NoneType",
            built,
            **with_value(column, None),
        )
        refused(
            ValueError,
            f"invalid {column}[1] {I64_MAX + 1}: must be between {I64_MIN} and {I64_MAX}",
            built,
            **with_value(column, I64_MAX + 1),
        )
    refused(
        TypeError,
        "trades[1] must be an int or None, not float",
        built,
        **with_value("trades", 412.0),
    )
    refused(
        ValueError,
        f"invalid trades[1] -1: must be between 0 and {U64_MAX}",
        built,
        **with_value("trades", -1),
    )


def test_a_column_is_something_that_iterates_values_and_is_not_text(refused):
    full = {column: [FIELDS[column]] * 3 for column in COLUMNS}
    for column in COLUMNS:
        for value, named in [(5, "int"), (None, "NoneType"), ("123", "str")]:
            refused(
                TypeError,
                f"{column} must be an iterable of values, not {named}",
                qf.CandleBatch.from_columns,
                **{**full, column: value},
            )
    with pytest.raises(TypeError, match="takes 0 positional arguments"):
        qf.CandleBatch.from_columns(*full.values())
    for column in COLUMNS:
        without = {name: values for name, values in full.items() if name != column}
        with pytest.raises(TypeError, match=f"missing 1 required keyword argument: '{column}'"):
            qf.CandleBatch.from_columns(**without)


def test_a_batch_is_made_of_candles_and_nothing_else(refused):
    built = qf.CandleBatch.from_candles
    refused(TypeError, "candles must be an iterable of Candle, not int", built, 5)
    refused(TypeError, "candles must be an iterable of Candle, not NoneType", built, None)
    for value, named in [(FIELDS, "dict"), (None, "NoneType"), (5, "int")]:
        refused(
            TypeError,
            f"candles[1] must be a Candle, not {named}",
            built,
            [candle(), value, candle()],
        )
    # Text iterates too, one character at a time, and is never candles.
    for text in ["candles", ""]:
        refused(TypeError, "candles must be an iterable of Candle, not str", built, text)
    with pytest.raises(TypeError):
        built()
    with pytest.raises(TypeError):
        built(candles=[candle()])


class Opens:
    """A source of values that fails as it is opened: its `__iter__` raises."""

    def __init__(self, raised):
        self.raised = raised

    def __iter__(self):
        raise self.raised


class RunsDry:
    """A source of values that fails after the first of them."""

    def __init__(self, first, raised):
        self.first, self.raised, self.read = first, raised, 0

    def __iter__(self):
        return self

    def __next__(self):
        self.read += 1
        if self.read == 1:
            return self.first
        raise self.raised


class Unlisted:
    """A value that says it does not iterate."""

    __iter__ = None


@pytest.mark.parametrize(
    "raised",
    [
        OSError("the store is closed"),
        LookupError("no such range"),
        TypeError("the source's own"),
        MemoryError(),
        KeyboardInterrupt(),
    ],
    ids=lambda raised: type(raised).__name__,
)
def test_what_reading_a_batchs_values_raises_is_raised_as_it_is(raised):
    full = {column: [FIELDS[column]] * 3 for column in COLUMNS}
    for build in [
        lambda: qf.CandleBatch.from_candles(Opens(raised)),
        lambda: qf.CandleBatch.from_candles(RunsDry(candle(), raised)),
        lambda: qf.CandleBatch.from_columns(**{**full, "open": Opens(raised)}),
        lambda: qf.CandleBatch.from_columns(**{**full, "trades": RunsDry(1, raised)}),
    ]:
        # The source's own error, the object it raised: it is no value of
        # the wrong type, and an interrupt is an interrupt.
        with pytest.raises(type(raised)) as caught:
            build()
        assert caught.value is raised


def test_a_value_that_does_not_iterate_is_of_the_wrong_type(refused):
    full = {column: [FIELDS[column]] * 3 for column in COLUMNS}
    message = "candles must be an iterable of Candle, not Unlisted"
    refused(TypeError, message, qf.CandleBatch.from_candles, Unlisted())
    message = "low must be an iterable of values, not Unlisted"
    refused(TypeError, message, qf.CandleBatch.from_columns, **{**full, "low": Unlisted()})


def test_a_batch_is_built_from_candles_or_columns_and_cannot_be_changed():
    with pytest.raises(TypeError, match="cannot create"):
        qf.CandleBatch()
    batch = qf.CandleBatch.from_candles([candle()])
    for column in COLUMNS:
        with pytest.raises(AttributeError):
            setattr(batch, column, ())
    with pytest.raises(AttributeError):
        batch.market = "binance_spot"
    # A column read is the reader's own: changing what it was built from
    # changes nothing in the batch.
    opens = [Decimal("1"), Decimal("2")]
    columns = {column: [FIELDS[column]] * 2 for column in COLUMNS}
    built = qf.CandleBatch.from_columns(**{**columns, "open": opens})
    opens[0] = Decimal("9")
    assert built.open == (Decimal("1"), Decimal("2"))
