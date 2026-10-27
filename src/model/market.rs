//! Market identity: the normalized trading symbol, the venue and product
//! line it trades on, and the (exchange, symbol, interval) key every candle
//! and run is stored under.

use super::{Interval, ModelError};
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String")]
pub struct Symbol(String);

impl TryFrom<String> for Symbol {
    type Error = ModelError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl Symbol {
    pub fn new(value: impl Into<String>) -> Result<Self, ModelError> {
        let value = value.into().trim().to_string();
        if value.is_empty() {
            return Err(ModelError::InvalidSymbol("empty".to_string()));
        }
        Ok(Self(value.to_ascii_uppercase()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Symbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Symbol {
    type Err = ModelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

/// An exchange, by the name operators select it with: `binance` or `bybit`.
/// A venue sells several products; an [`ExchangeId`] names one of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Venue {
    Binance,
    Bybit,
}

impl Venue {
    /// Every venue, in the order operators see them listed.
    pub const ALL: [Self; 2] = [Self::Binance, Self::Bybit];

    /// The name operators select the venue by, which is also its registry
    /// name and its spelling on the wire.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Binance => "binance",
            Self::Bybit => "bybit",
        }
    }
}

impl fmt::Display for Venue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A product family: Spot inventory, linear futures settled in the quote
/// asset, or inverse futures settled in the base coin. Only the two futures
/// families have positions, leverage and liquidation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Category {
    Spot,
    Linear,
    Inverse,
}

impl Category {
    /// Every family, Spot first.
    pub const ALL: [Self; 3] = [Self::Spot, Self::Linear, Self::Inverse];

    /// The family's name on the wire and in reports: `spot`, `linear` or
    /// `inverse`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Spot => "spot",
            Self::Linear => "linear",
            Self::Inverse => "inverse",
        }
    }

    /// Whether the family trades futures contracts rather than inventory.
    pub const fn is_futures(self) -> bool {
        !matches!(self, Self::Spot)
    }
}

impl fmt::Display for Category {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A product line: one venue's Spot, linear or inverse market. The wire and
/// the CLI spell it as its venue and category; storage columns use
/// [`as_str`](Self::as_str), such as `binance_linear`; and `state_json`
/// holds the serde token, such as `BinanceLinear`. All three spellings stay
/// fixed once released.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ExchangeId {
    BinanceSpot,
    BinanceLinear,
    BinanceInverse,
    BybitSpot,
    BybitLinear,
    BybitInverse,
}

impl ExchangeId {
    /// The line's venue, category and storage name, from one table so that
    /// the three can never disagree.
    const fn parts(self) -> (Venue, Category, &'static str) {
        match self {
            Self::BinanceSpot => (Venue::Binance, Category::Spot, "binance_spot"),
            Self::BinanceLinear => (Venue::Binance, Category::Linear, "binance_linear"),
            Self::BinanceInverse => (Venue::Binance, Category::Inverse, "binance_inverse"),
            Self::BybitSpot => (Venue::Bybit, Category::Spot, "bybit_spot"),
            Self::BybitLinear => (Venue::Bybit, Category::Linear, "bybit_linear"),
            Self::BybitInverse => (Venue::Bybit, Category::Inverse, "bybit_inverse"),
        }
    }

    /// The name the line's candles, runs and segments are stored under.
    pub const fn as_str(self) -> &'static str {
        self.parts().2
    }

    /// The venue that sells the line.
    pub const fn venue(self) -> Venue {
        self.parts().0
    }

    /// The line's product family.
    pub const fn category(self) -> Category {
        self.parts().1
    }

    /// The line stored under `name`, the inverse of [`as_str`](Self::as_str):
    /// how a storage column is read back. It is not the serde channel, which
    /// reads the token only.
    pub fn from_storage_name(name: &str) -> Option<Self> {
        Venue::ALL
            .iter()
            .flat_map(|&venue| {
                Category::ALL
                    .iter()
                    .map(move |&category| Self::from_parts(venue, category))
            })
            .find(|line| line.as_str() == name)
    }

    /// The line that sells `category` on `venue`.
    pub const fn from_parts(venue: Venue, category: Category) -> Self {
        match (venue, category) {
            (Venue::Binance, Category::Spot) => Self::BinanceSpot,
            (Venue::Binance, Category::Linear) => Self::BinanceLinear,
            (Venue::Binance, Category::Inverse) => Self::BinanceInverse,
            (Venue::Bybit, Category::Spot) => Self::BybitSpot,
            (Venue::Bybit, Category::Linear) => Self::BybitLinear,
            (Venue::Bybit, Category::Inverse) => Self::BybitInverse,
        }
    }
}

impl fmt::Display for ExchangeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The key market data is stored and served under: a product line, a
/// symbol and an interval. ADR 0002 calls it the data-series key. It carries
/// no settlement asset, because the symbol fixes that within a product line,
/// and nothing that owns a position depends on its interval.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MarketId {
    pub exchange: ExchangeId,
    pub symbol: Symbol,
    pub interval: Interval,
}

impl MarketId {
    pub fn new(exchange: ExchangeId, symbol: Symbol, interval: Interval) -> Self {
        Self {
            exchange,
            symbol,
            interval,
        }
    }

    /// The product family the series belongs to.
    pub const fn category(&self) -> Category {
        self.exchange.category()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Inputs `Symbol::new` must accept, paired with the normalized form.
    const VALID_SYMBOL_CASES: [(&str, &str); 3] = [
        ("BTCUSDT", "BTCUSDT"),
        ("btcusdt", "BTCUSDT"),
        ("  ethusdt  ", "ETHUSDT"),
    ];

    #[test]
    fn symbol_new_trims_uppercases_and_preserves_valid_input() {
        for (input, expected) in VALID_SYMBOL_CASES {
            let symbol = Symbol::new(input).expect("symbol");
            assert_eq!(symbol.as_str(), expected, "for input {input:?}");
        }
    }

    #[test]
    fn empty_symbols_are_rejected_with_exact_message() {
        for input in ["", "   ", "\t\n"] {
            let error = Symbol::new(input).expect_err("symbol error");
            assert!(
                matches!(error, ModelError::InvalidSymbol(_)),
                "for input {input:?}"
            );
            assert_eq!(
                error.to_string(),
                "invalid symbol: empty",
                "for input {input:?}"
            );
        }
    }

    #[test]
    fn symbol_serializes_as_plain_string() {
        let symbol = Symbol::new("BTCUSDT").expect("symbol");
        let json = serde_json::to_string(&symbol).expect("serialize symbol");
        assert_eq!(json, "\"BTCUSDT\"");
    }

    // Deserialization must apply the same normalization as `Symbol::new`;
    // the derived impl used to bypass it entirely on `state_json`/`raw_json`
    // reloads from storage.
    #[test]
    fn symbol_deserialization_normalizes_like_new() {
        for (input, expected) in VALID_SYMBOL_CASES {
            let json = serde_json::to_string(input).expect("encode input");
            let symbol: Symbol = serde_json::from_str(&json).expect("deserialize symbol");
            assert_eq!(symbol.as_str(), expected, "for input {input:?}");
        }
    }

    #[test]
    fn symbol_deserialization_rejects_empty_with_model_error_message() {
        for raw in ["\"\"", "\"  \""] {
            let error = serde_json::from_str::<Symbol>(raw).expect_err("deserialize error");
            assert!(
                error.to_string().contains("invalid symbol: empty"),
                "for input {raw:?}, got {error}"
            );
        }
    }

    #[test]
    fn symbol_round_trips_through_serde_json() {
        let symbol = Symbol::new("BTCUSDT").expect("symbol");
        let json = serde_json::to_string(&symbol).expect("serialize symbol");
        let parsed: Symbol = serde_json::from_str(&json).expect("deserialize symbol");
        assert_eq!(parsed, symbol);
    }

    /// ADR 0002 §2's spellings for every product line: venue, category,
    /// storage name and serde token.
    const PRODUCT_LINES: [(ExchangeId, Venue, Category, &str, &str); 6] = [
        (
            ExchangeId::BinanceSpot,
            Venue::Binance,
            Category::Spot,
            "binance_spot",
            "BinanceSpot",
        ),
        (
            ExchangeId::BinanceLinear,
            Venue::Binance,
            Category::Linear,
            "binance_linear",
            "BinanceLinear",
        ),
        (
            ExchangeId::BinanceInverse,
            Venue::Binance,
            Category::Inverse,
            "binance_inverse",
            "BinanceInverse",
        ),
        (
            ExchangeId::BybitSpot,
            Venue::Bybit,
            Category::Spot,
            "bybit_spot",
            "BybitSpot",
        ),
        (
            ExchangeId::BybitLinear,
            Venue::Bybit,
            Category::Linear,
            "bybit_linear",
            "BybitLinear",
        ),
        (
            ExchangeId::BybitInverse,
            Venue::Bybit,
            Category::Inverse,
            "bybit_inverse",
            "BybitInverse",
        ),
    ];

    #[test]
    fn every_product_line_keeps_the_adr_spellings_on_both_channels() {
        for (exchange, venue, category, storage, token) in PRODUCT_LINES {
            assert_eq!(exchange.venue(), venue, "{exchange:?}");
            assert_eq!(exchange.category(), category, "{exchange:?}");
            assert_eq!(exchange.as_str(), storage, "{exchange:?}");
            assert_eq!(exchange.to_string(), storage, "{exchange:?}");
            assert_eq!(ExchangeId::from_storage_name(storage), Some(exchange));
            let json = serde_json::to_string(&exchange).expect("serialize exchange");
            assert_eq!(json, format!("\"{token}\""), "{exchange:?}");
            // The two channels never read each other.
            let storage_json = format!("\"{storage}\"");
            assert!(serde_json::from_str::<ExchangeId>(&storage_json).is_err());
            let parsed: ExchangeId = serde_json::from_str(&json).expect("deserialize exchange");
            assert_eq!(parsed, exchange);
        }
    }

    #[test]
    fn from_parts_names_one_line_for_each_venue_and_category() {
        let mut seen = std::collections::HashSet::new();
        for venue in Venue::ALL {
            for category in Category::ALL {
                let exchange = ExchangeId::from_parts(venue, category);
                assert_eq!((exchange.venue(), exchange.category()), (venue, category));
                assert!(seen.insert(exchange), "{exchange:?} named twice");
            }
        }
        assert_eq!(seen.len(), PRODUCT_LINES.len());
    }

    #[test]
    fn venues_and_categories_have_their_wire_names() {
        let venues: Vec<&str> = Venue::ALL.iter().map(|venue| venue.as_str()).collect();
        assert_eq!(venues, ["binance", "bybit"]);
        let categories: Vec<&str> = Category::ALL.iter().map(|c| c.as_str()).collect();
        assert_eq!(categories, ["spot", "linear", "inverse"]);
        let futures: Vec<bool> = Category::ALL.iter().map(|c| c.is_futures()).collect();
        assert_eq!(futures, [false, true, true]);
    }

    #[test]
    fn a_market_belongs_to_its_product_lines_category() {
        let symbol = Symbol::new("BTCUSDT").expect("symbol");
        let spot = MarketId::new(ExchangeId::BinanceSpot, symbol.clone(), Interval::M1);
        let linear = MarketId::new(ExchangeId::BybitLinear, symbol, Interval::M1);
        assert_eq!(spot.category(), Category::Spot);
        assert_eq!(linear.category(), Category::Linear);
    }

    #[test]
    fn market_id_json_with_lowercase_symbol_normalizes_on_deserialize() {
        let json = r#"{"exchange":"BinanceSpot","symbol":"btcusdt","interval":"M1"}"#;
        let market: MarketId = serde_json::from_str(json).expect("deserialize market");
        assert_eq!(market.symbol.as_str(), "BTCUSDT");
        assert_eq!(market.interval, Interval::M1);
    }
}
