//! Market identity: the normalized trading symbol, the exchange it lives
//! on, and the (exchange, symbol, interval) key every candle and run is
//! stored under.

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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ExchangeId {
    BinanceSpot,
}

impl ExchangeId {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BinanceSpot => "binance_spot",
        }
    }
}

impl fmt::Display for ExchangeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

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

    #[test]
    fn market_id_json_with_lowercase_symbol_normalizes_on_deserialize() {
        let json = r#"{"exchange":"BinanceSpot","symbol":"btcusdt","interval":"M1"}"#;
        let market: MarketId = serde_json::from_str(json).expect("deserialize market");
        assert_eq!(market.symbol.as_str(), "BTCUSDT");
        assert_eq!(market.interval, Interval::M1);
    }
}
