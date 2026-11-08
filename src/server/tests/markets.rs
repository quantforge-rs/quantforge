//! `GET /v1/markets`: every market the store holds, in order, with the
//! open times of its first and last bar.

use super::*;

#[tokio::test]
async fn markets_lists_every_stored_market_in_order_with_the_span_of_its_bars() {
    let bed = Bed::new();
    let empty = client()
        .get(format!("http://{}/v1/markets", start(bed.history()).await))
        .send()
        .await
        .expect("a response");
    assert_eq!(empty.status(), 200);
    assert_eq!(header(&empty, CACHE_CONTROL), Some("no-cache"));
    assert_eq!(empty.text().await.expect("a body"), r#"{"markets":[]}"#);

    let five = |open: TimestampMs| Candle {
        close_time_ms: open + 5 * MINUTE - 1,
        ..candle(open, "1", None)
    };
    let seeds = [
        (
            market(Category::Spot),
            vec![candle(T, "1", None), candle(T + DAY, "1", None)],
        ),
        (
            market(Category::Linear),
            vec![candle(T + MINUTE, "1", None)],
        ),
        (
            market_of(VENUE, Category::Inverse, "BTCUSD", Interval::M1),
            vec![candle(-MINUTE, "1", None)],
        ),
        (
            market_of(VENUE, Category::Spot, "ETHUSDT", Interval::M5),
            vec![five(T), five(T + 5 * MINUTE)],
        ),
        (
            market_of(OTHER_VENUE, Category::Spot, "BTCUSDT", Interval::M1),
            vec![candle(T - DAY, "1", None)],
        ),
    ];
    for (market, candles) in &seeds {
        bed.store
            .upsert_candles(market, candles)
            .expect("the candles");
    }
    let address = start(bed.history()).await;
    let response = client()
        .get(format!("http://{address}/v1/markets"))
        .send()
        .await
        .expect("a response");
    assert_eq!(response.status(), 200);
    let entry = |venue: Venue,
                 category: &str,
                 symbol: &str,
                 interval: &str,
                 first: i64,
                 last: i64| {
        format!(
            r#"{{"exchange":"{}","category":"{category}","symbol":"{symbol}","interval":"{interval}","first_open_ms":{first},"last_open_ms":{last}}}"#,
            venue.as_str()
        )
    };
    let expected = format!(
        r#"{{"markets":[{},{},{},{},{}]}}"#,
        entry(VENUE, "inverse", "BTCUSD", "1m", -MINUTE, -MINUTE),
        entry(VENUE, "linear", "BTCUSDT", "1m", T + MINUTE, T + MINUTE),
        entry(VENUE, "spot", "BTCUSDT", "1m", T, T + DAY),
        entry(VENUE, "spot", "ETHUSDT", "5m", T, T + 5 * MINUTE),
        entry(OTHER_VENUE, "spot", "BTCUSDT", "1m", T - DAY, T - DAY),
    );
    assert_eq!(response.text().await.expect("a body"), expected);

    // In that order whatever order the store lists them in.
    let backwards = Faulty::backwards(bed.store.within(Duration::from_secs(2)));
    let address = start(History::new(backwards, HistoryLimits::default())).await;
    let response = client()
        .get(format!("http://{address}/v1/markets"))
        .send()
        .await
        .expect("a response");
    assert_eq!(response.text().await.expect("a body"), expected);
}
