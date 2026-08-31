//! The candle bar and the validation pass that checks a loaded range for
//! ordering faults, duplicates, gaps, and impossible OHLCV values.

use super::{MarketId, TimestampMs};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Candle {
    pub open_time_ms: TimestampMs,
    pub close_time_ms: TimestampMs,
    pub open: Decimal,
    pub high: Decimal,
    pub low: Decimal,
    pub close: Decimal,
    pub volume: Decimal,
    pub trades: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ValidationIssue {
    OutOfOrder {
        prev_open_time_ms: TimestampMs,
        open_time_ms: TimestampMs,
    },
    DuplicateOpenTime {
        open_time_ms: TimestampMs,
    },
    Gap {
        expected_open_time_ms: TimestampMs,
        open_time_ms: TimestampMs,
    },
    OhlcInvalid {
        open_time_ms: TimestampMs,
        reason: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidationReport {
    pub market: MarketId,
    pub candle_count: usize,
    pub issues: Vec<ValidationIssue>,
}

impl ValidationReport {
    pub fn is_ok(&self) -> bool {
        self.issues.is_empty()
    }
}

pub fn validate_candles(market: &MarketId, candles: &[Candle]) -> ValidationReport {
    let mut issues = Vec::new();
    let mut seen = HashSet::<TimestampMs>::new();
    let step = market.interval.step_ms();
    let mut prev_open: Option<TimestampMs> = None;

    for candle in candles {
        if !seen.insert(candle.open_time_ms) {
            issues.push(ValidationIssue::DuplicateOpenTime {
                open_time_ms: candle.open_time_ms,
            });
        }

        if let Some(prev) = prev_open {
            if candle.open_time_ms <= prev {
                issues.push(ValidationIssue::OutOfOrder {
                    prev_open_time_ms: prev,
                    open_time_ms: candle.open_time_ms,
                });
            } else {
                match prev.checked_add(step) {
                    Some(expected) if candle.open_time_ms == expected => {}
                    Some(expected) => issues.push(ValidationIssue::Gap {
                        expected_open_time_ms: expected,
                        open_time_ms: candle.open_time_ms,
                    }),
                    // Past i64::MAX no representable open time is on-grid.
                    None => issues.push(ValidationIssue::Gap {
                        expected_open_time_ms: i64::MAX,
                        open_time_ms: candle.open_time_ms,
                    }),
                }
            }
        }

        if candle.low > candle.high {
            issues.push(ValidationIssue::OhlcInvalid {
                open_time_ms: candle.open_time_ms,
                reason: "low > high".to_string(),
            });
        }
        if candle.open < candle.low || candle.open > candle.high {
            issues.push(ValidationIssue::OhlcInvalid {
                open_time_ms: candle.open_time_ms,
                reason: "open not within [low, high]".to_string(),
            });
        }
        if candle.close < candle.low || candle.close > candle.high {
            issues.push(ValidationIssue::OhlcInvalid {
                open_time_ms: candle.open_time_ms,
                reason: "close not within [low, high]".to_string(),
            });
        }
        if candle.close_time_ms < candle.open_time_ms {
            issues.push(ValidationIssue::OhlcInvalid {
                open_time_ms: candle.open_time_ms,
                reason: "close_time_ms < open_time_ms".to_string(),
            });
        }
        if candle.open <= Decimal::ZERO {
            issues.push(ValidationIssue::OhlcInvalid {
                open_time_ms: candle.open_time_ms,
                reason: "open <= 0".to_string(),
            });
        }
        if candle.high <= Decimal::ZERO {
            issues.push(ValidationIssue::OhlcInvalid {
                open_time_ms: candle.open_time_ms,
                reason: "high <= 0".to_string(),
            });
        }
        if candle.low <= Decimal::ZERO {
            issues.push(ValidationIssue::OhlcInvalid {
                open_time_ms: candle.open_time_ms,
                reason: "low <= 0".to_string(),
            });
        }
        if candle.close <= Decimal::ZERO {
            issues.push(ValidationIssue::OhlcInvalid {
                open_time_ms: candle.open_time_ms,
                reason: "close <= 0".to_string(),
            });
        }
        if candle.volume < Decimal::ZERO {
            issues.push(ValidationIssue::OhlcInvalid {
                open_time_ms: candle.open_time_ms,
                reason: "volume < 0".to_string(),
            });
        }

        prev_open = Some(candle.open_time_ms);
    }

    ValidationReport {
        market: market.clone(),
        candle_count: candles.len(),
        issues,
    }
}
