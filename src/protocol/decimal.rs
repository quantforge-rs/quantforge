//! Exact decimals, sent as JSON strings.

use std::{fmt, str::FromStr};

use rust_decimal::Decimal;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use super::ProtocolError;

/// A price, quantity, or amount: an exact decimal, sent as a JSON string of
/// its digits.
///
/// The text is an optional minus sign, the integer digits without leading
/// zeros, and an optional fraction, `-?(0|[1-9][0-9]*)(\.[0-9]+)?`, with no
/// minus sign on zero. Trailing zeros are kept: `"81940.01000000"` reads
/// back as `"81940.01000000"`. A value holds at most 28 decimal places and
/// 96 bits of digits.
///
/// A receiver refuses everything else rather than reading it approximately:
/// a JSON number, which may already have been rounded through a float, an
/// exponent, `+`, `_`, spaces, `NaN`, and any value a decimal cannot hold
/// without rounding.
///
/// Equality is numeric, as for any decimal: `"1.10"` equals `"1.1"`, though
/// each keeps its own text.
///
/// ```
/// use quantforge::protocol::ExactDecimal;
///
/// let price: ExactDecimal = serde_json::from_str(r#""81940.01000000""#)?;
/// assert_eq!(serde_json::to_string(&price)?, r#""81940.01000000""#);
///
/// assert!(serde_json::from_str::<ExactDecimal>("81940.01").is_err());
/// assert!(serde_json::from_str::<ExactDecimal>(r#""8.194001e4""#).is_err());
/// # Ok::<(), serde_json::Error>(())
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ExactDecimal(Decimal);

impl ExactDecimal {
    /// Wraps a decimal, keeping its scale. A negative zero becomes zero,
    /// which has no sign on the wire.
    pub fn new(mut value: Decimal) -> Self {
        if value.is_zero() {
            value.set_sign_positive(true);
        }
        Self(value)
    }

    /// The decimal, with the scale it was sent with.
    pub fn get(self) -> Decimal {
        self.0
    }
}

impl From<Decimal> for ExactDecimal {
    fn from(value: Decimal) -> Self {
        Self::new(value)
    }
}

impl From<ExactDecimal> for Decimal {
    fn from(value: ExactDecimal) -> Self {
        value.0
    }
}

/// The wire text, without the quotes.
impl fmt::Display for ExactDecimal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

/// Reads the wire text, without the quotes.
impl FromStr for ExactDecimal {
    type Err = ProtocolError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let refuse = |reason| ProtocolError::InvalidDecimal {
            input: text.to_string(),
            reason,
        };
        if !is_exact_form(text) {
            return Err(refuse(
                "expected digits with an optional fraction, such as \"67012.34\", and no sign on zero",
            ));
        }
        // A plain `from_str` would round a value with too many digits.
        Decimal::from_str_exact(text)
            .map(Self)
            .map_err(|_| refuse("a decimal cannot hold it without rounding"))
    }
}

/// `-?(0|[1-9][0-9]*)(\.[0-9]+)?`, with no minus sign on zero. Checked before
/// parsing, because the decimal parser also takes `+`, `_`, and a bare
/// leading point.
fn is_exact_form(text: &str) -> bool {
    let (negative, unsigned) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let (integer, fraction) = match unsigned.split_once('.') {
        Some((integer, fraction)) => (integer, Some(fraction)),
        None => (unsigned, None),
    };
    let digits = |part: &str| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
    let well_formed = digits(integer)
        && (integer == "0" || !integer.starts_with('0'))
        && fraction.is_none_or(digits);
    let zero = unsigned.bytes().all(|byte| byte == b'0' || byte == b'.');
    well_formed && !(negative && zero)
}

impl Serialize for ExactDecimal {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for ExactDecimal {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Only a string: `rust_decimal`'s own impl also takes JSON numbers,
        // which a float may already have rounded.
        deserializer.deserialize_str(DecimalText)
    }
}

struct DecimalText;

impl de::Visitor<'_> for DecimalText {
    type Value = ExactDecimal;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a decimal as a JSON string, such as \"67012.34\"")
    }

    fn visit_str<E: de::Error>(self, text: &str) -> Result<Self::Value, E> {
        text.parse().map_err(E::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXACT: [&str; 14] = [
        "0",
        "0.00",
        "1",
        "-1",
        "10",
        "100.5",
        "-0.5",
        "67012.34",
        "81940.01000000",
        "0.00010000",
        // The most decimal places, and the largest and smallest values.
        "0.0000000000000000000000000001",
        "79228162514264337593543950335",
        "-79228162514264337593543950335",
        "7.9228162514264337593543950335",
    ];

    #[test]
    fn a_decimal_in_its_exact_form_reads_and_writes_the_same_text() {
        for text in EXACT {
            let value: ExactDecimal = text.parse().expect("exact decimal");
            assert_eq!(value.to_string(), text);
            let json = format!("\"{text}\"");
            let read: ExactDecimal = serde_json::from_str(&json).expect("decimal string");
            assert_eq!(serde_json::to_string(&read).expect("serialize"), json);
        }
    }

    #[test]
    fn anything_but_the_exact_form_is_refused() {
        for text in [
            "", " 1", "1 ", "+1", "01", "00.5", "-01", ".5", "-.5", "5.", "1e5", "1E5", "1.5e-3",
            "1_000", "NaN", "nan", "inf", "-inf", "-0", "-0.00", "--1", "1.2.3", "0x10", "1,5",
            "- 1", "١", "½",
        ] {
            // Refused for its form, before the decimal parser, which takes
            // some of these, sees it.
            assert!(
                matches!(
                    text.parse::<ExactDecimal>(),
                    Err(ProtocolError::InvalidDecimal { ref input, reason })
                        if input == text && reason.starts_with("expected digits")
                ),
                "{text:?} must be refused for its form"
            );
            let json = serde_json::to_string(text).expect("encode the text");
            assert!(
                serde_json::from_str::<ExactDecimal>(&json).is_err(),
                "{json} must be refused"
            );
        }
    }

    // `Decimal::from_str` would round these; the wire refuses them instead.
    #[test]
    fn a_value_a_decimal_cannot_hold_is_refused_instead_of_rounded() {
        for text in [
            "0.00000000000000000000000000001",
            "1.00000000000000000000000000001",
            "79228162514264337593543950336",
            "9.9999999999999999999999999999",
        ] {
            assert!(
                matches!(
                    text.parse::<ExactDecimal>(),
                    Err(ProtocolError::InvalidDecimal { reason, .. })
                        if reason.contains("without rounding")
                ),
                "{text:?} must be refused"
            );
        }
    }

    #[test]
    fn a_json_number_or_any_other_value_is_never_read_as_a_decimal() {
        for json in [
            "1", "1.5", "67012.34", "1e5", "-0", "null", "true", "[]", "{}",
        ] {
            assert!(
                serde_json::from_str::<ExactDecimal>(json).is_err(),
                "{json} must be refused"
            );
        }
    }

    #[test]
    fn a_negative_zero_is_written_as_zero() {
        let mut zero = Decimal::new(0, 2);
        zero.set_sign_negative(true);
        assert!(zero.is_sign_negative());
        let wire = ExactDecimal::from(zero);
        assert_eq!(wire.to_string(), "0.00");
        assert_eq!(serde_json::to_string(&wire).expect("serialize"), "\"0.00\"");
        assert_eq!(wire.get().to_string(), "0.00");
    }

    #[test]
    fn equality_is_numeric_while_each_value_keeps_its_text() {
        let short: ExactDecimal = "1.1".parse().expect("decimal");
        let long: ExactDecimal = "1.10".parse().expect("decimal");
        assert_eq!(short, long);
        assert_eq!(short.to_string(), "1.1");
        assert_eq!(long.to_string(), "1.10");
        assert_eq!(Decimal::from(long).scale(), 2);
    }
}
