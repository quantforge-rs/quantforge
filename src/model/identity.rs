//! Who and what an execution is for (ADR 0002 §2): the environment a store
//! and its connectors are bound to, the instrument traded, the venue's own
//! order, algo-order and trade IDs as opaque text, and the operations that
//! change anything on an exchange. The venue account is in
//! [`account`](super::account).

use super::{Asset, Category, ExchangeId, Interval, MarketId, ModelError, Symbol, Venue};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Where a venue's systems run: real funds on mainnet, or a test
/// environment. It is declared by the operator and never inferred from a
/// URL; every store is bound to one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Environment {
    Mainnet,
    Testnet,
    Demo,
}

impl Environment {
    pub const ALL: [Self; 3] = [Self::Mainnet, Self::Testnet, Self::Demo];

    /// `mainnet`, `testnet` or `demo`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Mainnet => "mainnet",
            Self::Testnet => "testnet",
            Self::Demo => "demo",
        }
    }

    /// Whether orders here trade real funds.
    pub const fn is_production(self) -> bool {
        matches!(self, Self::Mainnet)
    }
}

impl fmt::Display for Environment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What is traded: a symbol on a product line, and the asset the venue
/// books its PnL in. That is the settle coin for linear, the base coin for
/// inverse and the quote asset for Spot. It comes from venue metadata and
/// is checked again whenever the instrument is resolved.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct InstrumentId {
    pub exchange: ExchangeId,
    pub symbol: Symbol,
    pub settle: Asset,
}

impl InstrumentId {
    pub fn new(exchange: ExchangeId, symbol: Symbol, settle: Asset) -> Self {
        Self {
            exchange,
            symbol,
            settle,
        }
    }

    pub const fn venue(&self) -> Venue {
        self.exchange.venue()
    }

    pub const fn category(&self) -> Category {
        self.exchange.category()
    }

    /// The data-series key of this instrument's candles at `interval`.
    pub fn series(&self, interval: Interval) -> MarketId {
        MarketId::new(self.exchange, self.symbol.clone(), interval)
    }
}

impl fmt::Display for InstrumentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.exchange, self.symbol)
    }
}

/// Whether identifier text is non-empty, at most `max` characters, and
/// free of spaces and control characters.
pub(super) fn plain_text(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.chars().count() <= max
        && !value.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// Defines a text identifier whose rule `$valid` checks on construction
/// and again on load, so no stored value bypasses it.
macro_rules! text_id {
    ($(#[$doc:meta])* $name:ident, $what:literal, $rule:literal, $valid:expr) => {
        $(#[$doc])*
        #[derive(
            Clone,
            Debug,
            PartialEq,
            Eq,
            Hash,
            PartialOrd,
            Ord,
            ::serde::Serialize,
            ::serde::Deserialize,
        )]
        #[serde(try_from = "String")]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, $crate::model::ModelError> {
                let value = value.into();
                let valid: fn(&str) -> bool = $valid;
                if !valid(&value) {
                    return Err($crate::model::ModelError::InvalidIdentifier {
                        what: $what,
                        value,
                        rule: $rule,
                    });
                }
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl TryFrom<String> for $name {
            type Error = $crate::model::ModelError;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::new(value)
            }
        }

        impl ::std::fmt::Display for $name {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}
pub(crate) use text_id;

// IDs a venue issues are kept as opaque text: they are numbers on some
// venues and strings on others, and are never computed with.

text_id!(
    /// A venue's ID for an ordinary order.
    VenueOrderId,
    "venue order id",
    "1 to 128 characters without spaces",
    |text| plain_text(text, 128)
);

text_id!(
    /// A venue's ID for a conditional (algo) order, which lives apart from
    /// its ordinary orders.
    VenueAlgoId,
    "venue algo order id",
    "1 to 128 characters without spaces",
    |text| plain_text(text, 128)
);

text_id!(
    /// A venue's ID for one fill: a trade or an execution.
    VenueTradeId,
    "venue trade id",
    "1 to 128 characters without spaces",
    |text| plain_text(text, 128)
);

/// One action that changes something on an exchange, numbered by the store
/// that records it. Spot replacements use it as well as futures orders.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "u64")]
pub struct OperationId(u64);

impl OperationId {
    /// Refuses 0, which no store issues.
    pub fn new(value: u64) -> Result<Self, ModelError> {
        if value == 0 {
            return Err(ModelError::InvalidIdentifier {
                what: "operation id",
                value: value.to_string(),
                rule: "positive",
            });
        }
        Ok(Self(value))
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

impl TryFrom<u64> for OperationId {
    type Error = ModelError;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl fmt::Display for OperationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environments_are_declared_names_and_only_mainnet_is_production() {
        let names = Environment::ALL.map(|env| (env.as_str(), env.is_production()));
        assert_eq!(
            names,
            [("mainnet", true), ("testnet", false), ("demo", false)]
        );
    }

    #[test]
    fn an_instruments_data_series_drops_the_settlement_asset_and_adds_an_interval() {
        let instrument = InstrumentId::new(
            ExchangeId::BybitInverse,
            Symbol::new("BTCUSD").expect("symbol"),
            Asset::new("BTC").expect("asset"),
        );
        let series = instrument.series(Interval::M1);
        assert_eq!(series.exchange, ExchangeId::BybitInverse);
        assert_eq!(series.symbol.as_str(), "BTCUSD");
        assert_eq!(instrument.category(), Category::Inverse);
        assert_eq!(instrument.to_string(), "bybit_inverse BTCUSD");
    }

    #[test]
    fn venue_ids_are_opaque_text() {
        let order = VenueOrderId::new("8389765519").expect("id");
        assert_eq!(order.as_str(), "8389765519");
        assert!(VenueTradeId::new("1321003749386327552").is_ok());
        for bad in ["", "has space", "tab\t"] {
            assert!(VenueAlgoId::new(bad).is_err(), "{bad:?}");
        }
        let number = serde_json::from_str::<VenueOrderId>("123");
        assert!(number.is_err(), "a number is not text");
        assert!(OperationId::new(0).is_err());
        let seven = OperationId::new(7).expect("id");
        assert_eq!(serde_json::to_string(&seven).expect("json"), "7");
        assert!(serde_json::from_str::<OperationId>("0").is_err());
    }
}
