//! The history routes: the markets the store holds, a range read back from
//! its segments and tip exactly as the store holds it, the answer when
//! nothing is stored, and a query without category.

use super::*;
use crate::model::ms_to_rfc3339;
use crate::protocol::segments::CandleRange;
use crate::protocol::stream::Bar;
use crate::storage::segments;

/// Three days of BTCUSDT `1m` Spot: two windows a refresh packs, the first
/// with gaps, and three bars of the third, the tip. The decimals carry
/// every scale, trailing zeros included, and some bars have no trade count.
fn three_days() -> Vec<Candle> {
    let first = (0..1440).filter(|minute| minute % 97 != 13).map(|minute| {
        let close = format!("{}.{:08}", 67_000 + minute % 50, minute);
        let trades = (minute % 5 != 0).then_some(minute as u64);
        candle(T + minute * MINUTE, &close, trades)
    });
    let second = (0..1440).map(|minute| {
        let close = match minute % 3 {
            0 => format!("1.{}", minute % 10),
            1 => format!("{}", 100 + minute),
            _ => "0.00000001".to_string(),
        };
        candle(T + DAY + minute * MINUTE, &close, Some(0))
    });
    let tip = [
        ("123456789.123456789", None),
        ("5", Some(7)),
        ("0.10", Some(u64::MAX)),
    ]
    .into_iter()
    .enumerate()
    .map(|(minute, (close, trades))| candle(T + 2 * DAY + minute as i64 * MINUTE, close, trades));
    first.chain(second).chain(tip).collect()
}

/// A bed holding [`three_days`] of the Spot market, its two closed windows
/// packed.
fn seeded() -> Bed {
    let bed = Bed::new();
    let spot = market(Category::Spot);
    bed.store
        .upsert_candles(&spot, &three_days())
        .expect("the candles");
    let refreshed = bed.store.refresh_segments(&spot).expect("a refresh");
    assert_eq!(refreshed.built, 2, "{refreshed:?}");
    bed
}

async fn range(address: SocketAddr, query: &[(&str, String)]) -> CandleRange {
    let response = get_candles(address, query).await;
    assert_eq!(response.status(), 200);
    assert_eq!(header(&response, CACHE_CONTROL), Some("no-cache"));
    response.json().await.expect("a candle range")
}

fn bar_text(bar: &Bar) -> String {
    text(&Candle {
        open_time_ms: bar.open_time_ms,
        close_time_ms: bar.close_time_ms,
        open: bar.open.get(),
        high: bar.high.get(),
        low: bar.low.get(),
        close: bar.close.get(),
        volume: bar.volume.get(),
        trades: bar.trades,
    })
}

/// The bars a client reads from `range`, as the protocol says: each segment
/// fetched by its hash and checked against its entry and its name, its bars
/// in the range kept, then the tip.
async fn read_back(address: SocketAddr, range: &CandleRange) -> Vec<String> {
    let mut bars = Vec::new();
    let mut previous_last = None;
    for entry in &range.segments {
        assert_eq!(entry.format, 1);
        let market = (
            &entry.exchange,
            &entry.category,
            &entry.symbol,
            &entry.interval,
        );
        assert_eq!(
            market,
            (
                &range.exchange,
                &range.category,
                &range.symbol,
                &range.interval
            )
        );
        assert!(
            previous_last < Some(entry.range.first_open_ms),
            "oldest first"
        );
        previous_last = Some(entry.range.last_open_ms);
        let response = client()
            .get(format!("http://{address}/v1/segments/{}", entry.hash))
            .send()
            .await
            .expect("a response");
        assert_eq!(response.status(), 200);
        let bytes = response.bytes().await.expect("the bytes");
        assert_eq!(bytes.len() as u64, entry.size_bytes);
        let name = SegmentHash::from_digest(*entry.hash.digest());
        let candles = segments::open(&bytes, &name).expect("a segment that matches its name");
        assert_eq!(candles.len(), entry.range.bar_count as usize);
        assert_eq!(
            (
                candles.first().map(|c| c.open_time_ms),
                candles.last().map(|c| c.open_time_ms)
            ),
            (
                Some(entry.range.first_open_ms),
                Some(entry.range.last_open_ms)
            )
        );
        let wanted = range.start_open_ms..=range.end_open_ms;
        bars.extend(
            candles
                .iter()
                .filter(|candle| wanted.contains(&candle.open_time_ms))
                .map(text),
        );
    }
    if let (Some(last), Some(first_of_tip)) = (previous_last, range.tip.first()) {
        assert!(
            last < first_of_tip.open_time_ms,
            "the tip comes after every segment"
        );
    }
    bars.extend(range.tip.iter().map(bar_text));
    bars
}

#[tokio::test]
async fn a_range_reads_back_exactly_what_the_store_holds_from_its_segments_and_tip() {
    let bed = seeded();
    let spot = market(Category::Spot);
    let address = start(bed.history()).await;
    // (start, end, segments, bars in the tip)
    let cases = [
        (T - DAY, T + 3 * DAY, 2, 3),
        (T + 100 * MINUTE, T + DAY + 50 * MINUTE, 2, 0),
        (T + 2 * DAY, T + 2 * DAY + 10 * MINUTE, 0, 3),
        (T + DAY + 1439 * MINUTE, T + 2 * DAY + MINUTE, 1, 2),
        (T + 7 * MINUTE, T + 7 * MINUTE, 1, 0),
    ];
    for (start_ms, end_ms, segment_count, tip_count) in cases {
        let answer = range(
            address,
            &query(VENUE, Some(Category::Spot), "BTCUSDT", start_ms, end_ms),
        )
        .await;
        let what = format!("{start_ms}..={end_ms}");
        assert_eq!(
            (answer.exchange.as_str(), answer.category.as_str()),
            (VENUE.as_str(), "spot")
        );
        assert_eq!(
            (answer.symbol.as_str(), answer.interval.as_str()),
            ("BTCUSDT", "1m")
        );
        assert_eq!(
            (answer.start_open_ms, answer.end_open_ms),
            (start_ms, end_ms)
        );
        assert_eq!(answer.segments.len(), segment_count, "{what}");
        assert_eq!(answer.tip.len(), tip_count, "{what}");
        assert_eq!(answer.message, None, "{what}");

        let stored: Vec<String> = bed
            .store
            .load_candles(
                &spot,
                CandleQuery {
                    start_time_ms: Some(start_ms),
                    end_time_ms: Some(end_ms),
                    limit: None,
                },
            )
            .expect("a direct read")
            .iter()
            .map(text)
            .collect();
        assert!(!stored.is_empty(), "{what}");
        assert_eq!(read_back(address, &answer).await, stored, "{what}");
    }
}

#[tokio::test]
async fn a_range_with_nothing_stored_is_an_empty_answer_that_says_why() {
    let bed = seeded();
    let address = start(bed.history()).await;

    let unknown = range(
        address,
        &query(VENUE, Some(Category::Spot), "DOGEUSDT", T, T + DAY),
    )
    .await;
    assert!(unknown.segments.is_empty() && unknown.tip.is_empty());
    assert_eq!(
        unknown.message.as_deref(),
        Some(
            format!(
                "the server stores no bars of {} spot DOGEUSDT 1m",
                VENUE.as_str()
            )
            .as_str()
        )
    );

    let (start_ms, end_ms) = (T - 2 * DAY, T - DAY);
    let before = range(
        address,
        &query(VENUE, Some(Category::Spot), "BTCUSDT", start_ms, end_ms),
    )
    .await;
    assert!(before.segments.is_empty() && before.tip.is_empty());
    assert_eq!(
        before.message,
        Some(format!(
            "the server stores no bars of {} spot BTCUSDT 1m from {} to {}; it holds them from {} \
             to {}",
            VENUE.as_str(),
            ms_to_rfc3339(start_ms),
            ms_to_rfc3339(end_ms),
            ms_to_rfc3339(T),
            ms_to_rfc3339(T + 2 * DAY + 2 * MINUTE)
        ))
    );

    // The same symbol in another family, and on another venue, is another
    // market.
    let linear = range(
        address,
        &query(VENUE, Some(Category::Linear), "BTCUSDT", T, T + DAY),
    )
    .await;
    assert!(linear.message.is_some(), "{linear:?}");
    let elsewhere = range(
        address,
        &query(OTHER_VENUE, Some(Category::Spot), "BTCUSDT", T, T + DAY),
    )
    .await;
    assert!(elsewhere.message.is_some(), "{elsewhere:?}");
}

#[tokio::test]
async fn a_query_without_category_reads_spot_only_while_no_other_family_holds_the_symbol() {
    let bed = seeded();
    let address = start(bed.history()).await;
    let (start_ms, end_ms) = (T - DAY, T + 3 * DAY);

    let named = range(
        address,
        &query(VENUE, Some(Category::Spot), "BTCUSDT", start_ms, end_ms),
    )
    .await;
    let legacy = range(address, &query(VENUE, None, "BTCUSDT", start_ms, end_ms)).await;
    assert_eq!(legacy, named);
    assert_eq!(legacy.category, "spot");

    // Another venue's linear market of the same name changes nothing here.
    bed.store
        .upsert_candles(
            &market_of(OTHER_VENUE, Category::Linear, "BTCUSDT", Interval::M1),
            &[candle(T, "1", None)],
        )
        .expect("the other venue's candle");
    let still = range(address, &query(VENUE, None, "BTCUSDT", start_ms, end_ms)).await;
    assert_eq!(still, named);

    // Once the venue holds the symbol and interval as linear too, a query
    // without category could mean either.
    bed.store
        .upsert_candles(&market(Category::Linear), &[candle(T, "2", None)])
        .expect("the linear candle");
    let response = get_candles(address, &query(VENUE, None, "BTCUSDT", start_ms, end_ms)).await;
    let (status, code, message) = refusal(response).await;
    assert_eq!((status, code.as_str()), (400, "invalid_request"));
    assert_eq!(
        message,
        format!(
            "{} stores BTCUSDT 1m as spot and linear; a market without category reads spot only \
             when no other family holds the symbol and interval, so name the category",
            VENUE.as_str()
        )
    );
    let linear = range(
        address,
        &query(VENUE, Some(Category::Linear), "BTCUSDT", start_ms, end_ms),
    )
    .await;
    assert_eq!(linear.category, "linear");
    assert_eq!(linear.tip.len(), 1);

    // Linear alone is ambiguous as well: a query without category never
    // reads it.
    bed.store
        .upsert_candles(
            &market_of(VENUE, Category::Inverse, "BTCUSD", Interval::M1),
            &[candle(T, "3", None)],
        )
        .expect("the inverse candle");
    let response = get_candles(address, &query(VENUE, None, "BTCUSD", start_ms, end_ms)).await;
    let (status, _, message) = refusal(response).await;
    assert_eq!(status, 400);
    assert!(
        message.contains("stores BTCUSD 1m as inverse;"),
        "{message}"
    );

    // Every family named, in order.
    bed.store
        .upsert_candles(&market(Category::Inverse), &[candle(T, "4", None)])
        .expect("the inverse candle");
    let response = get_candles(address, &query(VENUE, None, "BTCUSDT", start_ms, end_ms)).await;
    let (status, _, message) = refusal(response).await;
    assert_eq!(status, 400);
    assert!(
        message.contains("stores BTCUSDT 1m as spot, linear and inverse;"),
        "{message}"
    );
}

#[tokio::test]
async fn a_tip_longer_than_the_server_sends_in_one_answer_is_refused_never_cut_short() {
    // Two packed windows, then three bars in the tip.
    let bed = seeded();
    let spot = |end_ms| query(VENUE, Some(Category::Spot), "BTCUSDT", T - DAY, end_ms);
    let with_tip = |max_tip_bars| {
        bed.history_with(HistoryLimits {
            max_tip_bars,
            ..HistoryLimits::default()
        })
    };

    // Only the tip counts: the thousands of packed bars do not.
    let three = start(with_tip(3)).await;
    let whole = range(three, &spot(T + 3 * DAY)).await;
    assert_eq!((whole.segments.len(), whole.tip.len()), (2, 3));

    let two = start(with_tip(2)).await;
    let (status, code, message) = refusal(get_candles(two, &spot(T + 3 * DAY)).await).await;
    assert_eq!((status, code.as_str()), (400, "invalid_request"));
    assert_eq!(
        message,
        format!(
            "the range holds more than 2 bars from {} on that are not packed into segments yet, \
             more than this server sends in one answer; ask for a shorter range",
            ms_to_rfc3339(T + 2 * DAY)
        )
    );
    // A range that ends sooner fits, whole.
    let shorter = range(two, &spot(T + 2 * DAY + MINUTE)).await;
    assert_eq!((shorter.segments.len(), shorter.tip.len()), (2, 2));

    // No limit the server can be given overflows a backend's.
    let unbounded = start(with_tip(u64::MAX)).await;
    let all = range(unbounded, &spot(T + 3 * DAY)).await;
    assert_eq!(all.tip.len(), 3);
}
