//! The fraction helper held to the arithmetic an independent model worked
//! out (`spikes/futures-contract/sizing.py`, the `arithmetic` list of
//! `tests/fixtures/sizing_cases.json`), and its whole numbers held to the
//! machine's own on values small enough for both.

use super::*;
use serde_json::Value;

const CASES: &str = include_str!("../../../tests/fixtures/sizing_cases.json");

fn decimal(text: &str) -> Decimal {
    Decimal::from_str_exact(text).unwrap_or_else(|error| panic!("{text}: {error}"))
}

/// An expression of the fixture: a decimal, or an operator and its two
/// operands. `None` where it divides by zero.
fn value_of(expression: &Value) -> Option<Exact> {
    if let Some(leaf) = expression.as_str() {
        return Some(Exact::magnitude(decimal(leaf)));
    }
    let parts = expression.as_array().expect("an operator and two operands");
    let operator = parts[0].as_str().expect("an operator");
    let (left, right) = (value_of(&parts[1])?, value_of(&parts[2])?);
    match operator {
        "mul" => Some(left.times(&right)),
        "add" => Some(left.plus(&right)),
        "diff" => Some(left.distance(&right)),
        "div" => left.over(&right),
        other => panic!("the fixture has an operator this test does not know: {other}"),
    }
}

fn answer(decimal: Option<Decimal>) -> Value {
    decimal.map_or(Value::Null, |decimal| Value::String(decimal.to_string()))
}

#[test]
fn every_arithmetic_vector_of_the_oracle_is_answered_exactly() {
    let fixture: Value = serde_json::from_str(CASES).expect("the sizing cases");
    let vectors = fixture["arithmetic"].as_array().expect("the vectors");
    assert!(vectors.len() >= 100, "only {} vectors", vectors.len());
    let (mut out_of_range, mut undefined) = (0, 0);
    for vector in vectors {
        let note = &vector["note"];
        let expected = &vector["expect"];
        if let Some(pair) = vector.get("compare") {
            let (left, right) = (value_of(&pair[0]), value_of(&pair[1]));
            let order = left.expect("a value").cmp(&right.expect("a value"));
            let word = match order {
                Ordering::Less => "less",
                Ordering::Equal => "equal",
                Ordering::Greater => "greater",
            };
            assert_eq!(expected, word, "{note}");
            continue;
        }
        let Some(value) = value_of(&vector["expr"]) else {
            assert_eq!(vector["undefined"], true, "{note}");
            undefined += 1;
            continue;
        };
        assert!(vector.get("undefined").is_none(), "{note}");
        let got = match vector.get("places") {
            Some(places) => {
                let places = u32::try_from(places.as_u64().expect("places")).expect("few places");
                value.rounded(places)
            }
            None => {
                let grid = decimal(vector["grid"].as_str().expect("a grid"));
                match vector["round"].as_str() {
                    Some("floor") => value.floor_on(grid),
                    Some("ceil") => value.ceil_on(grid),
                    other => panic!("{note}: no rounding called {other:?}"),
                }
            }
        };
        out_of_range += usize::from(got.is_none());
        assert_eq!(&answer(got), expected, "{note}");
    }
    assert!(out_of_range >= 5, "the vectors must reach past a decimal");
    assert_eq!(undefined, 1, "one vector divides by zero");
}

#[test]
fn a_value_is_read_by_its_size_and_a_grid_must_be_above_zero() {
    assert_eq!(
        Exact::magnitude(decimal("-0.01")),
        Exact::magnitude(decimal("0.010"))
    );
    let mut zero = Decimal::ZERO;
    zero.set_sign_negative(true);
    assert!(Exact::magnitude(zero).is_zero());
    assert_eq!(Exact::whole(3), Exact::magnitude(decimal("3.000")));
    let third = Exact::whole(1).over(&Exact::whole(3)).expect("a third");
    assert_eq!(third.floor_on(decimal("0.01")), Some(decimal("0.33")));
    assert_eq!(third.ceil_on(decimal("0.01")), Some(decimal("0.34")));
    assert!(third.floor_on(decimal("-0.01")).is_none());
    assert!(third.ceil_on(Decimal::ZERO).is_none());
    assert!(third.over(&Exact::whole(0)).is_none());
}

/// A fixed sequence of 64-bit numbers.
struct Sequence(u64);

impl Sequence {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 4
    }
}

#[test]
fn whole_numbers_agree_with_the_machines_own_arithmetic() {
    let mut sequence = Sequence(736);
    for _ in 0..2_000 {
        let (a, b) = (u128::from(sequence.next()), u128::from(sequence.next()));
        let (left, right) = (Natural::of(a), Natural::of(b));
        assert_eq!(left.plus(&right), Natural::of(a + b));
        assert_eq!(left.times(&right), Natural::of(a * b));
        assert_eq!(left.cmp(&right), a.cmp(&b));
        let (mut more, less) = if a < b {
            (right.clone(), left.clone())
        } else {
            (left.clone(), right.clone())
        };
        more.take(&less);
        assert_eq!(more, Natural::of(a.abs_diff(b)));
        let product = Natural::of(a * b + a / 3);
        let divisor = b.max(1);
        let (quotient, rest) = product.divided_by(&Natural::of(divisor)).expect("divided");
        assert_eq!(quotient, Natural::of((a * b + a / 3) / divisor));
        assert_eq!(rest, Natural::of((a * b + a / 3) % divisor));
        assert_eq!(Natural::of(a * 10).tenth(), Some(Natural::of(a)));
        assert_eq!(Natural::of(a * 10 + 7).tenth(), None);
    }
    assert!(Natural::of(5).divided_by(&Natural::of(0)).is_none());
    assert_eq!(Natural::power_of_ten(30), Natural::of(10u128.pow(30)));
}

#[test]
fn numbers_far_beyond_a_decimal_divide_back_exactly() {
    // (2^96 - 1)^4 × 7 + 5, divided by (2^96 - 1)^3: 7 × (2^96 - 1), rest 5.
    let largest = Natural::of(u128::from(u64::MAX) << 32 | u128::from(u32::MAX));
    let cube = largest.times(&largest).times(&largest);
    let dividend = cube
        .times(&largest)
        .times(&Natural::of(7))
        .plus(&Natural::of(5));
    let (quotient, rest) = dividend.divided_by(&cube).expect("divided");
    assert_eq!(quotient, largest.times(&Natural::of(7)));
    assert_eq!(rest, Natural::of(5));
    assert_eq!(largest.decimal(0), Some(Decimal::MAX));
    assert_eq!(largest.plus(&Natural::of(1)).decimal(0), None);
    assert_eq!(Natural::of(1).decimal(29), None);
}
