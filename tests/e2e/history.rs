//! `serve run`'s history routes as a process, on each backend: the markets
//! the store holds; a range read back from its segments and tip exactly as
//! the store holds it, product lines and venues kept apart; the empty
//! answers and a query without category; and a segment the same on every
//! request, its tag answered `304`. On PostgreSQL every scenario also
//! proves that nothing was created beside the store.
//!
//! The tests after the scenarios need a real server: the two backends
//! answering byte for byte alike, and a store that stops answering, which
//! no client hears named.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use quantforge::protocol::segments::{CandleRange, MarketIndex};
use quantforge::storage::segments;
use quantforge::{
    Candle, CandleQuery, CandleStore, Category, ExchangeId, Interval, MarketId, SegmentHash,
    SegmentStore, Symbol, Venue,
};
use rust_decimal::Decimal;
use serde::de::DeserializeOwned;

use crate::harness::{TestBed, on_each_backend};
use crate::serve::Served;

/// 2026-10-20T00:00:00Z, a UTC midnight: the start of a `1m` window.
const T: i64 = 1_792_454_400_000;
const MINUTE: i64 = 60_000;
const DAY: i64 = 86_400_000;

/// A response read to the end of its connection.
pub(crate) struct Fetched {
    pub(crate) status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Fetched {
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    pub(crate) fn json<T: DeserializeOwned>(&self) -> T {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|error| panic!("{error}: {}", self.text()))
    }

    pub(crate) fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// `GET path`, with `extra` headers, on a connection the server closes once
/// it has answered.
fn fetch(address: SocketAddr, path: &str, extra: &[(&str, &str)]) -> Fetched {
    request(address, "GET", path, extra, None)
}

/// `method path`, with `extra` headers and `body` if any, on a connection
/// the server closes once it has answered.
pub(crate) fn request(
    address: SocketAddr,
    method: &str,
    path: &str,
    extra: &[(&str, &str)],
    body: Option<&[u8]>,
) -> Fetched {
    let mut stream = TcpStream::connect(address).expect("a connection");
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .expect("a read timeout");
    let mut request =
        format!("{method} {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n");
    for (name, value) in extra {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    if let Some(body) = body {
        request.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    request.push_str("\r\n");
    let mut bytes = request.into_bytes();
    bytes.extend_from_slice(body.unwrap_or_default());
    stream.write_all(&bytes).expect("send the request");
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).expect("a response");
    let end = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("the end of the headers");
    let head = String::from_utf8(raw[..end].to_vec()).expect("text headers");
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .and_then(|line| line.split(' ').nth(1))
        .and_then(|status| status.parse().ok())
        .unwrap_or_else(|| panic!("no status in {head:?}"));
    let headers = lines
        .filter_map(|line| line.split_once(": "))
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect();
    let fetched = Fetched {
        status,
        headers,
        body: raw[end + 4..].to_vec(),
    };
    assert_eq!(fetched.header("transfer-encoding"), None, "a chunked body");
    if let Some(length) = fetched.header("content-length") {
        assert_eq!(length, fetched.body.len().to_string(), "{path}");
    }
    fetched
}

fn decimal(text: &str) -> Decimal {
    Decimal::from_str_exact(text).unwrap_or_else(|error| panic!("{text}: {error}"))
}

fn candle(open_time_ms: i64, close: &str, trades: Option<u64>) -> Candle {
    let close = decimal(close);
    Candle {
        open_time_ms,
        close_time_ms: open_time_ms + MINUTE - 1,
        open: close,
        high: close,
        low: close,
        close,
        volume: decimal("0.50000000"),
        trades,
    }
}

/// A candle as text, every decimal with its scale.
fn text(candle: &Candle) -> String {
    format!(
        "{} {} {} {} {} {} {} {:?}",
        candle.open_time_ms,
        candle.close_time_ms,
        candle.open,
        candle.high,
        candle.low,
        candle.close,
        candle.volume,
        candle.trades
    )
}

fn market(venue: Venue, category: Category) -> MarketId {
    MarketId::new(
        ExchangeId::from_parts(venue, category),
        Symbol::new("BTCUSDT").expect("a symbol"),
        Interval::M1,
    )
}

/// Three days of `1m` bars from `T`: two windows a refresh packs, the first
/// with gaps, and three bars of the third, the tip; prices offset by
/// `offset` so that each market's bars are its own, every scale kept.
fn three_days(offset: i64) -> Vec<Candle> {
    let first = (0..1440).filter(|minute| minute % 97 != 13).map(|minute| {
        let close = format!("{}.{:08}", 67_000 + offset + minute % 50, minute);
        candle(
            T + minute * MINUTE,
            &close,
            (minute % 5 != 0).then_some(minute as u64),
        )
    });
    let second = (0..1440).map(|minute| {
        let close = match minute % 3 {
            0 => format!("{}.{}", 1 + offset, minute % 10),
            1 => format!("{}", 100 + offset + minute),
            _ => "0.00000001".to_string(),
        };
        candle(T + DAY + minute * MINUTE, &close, Some(0))
    });
    let tip = (0..3).map(|minute| {
        candle(
            T + 2 * DAY + minute * MINUTE,
            &format!("{}.10", 5 + offset),
            None,
        )
    });
    first.chain(second).chain(tip).collect()
}

/// The three markets the scenarios serve: Spot, its venue's linear line,
/// and the other venue's Spot, all named BTCUSDT `1m`, each with bars of
/// its own and its closed windows packed.
fn seed(bed: &TestBed) -> Vec<MarketId> {
    let store = bed.store();
    let markets = vec![
        market(Venue::Binance, Category::Spot),
        market(Venue::Binance, Category::Linear),
        market(Venue::Bybit, Category::Spot),
    ];
    for (offset, market) in markets.iter().enumerate() {
        store
            .upsert_candles(market, &three_days(offset as i64 * 1_000))
            .expect("the candles");
        let refreshed = store.refresh_segments(market).expect("a refresh");
        assert_eq!(refreshed.built, 2, "{refreshed:?}");
    }
    markets
}

fn candles_path(market: &MarketId, category: bool, start: i64, end: i64) -> String {
    let category = if category {
        format!("&category={}", market.category())
    } else {
        String::new()
    };
    format!(
        "/v1/candles?exchange={}{category}&symbol={}&interval={}&start_open_ms={start}&end_open_ms={end}",
        market.exchange.venue(),
        market.symbol,
        market.interval.as_str()
    )
}

/// The bars a client reads from `range`, as the protocol says: each segment
/// fetched by its hash and checked against its entry and its name, its bars
/// in the range kept, then the tip.
fn read_back(address: SocketAddr, range: &CandleRange) -> Vec<String> {
    let mut bars = Vec::new();
    for entry in &range.segments {
        let fetched = fetch(address, &format!("/v1/segments/{}", entry.hash), &[]);
        assert_eq!(fetched.status, 200);
        assert_eq!(fetched.body.len() as u64, entry.size_bytes);
        let name = SegmentHash::from_digest(*entry.hash.digest());
        let candles =
            segments::open(&fetched.body, &name).expect("a segment that matches its name");
        assert_eq!(candles.len(), entry.range.bar_count as usize);
        bars.extend(
            candles
                .iter()
                .filter(|candle| {
                    (range.start_open_ms..=range.end_open_ms).contains(&candle.open_time_ms)
                })
                .map(text),
        );
    }
    bars.extend(range.tip.iter().map(|bar| {
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
    }));
    bars
}

fn history_reads_back_exactly_what_the_store_holds(bed: &TestBed) {
    let markets = seed(bed);
    let served = Served::start(bed);
    let store = bed.store();

    let index: MarketIndex = fetch(served.address, "/v1/markets", &[]).json();
    let listed: Vec<String> = index
        .markets
        .iter()
        .map(|entry| {
            format!(
                "{} {} {} {} {} {}",
                entry.exchange,
                entry.category,
                entry.symbol,
                entry.interval,
                entry.first_open_ms,
                entry.last_open_ms
            )
        })
        .collect();
    let last = T + 2 * DAY + 2 * MINUTE;
    assert_eq!(
        listed,
        [
            format!("binance linear BTCUSDT 1m {T} {last}"),
            format!("binance spot BTCUSDT 1m {T} {last}"),
            format!("bybit spot BTCUSDT 1m {T} {last}"),
        ]
    );

    let ranges = [
        (T - DAY, T + 3 * DAY),
        (T + 100 * MINUTE, T + DAY + 50 * MINUTE),
        (T + DAY + 1439 * MINUTE, T + 2 * DAY + MINUTE),
    ];
    for market in &markets {
        for (start, end) in ranges {
            let fetched = fetch(served.address, &candles_path(market, true, start, end), &[]);
            assert_eq!(fetched.status, 200, "{}", fetched.text());
            assert_eq!(fetched.header("cache-control"), Some("no-cache"));
            let range: CandleRange = fetched.json();
            assert_eq!(range.category, market.category().as_str());
            assert_eq!(range.message, None);
            let query = CandleQuery {
                start_time_ms: Some(start),
                end_time_ms: Some(end),
                limit: None,
            };
            let stored: Vec<String> = store
                .load_candles(market, query)
                .expect("a direct read")
                .iter()
                .map(text)
                .collect();
            assert!(!stored.is_empty());
            assert_eq!(
                read_back(served.address, &range),
                stored,
                "{market:?} {start}..={end}"
            );
        }
    }
}

fn history_answers_nothing_stored_and_reads_a_query_without_category(bed: &TestBed) {
    let spot = market(Venue::Binance, Category::Spot);
    let store = bed.store();
    store
        .upsert_candles(&spot, &three_days(0))
        .expect("the candles");
    store.refresh_segments(&spot).expect("a refresh");
    let served = Served::start(bed);

    let unknown = fetch(
        served.address,
        "/v1/candles?exchange=binance&category=spot&symbol=DOGEUSDT&interval=1m\
         &start_open_ms=0&end_open_ms=60000",
        &[],
    );
    assert_eq!(unknown.status, 200);
    assert_eq!(
        unknown.text(),
        r#"{"exchange":"binance","category":"spot","symbol":"DOGEUSDT","interval":"1m","start_open_ms":0,"end_open_ms":60000,"segments":[],"tip":[],"message":"the server stores no bars of binance spot DOGEUSDT 1m"}"#
    );

    let (start, end) = (T - DAY, T + 3 * DAY);
    let named = fetch(served.address, &candles_path(&spot, true, start, end), &[]);
    let legacy = fetch(served.address, &candles_path(&spot, false, start, end), &[]);
    assert_eq!((legacy.status, legacy.text()), (200, named.text()));

    store
        .upsert_candles(
            &market(Venue::Binance, Category::Linear),
            &[candle(T, "1", None)],
        )
        .expect("a linear candle");
    let ambiguous = fetch(served.address, &candles_path(&spot, false, start, end), &[]);
    assert_eq!(ambiguous.status, 400);
    assert!(
        ambiguous
            .text()
            .contains("binance stores BTCUSDT 1m as spot and linear"),
        "{}",
        ambiguous.text()
    );

    let lowercase = fetch(
        served.address,
        &candles_path(&spot, true, start, end).replace("BTCUSDT", "btcusdt"),
        &[],
    );
    assert_eq!(lowercase.status, 400);
    assert!(lowercase.text().contains(r#""code":"invalid_request""#));
}

fn a_segment_is_the_same_on_every_request_and_its_tag_answers_not_modified(bed: &TestBed) {
    let markets = seed(bed);
    let store = bed.store();
    let plan = store
        .resolve_segments(&markets[0], T, T + 3 * DAY)
        .expect("a plan");
    let hash = plan.segments[0].hash;
    let stored = store
        .load_segment(&hash)
        .expect("a read")
        .expect("the segment");
    let served = Served::start(bed);
    let path = format!("/v1/segments/{hash}");
    let tag = format!("\"{hash}\"");
    for _ in 0..2 {
        let fetched = fetch(served.address, &path, &[]);
        assert_eq!(fetched.status, 200);
        assert_eq!(fetched.body, stored);
        assert_eq!(fetched.header("content-type"), Some("application/zstd"));
        assert_eq!(fetched.header("etag"), Some(tag.as_str()));
        assert_eq!(
            fetched.header("cache-control"),
            Some("public, max-age=31536000, immutable")
        );
    }
    let cached = fetch(served.address, &path, &[("If-None-Match", &tag)]);
    assert_eq!(cached.status, 304);
    assert!(cached.body.is_empty());
    assert_eq!(cached.header("etag"), Some(tag.as_str()));

    let unknown = fetch(
        served.address,
        &format!("/v1/segments/sha256:{}", "0".repeat(64)),
        &[],
    );
    assert_eq!(unknown.status, 404);
    assert!(unknown.text().contains(r#""code":"not_found""#));
}

on_each_backend!(
    history_reads_back_exactly_what_the_store_holds,
    history_answers_nothing_stored_and_reads_a_query_without_category,
    a_segment_is_the_same_on_every_request_and_its_tag_answers_not_modified,
);

#[cfg(feature = "postgres")]
mod real_server {
    use url::Url;

    use super::*;
    use crate::server::{query, server_url};

    /// The same candles in a SQLite file and a PostgreSQL database give the
    /// same answers and the same segments, byte for byte.
    #[test]
    fn both_backends_answer_byte_for_byte_alike() {
        let Some(postgres) = TestBed::on_postgres() else {
            return;
        };
        let sqlite = TestBed::new();
        let markets = seed(&sqlite);
        assert_eq!(seed(&postgres), markets);
        let (on_sqlite, on_postgres) = (Served::start(&sqlite), Served::start(&postgres));
        let both = |path: &str| {
            let (a, b) = (
                fetch(on_sqlite.address, path, &[]),
                fetch(on_postgres.address, path, &[]),
            );
            assert_eq!(
                (a.status, &a.body),
                (b.status, &b.body),
                "{path}: {}",
                a.text()
            );
            a
        };
        both("/v1/markets");
        for market in &markets {
            for (start, end) in [(T - DAY, T + 3 * DAY), (T + 7 * MINUTE, T + DAY)] {
                let range: CandleRange = both(&candles_path(market, true, start, end)).json();
                for entry in &range.segments {
                    both(&format!("/v1/segments/{}", entry.hash));
                }
            }
        }
        both(&candles_path(&markets[0], true, 0, 60_000));
    }

    /// A database that stops taking connections is a retry for a client,
    /// never its name, and the server serves again once it is back.
    #[test]
    fn a_store_that_stops_answering_is_unavailable_and_never_named() {
        let Some(bed) = TestBed::on_postgres() else {
            return;
        };
        let server = server_url().expect("a server");
        let database = Url::parse(bed.postgres_url().expect("a URL"))
            .expect("the URL")
            .path()
            .trim_start_matches('/')
            .to_string();
        let markets = seed(&bed);
        let served = Served::start(&bed);
        let path = candles_path(&markets[0], true, T, T + 3 * DAY);
        assert_eq!(fetch(served.address, &path, &[]).status, 200);

        query(
            &server,
            &format!("ALTER DATABASE {database} ALLOW_CONNECTIONS false"),
        );
        query(
            &server,
            &format!(
                "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = \
                 '{database}'"
            ),
        );
        let refused = fetch(served.address, &path, &[]);
        query(
            &server,
            &format!("ALTER DATABASE {database} ALLOW_CONNECTIONS true"),
        );
        assert_eq!(refused.status, 503, "{}", refused.text());
        assert_eq!(refused.header("retry-after"), Some("1"));
        assert_eq!(
            refused.text(),
            r#"{"error":{"code":"unavailable","message":"the server's store could not answer in time; retry later"}}"#
        );
        assert!(!refused.text().contains(&database));

        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        loop {
            let again = fetch(served.address, &path, &[]);
            if again.status == 200 {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "{}", again.text());
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}
