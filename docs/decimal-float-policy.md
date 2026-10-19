# Decimal and float64

QuantForge keeps every price and volume as an exact decimal: a
`rust_decimal::Decimal`, a 96-bit integer with up to 28 decimal places.
Python strategies compute on NumPy float64 arrays. The two meet at the
Python interchange, `CandleBatch`, and this policy fixes how a value
crosses, so that a backtest gives the same result in Rust, in a local
Python process, and in a remote worker.

The policy is implemented once, by `quantforge::decimal_to_f64` and
`quantforge::decimal_from_f64`. Every conversion between the two types goes
through them — never through NumPy, `rust_decimal`'s own `to_f64` or
`from_f64`, or ad-hoc Python.

## Decimal to float64

- The result is the float64 nearest to the decimal's exact value. A value
  exactly halfway between two float64s goes to the one whose last bit is
  even (IEEE 754 round half to even).
- Zero of either sign becomes `+0.0`.
- Every decimal converts. Decimals stay within
  ±79228162514264337593543950335, far inside float64's range, so the result
  is never infinite or NaN.
- In Python this is `float(str(d))`, which is also what `float(d)` does for
  a `decimal.Decimal`, except that Python keeps the sign of a negative zero.

`rust_decimal`'s `to_f64` is not used: it rounds twice and misses the
nearest float64 for some decimals with 16 or more significant digits or
more than 22 decimal places, such as `66633140290.84541427` and
`0.0000000000000000000000000001`.

## Float64 to decimal

- The result is the decimal value of the shortest digit string that reads
  back as the same float64: Python's `repr`, Rust's `Display`. `0.1` becomes
  `0.1`, not `0.1000000000000000055511151231`.
- Trailing zeros are not restored: the decimal `81940.01000000` becomes the
  float64 `81940.01`, which comes back as `81940.01`.
- Negative zero becomes `0`.
- NaN, `inf`, and `-inf` are errors.
- A float64 whose shortest form lies beyond ±79228162514264337593543950335,
  or needs more than 28 decimal places, is an error and is never rounded to
  fit. That covers every float64 whose shortest form is below 1e-28 in
  magnitude, and some below 1e-12 whose shortest form has many digits;
  round them in Python first if they are intended.
- In Python this is `Decimal(repr(x))`, with the same zero handling and the
  same errors.

## Values that never become float64

- `open_time_ms` and `close_time_ms` are int64 milliseconds since the Unix
  epoch, UTC.
- `trades` is an unsigned trade count, or null when the exchange did not
  report it. A null is never written as NaN, 0, or -1.

## What survives a round trip

- Float64 to decimal and back returns the same float64, bit for bit,
  whenever the first step succeeds. A negative zero comes back positive.
- Decimal to float64 and back returns the same value for every decimal with
  at most 15 significant digits. A Binance price or volume with 8 decimal
  places qualifies when it has at most 7 digits before the point. Longer
  decimals come back as the shortest form of their nearest float64 — except
  those of magnitude 79228162514264333195497439232 or more, the decimal
  maximum included, which round to 2^96, whose shortest form is beyond the
  decimal range and is refused.
- `Vec<Candle>` to `CandleBatch` and back converts nothing, so it is exact,
  trailing zeros included.

Compare decimals by value, never as strings: `81940.01000000` and
`81940.01` are the same price.

## Parity

Float64 arithmetic in a strategy can differ from decimal arithmetic in the
engine, so parity fixtures store exact decimal strings, and results are
compared as decimals, never as float64s. A remote worker converts with the
same functions, through the same bindings, as a local run. It must also pin
the NumPy version used locally, so that float64 arithmetic in strategies
matches too.

## Conformance cases

`tests/fixtures/decimal_float_cases.csv` pins these rules case by case. Each
row has a `direction` (`to_f64` or `from_f64`), an `input`, the `expected`
result, and a `note`. A float64 is written as its IEEE 754 bit pattern in 16
hex digits, a decimal as an exact string, and an error as `error:` followed
by its reason. The expected values were computed with Python's correctly
rounded `float()` and `repr()`, not with the Rust functions.
`tests/decimal_float.rs` checks every row, and the Python bindings' tests
must check the same file. Rust prints a decimal without an exponent and
compares strings; Python compares `from_f64` results by value, because
`str(Decimal(repr(1e21)))` is `'1E+21'`.
