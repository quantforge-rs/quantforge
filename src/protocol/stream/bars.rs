//! The frames that carry bars: a subscription, its bars, and its gaps.

use serde::{Deserialize, Serialize};

use super::super::ExactDecimal;

/// Asks for one market's closed bars: from `resume_from_open_ms` when it is
/// given, and otherwise from the next bar to close.
///
/// ```
/// use quantforge::protocol::stream::ClientFrame;
///
/// let json = r#"{"type":"subscribe","exchange":"binance","category":"spot","symbol":"BTCUSDT","interval":"1m"}"#;
/// let ClientFrame::Subscribe(subscribe) = serde_json::from_str(json)? else {
///     panic!("a subscribe frame");
/// };
/// assert_eq!(subscribe.resume_from_open_ms, None);
/// assert_eq!(serde_json::to_string(&ClientFrame::Subscribe(subscribe))?, json);
/// # Ok::<(), serde_json::Error>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subscribe {
    /// The exchange, by the name QuantForge registers it under.
    pub exchange: String,
    /// The product family: `spot`, `linear` or `inverse`. With `exchange` it
    /// names the product line.
    pub category: String,
    /// The exchange's symbol, in uppercase.
    pub symbol: String,
    /// The bar interval, such as `1m`.
    pub interval: String,
    /// The open time of the first bar the client wants. The server replays
    /// its stored bars from there, at most
    /// [`SessionLimits::max_resume_bars`](super::SessionLimits::max_resume_bars)
    /// of them, and then goes on with live ones. Absent: live bars only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resume_from_open_ms: Option<i64>,
}

/// One closed bar of a subscribed market.
///
/// ```
/// use quantforge::protocol::stream::ServerFrame;
///
/// let json = r#"{"type":"bar_closed","exchange":"binance","category":"spot","symbol":"BTCUSDT","interval":"1m","bar":{"open_time_ms":1792454400000,"close_time_ms":1792454459999,"open":"67012.34000000","high":"67020.00000000","low":"67001.10000000","close":"67015.50000000","volume":"12.04510000","trades":1234}}"#;
/// let ServerFrame::BarClosed(closed) = serde_json::from_str(json)? else {
///     panic!("a bar_closed frame");
/// };
/// assert_eq!(closed.bar.close.to_string(), "67015.50000000");
/// assert_eq!(closed.bar.close_time_ms - closed.bar.open_time_ms, 59_999);
/// assert_eq!(serde_json::to_string(&ServerFrame::BarClosed(closed))?, json);
/// # Ok::<(), serde_json::Error>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BarClosed {
    /// The exchange, by the name QuantForge registers it under.
    pub exchange: String,
    /// The product family: `spot`, `linear` or `inverse`. With `exchange` it
    /// names the product line.
    pub category: String,
    /// The exchange's symbol, in uppercase.
    pub symbol: String,
    /// The bar interval, such as `1m`.
    pub interval: String,
    /// The bar.
    pub bar: Bar,
}

/// A closed bar: the prices and volume of one interval.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bar {
    /// When the bar opened, on the interval's grid.
    pub open_time_ms: i64,
    /// The bar's last millisecond: its open time, plus the interval, minus
    /// one.
    pub close_time_ms: i64,
    /// The first trade price.
    pub open: ExactDecimal,
    /// The highest trade price.
    pub high: ExactDecimal,
    /// The lowest trade price.
    pub low: ExactDecimal,
    /// The last trade price.
    pub close: ExactDecimal,
    /// The quantity traded, in the base asset.
    pub volume: ExactDecimal,
    /// How many trades the exchange counted; absent when it reported no
    /// count, which is never the same as zero.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trades: Option<u64>,
}

/// Bars of a subscribed market that the server cannot deliver: every bar
/// whose open time lies from `first_open_ms` to `last_open_ms`.
///
/// ```
/// use quantforge::protocol::stream::{GapReason, ServerFrame};
///
/// let json = r#"{"type":"gap_notice","exchange":"binance","category":"spot","symbol":"BTCUSDT","interval":"1m","first_open_ms":1792454460000,"last_open_ms":1792454580000,"reason":"no_data"}"#;
/// let ServerFrame::GapNotice(gap) = serde_json::from_str(json)? else {
///     panic!("a gap_notice frame");
/// };
/// assert_eq!(gap.reason, GapReason::NoData);
/// assert_eq!(serde_json::to_string(&ServerFrame::GapNotice(gap))?, json);
/// # Ok::<(), serde_json::Error>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GapNotice {
    /// The exchange, by the name QuantForge registers it under.
    pub exchange: String,
    /// The product family: `spot`, `linear` or `inverse`. With `exchange` it
    /// names the product line.
    pub category: String,
    /// The exchange's symbol, in uppercase.
    pub symbol: String,
    /// The bar interval, such as `1m`.
    pub interval: String,
    /// The open time of the first missing bar.
    pub first_open_ms: i64,
    /// The open time of the last missing bar.
    pub last_open_ms: i64,
    /// Why they are missing.
    pub reason: GapReason,
}

/// Why bars are missing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum GapReason {
    /// The server holds no bars there: the exchange published none, or the
    /// server never received them.
    NoData,
    /// The bars are older than a resume replays
    /// ([`SessionLimits::max_resume_bars`](super::SessionLimits::max_resume_bars));
    /// they can be fetched as segments instead.
    BeyondResume,
    /// The client fell too far behind, and the server dropped these bars to
    /// catch it up. A later `subscribe` can resume from them while they are
    /// recent enough.
    SlowConsumer,
    /// A reason this build does not know, from a later revision of the
    /// protocol. Never sent: serializing it fails.
    #[serde(other, skip_serializing)]
    Unrecognized,
}

#[cfg(test)]
mod tests {
    use super::super::ServerFrame;
    use super::*;

    const BAR_CLOSED: &str = r#"{"type":"bar_closed","exchange":"binance","category":"spot","symbol":"BTCUSDT","interval":"1m","bar":{"open_time_ms":1792454400000,"close_time_ms":1792454459999,"open":"67012.34000000","high":"67020.00000000","low":"67001.10000000","close":"67015.50000000","volume":"12.04510000","trades":1234}}"#;

    /// Every reason a server sends, with its wire name.
    const REASONS: [(GapReason, &str); 3] = [
        (GapReason::NoData, "no_data"),
        (GapReason::BeyondResume, "beyond_resume"),
        (GapReason::SlowConsumer, "slow_consumer"),
    ];

    #[test]
    fn every_gap_reason_has_one_wire_name() {
        for (reason, name) in REASONS {
            // Exhaustive on purpose: a new reason stops this module from
            // compiling until it is added to REASONS.
            match reason {
                GapReason::NoData
                | GapReason::BeyondResume
                | GapReason::SlowConsumer
                | GapReason::Unrecognized => {}
            }
            let json = format!("\"{name}\"");
            assert_eq!(serde_json::to_string(&reason).expect("serialize"), json);
            assert_eq!(
                serde_json::from_str::<GapReason>(&json).expect("reason"),
                reason
            );
        }
        assert!(serde_json::to_string(&GapReason::Unrecognized).is_err());
    }

    #[test]
    fn a_gap_reason_this_build_does_not_know_is_unrecognized_and_keeps_the_gap() {
        let json = r#"{"type":"gap_notice","exchange":"binance","category":"spot","symbol":"BTCUSDT","interval":"1m","first_open_ms":1,"last_open_ms":2,"reason":"maintenance"}"#;
        let frame: ServerFrame = serde_json::from_str(json).expect("frame");
        assert_eq!(
            frame,
            ServerFrame::GapNotice(GapNotice {
                exchange: "binance".to_string(),
                category: "spot".to_string(),
                symbol: "BTCUSDT".to_string(),
                interval: "1m".to_string(),
                first_open_ms: 1,
                last_open_ms: 2,
                reason: GapReason::Unrecognized,
            })
        );
    }

    // A frame's fields are buffered before they are read, which must not
    // loosen what a decimal accepts.
    #[test]
    fn every_price_and_the_volume_must_be_decimal_strings_inside_a_frame() {
        assert!(serde_json::from_str::<ServerFrame>(BAR_CLOSED).is_ok());
        for field in ["open", "high", "low", "close", "volume"] {
            let start = BAR_CLOSED
                .find(&format!(r#""{field}":""#))
                .expect("the field is in the fixture");
            // Drop the quotes around the value, leaving the same digits.
            let open_quote = start + field.len() + 3;
            let close_quote = open_quote
                + 1
                + BAR_CLOSED[open_quote + 1..]
                    .find('"')
                    .expect("closing quote");
            let number = format!(
                "{}{}{}",
                &BAR_CLOSED[..open_quote],
                &BAR_CLOSED[open_quote + 1..close_quote],
                &BAR_CLOSED[close_quote + 1..]
            );
            serde_json::from_str::<serde_json::Value>(&number).expect("still valid JSON");
            assert!(
                serde_json::from_str::<ServerFrame>(&number).is_err(),
                "{field} as a JSON number must be refused: {number}"
            );
        }
    }

    #[test]
    fn an_unreported_trade_count_is_left_out_and_read_from_null_or_absence() {
        let without = BAR_CLOSED.replace(r#","trades":1234"#, "");
        let frame: ServerFrame = serde_json::from_str(&without).expect("frame");
        let ServerFrame::BarClosed(closed) = &frame else {
            panic!("a bar_closed frame");
        };
        assert_eq!(closed.bar.trades, None);
        assert_eq!(serde_json::to_string(&frame).expect("serialize"), without);
        let null = BAR_CLOSED.replace(r#""trades":1234"#, r#""trades":null"#);
        assert_eq!(
            serde_json::from_str::<ServerFrame>(&null).expect("frame"),
            frame
        );
    }

    #[test]
    fn a_subscribe_without_a_resume_point_is_left_without_one() {
        let live = Subscribe {
            exchange: "binance".to_string(),
            category: "spot".to_string(),
            symbol: "BTCUSDT".to_string(),
            interval: "1m".to_string(),
            resume_from_open_ms: None,
        };
        let json = r#"{"exchange":"binance","category":"spot","symbol":"BTCUSDT","interval":"1m"}"#;
        assert_eq!(serde_json::to_string(&live).expect("serialize"), json);
        let null = r#"{"exchange":"binance","category":"spot","symbol":"BTCUSDT","interval":"1m","resume_from_open_ms":null}"#;
        assert_eq!(
            serde_json::from_str::<Subscribe>(null).expect("subscribe"),
            live
        );
    }
}
