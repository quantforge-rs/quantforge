//! Data sync storing settled candles only, and packing the windows its
//! passes close.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Duration;

use quantforge::{
    Candle, CandleQuery, CandleStore, DataSyncConfig, DataSyncEngine, ExchangeCapabilities,
    ExchangeConnector, ExchangeError, ExchangeId, Interval, KlineRequest, MarketDataConnector,
    SegmentStore, Symbol, SymbolRules, TimestampMs, now_utc_ms,
};

use crate::store::{DAY, MINUTE, candle, m1, minutes, plan, store};

/// 2024-01-01T00:00:00Z: long settled.
const T: i64 = 1_704_067_200_000;

/// Serves its pages in order, then nothing.
struct ScriptedSource {
    pages: Mutex<VecDeque<Vec<Candle>>>,
}

impl ScriptedSource {
    fn new(pages: Vec<Vec<Candle>>) -> Self {
        Self {
            pages: Mutex::new(pages.into()),
        }
    }
}

impl ExchangeConnector for ScriptedSource {
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
impl MarketDataConnector for ScriptedSource {
    async fn fetch_klines(&self, _request: &KlineRequest) -> Result<Vec<Candle>, ExchangeError> {
        Ok(self
            .pages
            .lock()
            .expect("lock")
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

/// One pass over `start..=end`.
fn config(start: i64, end: Option<i64>) -> DataSyncConfig {
    DataSyncConfig {
        market: m1(),
        start_time_ms: Some(start),
        end_time_ms: end,
        batch_limit: 1000,
        follow: false,
        poll_interval: Duration::from_millis(1),
        max_loops: Some(1),
    }
}

#[tokio::test]
async fn data_sync_packs_each_window_its_pass_closes() {
    let (_tempdir, store) = store();
    let candles = minutes(T, 1441);
    let source = ScriptedSource::new(vec![candles[..1000].to_vec(), candles[1000..].to_vec()]);
    let summary = DataSyncEngine::new(&source, &store)
        .with_segments(&store)
        .run(&config(T, Some(T + DAY)))
        .await
        .expect("sync");
    assert_eq!(summary.written, 1441);
    assert_eq!(summary.segments_built, 1);

    let plan = plan(&store, &m1(), T, T + DAY);
    assert_eq!(plan.segments.len(), 1);
    assert_eq!(plan.segments[0].window_start_ms, T);
    assert_eq!(plan.tip.map(|tip| tip.first_open_ms), Some(T + DAY));
}

#[tokio::test]
async fn data_sync_without_a_segment_store_packs_nothing() {
    let (_tempdir, store) = store();
    let source = ScriptedSource::new(vec![minutes(T, 1441)]);
    let summary = DataSyncEngine::new(&source, &store)
        .run(&config(T, Some(T + DAY)))
        .await
        .expect("sync");
    assert_eq!(summary.segments_built, 0);
    assert!(plan(&store, &m1(), T, T + DAY).segments.is_empty());
}

#[tokio::test]
async fn the_first_pass_packs_what_earlier_writes_left_even_when_it_writes_nothing() {
    let (_tempdir, store) = store();
    store
        .upsert_candles(&m1(), &minutes(T, 1441))
        .expect("an earlier writer");
    let source = ScriptedSource::new(Vec::new());
    let summary = DataSyncEngine::new(&source, &store)
        .with_segments(&store)
        .run(&config(T + DAY + MINUTE, Some(T + 2 * DAY)))
        .await
        .expect("sync");
    assert_eq!(summary.written, 0);
    assert_eq!(summary.segments_built, 1);
    assert_eq!(store.refresh_segments(&m1()).expect("refresh").built, 0);
}

#[tokio::test]
async fn a_candle_still_forming_is_not_stored() {
    let (_tempdir, store) = store();
    let now = now_utc_ms();
    let forming = now - now.rem_euclid(MINUTE);
    // Two candles that closed over a minute ago, and the one forming now.
    let source = ScriptedSource::new(vec![vec![
        candle(Interval::M1, forming - 3 * MINUTE, "1"),
        candle(Interval::M1, forming - 2 * MINUTE, "2"),
        candle(Interval::M1, forming, "3"),
    ]]);
    let summary = DataSyncEngine::new(&source, &store)
        .with_segments(&store)
        .run(&config(forming - 3 * MINUTE, None))
        .await
        .expect("sync");
    assert_eq!(summary.written, 2);
    assert_eq!(summary.last_open_time_ms, Some(forming - 2 * MINUTE));
    let stored: Vec<i64> = store
        .load_candles(&m1(), CandleQuery::default())
        .expect("load")
        .iter()
        .map(|candle| candle.open_time_ms)
        .collect();
    assert_eq!(stored, vec![forming - 3 * MINUTE, forming - 2 * MINUTE]);
}
