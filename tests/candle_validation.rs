//! Black-box tests for candle validation, driven through the crate's
//! public API. Moved out of `src/model/candle.rs` so the module does not
//! carry a test block twice the size of the code it covers; every symbol
//! used here is part of the crate's public surface.

use quantforge::{
    Candle, ExchangeId, Interval, MarketId, Symbol, ValidationIssue, validate_candles,
};
use rust_decimal::Decimal;
use std::str::FromStr;

fn market() -> MarketId {
    MarketId::new(
        ExchangeId::BinanceSpot,
        Symbol::new("BTCUSDT").expect("symbol"),
        Interval::M1,
    )
}

// Saturating close_time so near-i64::MAX fixtures stay constructible.
fn candle(open_time_ms: i64) -> Candle {
    Candle {
        open_time_ms,
        close_time_ms: open_time_ms.saturating_add(59_999),
        open: Decimal::ONE,
        high: Decimal::ONE,
        low: Decimal::ONE,
        close: Decimal::ONE,
        volume: Decimal::ONE,
        trades: Some(1),
    }
}

#[test]
fn validation_detects_gap() {
    let candles = vec![candle(0), candle(120_000)];

    let report = validate_candles(&market(), &candles);
    assert_eq!(report.issues.len(), 1);
    assert!(matches!(report.issues[0], ValidationIssue::Gap { .. }));
}

#[test]
fn validation_accepts_contiguous_candles() {
    let candles = [candle(0), candle(60_000), candle(120_000)];

    let report = validate_candles(&market(), &candles);
    assert!(report.is_ok(), "got {:?}", report.issues);
    assert_eq!(report.candle_count, 3);
}

#[test]
fn validation_accepts_empty_slice() {
    let report = validate_candles(&market(), &[]);
    assert!(report.is_ok());
    assert_eq!(report.candle_count, 0);
}

#[test]
fn validation_accepts_single_candle() {
    let report = validate_candles(&market(), &[candle(0)]);
    assert!(report.is_ok(), "got {:?}", report.issues);
}

#[test]
fn validation_detects_adjacent_duplicate_as_duplicate_and_out_of_order() {
    let candles = [candle(0), candle(0)];

    let report = validate_candles(&market(), &candles);
    assert_eq!(
        report.issues,
        vec![
            ValidationIssue::DuplicateOpenTime { open_time_ms: 0 },
            ValidationIssue::OutOfOrder {
                prev_open_time_ms: 0,
                open_time_ms: 0,
            },
        ]
    );
}

#[test]
fn validation_detects_out_of_order_and_skips_gap_check() {
    let candles = [candle(120_000), candle(60_000)];

    let report = validate_candles(&market(), &candles);
    assert_eq!(
        report.issues,
        vec![ValidationIssue::OutOfOrder {
            prev_open_time_ms: 120_000,
            open_time_ms: 60_000,
        }]
    );
}

#[test]
fn validation_advances_prev_after_out_of_order() {
    // The third candle is on-grid relative to the out-of-order second one,
    // so it raises only the duplicate issue.
    let candles = [candle(60_000), candle(0), candle(60_000)];

    let report = validate_candles(&market(), &candles);
    assert_eq!(
        report.issues,
        vec![
            ValidationIssue::OutOfOrder {
                prev_open_time_ms: 60_000,
                open_time_ms: 0,
            },
            ValidationIssue::DuplicateOpenTime {
                open_time_ms: 60_000,
            },
        ]
    );
}

#[test]
fn validation_resyncs_after_gap() {
    let candles = [candle(0), candle(120_000), candle(180_000)];

    let report = validate_candles(&market(), &candles);
    assert_eq!(
        report.issues,
        vec![ValidationIssue::Gap {
            expected_open_time_ms: 60_000,
            open_time_ms: 120_000,
        }]
    );
}

#[test]
fn validation_detects_inverted_low_high_with_cascading_range_issues() {
    // An empty [low, high] range necessarily puts open and close outside
    // it, so "low > high" can never be the only issue.
    let candles = [Candle {
        low: Decimal::from(2),
        high: Decimal::ONE,
        open: Decimal::from_str("1.5").expect("decimal"),
        close: Decimal::from_str("1.5").expect("decimal"),
        ..candle(0)
    }];

    let report = validate_candles(&market(), &candles);
    assert_eq!(
        report.issues,
        vec![
            ValidationIssue::OhlcInvalid {
                open_time_ms: 0,
                reason: "low > high".to_string(),
            },
            ValidationIssue::OhlcInvalid {
                open_time_ms: 0,
                reason: "open not within [low, high]".to_string(),
            },
            ValidationIssue::OhlcInvalid {
                open_time_ms: 0,
                reason: "close not within [low, high]".to_string(),
            },
        ]
    );
}

#[test]
fn validation_detects_open_outside_range() {
    let candles = [Candle {
        open: Decimal::from(3),
        high: Decimal::from(2),
        low: Decimal::ONE,
        close: Decimal::from_str("1.5").expect("decimal"),
        ..candle(0)
    }];

    let report = validate_candles(&market(), &candles);
    assert_eq!(
        report.issues,
        vec![ValidationIssue::OhlcInvalid {
            open_time_ms: 0,
            reason: "open not within [low, high]".to_string(),
        }]
    );
}

#[test]
fn validation_detects_close_outside_range() {
    let candles = [Candle {
        open: Decimal::from_str("1.5").expect("decimal"),
        high: Decimal::from(2),
        low: Decimal::ONE,
        close: Decimal::from_str("0.5").expect("decimal"),
        ..candle(0)
    }];

    let report = validate_candles(&market(), &candles);
    assert_eq!(
        report.issues,
        vec![ValidationIssue::OhlcInvalid {
            open_time_ms: 0,
            reason: "close not within [low, high]".to_string(),
        }]
    );
}

#[test]
fn validation_detects_close_time_before_open_time() {
    let candles = [Candle {
        close_time_ms: 59_999,
        ..candle(60_000)
    }];

    let report = validate_candles(&market(), &candles);
    assert_eq!(
        report.issues,
        vec![ValidationIssue::OhlcInvalid {
            open_time_ms: 60_000,
            reason: "close_time_ms < open_time_ms".to_string(),
        }]
    );
}

#[test]
fn validation_accepts_candle_touching_its_own_bounds() {
    // Boundary equalities are valid: open == low, close == high,
    // close_time == open_time, and a zero-volume (quiet) bar.
    let candles = [Candle {
        open: Decimal::ONE,
        low: Decimal::ONE,
        close: Decimal::from(2),
        high: Decimal::from(2),
        close_time_ms: 0,
        volume: Decimal::ZERO,
        ..candle(0)
    }];

    let report = validate_candles(&market(), &candles);
    assert!(report.is_ok(), "got {:?}", report.issues);
}

#[test]
fn validation_detects_non_positive_prices() {
    for raw in ["-1", "0"] {
        let value = Decimal::from_str(raw).expect("decimal");
        let candles = [Candle {
            open: value,
            high: value,
            low: value,
            close: value,
            ..candle(0)
        }];

        let report = validate_candles(&market(), &candles);
        assert_eq!(
            report.issues,
            vec![
                ValidationIssue::OhlcInvalid {
                    open_time_ms: 0,
                    reason: "open <= 0".to_string(),
                },
                ValidationIssue::OhlcInvalid {
                    open_time_ms: 0,
                    reason: "high <= 0".to_string(),
                },
                ValidationIssue::OhlcInvalid {
                    open_time_ms: 0,
                    reason: "low <= 0".to_string(),
                },
                ValidationIssue::OhlcInvalid {
                    open_time_ms: 0,
                    reason: "close <= 0".to_string(),
                },
            ],
            "for input {raw:?}"
        );
    }
}

#[test]
fn validation_detects_negative_volume() {
    let candles = [Candle {
        volume: Decimal::from(-1),
        ..candle(0)
    }];

    let report = validate_candles(&market(), &candles);
    assert_eq!(
        report.issues,
        vec![ValidationIssue::OhlcInvalid {
            open_time_ms: 0,
            reason: "volume < 0".to_string(),
        }]
    );
}

#[test]
fn validation_accepts_negative_open_time() {
    let candles = [candle(-120_000), candle(-60_000)];

    let report = validate_candles(&market(), &candles);
    assert!(report.is_ok(), "got {:?}", report.issues);
}

#[test]
fn validation_flags_gap_at_i64_max_instead_of_overflowing() {
    let candles = [candle(i64::MAX - 1), candle(i64::MAX)];

    let report = validate_candles(&market(), &candles);
    assert_eq!(
        report.issues,
        vec![ValidationIssue::Gap {
            expected_open_time_ms: i64::MAX,
            open_time_ms: i64::MAX,
        }]
    );
}
