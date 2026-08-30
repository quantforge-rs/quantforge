//! Decimal helpers used by the exchange-rule rails.

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
