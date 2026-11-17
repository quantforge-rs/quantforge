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
/// `value`: Python's `repr`. Negative zero gives `0`, and trailing zeros are
/// not restored.
///
/// Where two such strings of the same length are equally near `value`, the
/// one whose last digit is even is taken, as `repr` takes it: the float
/// `790101343014070.25` gives `790101343014070.2`, not `790101343014070.3`.
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
    let shortest = Decimal::from_str_exact(&value.to_string()).map_err(|err| match err {
        rust_decimal::Error::Underflow => refuse("more than 28 decimal places"),
        _ => refuse("beyond the decimal range"),
    })?;
    Ok(even_at_a_tie(value, shortest))
}

/// `shortest`, the decimal of `value`'s `Display`, unless `value` lies
/// exactly halfway between it and a neighbour of the same length that reads
/// back as `value` too: then whichever of the two ends in an even digit.
///
/// `Display` broke such a tie towards the larger magnitude when this was
/// written (Rust 1.99), and Python's `repr`, which the policy names as what
/// this conversion is, breaks it towards the even digit. Both read back as
/// the same float, so only this choice tells them apart. It is decided in
/// integers, with no rounding of its own, whichever of the two it is handed.
fn even_at_a_tie(value: f64, shortest: Decimal) -> Decimal {
    let digits = shortest.mantissa();
    if digits % 2 == 0 {
        return shortest;
    }
    // |value| is an odd mantissa times a power of two: the 53 bits a float
    // keeps, without their trailing zeros. A float that has a decimal is
    // never subnormal, the smallest decimal being 1e-28, so its leading
    // bit is the implicit one.
    let kept = (value.to_bits() & ((1 << 52) - 1)) | (1 << 52);
    let mantissa = u128::from(kept >> kept.trailing_zeros());

    // With `digits` and its neighbour at `scale` decimal places, the value
    // is halfway between them when
    //     2 * |value| * 10^scale = 2 * |digits| ± 1.
    // The right side is odd, so every power of two on the left cancels, and
    // what is left of it is mantissa * 5^scale: below 2^53 * 5^28 < 2^119.
    let scale = shortest.scale();
    let doubled = mantissa * 5u128.pow(scale);
    let twice = 2 * digits.unsigned_abs();
    let neighbour = if doubled == twice - 1 {
        digits - digits.signum()
    } else if doubled == twice + 1 {
        digits + digits.signum()
    } else {
        return shortest;
    };

    // The neighbour is the answer only if it reads back as the same float.
    // Next to a power of two the floats below lie closer than those above,
    // so one of two candidates that are equally near may be another float;
    // and the digits of a tie say nothing of a float of another size.
    match Decimal::try_from_i128_with_scale(neighbour, scale) {
        Ok(even) if decimal_to_f64(even).to_bits() == value.to_bits() => even,
        _ => shortest,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn decimal(text: &str) -> Decimal {
        Decimal::from_str(text).expect("decimal")
    }

    /// A whole number over a power of two, which a float holds exactly:
    /// the floats that lie halfway between two decimals are such numbers.
    fn halved(whole: u64, times: i32) -> f64 {
        assert!(whole < 1 << 53, "a float holds {whole} exactly");
        whole as f64 / 2f64.powi(times)
    }

    /// What the rule makes of a float and the candidate it is handed, as
    /// text: a decimal's `==` is by value, and the digits are what counts.
    fn decided(value: f64, shortest: &str) -> String {
        even_at_a_tie(value, decimal(shortest)).to_string()
    }

    #[test]
    fn a_tie_goes_to_the_even_digit_whichever_candidate_it_is_handed() {
        // 790101343014070.25 lies halfway between .2 and .3, and .75
        // between .7 and .8: the even one is taken, below or above. The
        // last is 0.0210056304931640625, below one.
        for (value, odd, even) in [
            (
                halved(3_160_405_372_056_281, 2),
                "790101343014070.3",
                "790101343014070.2",
            ),
            (
                halved(3_160_405_372_056_283, 2),
                "790101343014070.7",
                "790101343014070.8",
            ),
            (
                halved(11_013, 19),
                "0.021005630493164063",
                "0.021005630493164062",
            ),
        ] {
            assert_eq!(decided(value, odd), even, "{value:?} handed {odd}");
            assert_eq!(decided(value, even), even, "{value:?} handed {even}");
            // And the same of the float below zero.
            let (odd, even) = (format!("-{odd}"), format!("-{even}"));
            assert_eq!(decided(-value, &odd), even, "{value:?} handed {odd}");
            assert_eq!(decided(-value, &even), even, "{value:?} handed {even}");
        }
    }

    #[test]
    fn a_decimal_the_float_is_not_halfway_next_to_is_left_as_it_is() {
        // An odd last digit alone decides nothing: 790101343014070.125 is
        // nearer to .1 than to .2.
        for (value, shortest) in [
            (0.3, "0.3"),
            (81940.01, "81940.01"),
            (0.1 + 0.2, "0.30000000000000004"),
            (halved(6_320_810_744_112_561, 3), "790101343014070.1"),
        ] {
            assert_eq!(decided(value, shortest), shortest, "{value:?}");
        }
        // One is odd, and one less than twice one is the mantissa of every
        // power of two: only reading the neighbour back tells one from a
        // float that lies halfway between zero and one.
        assert_eq!(decided(1.0, "1"), "1");
        assert_eq!(decided(-1.0, "-1"), "-1");
        // Nor do the digits of a tie beside a float twice or half as large:
        // the float has to lie between the two candidates itself.
        for times in [1, 3] {
            let value = halved(3_160_405_372_056_281, times);
            assert_eq!(
                decided(value, "790101343014070.3"),
                "790101343014070.3",
                "{value:?}"
            );
        }
    }

    #[test]
    fn the_even_candidate_is_not_taken_where_it_reads_back_as_another_float() {
        // 2^-24 lies halfway between two candidates, and the floats below
        // it lie twice as close as those above: the smaller candidate,
        // whose last digit is even, is the float below.
        let value = halved(1, 24);
        let (odd, even) = ("0.00000005960464477539063", "0.00000005960464477539062");
        assert_eq!(decimal_to_f64(decimal(odd)).to_bits(), value.to_bits());
        assert_ne!(decimal_to_f64(decimal(even)).to_bits(), value.to_bits());
        assert_eq!(decided(value, odd), odd);
        assert_eq!(decided(-value, &format!("-{odd}")), format!("-{odd}"));
    }

    #[test]
    fn the_largest_decimal_is_left_as_it_is() {
        // Its digits are odd, and the number above them is no decimal.
        let largest = Decimal::MAX;
        assert_eq!(even_at_a_tie(7.9e28, largest), largest);
        assert_eq!(even_at_a_tie(-7.9e28, -largest), -largest);
    }

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
