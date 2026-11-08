//! The server's routes in process, over a real SQLite store in a directory
//! of its own: health and version, and every refusal of a path or method,
//! here; a shutdown that finishes a request in progress in `shutdown`; a
//! range in `history`, the market list in `markets`, the queries refused in
//! `queries`, the segment route in `segments`, uploads in `jobs`, the other
//! job routes in `submissions`, a busy store at a job's start and a stop
//! racing the dispatcher in `queue`, and storage failures and the turns at
//! the store in `store`, over the store of `faulty`. Literal venue names
//! live in this file only: `tests/exchange_boundary.rs` reads the others as
//! code.

mod faulty;
mod history;
mod jobs;
mod markets;
mod queries;
mod queue;
mod segments;
mod shutdown;
mod store;
mod submissions;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use reqwest::header::{ALLOW, CACHE_CONTROL, CONTENT_TYPE};
use rust_decimal::Decimal;
use serde_json::Value;
use tempfile::TempDir;

use super::*;
use crate::model::{
    Candle, Category, ExchangeId, Interval, MarketId, SegmentHash, Symbol, TimestampMs, Venue,
    ms_to_rfc3339,
};
use crate::ports::{CandleQuery, CandleStore, SegmentStore, StorageError};
use crate::storage::SqliteStore;
use faulty::Faulty;

/// 2026-10-20T00:00:00Z, a UTC midnight: the start of a `1m` window.
const T: TimestampMs = 1_792_454_400_000;
const MINUTE: TimestampMs = 60_000;
const DAY: TimestampMs = 86_400_000;

/// The venue the tests' markets trade on.
const VENUE: Venue = Venue::Binance;
/// Another venue, whose markets share the first one's names.
const OTHER_VENUE: Venue = Venue::Bybit;

/// A client that never routes loopback requests through a proxy from the
/// environment.
fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("an HTTP client")
}

/// Serves `history`, and job routes whose queue never starts, on a free
/// loopback port for the rest of the test.
async fn start(history: History) -> SocketAddr {
    let jobs = idle(&history);
    start_with(history, jobs).await
}

/// Serves `history` and `jobs` on a free loopback port for the rest of the
/// test.
async fn start_with(history: History, jobs: Jobs) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("a listener");
    let address = listener.local_addr().expect("its address");
    tokio::spawn(serve(listener, history, jobs, std::future::pending()));
    address
}

/// Job routes over `history`'s store whose queue is never started, so that
/// they make no store call of their own.
fn idle(history: &History) -> Jobs {
    Jobs::new(
        history.store().clone(),
        NoRuntime,
        JobLimits {
            concurrency: 1,
            max_span_bars: DEFAULT_MAX_SPAN_BARS,
        },
    )
}

/// `store` behind the server's default turns.
fn served<S: ServedStore + 'static>(store: S) -> ServerStore {
    ServerStore::new(store, StoreLimits::default())
}

/// `future`'s output, or a failed test after ten seconds, so that a server
/// that never lets go fails the test instead of hanging it.
async fn within<F: Future>(what: &str, future: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(10), future)
        .await
        .unwrap_or_else(|_| panic!("no {what} within ten seconds"))
}

fn header(response: &reqwest::Response, name: impl reqwest::header::AsHeaderName) -> Option<&str> {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
}

/// `GET /v1/candles` with `query`.
async fn get_candles(address: SocketAddr, query: &[(&str, String)]) -> reqwest::Response {
    client()
        .get(format!("http://{address}/v1/candles"))
        .query(query)
        .send()
        .await
        .expect("a response")
}

/// A failed answer's status, code and message, after checking that it is
/// an error envelope no cache keeps.
async fn refusal(response: reqwest::Response) -> (u16, String, String) {
    let status = response.status().as_u16();
    assert_eq!(header(&response, CONTENT_TYPE), Some("application/json"));
    assert_eq!(header(&response, CACHE_CONTROL), Some("no-store"));
    let body: Value = response.json().await.expect("an error envelope");
    let error = &body["error"];
    (
        status,
        error["code"].as_str().expect("a code").to_string(),
        error["message"].as_str().expect("a message").to_string(),
    )
}

/// A SQLite store in a directory of its own, its schema in place.
struct Bed {
    _dir: TempDir,
    store: SqliteStore,
}

impl Bed {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("a directory");
        let store = SqliteStore::new(dir.path().join("market.sqlite"));
        CandleStore::init(&store).expect("init");
        Self { _dir: dir, store }
    }

    /// The history routes over the bed's store, each call bounded as
    /// `serve run` bounds them.
    fn history(&self) -> History {
        self.history_with(HistoryLimits::default())
    }

    fn history_with(&self, limits: HistoryLimits) -> History {
        History::new(self.server_store(), limits)
    }

    /// The bed's store as the server holds it.
    fn server_store(&self) -> ServerStore {
        served(self.store.within(Duration::from_secs(2)))
    }

    /// The bed's database file, for a state no port leaves.
    fn connection(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(self._dir.path().join("market.sqlite")).expect("the bed's file")
    }
}

/// A market of the tests' venue: `symbol` at `interval` in `category`.
fn market_of(venue: Venue, category: Category, symbol: &str, interval: Interval) -> MarketId {
    MarketId::new(
        ExchangeId::from_parts(venue, category),
        Symbol::new(symbol).expect("a symbol"),
        interval,
    )
}

/// BTCUSDT `1m` in `category` on the tests' venue.
fn market(category: Category) -> MarketId {
    market_of(VENUE, category, "BTCUSDT", Interval::M1)
}

fn decimal(text: &str) -> Decimal {
    Decimal::from_str_exact(text).unwrap_or_else(|error| panic!("{text}: {error}"))
}

/// A `1m` candle whose prices are all `close`, its text as given.
fn candle(open_time_ms: TimestampMs, close: &str, trades: Option<u64>) -> Candle {
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

/// A candle as text, every decimal with its scale: what exactness compares.
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

/// The query string of `/v1/candles` for `market` from `start` to `end`;
/// `None` for a query without category.
fn query(
    venue: Venue,
    category: Option<Category>,
    symbol: &str,
    start: TimestampMs,
    end: TimestampMs,
) -> Vec<(&'static str, String)> {
    let mut query = vec![
        ("exchange", venue.as_str().to_string()),
        ("symbol", symbol.to_string()),
        ("interval", "1m".to_string()),
        ("start_open_ms", start.to_string()),
        ("end_open_ms", end.to_string()),
    ];
    if let Some(category) = category {
        query.push(("category", category.as_str().to_string()));
    }
    query
}

#[tokio::test]
async fn healthz_answers_ok_as_plain_text() {
    let address = start(Bed::new().history()).await;
    let response = client()
        .get(format!("http://{address}/healthz"))
        .send()
        .await
        .expect("a response");
    assert_eq!(response.status(), 200);
    assert_eq!(
        header(&response, CONTENT_TYPE),
        Some("text/plain; charset=utf-8")
    );
    assert_eq!(response.text().await.expect("a body"), "ok");

    let head = client()
        .head(format!("http://{address}/healthz"))
        .send()
        .await
        .expect("a response");
    assert_eq!(head.status(), 200);
}

#[tokio::test]
async fn version_answers_this_release_and_the_protocol_versions_it_speaks() {
    let address = start(Bed::new().history()).await;
    let response = client()
        .get(format!("http://{address}/version"))
        .send()
        .await
        .expect("a response");
    assert_eq!(response.status(), 200);
    assert_eq!(header(&response, CONTENT_TYPE), Some("application/json"));
    let expected = format!(
        r#"{{"version":"{}","proto_version":{},"min_supported":{}}}"#,
        env!("CARGO_PKG_VERSION"),
        PROTO_VERSION.0,
        MIN_SUPPORTED.0
    );
    assert_eq!(response.text().await.expect("a body"), expected);
}

#[tokio::test]
async fn a_path_no_route_answers_is_not_found_and_another_method_not_allowed() {
    let address = start(Bed::new().history()).await;
    for path in ["/v1/jobs", "/nope", "/v1", "/v1/segments/", "/v0/candles"] {
        let response = client()
            .get(format!("http://{address}{path}"))
            .send()
            .await
            .expect("a response");
        let (status, code, message) = refusal(response).await;
        assert_eq!((status, code.as_str()), (404, "not_found"), "{path}");
        assert_eq!(message, format!("this server answers no GET {path}"));
    }

    for path in ["/healthz", "/v1/candles", "/v1/markets"] {
        let response = client()
            .post(format!("http://{address}{path}"))
            .send()
            .await
            .expect("a response");
        assert_eq!(header(&response, ALLOW), Some("GET,HEAD"), "{path}");
        let (status, code, message) = refusal(response).await;
        assert_eq!(
            (status, code.as_str()),
            (405, "method_not_allowed"),
            "{path}"
        );
        assert_eq!(
            message,
            format!("{path} does not take POST; the Allow header lists what it takes")
        );
    }
}

#[tokio::test]
async fn a_path_under_a_version_the_server_does_not_speak_names_the_ones_it_does() {
    let address = start(Bed::new().history()).await;
    for path in ["/v2/candles", "/v9/segments/x", "/v2"] {
        let response = client()
            .get(format!("http://{address}{path}"))
            .send()
            .await
            .expect("a response");
        assert_eq!(response.status(), 400, "{path}");
        let body: Value = response.json().await.expect("an envelope");
        assert_eq!(body["error"]["code"], "unsupported_version", "{path}");
        assert_eq!(
            body["error"]["supported"],
            serde_json::json!({"min_supported": MIN_SUPPORTED.0, "proto_version": PROTO_VERSION.0})
        );
    }
}
