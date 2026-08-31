//! The candle interval grid: exchange wire format, step duration, and
//! parsing. The wire form (`8h`) and the derived serde token (`H8`) are
//! deliberately separate channels.

use super::ModelError;
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Interval {
    S1,
    M1,
    M3,
    M5,
    M15,
    M30,
    H1,
    H2,
    H4,
    H6,
    H8,
    H12,
    D1,
    D3,
    W1,
}

impl Interval {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::S1 => "1s",
            Self::M1 => "1m",
            Self::M3 => "3m",
            Self::M5 => "5m",
            Self::M15 => "15m",
            Self::M30 => "30m",
            Self::H1 => "1h",
            Self::H2 => "2h",
            Self::H4 => "4h",
            Self::H6 => "6h",
            Self::H8 => "8h",
            Self::H12 => "12h",
            Self::D1 => "1d",
            Self::D3 => "3d",
            Self::W1 => "1w",
        }
    }

    pub fn step_ms(self) -> i64 {
        match self {
            Self::S1 => 1_000,
            Self::M1 => 60 * 1_000,
            Self::M3 => 3 * 60 * 1_000,
            Self::M5 => 5 * 60 * 1_000,
            Self::M15 => 15 * 60 * 1_000,
            Self::M30 => 30 * 60 * 1_000,
            Self::H1 => 60 * 60 * 1_000,
            Self::H2 => 2 * 60 * 60 * 1_000,
            Self::H4 => 4 * 60 * 60 * 1_000,
            Self::H6 => 6 * 60 * 60 * 1_000,
            Self::H8 => 8 * 60 * 60 * 1_000,
            Self::H12 => 12 * 60 * 60 * 1_000,
            Self::D1 => 24 * 60 * 60 * 1_000,
            Self::D3 => 3 * 24 * 60 * 60 * 1_000,
            Self::W1 => 7 * 24 * 60 * 60 * 1_000,
        }
    }
}

impl fmt::Display for Interval {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Interval {
    type Err = ModelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim() {
            "1s" => Ok(Self::S1),
            "1m" => Ok(Self::M1),
            "3m" => Ok(Self::M3),
            "5m" => Ok(Self::M5),
            "15m" => Ok(Self::M15),
            "30m" => Ok(Self::M30),
            "1h" => Ok(Self::H1),
            "2h" => Ok(Self::H2),
            "4h" => Ok(Self::H4),
            "6h" => Ok(Self::H6),
            "8h" => Ok(Self::H8),
            "12h" => Ok(Self::H12),
            "1d" => Ok(Self::D1),
            "3d" => Ok(Self::D3),
            "1w" => Ok(Self::W1),
            other => Err(ModelError::InvalidInterval(other.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// Every `Interval` variant with the exact wire string Binance expects,
    /// the candle step duration in milliseconds, and the serde token (the
    /// JSON form persisted in `state_json`, distinct from the SQLite
    /// `interval` column which stores `as_str()`). Rows are ordered by
    /// ascending duration, matching enum declaration order. Steps are spelled
    /// as literal milliseconds so the table stays an independent oracle
    /// instead of mirroring the arithmetic in `step_ms()`.
    const INTERVAL_CASES: [(Interval, &str, i64, &str); 15] = [
        (Interval::S1, "1s", 1_000, "S1"),
        (Interval::M1, "1m", 60_000, "M1"),
        (Interval::M3, "3m", 180_000, "M3"),
        (Interval::M5, "5m", 300_000, "M5"),
        (Interval::M15, "15m", 900_000, "M15"),
        (Interval::M30, "30m", 1_800_000, "M30"),
        (Interval::H1, "1h", 3_600_000, "H1"),
        (Interval::H2, "2h", 7_200_000, "H2"),
        (Interval::H4, "4h", 14_400_000, "H4"),
        (Interval::H6, "6h", 21_600_000, "H6"),
        (Interval::H8, "8h", 28_800_000, "H8"),
        (Interval::H12, "12h", 43_200_000, "H12"),
        (Interval::D1, "1d", 86_400_000, "D1"),
        (Interval::D3, "3d", 259_200_000, "D3"),
        (Interval::W1, "1w", 604_800_000, "W1"),
    ];

    #[test]
    fn interval_cases_cover_every_variant() {
        let variants: HashSet<Interval> = INTERVAL_CASES
            .into_iter()
            .map(|(interval, ..)| interval)
            .collect();
        assert_eq!(variants.len(), 15);

        for interval in variants {
            // Exhaustive on purpose: a new variant stops this module from
            // compiling until it is added to INTERVAL_CASES.
            match interval {
                Interval::S1
                | Interval::M1
                | Interval::M3
                | Interval::M5
                | Interval::M15
                | Interval::M30
                | Interval::H1
                | Interval::H2
                | Interval::H4
                | Interval::H6
                | Interval::H8
                | Interval::H12
                | Interval::D1
                | Interval::D3
                | Interval::W1 => {}
            }
        }
    }

    #[test]
    fn interval_as_str_matches_exchange_wire_format() {
        for (interval, expected, ..) in INTERVAL_CASES {
            assert_eq!(interval.as_str(), expected, "for variant {interval:?}");
        }
    }

    #[test]
    fn interval_display_matches_as_str() {
        for (interval, expected, ..) in INTERVAL_CASES {
            assert_eq!(interval.to_string(), expected, "for variant {interval:?}");
        }
    }

    #[test]
    fn interval_wire_format_round_trips_through_parsing() {
        for (interval, expected, ..) in INTERVAL_CASES {
            assert_eq!(
                expected.parse::<Interval>().expect("interval"),
                interval,
                "for variant {interval:?}"
            );
            assert_eq!(
                interval.as_str().parse::<Interval>().expect("interval"),
                interval,
                "for variant {interval:?}"
            );
        }
    }

    #[test]
    fn interval_wire_formats_are_unique_per_variant() {
        let formatted: HashSet<&'static str> = INTERVAL_CASES
            .into_iter()
            .map(|(interval, ..)| interval.as_str())
            .collect();
        assert_eq!(formatted.len(), 15);
    }

    #[test]
    fn interval_step_ms_matches_expected_duration() {
        for (interval, _, expected_step_ms, _) in INTERVAL_CASES {
            assert_eq!(
                interval.step_ms(),
                expected_step_ms,
                "for variant {interval:?}"
            );
        }
    }

    #[test]
    fn interval_step_ms_strictly_increases_in_declaration_order() {
        for pair in INTERVAL_CASES.windows(2) {
            let (shorter, ..) = pair[0];
            let (longer, ..) = pair[1];
            assert!(
                shorter.step_ms() < longer.step_ms(),
                "expected {shorter:?} ({} ms) < {longer:?} ({} ms)",
                shorter.step_ms(),
                longer.step_ms()
            );
        }
    }

    #[test]
    fn interval_serde_json_round_trips_per_variant() {
        for (interval, _, _, serde_token) in INTERVAL_CASES {
            let json = serde_json::to_string(&interval).expect("serialize interval");
            assert_eq!(
                json,
                format!("\"{serde_token}\""),
                "for variant {interval:?}"
            );
            let parsed: Interval = serde_json::from_str(&json).expect("deserialize interval");
            assert_eq!(parsed, interval, "for variant {interval:?}");
        }
    }

    // The SQLite `interval` column stores `as_str()` ("8h") while `state_json`
    // stores the serde token ("H8"). QF-001 corrupted the wire channel (H8
    // formatted as "1m"); keep the two channels pinned as deliberately
    // distinct and mutually non-parseable so drift in either is caught.
    #[test]
    fn interval_serde_token_is_variant_name_not_wire_format() {
        for (interval, wire, ..) in INTERVAL_CASES {
            let json = serde_json::to_string(&interval).expect("serialize interval");
            assert_ne!(json, format!("\"{wire}\""), "for variant {interval:?}");
            assert!(
                serde_json::from_str::<Interval>(&format!("\"{wire}\"")).is_err(),
                "wire format {wire:?} must not deserialize as an Interval"
            );
        }
    }

    /// Rejected inputs with the exact error message each must produce.
    const INVALID_INTERVAL_CASES: [(&str, &str); 17] = [
        ("", "invalid interval: "),
        ("   ", "invalid interval: "),
        ("7m", "invalid interval: 7m"),
        (" 7m ", "invalid interval: 7m"),
        ("\t9x\n", "invalid interval: 9x"),
        ("8H", "invalid interval: 8H"),
        ("1S", "invalid interval: 1S"),
        ("1D", "invalid interval: 1D"),
        ("1W", "invalid interval: 1W"),
        ("H8", "invalid interval: H8"),
        ("60m", "invalid interval: 60m"),
        ("1min", "invalid interval: 1min"),
        ("2d", "invalid interval: 2d"),
        ("2w", "invalid interval: 2w"),
        ("1y", "invalid interval: 1y"),
        ("1M", "invalid interval: 1M"),
        ("8 h", "invalid interval: 8 h"),
    ];

    #[test]
    fn invalid_interval_strings_are_rejected_with_exact_messages() {
        for (input, expected_message) in INVALID_INTERVAL_CASES {
            let error = match input.parse::<Interval>() {
                Ok(parsed) => panic!("input {input:?} unexpectedly parsed as {parsed:?}"),
                Err(error) => error,
            };
            assert!(
                matches!(error, ModelError::InvalidInterval(_)),
                "for input {input:?}"
            );
            assert_eq!(error.to_string(), expected_message, "for input {input:?}");
        }
    }

    // Regression: QF-001, Interval::H8 formatted as "1m".
    #[test]
    fn h8_formats_as_eight_hours_and_not_one_minute() {
        assert_eq!(Interval::H8.as_str(), "8h");
        assert_eq!(Interval::H8.to_string(), "8h");
        assert_eq!("8h".parse::<Interval>().expect("interval"), Interval::H8);
    }

    #[test]
    fn surrounding_whitespace_is_trimmed_when_parsing() {
        assert_eq!(" 8h ".parse::<Interval>().expect("interval"), Interval::H8);
        assert_eq!(
            "\t1d\n".parse::<Interval>().expect("interval"),
            Interval::D1
        );
    }
}
