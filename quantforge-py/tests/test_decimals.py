"""How a decimal crosses between Python and the engine, and the one way a
float does.

A price, a quantity or an amount is a `decimal.Decimal` on the Python side
and the engine's exact decimal on the other: 96 bits of digits with at most
28 decimal places. It crosses with every digit and every decimal place, or
it is refused; a float is never read as one. `decimal_to_f64` and
`decimal_from_f64` are the engine's two conversions, held here to the cases
the engine's own tests hold them to (docs/decimal-float-policy.md)."""

import csv
import decimal
import fractions
import math
import random
import struct
import sys
import tracemalloc
from decimal import Decimal

import pytest

import quantforge_py as qf

# The largest number the engine's decimal holds: 2^96 - 1.
LARGEST = 79228162514264337593543950335

FITS = "must fit a 96-bit decimal with at most 28 decimal places"

# Decimals that cross as they are: the same sign, digits and decimal places
# on the way back.
KEPT = [
    "0",
    "1",
    "-1",
    "0.1",
    "-2.5",
    # Trailing zeros are decimal places like any other.
    "1.10",
    "81940.01000000",
    "0.17235000",
    "100.00",
    "0.00",
    "0.00000001",
    # 28 decimal places, the most there are.
    "0.0000000000000000000000000001",
    "0.0000000000000000000000000000",
    "1.0000000000000000000000000000",
    # 96 bits of digits, with the point at either end and in between.
    "79228162514264337593543950335",
    "-79228162514264337593543950335",
    "7.9228162514264337593543950335",
    "-7.9228162514264337593543950335",
    "792281625142643.37593543950335",
]

# Decimals that are a number the engine holds, written in a way it does not
# keep: the value they come back as, digit for digit.
WRITTEN_OUT = [
    # An exponent above zero is written out.
    ("1E+3", "1000"),
    ("1.5E+2", "150"),
    ("-2.50E+1", "-25.0"),
    ("7.9228162514264337593543950335E+28", "79228162514264337593543950335"),
    # Zero is zero whatever its exponent, and keeps the places it has.
    ("0E+3", "0"),
    ("0E+50", "0"),
    ("0E-3", "0.000"),
    # A negative zero becomes zero, as everywhere in the engine.
    ("-0", "0"),
    ("-0.00", "0.00"),
    ("-0E+2", "0"),
]

# Decimals of the right type that the engine cannot hold, with the rule each
# is refused by. The value is quoted as Python writes it.
REFUSED = [
    ("NaN", "must be a finite number"),
    ("-NaN", "must be a finite number"),
    ("sNaN", "must be a finite number"),
    ("NaN123", "must be a finite number"),
    ("Infinity", "must be a finite number"),
    ("-Infinity", "must be a finite number"),
    # A 29th decimal place, even one that holds a zero.
    ("1E-29", FITS),
    ("0.00000000000000000000000000001", FITS),
    ("0E-29", FITS),
    ("1.00000000000000000000000000000", FITS),
    # A 97th bit of digits, wherever the point is.
    ("79228162514264337593543950336", FITS),
    ("-79228162514264337593543950336", FITS),
    ("7.9228162514264337593543950336", FITS),
    ("1E+29", FITS),
    ("0.12345678901234567890123456789", FITS),
]


def crossed(value):
    """`value` after it has gone to the engine and come back."""
    return qf.EquityPoint(open_time_ms=0, equity=value).equity


def parts(value: Decimal):
    """A decimal's sign, digits and exponent: what `==` does not tell apart."""
    return value.as_tuple()


def bits(value: float) -> str:
    """A float as the fixture writes one: its IEEE 754 bits, in 16 hex digits."""
    return struct.pack(">d", value).hex()


def float_of(written: str) -> float:
    return struct.unpack(">d", bytes.fromhex(written))[0]


def test_a_decimal_crosses_with_every_digit_and_every_decimal_place():
    for text in KEPT:
        value = Decimal(text)
        back = crossed(value)
        assert type(back) is Decimal
        assert parts(back) == parts(value), text


def test_an_int_crosses_as_the_whole_number_it_is():
    for whole in [0, 1, -1, 10_000, LARGEST, -LARGEST]:
        back = crossed(whole)
        assert type(back) is Decimal
        assert parts(back) == parts(Decimal(whole)), whole


def test_an_exponent_is_written_out_and_a_negative_zero_becomes_zero():
    for text, written in WRITTEN_OUT:
        back = crossed(Decimal(text))
        assert parts(back) == parts(Decimal(written)), text
        assert back == Decimal(text), text


def test_no_decimal_context_rounds_a_value_on_its_way(refused):
    value = Decimal("123456789012345.6789012345678")
    with decimal.localcontext() as context:
        context.prec = 3
        context.rounding = decimal.ROUND_DOWN
        context.Emax = 5
        context.Emin = -5
        context.traps[decimal.Inexact] = True
        assert parts(crossed(value)) == parts(value)
        assert parts(crossed(Decimal("1E+3"))) == parts(Decimal("1000"))
        assert parts(qf.decimal_from_f64(0.30000000000000004)) == parts(
            Decimal("0.30000000000000004")
        )
        refused(ValueError, f"invalid equity 1E-29: {FITS}", crossed, Decimal("1E-29"))


def test_a_decimal_of_a_type_derived_from_decimal_crosses_and_comes_back_plain():
    class Price(Decimal):
        pass

    back = crossed(Price("81940.0100"))
    assert type(back) is Decimal
    assert parts(back) == parts(Decimal("81940.0100"))


def test_a_value_of_a_derived_type_crosses_by_the_number_it_holds_whatever_it_says(refused):
    class Shown(Decimal):
        """A price that writes itself to two places, and answers what the
        crossing asks of a decimal as if it were a small whole number."""

        def __format__(self, spec):
            return Decimal.__format__(self, ".2f")

        def __str__(self):
            return "a price"

        def __repr__(self):
            return "Shown()"

        def adjusted(self):
            return 0

        def is_zero(self):
            return True

        def is_finite(self):
            return True

    back = crossed(Shown("81940.0199"))
    assert type(back) is Decimal
    assert parts(back) == parts(Decimal("81940.0199"))
    assert bits(qf.decimal_to_f64(Shown("0.129"))) == bits(0.129)
    # It is refused for the number it holds as well, quoted as that number.
    refused(ValueError, f"invalid equity 1E-29: {FITS}", crossed, Shown("1E-29"))
    refused(ValueError, f"invalid equity 1E+29: {FITS}", crossed, Shown("1E+29"))
    refused(ValueError, "invalid equity NaN: must be a finite number", crossed, Shown("NaN"))

    class Counted(int):
        """A whole number that shifts to nothing, says it is seven, and
        writes itself in words."""

        def __rshift__(self, places):
            return 0

        def __and__(self, mask):
            return 0

        def __index__(self):
            return 7

        def __int__(self):
            return 7

        def __str__(self):
            return "seven"

        def __repr__(self):
            return "Counted()"

    large = 2**70 + 5
    assert parts(crossed(Counted(large))) == parts(Decimal(large))
    assert parts(crossed(Counted(-large))) == parts(Decimal(-large))
    refused(ValueError, f"invalid equity {2**96}: {FITS}", crossed, Counted(2**96))
    # A time is the number an int holds too, and is refused as that number.
    point = qf.EquityPoint(open_time_ms=Counted(1_700_000_000_000), equity=Counted(3))
    assert (point.open_time_ms, point.equity) == (1_700_000_000_000, 3)
    refused(
        ValueError,
        f"invalid open_time_ms {2**63}: must be between {-(2**63)} and {2**63 - 1}",
        qf.EquityPoint,
        open_time_ms=Counted(2**63),
        equity=1,
    )


def test_a_decimal_the_engine_cannot_hold_is_refused_and_never_rounded(refused):
    for text, rule in REFUSED:
        value = Decimal(text)
        refused(ValueError, f"invalid equity {value}: {rule}", crossed, value)
    for whole in [LARGEST + 1, -LARGEST - 1, 2**127, -(2**127) - 1, 2**200]:
        refused(ValueError, f"invalid equity {whole}: {FITS}", crossed, whole)


def test_a_refusal_names_the_field_it_refuses():
    candle = dict(open_time_ms=0, close_time_ms=59_999, open=1, high=1, low=1, close=1, volume=1)
    for field in ["open", "high", "low", "close", "volume"]:
        with pytest.raises(ValueError) as caught:
            qf.Candle(**{**candle, field: Decimal("NaN")})
        assert str(caught.value) == f"invalid {field} NaN: must be a finite number"
        with pytest.raises(TypeError) as caught:
            qf.Candle(**{**candle, field: "1"})
        assert str(caught.value) == f"{field} must be a decimal.Decimal or an int, not str"


def test_a_decimal_with_an_exponent_far_out_of_range_is_refused_before_it_is_written_out(refused):
    # Written out, either would take fifty megabytes of text. The engine can
    # hold neither, which the exponent alone says.
    for text in ["1E+50000000", "-1E+50000000", "1E-50000000", "0E-50000000"]:
        value = Decimal(text)
        tracemalloc.start()
        try:
            refused(ValueError, f"invalid equity {value}: {FITS}", crossed, value)
            _, peak = tracemalloc.get_traced_memory()
        finally:
            tracemalloc.stop()
        assert peak < 1_000_000, f"{text} took {peak} bytes"
    # A zero with an exponent above zero is zero, and takes no more.
    assert parts(crossed(Decimal("0E+50000000"))) == parts(Decimal("0"))


def test_a_very_long_value_is_quoted_in_part(refused):
    long = 10**100
    refused(ValueError, f"invalid equity {'1' + '0' * 79}...: {FITS}", crossed, long)
    places = Decimal("0." + "1" * 100)
    refused(ValueError, f"invalid equity {'0.' + '1' * 78}...: {FITS}", crossed, places)


def test_an_int_too_long_for_python_to_write_is_refused_without_being_quoted(refused):
    limit = sys.get_int_max_str_digits()
    sys.set_int_max_str_digits(4300)
    try:
        refused(ValueError, f"invalid equity: {FITS}", crossed, 10**5000)
    finally:
        sys.set_int_max_str_digits(limit)


def test_a_float_is_never_read_as_a_decimal(refused):
    message = (
        "equity must be a decimal.Decimal or an int, not float: a float is never read as a "
        "decimal; convert it with decimal_from_f64"
    )
    for value in [0.1, 1.0, 0.0, float("nan"), float("inf")]:
        refused(TypeError, message, crossed, value)

    class Ratio(float):
        pass

    refused(TypeError, message, crossed, Ratio(0.5))
    # The conversion the message names is the way across.
    assert parts(crossed(qf.decimal_from_f64(0.1))) == parts(Decimal("0.1"))


def test_nothing_else_is_read_as_a_decimal_either(refused):
    for value, named in [
        ("100.5", "str"),
        (b"100.5", "bytes"),
        (True, "bool"),
        (False, "bool"),
        (None, "NoneType"),
        (1 + 0j, "complex"),
        (fractions.Fraction(1, 2), "Fraction"),
        ([1], "list"),
        ((1, (5,), -1), "tuple"),
    ]:
        message = f"equity must be a decimal.Decimal or an int, not {named}"
        refused(TypeError, message, crossed, value)


def test_a_number_that_says_it_is_whole_crosses_as_one():
    class Lots:
        def __index__(self):
            return 25

    assert parts(crossed(Lots())) == parts(Decimal("25"))


def cases(repository):
    """The shared cases: direction, input, expected and note, row by row."""
    path = repository / "tests" / "fixtures" / "decimal_float_cases.csv"
    with open(path, encoding="utf-8", newline="") as file:
        rows = list(csv.reader(file))
    assert rows[0] == ["direction", "input", "expected", "note"]
    assert all(len(row) == 4 for row in rows)
    return rows[1:]


def test_every_shared_case_of_the_float_policy_holds(repository, refused):
    to_f64 = from_f64 = 0
    for direction, written, expected, note in cases(repository):
        if direction == "to_f64":
            result = qf.decimal_to_f64(Decimal(written))
            assert type(result) is float
            assert bits(result) == expected, note
            to_f64 += 1
        elif expected.startswith("error:"):
            with pytest.raises(ValueError) as caught:
                qf.decimal_from_f64(float_of(written))
            assert type(caught.value) is ValueError
            assert str(caught.value).endswith(expected.removeprefix("error:")), note
            from_f64 += 1
        else:
            assert direction == "from_f64", direction
            result = qf.decimal_from_f64(float_of(written))
            assert type(result) is Decimal
            # By value, as the policy says, and digit for digit as well: the
            # engine writes no exponent.
            assert result == Decimal(expected), note
            assert format(result, "f") == expected, note
            assert parts(result) == parts(Decimal(expected)), note
            from_f64 += 1
    assert (to_f64, from_f64) == (17, 28)


def test_a_float_that_has_no_decimal_is_refused_in_the_engines_words(refused):
    for value, message in [
        (float("nan"), "cannot convert NaN to a decimal: not finite"),
        (float("inf"), "cannot convert inf to a decimal: not finite"),
        (float("-inf"), "cannot convert -inf to a decimal: not finite"),
        (5e-324, "cannot convert 5e-324 to a decimal: more than 28 decimal places"),
        (1e29, "cannot convert 1e29 to a decimal: beyond the decimal range"),
        (
            1.7976931348623157e308,
            "cannot convert 1.7976931348623157e308 to a decimal: beyond the decimal range",
        ),
    ]:
        refused(ValueError, message, qf.decimal_from_f64, value)


def test_the_two_conversions_take_what_they_convert_and_nothing_else(refused):
    for value, named in [
        (1, "int"),
        (Decimal("0.1"), "Decimal"),
        ("0.1", "str"),
        (None, "NoneType"),
    ]:
        refused(TypeError, f"value must be a float, not {named}", qf.decimal_from_f64, value)
    for value, named in [(0.1, "float"), ("0.1", "str"), (None, "NoneType"), (True, "bool")]:
        refused(
            TypeError,
            f"value must be a decimal.Decimal or an int, not {named}",
            qf.decimal_to_f64,
            value,
        )
    message = "invalid value NaN: must be a finite number"
    refused(ValueError, message, qf.decimal_to_f64, Decimal("NaN"))
    assert bits(qf.decimal_to_f64(3)) == bits(3.0)
    for function in [qf.decimal_to_f64, qf.decimal_from_f64]:
        with pytest.raises(TypeError):
            function()
        with pytest.raises(TypeError):
            function(value=1)


def test_a_float_comes_back_bit_for_bit_and_python_agrees_on_the_way():
    # What the policy promises of a round trip, and what it says the two
    # conversions are in Python's own terms.
    for value in [0.1, 0.30000000000000004, 81940.01, -2.5, 1e21, 1e-7, 2.0**53, 5e-28]:
        exact = qf.decimal_from_f64(value)
        assert exact == Decimal(repr(value))
        assert bits(qf.decimal_to_f64(exact)) == bits(value)
        assert bits(qf.decimal_to_f64(exact)) == bits(float(str(exact)))
    assert bits(qf.decimal_to_f64(Decimal("-0"))) == bits(0.0)
    assert parts(qf.decimal_from_f64(-0.0)) == parts(Decimal("0"))


def test_a_float_halfway_between_two_shortest_texts_gives_the_one_python_writes():
    # Such a float has two shortest texts, equally near it, and both read
    # back as it. Python's `repr` writes the one whose last digit is even,
    # and so does the engine, in whichever direction that one lies.
    for value, text, other in [
        (790101343014070.25, "790101343014070.2", "790101343014070.3"),
        (790101343014070.75, "790101343014070.8", "790101343014070.7"),
        (-562949953421312.25, "-562949953421312.2", "-562949953421312.3"),
        (87838979419517.125, "87838979419517.12", "87838979419517.13"),
        (82957.940185546875, "82957.94018554688", "82957.94018554687"),
        (0.0210056304931640625, "0.021005630493164062", "0.021005630493164063"),
    ]:
        exact = fractions.Fraction(value)
        # The two are equally near, and both read back as the float.
        assert abs(fractions.Fraction(text) - exact) == abs(fractions.Fraction(other) - exact)
        assert float(text) == float(other) == value
        assert repr(value) == text
        assert format(qf.decimal_from_f64(value), "f") == text
        assert int(text[-1]) % 2 == 0 and int(other[-1]) % 2 == 1


def test_next_to_a_power_of_two_a_text_that_does_not_read_back_is_never_taken():
    # The floats below a power of two lie twice as close as those above it.
    # 2^-24 is exactly halfway between two texts of sixteen digits, and only
    # the larger reads back as it: the smaller, whose last digit is even,
    # reads back as the float below.
    value = 2.0**-24
    larger, smaller = "0.00000005960464477539063", "0.00000005960464477539062"
    exact = fractions.Fraction(value)
    assert abs(fractions.Fraction(larger) - exact) == abs(fractions.Fraction(smaller) - exact)
    assert float(larger) == value and float(smaller) != value
    assert format(Decimal(repr(value)), "f") == larger
    assert format(qf.decimal_from_f64(value), "f") == larger
    assert format(qf.decimal_from_f64(-value), "f") == "-" + larger
    # And so for every power of two a decimal holds, and its neighbours.
    compared = 0
    for power in range(-93, 96):
        for value in [
            2.0**power,
            math.nextafter(2.0**power, 0.0),
            math.nextafter(2.0**power, 2.0**97),
        ]:
            written = Decimal(repr(value))
            if -written.as_tuple().exponent > 28 or abs(written) > 2**96 - 1:
                continue
            assert qf.decimal_from_f64(value) == written, power
            assert bits(qf.decimal_to_f64(qf.decimal_from_f64(value))) == bits(value), power
            compared += 1
    assert compared > 400


def test_the_decimal_of_a_float_is_the_number_pythons_repr_writes():
    drawn = random.Random(475)
    halfway = compared = 0
    for _ in range(20_000):
        # Floats with a short binary fraction, where the ties are, and
        # floats of every size a decimal holds.
        if drawn.random() < 0.5:
            fraction_bits = drawn.randint(1, 40)
            whole = drawn.randrange(2 ** drawn.randint(1, 52))
            odd = 2 * drawn.randrange(2 ** (fraction_bits - 1)) + 1
            value = float(whole) + odd / 2.0**fraction_bits
        else:
            value = drawn.random() * 10.0 ** drawn.randint(-12, 28)
        value *= drawn.choice([1, -1])
        written = Decimal(repr(value))
        try:
            exact = qf.decimal_from_f64(value)
        except ValueError:
            # Beyond the decimal's range, or more than 28 places.
            assert abs(written) > 2**96 - 1 or -written.as_tuple().exponent > 28, value
            continue
        compared += 1
        assert exact == written, value
        assert bits(qf.decimal_to_f64(exact)) == bits(value), value
        # A neighbour of the same length that reads back as the float and
        # is exactly as near: a tie, which the even digit decides.
        unit = Decimal(1).scaleb(written.as_tuple().exponent)
        near = abs(fractions.Fraction(written) - fractions.Fraction(value))
        for neighbour in (written - unit, written + unit):
            as_near = abs(fractions.Fraction(neighbour) - fractions.Fraction(value)) == near
            if as_near and float(neighbour) == value:
                assert exact.as_tuple().digits[-1] % 2 == 0, value
                halfway += 1
    assert compared > 18_000
    assert halfway > 100
