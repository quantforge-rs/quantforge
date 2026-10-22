//! Units: prices, base quantities and inverse contracts, face value,
//! leverage, and the side a position is on. Each is its own type, so a base
//! quantity is never read as a contract count (ADR 0002 §6); money and its
//! asset are in [`super::money`]. Every amount is an exact `Decimal`,
//! stored as text that keeps its scale and read back only from text.

use super::{Category, ModelError, Side};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Parses decimal text exactly: `-?(0|[1-9][0-9]*)(\.[0-9]+)?`, the
/// protocol's grammar. A sign, exponent, separator, negative zero or more
/// precision than a `Decimal` holds is refused rather than rounded.
pub(crate) fn parse_decimal(text: &str) -> Result<Decimal, ModelError> {
    let invalid = || ModelError::InvalidDecimal(text.to_string());
    let digits = text.strip_prefix('-').unwrap_or(text);
    let (whole, fraction) = match digits.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (digits, None),
    };
    let numeral = |part: &str| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
    let whole_ok = whole == "0" || (numeral(whole) && !whole.starts_with('0'));
    if !whole_ok || !fraction.is_none_or(numeral) {
        return Err(invalid());
    }
    let value = Decimal::from_str_exact(text).map_err(|_| invalid())?;
    // rust_decimal reads "-0" as a positive zero, so the text decides.
    if value.is_zero() && text.starts_with('-') {
        return Err(invalid());
    }
    Ok(value)
}

/// `value` with a negative zero made positive, keeping its scale: the one
/// spelling every zero is stored with.
pub(crate) fn canonical(mut value: Decimal) -> Decimal {
    if value.is_zero() {
        value.set_sign_positive(true);
    }
    value
}

/// Serde for a `Decimal` field as exact text: written with its scale, read
/// back only from a string, never from a JSON number, so no float ever
/// reaches an amount.
pub(crate) mod decimal_text {
    use rust_decimal::Decimal;
    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    pub(crate) fn serialize<S: Serializer>(
        value: &Decimal,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.collect_str(&super::canonical(*value))
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Decimal, D::Error> {
        let text = String::deserialize(deserializer)?;
        super::parse_decimal(&text).map_err(D::Error::custom)
    }
}

/// A `Decimal` newtype that holds only values its rule accepts, stored as
/// exact text and checked again whenever it is read back.
macro_rules! decimal_unit {
    ($(#[$doc:meta])* $name:ident, $what:literal, $rule:literal, $accepts:expr) => {
        $(#[$doc])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(Decimal);

        impl $name {
            #[doc = concat!("Refuses a value that is not ", $rule, ".")]
            pub fn new(value: Decimal) -> Result<Self, ModelError> {
                let accepts: fn(Decimal) -> bool = $accepts;
                if !accepts(value) {
                    return Err(ModelError::InvalidAmount {
                        what: $what,
                        value: value.to_string(),
                        rule: $rule,
                    });
                }
                Ok(Self(canonical(value)))
            }

            pub fn value(self) -> Decimal {
                self.0
            }
        }

        impl TryFrom<String> for $name {
            type Error = ModelError;

            fn try_from(text: String) -> Result<Self, Self::Error> {
                Self::new(parse_decimal(&text)?)
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0.to_string()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0, f)
            }
        }
    };
}

decimal_unit!(
    /// A price in the quote asset, on the instrument's tick. An inverse
    /// instrument quotes USD per coin.
    Price,
    "price",
    "positive",
    |value| value > Decimal::ZERO
);

decimal_unit!(
    /// A quantity in the base asset, as Spot and linear futures are sized.
    BaseQty,
    "base quantity",
    "zero or positive",
    |value| value >= Decimal::ZERO
);

decimal_unit!(
    /// A count of whole inverse contracts. Each is worth its instrument's
    /// [`FaceValue`] in USD.
    Contracts,
    "contract count",
    "a whole number, zero or positive",
    |value| value >= Decimal::ZERO && value.fract().is_zero()
);

decimal_unit!(
    /// What one inverse contract is worth, in USD: the venue's contract
    /// size, or its documented rule where no metadata field carries it.
    FaceValue,
    "face value",
    "positive",
    |value| value > Decimal::ZERO
);

decimal_unit!(
    /// A leverage multiplier. It changes the margin a position needs, never
    /// its size.
    Leverage,
    "leverage",
    "at least 1",
    |value| value >= Decimal::ONE
);

/// A quantity with its unit: base asset for Spot and linear futures, whole
/// contracts for inverse futures. Stored with its unit, so a number alone
/// is never read as either.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Quantity {
    Base(BaseQty),
    Contracts(Contracts),
}

impl Quantity {
    /// `value` in the unit `category` trades in.
    pub fn for_category(category: Category, value: Decimal) -> Result<Self, ModelError> {
        Ok(match category {
            Category::Spot | Category::Linear => Self::Base(BaseQty::new(value)?),
            Category::Inverse => Self::Contracts(Contracts::new(value)?),
        })
    }

    /// The unit `category` trades in, as error messages name it.
    pub const fn unit_of(category: Category) -> &'static str {
        match category {
            Category::Spot | Category::Linear => "base quantity",
            Category::Inverse => "contracts",
        }
    }

    /// This quantity's unit, as error messages name it.
    pub const fn unit(self) -> &'static str {
        match self {
            Self::Base(_) => "base quantity",
            Self::Contracts(_) => "contracts",
        }
    }

    pub fn value(self) -> Decimal {
        match self {
            Self::Base(qty) => qty.value(),
            Self::Contracts(contracts) => contracts.value(),
        }
    }

    pub fn is_zero(self) -> bool {
        self.value().is_zero()
    }

    /// Refuses a quantity whose unit is not the one `category` trades in.
    pub fn in_unit_of(self, category: Category) -> Result<Self, ModelError> {
        if self.unit() != Self::unit_of(category) {
            return Err(ModelError::UnitMismatch {
                subject: format!("{category} quantities"),
                expected: Self::unit_of(category),
                found: self.unit(),
            });
        }
        Ok(self)
    }

    /// The sum, in the same unit.
    pub fn checked_add(self, other: Self) -> Result<Self, ModelError> {
        self.combine(other, Decimal::checked_add)
    }

    /// The difference, in the same unit; never below zero.
    pub fn checked_sub(self, other: Self) -> Result<Self, ModelError> {
        self.combine(other, Decimal::checked_sub)
    }

    fn combine(
        self,
        other: Self,
        op: fn(Decimal, Decimal) -> Option<Decimal>,
    ) -> Result<Self, ModelError> {
        let overflow = || ModelError::InvalidAmount {
            what: "quantity",
            value: format!("{} and {}", self.value(), other.value()),
            rule: "within range",
        };
        match (self, other) {
            (Self::Base(a), Self::Base(b)) => Ok(Self::Base(BaseQty::new(
                op(a.value(), b.value()).ok_or_else(overflow)?,
            )?)),
            (Self::Contracts(a), Self::Contracts(b)) => Ok(Self::Contracts(Contracts::new(
                op(a.value(), b.value()).ok_or_else(overflow)?,
            )?)),
            _ => Err(ModelError::UnitMismatch {
                subject: "both quantities".to_string(),
                expected: self.unit(),
                found: other.unit(),
            }),
        }
    }
}

impl fmt::Display for Quantity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Base(qty) => write!(f, "{qty}"),
            Self::Contracts(contracts) => write!(f, "{contracts} contracts"),
        }
    }
}

/// The side a futures position is on. In one-way mode a position has one
/// side at a time; the venues' own side fields stay inside their adapters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum PositionSide {
    Long,
    Short,
}

impl PositionSide {
    pub const ALL: [Self; 2] = [Self::Long, Self::Short];

    /// `long` or `short`, as reports and the parity matrix spell it.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Long => "long",
            Self::Short => "short",
        }
    }

    /// The order side that opens or adds to the position.
    pub const fn opening_side(self) -> Side {
        match self {
            Self::Long => Side::Buy,
            Self::Short => Side::Sell,
        }
    }

    /// The order side that reduces or closes it.
    pub const fn closing_side(self) -> Side {
        match self {
            Self::Long => Side::Sell,
            Self::Short => Side::Buy,
        }
    }

    pub const fn opposite(self) -> Self {
        match self {
            Self::Long => Self::Short,
            Self::Short => Self::Long,
        }
    }
}

impl fmt::Display for PositionSide {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn dec(text: &str) -> Decimal {
        Decimal::from_str(text).expect("decimal")
    }

    #[test]
    fn decimal_text_reads_only_the_exact_grammar() {
        for good in ["0", "1", "0.10", "-1.5", "234.23", "1000000"] {
            assert_eq!(parse_decimal(good).expect("decimal").to_string(), good);
        }
        for bad in [
            "", "-", "+1", "01", "1.", ".5", "1e5", "1_000", " 1", "-0", "-0.00", "NaN",
        ] {
            assert!(parse_decimal(bad).is_err(), "{bad:?} must be refused");
        }
    }

    #[test]
    fn units_keep_their_scale_and_refuse_json_numbers() {
        let price: Price = serde_json::from_str("\"234.230\"").expect("price");
        assert_eq!(serde_json::to_string(&price).expect("json"), "\"234.230\"");
        assert!(serde_json::from_str::<Price>("234.23").is_err());
    }

    #[test]
    fn each_unit_refuses_values_outside_its_rule() {
        assert!(Price::new(Decimal::ZERO).is_err());
        assert!(BaseQty::new(dec("-0.001")).is_err());
        assert!(BaseQty::new(Decimal::ZERO).is_ok());
        assert!(Contracts::new(dec("1.5")).is_err());
        assert!(Contracts::new(dec("135.0")).is_ok());
        assert!(FaceValue::new(Decimal::ZERO).is_err());
        assert!(Leverage::new(dec("0.99")).is_err());
        assert!(Leverage::new(dec("1.5")).is_ok());
        let error = Price::new(dec("-1")).expect_err("negative price");
        assert_eq!(error.to_string(), "invalid price -1: must be positive");
        assert!(serde_json::from_str::<Contracts>("\"2.5\"").is_err());
    }

    #[test]
    fn quantities_carry_the_unit_their_category_trades_in() {
        let linear = Quantity::for_category(Category::Linear, dec("0.43")).expect("qty");
        let inverse = Quantity::for_category(Category::Inverse, dec("135")).expect("qty");
        assert!(matches!(linear, Quantity::Base(_)));
        assert!(matches!(inverse, Quantity::Contracts(_)));
        assert!(Quantity::for_category(Category::Inverse, dec("0.5")).is_err());
        let error = linear
            .in_unit_of(Category::Inverse)
            .expect_err("wrong unit");
        assert_eq!(
            error.to_string(),
            "inverse quantities must be in contracts, not base quantity"
        );
        assert!(linear.checked_add(inverse).is_err());
        let json = serde_json::to_string(&inverse).expect("json");
        assert_eq!(json, r#"{"Contracts":"135"}"#);
        assert!(serde_json::from_str::<Quantity>("\"135\"").is_err());
        let less = inverse.checked_sub(inverse).expect("difference");
        assert!(less.is_zero());
        assert!(less.checked_sub(inverse).is_err(), "never below zero");
    }

    #[test]
    fn position_sides_open_and_close_with_opposite_order_sides() {
        assert_eq!(PositionSide::Long.opening_side(), Side::Buy);
        assert_eq!(PositionSide::Long.closing_side(), Side::Sell);
        assert_eq!(PositionSide::Short.opening_side(), Side::Sell);
        assert_eq!(PositionSide::Short.closing_side(), Side::Buy);
        let names: Vec<&str> = PositionSide::ALL.iter().map(|side| side.as_str()).collect();
        assert_eq!(names, ["long", "short"]);
    }
}
