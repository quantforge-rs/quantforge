//! The `/v1/candles` queries refused before the store is asked anything: a
//! value not in its canonical form, a reversed range, an integer the wire
//! cannot carry, and a range longer than the server serves.

use super::*;
use crate::protocol::segments::CandleRangeQuery;
use crate::server::selection::Selection;

#[tokio::test]
async fn a_query_is_refused_before_the_store_is_asked_unless_every_value_is_canonical() {
    let bed = Bed::new();
    // Any store call fails this test with a 500, not the 400 it expects.
    let store = Faulty::failing(bed.store.within(Duration::from_secs(2)), || {
        StorageError::Other("the store was asked".into())
    });
    let limits = HistoryLimits {
        max_span_bars: 10,
        ..HistoryLimits::default()
    };
    let address = start(History::new(store, limits)).await;
    let good = query(VENUE, Some(Category::Spot), "BTCUSDT", T, T + 9 * MINUTE);
    let with = |name: &str, value: &str| -> Vec<(&'static str, String)> {
        good.iter()
            .map(|(key, old)| {
                (
                    *key,
                    if *key == name {
                        value.to_string()
                    } else {
                        old.clone()
                    },
                )
            })
            .collect()
    };
    let without = |name: &str| -> Vec<(&'static str, String)> {
        good.iter()
            .filter(|(key, _)| *key != name)
            .cloned()
            .collect()
    };
    let exchanges: Vec<&str> = Venue::ALL.iter().map(|venue| venue.as_str()).collect();
    let cases: Vec<(Vec<(&'static str, String)>, String)> = vec![
        (with("exchange", "Kraken"), format!("exchange \"Kraken\" is not one QuantForge names; write one of {}", exchanges.join(", "))),
        (with("exchange", &VENUE.as_str().to_uppercase()), "is not one QuantForge names".to_string()),
        (with("category", "SPOT"), "category \"SPOT\" is not spot, linear or inverse".to_string()),
        (with("category", ""), "category \"\" is not spot, linear or inverse".to_string()),
        (with("symbol", "btcusdt"), "symbol \"btcusdt\" is not in its canonical form; write \"BTCUSDT\"".to_string()),
        (with("symbol", " BTCUSDT"), "symbol \" BTCUSDT\" is not in its canonical form".to_string()),
        (with("symbol", ""), "symbol is empty".to_string()),
        (with("interval", "1M"), "interval \"1M\" is not one of QuantForge's intervals".to_string()),
        (with("interval", "60s"), "interval \"60s\" is not one of".to_string()),
        (with("start_open_ms", &(T + MINUTE * 10).to_string()), format!("start_open_ms {} is after end_open_ms {}", T + 10 * MINUTE, T + 9 * MINUTE)),
        (with("start_open_ms", "9007199254740992"), "start_open_ms 9007199254740992 is not below 2^53 in magnitude".to_string()),
        (with("end_open_ms", "-9007199254740992"), "end_open_ms -9007199254740992 is not below 2^53".to_string()),
        (with("end_open_ms", "soon"), "the query string is not a /v1/candles query".to_string()),
        (without("symbol"), "missing field `symbol`".to_string()),
        (without("start_open_ms"), "missing field `start_open_ms`".to_string()),
        ([good.clone(), vec![("symbol", "ETHUSDT".to_string())]].concat(), "duplicate field `symbol`".to_string()),
        (with("end_open_ms", &(T + 10 * MINUTE).to_string()), "the range spans 11 bars of 1m, more than the 10 this server serves in one answer; ask for a shorter range".to_string()),
    ];
    for (query, expected) in cases {
        let (status, code, message) = refusal(get_candles(address, &query).await).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "invalid_request"),
            "{query:?}: {message}"
        );
        assert!(
            message.contains(&expected),
            "{query:?}: {message:?} lacks {expected:?}"
        );
    }

    // A query in its canonical form reaches the store, which this one
    // refuses.
    let (status, ..) = refusal(get_candles(address, &good).await).await;
    assert_eq!(status, 500);
}

#[test]
fn a_span_of_exactly_the_limit_is_served_and_one_bar_more_refused() {
    let read = |start: TimestampMs, end: TimestampMs, max: u64| {
        let query = CandleRangeQuery {
            exchange: VENUE.as_str().to_string(),
            category: Some("spot".to_string()),
            symbol: "BTCUSDT".to_string(),
            interval: "1m".to_string(),
            start_open_ms: start,
            end_open_ms: end,
        };
        Selection::read(query, max)
    };
    assert!(read(T, T, 1).is_ok());
    assert!(
        read(T, T + MINUTE - 1, 1).is_ok(),
        "one bar's open time at most"
    );
    assert!(read(T, T + MINUTE, 1).is_err());
    assert!(read(T, T + 527_039 * MINUTE, DEFAULT_MAX_SPAN_BARS).is_ok());
    let refused = read(T, T + 527_040 * MINUTE, DEFAULT_MAX_SPAN_BARS).expect_err("one bar more");
    assert_eq!(refused.code, crate::protocol::ErrorCode::InvalidRequest);
    assert!(
        refused
            .message
            .starts_with("the range spans 527041 bars of 1m")
    );
    let bound = (1_i64 << 53) - 1;
    assert!(
        read(-bound, bound, u64::MAX).is_ok(),
        "the widest range the wire carries"
    );
}
