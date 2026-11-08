//! The markets a server stores bars of: what `GET /v1/markets` answers.

use serde::{Deserialize, Serialize};

/// Every market the server stores bars of, in the order of their exchange,
/// category, symbol and interval texts. A server that stores none answers
/// an empty list.
///
/// ```
/// use quantforge::protocol::segments::MarketIndex;
///
/// let json = r#"{"markets":[{"exchange":"binance","category":"spot","symbol":"BTCUSDT","interval":"1m","first_open_ms":1792454400000,"last_open_ms":1792540740000}]}"#;
/// let index: MarketIndex = serde_json::from_str(json)?;
/// assert_eq!(index.markets[0].category, "spot");
/// assert_eq!(index.markets[0].last_open_ms, 1_792_540_740_000);
/// assert_eq!(serde_json::to_string(&index)?, json);
/// # Ok::<(), serde_json::Error>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarketIndex {
    /// One entry per market.
    pub markets: Vec<MarketEntry>,
}

/// One market the server stores bars of, and the open times its bars span.
/// Within the span the server can still lack bars: where the exchange
/// published none, or where the server never received them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarketEntry {
    /// The exchange, by the name QuantForge registers it under.
    pub exchange: String,
    /// The product family: `spot`, `linear` or `inverse`. With `exchange` it
    /// names the product line.
    pub category: String,
    /// The exchange's symbol, in uppercase.
    pub symbol: String,
    /// The bar interval, such as `1m`.
    pub interval: String,
    /// The open time of the first bar the server stores.
    pub first_open_ms: i64,
    /// The open time of the last bar the server stores.
    pub last_open_ms: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(category: &str) -> MarketEntry {
        MarketEntry {
            exchange: "binance".to_string(),
            category: category.to_string(),
            symbol: "BTCUSDT".to_string(),
            interval: "1m".to_string(),
            first_open_ms: -60_000,
            last_open_ms: 1_792_540_740_000,
        }
    }

    #[test]
    fn an_index_has_its_exact_json() {
        let index = MarketIndex {
            markets: vec![entry("spot"), entry("linear")],
        };
        let json = r#"{"markets":[{"exchange":"binance","category":"spot","symbol":"BTCUSDT","interval":"1m","first_open_ms":-60000,"last_open_ms":1792540740000},{"exchange":"binance","category":"linear","symbol":"BTCUSDT","interval":"1m","first_open_ms":-60000,"last_open_ms":1792540740000}]}"#;
        assert_eq!(serde_json::to_string(&index).expect("serialize"), json);
        assert_eq!(
            serde_json::from_str::<MarketIndex>(json).expect("index"),
            index
        );
    }

    #[test]
    fn an_empty_index_is_an_object_with_an_empty_list() {
        let empty = MarketIndex {
            markets: Vec::new(),
        };
        assert_eq!(
            serde_json::to_string(&empty).expect("serialize"),
            r#"{"markets":[]}"#
        );
        assert!(serde_json::from_str::<MarketIndex>("[]").is_err());
    }

    #[test]
    fn every_field_is_required_and_unknown_ones_are_ignored() {
        let json = r#"{"exchange":"binance","category":"spot","symbol":"BTCUSDT","interval":"1m","first_open_ms":-60000,"last_open_ms":1792540740000}"#;
        for field in [
            r#""category":"spot","#,
            r#""first_open_ms":-60000,"#,
            r#","last_open_ms":1792540740000"#,
        ] {
            let without = json.replace(field, "");
            assert_ne!(without, json, "{field} is in the fixture");
            assert!(
                serde_json::from_str::<MarketEntry>(&without).is_err(),
                "an entry without {field} must be refused"
            );
        }
        let extended = json.replace(r#""interval":"1m","#, r#""interval":"1m","bars":3,"#);
        assert_eq!(
            serde_json::from_str::<MarketEntry>(&extended).expect("entry"),
            entry("spot")
        );
    }
}
