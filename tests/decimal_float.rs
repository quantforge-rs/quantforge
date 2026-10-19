//! The decimal ↔ float64 policy (docs/decimal-float-policy.md): the shared
//! conformance cases, then round-trip properties over generated values.
//!
//! The cases' expected values come from Python's correctly rounded
//! `float()` and `repr()`, never from the functions under test. The
//! properties use std's own correctly rounded float parser as the oracle.

use quantforge::{decimal_from_f64, decimal_to_f64};
use rust_decimal::Decimal;

const CASES: &str = include_str!("fixtures/decimal_float_cases.csv");

/// xorshift64*: deterministic values without a dependency, the same on
/// every machine.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, bound: u64) -> u64 {
        self.next() % bound
    }

    fn sign(&mut self) -> i128 {
        if self.below(2) == 0 { 1 } else { -1 }
    }
}

// Lines, not a split on '\n', so a CRLF checkout parses the same.
#[test]
fn every_conformance_case_holds() {
    let mut lines = CASES.lines();
    assert_eq!(
        lines.next(),
        Some("direction,input,expected,note"),
        "fixture header"
    );
    let (mut to_f64, mut from_f64) = (0, 0);
    for line in lines {
        let fields: Vec<&str> = line.split(',').collect();
        let [direction, input, expected, _note] = fields[..] else {
            panic!("a case has four fields: {line}");
        };
        match direction {
            "to_f64" => {
                let mut decimal = Decimal::from_str_exact(input).expect("decimal input");
                // Parsing "-0" drops the sign; restore it so the case really
                // converts a negative zero.
                if input.starts_with('-') {
                    decimal.set_sign_negative(true);
                    assert!(decimal.is_sign_negative(), "negative input {input}");
                }
                let bits = u64::from_str_radix(expected, 16).expect("hex bits");
                assert_eq!(
                    decimal_to_f64(decimal).to_bits(),
                    bits,
                    "to_f64 {input} should give {expected}"
                );
                to_f64 += 1;
            }
            "from_f64" => {
                let float = f64::from_bits(u64::from_str_radix(input, 16).expect("hex bits"));
                match (decimal_from_f64(float), expected.strip_prefix("error:")) {
                    (Ok(decimal), None) => {
                        assert_eq!(decimal.to_string(), expected, "from_f64 {input}")
                    }
                    (Err(err), Some(reason)) => {
                        assert!(err.to_string().ends_with(reason), "from_f64 {input}: {err}")
                    }
                    (result, _) => {
                        panic!("from_f64 {input}: expected {expected}, got {result:?}")
                    }
                }
                from_f64 += 1;
            }
            other => panic!("unknown direction {other}: {line}"),
        }
    }
    assert!(
        to_f64 >= 15 && from_f64 >= 15,
        "both directions are covered, got {to_f64} and {from_f64} cases"
    );
}

// Every scale the decimal type has and mantissas of every width, checked
// against the correctly rounded parse of the decimal's exact string.
#[test]
fn decimal_to_f64_matches_the_correctly_rounded_parse() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for _ in 0..200_000 {
        let width = 1 + rng.below(96) as u32;
        let raw = ((u128::from(rng.next()) << 64) | u128::from(rng.next())) >> (128 - width);
        let mantissa = i128::try_from(raw.max(1)).expect("below 2^96") * rng.sign();
        let scale = rng.below(29) as u32;
        let decimal = Decimal::try_from_i128_with_scale(mantissa, scale).expect("decimal");
        let parsed: f64 = decimal.to_string().parse().expect("float");
        assert_eq!(
            decimal_to_f64(decimal).to_bits(),
            parsed.to_bits(),
            "{decimal}"
        );
    }
}

// Floats between 2^-40 and 2^92 of either sign: their shortest forms fit
// the decimal type, so every one must convert and come back bit for bit.
#[test]
fn float64_survives_a_trip_through_decimal() {
    let mut rng = Rng(0xD1B5_4A32_D192_ED03);
    let mut converted = 0;
    for _ in 0..200_000 {
        let exponent = rng.below(132) as i64 - 40;
        let sign = rng.below(2) << 63;
        let fraction = rng.next() & ((1 << 52) - 1);
        let float = f64::from_bits(sign | (((1023 + exponent) as u64) << 52) | fraction);
        match decimal_from_f64(float) {
            Ok(decimal) => {
                assert_eq!(
                    decimal_to_f64(decimal).to_bits(),
                    float.to_bits(),
                    "{float:?} via {decimal}"
                );
                converted += 1;
            }
            Err(err) => assert!(
                err.to_string().ends_with("more than 28 decimal places"),
                "{err}"
            ),
        }
    }
    assert!(
        converted >= 199_000,
        "only {converted} of 200000 floats converted"
    );
}

// The float64 guarantee for decimals: up to 15 significant digits come back
// as the same value, in their shortest (normalized) form. Includes large
// integers with trailing zeros, which take float64's integer Display path.
#[test]
fn decimals_with_at_most_15_significant_digits_survive_float64() {
    let mut rng = Rng(0x2545_F491_4F6C_DD1D);
    for _ in 0..200_000 {
        let digits = 1 + rng.below(15) as u32;
        let mantissa = i128::from(rng.below(10u64.pow(digits)).max(1)) * rng.sign();
        let decimal = if rng.below(4) == 0 {
            let zeros = rng.below(u64::from(29 - digits)) as u32;
            Decimal::try_from_i128_with_scale(mantissa * 10i128.pow(zeros), 0).expect("decimal")
        } else {
            Decimal::try_from_i128_with_scale(mantissa, rng.below(29) as u32).expect("decimal")
        };
        let back = decimal_from_f64(decimal_to_f64(decimal)).expect("converts back");
        assert_eq!(back, decimal, "{decimal}");
        assert_eq!(
            back.to_string(),
            decimal.normalize().to_string(),
            "{decimal}"
        );
    }
}
