//! The decimal ↔ float64 policy (docs/decimal-float-policy.md): the shared
//! conformance cases, then round-trip properties over generated values.
//!
//! The cases' expected values come from Python's correctly rounded
//! `float()` and `repr()`, never from the functions under test. The
//! properties use std's own correctly rounded float parser as the oracle.

use quantforge::{decimal_from_f64, decimal_to_f64};
use rust_decimal::{Decimal, RoundingStrategy};

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

// Floats with a short binary fraction, n + k/2^b: the ones that can lie
// exactly halfway between two decimal strings of the same length. Their
// exact value is a decimal too, built here from whole numbers alone, so the
// test decides "shortest", "nearest" and "halfway" itself, in exact decimal
// arithmetic: no decimal with a place fewer reads back as the float, the
// result is never farther from the float than a neighbour that reads back
// as it, and where one is exactly as near, the result ends in an even
// digit, as Python's `repr` does.
#[test]
fn the_decimal_of_a_float_is_the_nearest_shortest_and_even_at_a_tie() {
    let mut rng = Rng(0x0475_1F2E_3D4C_5B6A);
    let (mut ties, mut ties_below) = (0, 0);
    for _ in 0..200_000 {
        // n * 2^b + k stays below 2^53, so the float holds it exactly, and
        // times 5^b below 2^96, so a decimal holds its exact value.
        let b = 1 + rng.below(18) as u32;
        let width = 1 + rng.below(52 - u64::from(b));
        let whole = rng.below(1 << width);
        let odd = 2 * rng.below(1 << (b - 1)) + 1;
        let scaled = (whole << b) + odd;
        let sign = rng.sign();
        let float = sign as f64 * (scaled as f64 / (1u64 << b) as f64);
        assert_eq!(float.abs() * (1u64 << b) as f64, scaled as f64, "exact");
        let exact = Decimal::try_from_i128_with_scale(sign * i128::from(scaled) * 5i128.pow(b), b)
            .expect("the float's exact value");

        let decimal = decimal_from_f64(float).expect("converts");
        assert_eq!(
            decimal_to_f64(decimal).to_bits(),
            float.to_bits(),
            "{float:?} via {decimal}"
        );
        // It is the shortest: with one decimal place fewer, neither the
        // decimal below it nor the one above it reads back as the float.
        if decimal.scale() > 0 {
            for strategy in [RoundingStrategy::ToZero, RoundingStrategy::AwayFromZero] {
                let shorter = decimal.round_dp_with_strategy(decimal.scale() - 1, strategy);
                assert_ne!(
                    decimal_to_f64(shorter).to_bits(),
                    float.to_bits(),
                    "{float:?}: {shorter} is shorter than {decimal}"
                );
            }
        }
        let unit = Decimal::new(1, decimal.scale());
        let distance = (decimal - exact).abs();
        for neighbour in [decimal - unit, decimal + unit] {
            if decimal_to_f64(neighbour).to_bits() != float.to_bits() {
                continue;
            }
            let other = (neighbour - exact).abs();
            assert!(
                distance <= other,
                "{float:?}: {neighbour} is nearer than {decimal}"
            );
            if distance == other {
                assert_eq!(
                    decimal.mantissa() % 2,
                    0,
                    "{float:?} lies halfway between {decimal} and {neighbour}"
                );
                ties += 1;
                if neighbour.abs() > decimal.abs() {
                    ties_below += 1;
                }
            }
        }
    }
    // Both kinds are met: the even digit below the float, where `Display`
    // would give the one above, and the even digit above it.
    assert!(
        ties_below >= 1_000 && ties - ties_below >= 1_000,
        "{ties} ties, {ties_below} of them decided downwards"
    );
}
