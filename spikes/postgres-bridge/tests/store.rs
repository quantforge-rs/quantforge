//! `CandleStore` over the bridge: the same text as SQLite, written by the
//! real data-sync poll loop from both kinds of tokio runtime, and its
//! connections gone once it is dropped, even inside async code.

mod common;

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Duration;

use postgres_bridge_spike::bridge::BridgeConfig;
use quantforge::{
    Candle, CandleQuery, CandleStore, DataSyncConfig, DataSyncEngine, ExchangeCapabilities,
    ExchangeConnector, ExchangeError, ExchangeId, KlineRequest, MarketDataConnector, SqliteStore,
    Symbol, SymbolRules, TimestampMs,
};

use common::{Observer, T, candle, market, texts, tricky_candles, unique};

#[test]
fn postgres_hands_back_the_same_text_as_sqlite() {
    let url = database_or_skip!();
    let postgres = common::store(&BridgeConfig::new(&url, unique("parity")));
    let tempdir = tempfile::tempdir().expect("tempdir");
    let sqlite = SqliteStore::new(tempdir.path().join("market.sqlite"));
    CandleStore::init(&sqlite).expect("init");

    let candles = tricky_candles();
    for store in [&postgres as &dyn CandleStore, &sqlite] {
        assert_eq!(
            store.upsert_candles(&market(), &candles).expect("upsert"),
            5
        );
        // A repeat is counted, and changes nothing.
        assert_eq!(store.upsert_candles(&market(), &candles).expect("again"), 5);
    }
    let read = |store: &dyn CandleStore| {
        texts(
            &store
                .load_candles(&market(), CandleQuery::default())
                .expect("load"),
        )
    };
    assert_eq!(read(&postgres), read(&sqlite));
    assert_eq!(read(&postgres), texts(&candles));
    assert_eq!(
        texts(&postgres.load_recent_candles(&market(), 2).expect("recent")),
        texts(&candles[3..])
    );
    assert_eq!(
        postgres.max_open_time_ms(&market()).expect("max"),
        sqlite.max_open_time_ms(&market()).expect("max")
    );
}

/// Serves its pages in order, then nothing, as an exchange that has no more.
struct Pages(Mutex<VecDeque<Vec<Candle>>>);

impl ExchangeConnector for Pages {
    fn exchange_id(&self) -> ExchangeId {
        ExchangeId::BinanceSpot
    }

    fn capabilities(&self) -> ExchangeCapabilities {
        ExchangeCapabilities::default()
    }

    fn is_production(&self) -> bool {
        false
    }
}

#[async_trait::async_trait]
impl MarketDataConnector for Pages {
    async fn fetch_klines(&self, _request: &KlineRequest) -> Result<Vec<Candle>, ExchangeError> {
        Ok(self
            .0
            .lock()
            .expect("pages")
            .pop_front()
            .unwrap_or_default())
    }

    async fn fetch_symbol_rules(&self, _symbol: &Symbol) -> Result<SymbolRules, ExchangeError> {
        unreachable!("data sync never fetches symbol rules")
    }

    async fn server_time(&self) -> Result<TimestampMs, ExchangeError> {
        unreachable!("data sync never reads the server time")
    }
}

/// Three polls, each of which finds a new page of closed candles.
fn pages() -> Pages {
    let page = |first: i64| {
        (first..first + 3)
            .map(|minute| candle(T + minute * 60_000, "100.50", Some(7)))
            .collect()
    };
    Pages(Mutex::new(VecDeque::from([
        page(0),
        Vec::new(),
        page(3),
        Vec::new(),
        page(6),
    ])))
}

/// Runs the poll loop into `store`, as `data sync` does with no end.
async fn poll(store: &dyn CandleStore) -> usize {
    let source = pages();
    DataSyncEngine::new(&source, store)
        .run(&DataSyncConfig {
            market: market(),
            start_time_ms: Some(T),
            end_time_ms: None,
            batch_limit: 1000,
            follow: false,
            poll_interval: Duration::from_millis(10),
            max_loops: Some(3),
        })
        .await
        .expect("the poll loop")
        .written
}

async fn polls_like_sqlite(url: &str) {
    let postgres = common::store(&BridgeConfig::new(url, unique("poll")));
    let tempdir = tempfile::tempdir().expect("tempdir");
    let sqlite = SqliteStore::new(tempdir.path().join("market.sqlite"));
    CandleStore::init(&sqlite).expect("init");

    assert_eq!(poll(&postgres).await, 9);
    assert_eq!(poll(&sqlite).await, 9);
    let read = |store: &dyn CandleStore| {
        texts(
            &store
                .load_candles(&market(), CandleQuery::default())
                .expect("load"),
        )
    };
    assert_eq!(read(&postgres), read(&sqlite));
}

#[tokio::test]
async fn the_poll_loop_writes_through_the_bridge_on_a_current_thread_runtime() {
    let url = database_or_skip!();
    polls_like_sqlite(&url).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_poll_loop_writes_through_the_bridge_on_a_multi_thread_runtime() {
    let url = database_or_skip!();
    polls_like_sqlite(&url).await;
}

#[tokio::test]
async fn dropping_the_store_inside_async_code_closes_its_connections() {
    let url = database_or_skip!();
    let observer = Observer::connect(&url);
    let name = unique("drop");
    let store = common::store(&BridgeConfig::new(&url, &name));
    store.max_open_time_ms(&market()).expect("a query");
    assert!(observer.connections(&name) >= 1);

    drop(store);
    assert_eq!(
        observer.wait_for(0, |observer| observer.connections(&name)),
        0
    );
}
