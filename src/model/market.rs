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

impl FromStr for Category {
    type Err = ModelError;

    /// Reads the name [`as_str`](Self::as_str) writes, exactly: nothing is
    /// trimmed, and case counts.
    fn from_str(name: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|category| category.as_str() == name)
            .ok_or_else(|| ModelError::InvalidCategory(name.to_string()))
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

    /// Every product line: venue by venue, and within a venue Spot first.
    pub fn all() -> impl Iterator<Item = Self> {
        Venue::ALL
            .into_iter()
            .flat_map(|venue| Category::ALL.map(|category| Self::from_parts(venue, category)))
    }

    /// The line stored under `name`, the inverse of [`as_str`](Self::as_str):
    /// how a storage column is read back. It is not the serde channel, which
    /// reads the token only.
    pub fn from_storage_name(name: &str) -> Option<Self> {
        Self::all().find(|line| line.as_str() == name)
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

impl FromStr for ExchangeId {
    type Err = ModelError;

    /// Reads a storage name, as [`from_storage_name`](Self::from_storage_name)
    /// does, and names the lines there are when `name` is none of them. A
    /// venue's name alone is not a product line: nothing here takes it for
    /// the venue's Spot line.
    fn from_str(name: &str) -> Result<Self, Self::Err> {
        Self::from_storage_name(name).ok_or_else(|| ModelError::InvalidExchange(name.to_string()))
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
mod tests;
