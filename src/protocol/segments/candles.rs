//! One market's bars in a range: what `GET /v1/candles` asks for and
//! answers.

use serde::{Deserialize, Serialize};

use super::super::stream::Bar;
use super::SegmentEntry;

/// The query string of `GET /v1/candles`: one market's closed bars whose
/// open times lie from `start_open_ms` to `end_open_ms`, both included.
/// Each field is a parameter of the same name, its value percent-encoded as
/// in any query string:
///
/// ```text
/// GET /v1/candles?exchange=binance&category=spot&symbol=BTCUSDT&interval=1m&start_open_ms=1792454400000&end_open_ms=1792540740000
/// ```
///
/// A server ignores a parameter it does not know, and refuses one given
/// twice. A client in Rust can hand this struct to any HTTP client that
/// encodes a query string with serde.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandleRangeQuery {
    /// The exchange, by the name QuantForge registers it under.
    pub exchange: String,
    /// The product family: `spot`, `linear` or `inverse`. With `exchange` it
    /// names the product line. Left out only in the form a market took
    /// before it carried one, which a server reads as `spot` only while
    /// that is unambiguous (see the [module docs](super)).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// The exchange's symbol, in uppercase.
    pub symbol: String,
    /// The bar interval, such as `1m`.
    pub interval: String,
    /// The open time of the first bar wanted.
    pub start_open_ms: i64,
    /// The open time of the last bar wanted.
    pub end_open_ms: i64,
}

/// What `GET /v1/candles` answers: one market's closed bars whose open times
/// lie from `start_open_ms` to `end_open_ms`, as the segments that hold them
/// and the tip after them. The [module docs](super) say how to read them.
///
/// ```
/// use quantforge::protocol::segments::CandleRange;
///
/// let json = r#"{"exchange":"binance","category":"spot","symbol":"BTCUSDT","interval":"1m","start_open_ms":1792454400000,"end_open_ms":1792540800000,"segments":[{"hash":"sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855","format":1,"exchange":"binance","category":"spot","symbol":"BTCUSDT","interval":"1m","range":{"first_open_ms":1792454400000,"last_open_ms":1792540740000,"bar_count":1440},"size_bytes":48213}],"tip":[{"open_time_ms":1792540800000,"close_time_ms":1792540859999,"open":"67012.34000000","high":"67020.00000000","low":"67001.10000000","close":"67015.50000000","volume":"12.04510000","trades":1234}]}"#;
/// let range: CandleRange = serde_json::from_str(json)?;
/// assert_eq!(range.segments[0].range.bar_count, 1440);
/// assert_eq!(range.tip[0].close.to_string(), "67015.50000000");
/// assert_eq!(range.message, None);
/// assert_eq!(serde_json::to_string(&range)?, json);
/// # Ok::<(), serde_json::Error>(())
/// ```
///
/// Nothing stored is an answer like any other:
///
/// ```
/// use quantforge::protocol::segments::CandleRange;
///
/// let json = r#"{"exchange":"binance","category":"spot","symbol":"DOGEUSDT","interval":"1m","start_open_ms":1792454400000,"end_open_ms":1792540740000,"segments":[],"tip":[],"message":"the server stores no bars of binance spot DOGEUSDT 1m"}"#;
/// let range: CandleRange = serde_json::from_str(json)?;
/// assert!(range.segments.is_empty() && range.tip.is_empty());
/// assert!(range.message.is_some());
/// assert_eq!(serde_json::to_string(&range)?, json);
/// # Ok::<(), serde_json::Error>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandleRange {
    /// The exchange, by the name QuantForge registers it under.
    pub exchange: String,
    /// The product family the bars belong to: `spot`, `linear` or
    /// `inverse`, named even when the query left it out.
    pub category: String,
    /// The exchange's symbol, in uppercase.
    pub symbol: String,
    /// The bar interval, such as `1m`.
    pub interval: String,
    /// The open time of the first bar asked for.
    pub start_open_ms: i64,
    /// The open time of the last bar asked for.
    pub end_open_ms: i64,
    /// The segments whose bars span part of the range, oldest first. The
    /// first and the last can also hold bars outside it, which a reader
    /// drops.
    pub segments: Vec<SegmentEntry>,
    /// The range's bars after the last segment's, in open-time order: bars
    /// not packed into a segment yet, such as the newest window's.
    pub tip: Vec<Bar>,
    /// Why the answer lists neither segments nor a tip: the server stores
    /// no bars of the market, or none in the range. Absent otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::super::tests::entry;
    use super::*;

    fn query(category: Option<&str>) -> CandleRangeQuery {
        CandleRangeQuery {
            exchange: "binance".to_string(),
            category: category.map(str::to_string),
            symbol: "BTCUSDT".to_string(),
            interval: "1h".to_string(),
            start_open_ms: -3_600_000,
            end_open_ms: 1_792_537_200_000,
        }
    }

    #[test]
    fn a_query_names_its_parameters_and_leaves_out_an_absent_category() {
        // Serde's names are the parameter names in any encoding, the query
        // string's included.
        assert_eq!(
            serde_json::to_string(&query(Some("linear"))).expect("serialize"),
            r#"{"exchange":"binance","category":"linear","symbol":"BTCUSDT","interval":"1h","start_open_ms":-3600000,"end_open_ms":1792537200000}"#
        );
        let legacy = r#"{"exchange":"binance","symbol":"BTCUSDT","interval":"1h","start_open_ms":-3600000,"end_open_ms":1792537200000}"#;
        assert_eq!(
            serde_json::to_string(&query(None)).expect("serialize"),
            legacy
        );
        assert_eq!(
            serde_json::from_str::<CandleRangeQuery>(legacy).expect("query"),
            query(None)
        );
    }

    #[test]
    fn a_query_needs_every_parameter_but_category() {
        let json = serde_json::to_string(&query(Some("spot"))).expect("serialize");
        for field in [
            r#""exchange":"binance","#,
            r#""symbol":"BTCUSDT","#,
            r#""interval":"1h","#,
            r#""start_open_ms":-3600000,"#,
            r#","end_open_ms":1792537200000"#,
        ] {
            let without = json.replace(field, "");
            assert_ne!(without, json, "{field} is in the fixture");
            assert!(
                serde_json::from_str::<CandleRangeQuery>(&without).is_err(),
                "a query without {field} must be refused"
            );
        }
    }

    #[test]
    fn an_answer_has_its_exact_json_and_a_message_only_when_it_has_one() {
        let range = CandleRange {
            exchange: "binance".to_string(),
            category: "spot".to_string(),
            symbol: "BTCUSDT".to_string(),
            interval: "1h".to_string(),
            start_open_ms: 1_792_454_400_000,
            end_open_ms: 1_792_537_200_000,
            segments: vec![entry()],
            tip: Vec::new(),
            message: None,
        };
        let json = serde_json::to_string(&range).expect("serialize");
        assert!(json.ends_with(r#""size_bytes":1204}],"tip":[]}"#), "{json}");
        assert_eq!(
            serde_json::from_str::<CandleRange>(&json).expect("range"),
            range
        );

        let empty = CandleRange {
            segments: Vec::new(),
            message: Some("nothing".to_string()),
            ..range
        };
        let json = serde_json::to_string(&empty).expect("serialize");
        assert!(
            json.ends_with(r#""segments":[],"tip":[],"message":"nothing"}"#),
            "{json}"
        );
        assert!(
            serde_json::from_str::<CandleRange>(&json.replace(r#","tip":[]"#, "")).is_err(),
            "an answer without its tip must be refused"
        );
    }
}
