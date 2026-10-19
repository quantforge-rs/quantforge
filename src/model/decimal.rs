//! Decimal helpers: step rounding for the exchange-rule rails, and the
//! decimal ↔ float64 conversions at the Python interchange.
//!
//! The conversions implement the policy in
//! <https://github.com/quantforge-rs/quantforge/blob/main/docs/decimal-float-policy.md>.
//! `rust_decimal`'s own `to_f64` and `from_f64` are deliberately not used:
//! `to_f64` rounds twice and misses the nearest float64 for some decimals
//! with 16 or more significant digits or more than 22 decimal places, and
//! `from_f64` does not promise the shortest decimal.

use super::ModelError;
use rust_decimal::Decimal;

/// Rounds `value` down to the nearest multiple of `step`.
///
/// `step` must be positive; passing a non-positive step is a caller bug.
/// Debug builds assert. Release builds return `value` unchanged so a bad
/// step can never manufacture a different quantity.
pub fn round_down_to_step(value: Decimal, step: Decimal) -> Decimal {
    debug_assert!(
        step > Decimal::ZERO,
        "round_down_to_step requires a positive step, got {step}"
    );
    if step <= Decimal::ZERO {
        return value;
    }
    (value / step).trunc() * step
}

/// The float64 nearest to `value`; a value exactly halfway between two
/// float64s goes to the one whose last bit is even. Zero of either sign
/// gives `+0.0`. Every decimal converts, and no result is NaN or infinite.
///
/// This is what Python's `float(str(d))` returns. It is computed in integer
/// arithmetic rather than by parsing the decimal's string, so it neither
/// allocates nor has a failure path.
pub fn decimal_to_f64(value: Decimal) -> f64 {
    let mantissa = value.mantissa().unsigned_abs();
    if mantissa == 0 {
        return 0.0;
    }
    let bits = |x: u128| u128::BITS - x.leading_zeros();
    let scale = value.scale();

    // value = mantissa / 10^scale = (mantissa / 5^scale) * 2^-scale. The
    // power of two is exact in binary, so only the quotient needs rounding.
    // Shifting the dividend keeps the quotient at least 55 bits wide (the 53
    // a float64 keeps, a half bit, and one more for the tie test), and the
    // dividend stays below 2^121: the mantissa is below 2^96, 5^28 below 2^66.
    let divisor = 5u128.pow(scale);
    let shift = (55 + bits(divisor)).saturating_sub(bits(mantissa));
    let dividend = mantissa << shift;
    let mut quotient = dividend / divisor;
    let remainder = dividend % divisor;

    // Round half to even: drop the bits below the 53 kept, rounding up above
    // the halfway point, and at it only when the kept bits are odd. A nonzero
    // remainder means the exact value lies past the bits in hand.
    let dropped = bits(quotient) - 53;
    let half = 1u128 << (dropped - 1);
    let low = quotient & ((1u128 << dropped) - 1);
    quotient >>= dropped;
    if low > half || (low == half && (remainder != 0 || quotient & 1 == 1)) {
        quotient += 1;
    }

    // The rounded quotient is at most 2^53, exact as a float64, and the
    // result lies between 2^-94 and 2^96, so scaling it by a power of two,
    // built from its exponent bits, is exact as well.
    let exponent = dropped as i32 - shift as i32 - scale as i32;
    let magnitude = quotient as f64 * f64::from_bits(((1023 + exponent) as u64) << 52);
    if value.is_sign_negative() {
        -magnitude
    } else {
        magnitude
    }
}

/// The decimal value of the shortest digit string that reads back as
/// `value`: Rust's `Display`, Python's `repr`. Negative zero gives `0`, and
/// trailing zeros are not restored.
///
/// NaN and the infinities are errors, and so is any float whose shortest
/// form lies beyond the decimal range or needs more than 28 decimal places:
/// it is refused, never rounded to fit.
pub fn decimal_from_f64(value: f64) -> Result<Decimal, ModelError> {
    let refuse = |reason| ModelError::FloatToDecimal { value, reason };
    if !value.is_finite() {
        return Err(refuse("not finite"));
    }
    if value == 0.0 {
        return Ok(Decimal::ZERO);
    }
    // A float64's Display is its shortest round-trip digits, never in
    // exponent notation, and from_str_exact refuses rather than rounds.
    Decimal::from_str_exact(&value.to_string()).map_err(|err| match err {
        rust_decimal::Error::Underflow => refuse("more than 28 decimal places"),
        _ => refuse("beyond the decimal range"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn round_down_to_step_behaves() {
        assert_eq!(
            round_down_to_step(
                Decimal::from_str("1.234").expect("decimal"),
                Decimal::from_str("0.01").expect("decimal")
            ),
            Decimal::from_str("1.23").expect("decimal")
        );
    }
}
