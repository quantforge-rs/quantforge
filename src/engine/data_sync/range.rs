//! Fetching one range of klines into the candle store, settled candles
//! only. Data sync and the live engine both drive it.

use crate::EngineError;
use crate::{
    Candle, CandleStore, KlineRequest, MarketDataConnector, MarketId, TimestampMs, now_utc_ms,
};
use std::time::Duration;
use tracing::warn;

/// How long after its close a candle is stored: a second, the clock drift
/// the conformance suite tolerates between this host and the exchange.
/// Until then the exchange may still add trades to it, while a stored
/// candle is final: a segment packs it for good.
pub(crate) const SETTLE_MS: TimestampMs = 1_000;

/// Coverage achieved by a single `sync_market_range` call.
#[derive(Clone, Debug, Default)]
pub(crate) struct SyncRangeOutcome {
    pub written: usize,
    pub first_open_time_ms: Option<i64>,
    pub last_open_time_ms: Option<i64>,
    /// The open time of the first candle fetched but held back because it
    /// had not settled, where paging stopped.
    pub unsettled_from_ms: Option<i64>,
}

/// How many of `batch`'s leading candles had settled by `cutoff_ms`, the
/// clock read before the batch was requested: those whose close time plus
/// [`SETTLE_MS`] lies before it. Candles come oldest first, so the rest have
/// not settled either.
pub(crate) fn settled_len(batch: &[Candle], cutoff_ms: TimestampMs) -> usize {
    batch
        .iter()
        .take_while(|candle| {
            candle
                .close_time_ms
                .checked_add(SETTLE_MS)
                .is_some_and(|settled_ms| settled_ms < cutoff_ms)
        })
        .count()
}

pub(crate) async fn sync_market_range(
    source: &dyn MarketDataConnector,
    store: &dyn CandleStore,
    market: &MarketId,
    start_ms: i64,
    end_ms: i64,
    batch_limit: u16,
) -> Result<SyncRangeOutcome, EngineError> {
    let mut outcome = SyncRangeOutcome::default();
    if end_ms < start_ms {
        return Ok(outcome);
    }

    let step_ms = market.interval.step_ms();
    let mut cursor = start_ms;

    while cursor <= end_ms {
        // Read before the request: the exchange may read a candle that
        // closes while the request is in flight before it closes.
        let cutoff_ms = now_utc_ms();
        let batch = source
            .fetch_klines(&KlineRequest {
                symbol: market.symbol.clone(),
                interval: market.interval,
                start_time_ms: Some(cursor),
                end_time_ms: Some(end_ms),
                limit: Some(batch_limit.min(1000)),
            })
            .await?;

        let Some(last_fetched) = batch.last().map(|candle| candle.open_time_ms) else {
            break;
        };
        let (settled, unsettled) = batch.split_at(settled_len(&batch, cutoff_ms));
        if let (Some(first), Some(last)) = (settled.first(), settled.last()) {
            outcome.written += store.upsert_candles(market, settled)?;
            outcome.first_open_time_ms.get_or_insert(first.open_time_ms);
            outcome.last_open_time_ms = Some(last.open_time_ms);
        }
        if let Some(held_back) = unsettled.first() {
            outcome.unsettled_from_ms = Some(held_back.open_time_ms);
            break;
        }

        let next_cursor = last_fetched + step_ms;
        if next_cursor <= cursor {
            warn!(cursor, next_cursor, "data sync cursor did not advance");
            break;
        }
        cursor = next_cursor;
    }

    Ok(outcome)
}

pub(crate) async fn sleep_or_shutdown(duration: Duration) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(duration) => false,
        _ = tokio::signal::ctrl_c() => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        CandleQuery, ExchangeCapabilities, ExchangeConnector, ExchangeError, ExchangeId, Interval,
        SqliteStore, Symbol, SymbolRules,
    };
    use rust_decimal::Decimal;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    fn market() -> MarketId {
        MarketId::new(
            ExchangeId::BinanceSpot,
            Symbol::new("BTCUSDT").expect("symbol"),
            Interval::M1,
        )
    }

    fn candle(open_time_ms: i64) -> Candle {
        Candle {
            open_time_ms,
            close_time_ms: open_time_ms + 59_999,
            open: Decimal::ONE,
            high: Decimal::ONE,
            low: Decimal::ONE,
            close: Decimal::ONE,
            volume: Decimal::ONE,
            trades: Some(1),
        }
    }

    /// A page of candles, built from the clock at the moment it is
    /// requested.
    type Page = Box<dyn Fn(TimestampMs) -> Vec<Candle> + Send>;

    fn page(build: impl Fn(TimestampMs) -> Vec<Candle> + Send + 'static) -> Page {
        Box::new(build)
    }

    /// Serves scripted pages, each response taking `delay`.
    struct ScriptedSource {
        pages: Mutex<VecDeque<Page>>,
        delay: Duration,
    }

    impl ScriptedSource {
        fn new(pages: Vec<Page>) -> Self {
            Self {
                pages: Mutex::new(pages.into()),
                delay: Duration::ZERO,
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
        async fn fetch_klines(
            &self,
            _request: &KlineRequest,
        ) -> Result<Vec<Candle>, ExchangeError> {
            let page = self.pages.lock().expect("lock").pop_front();
            let candles = page.map(|page| page(now_utc_ms())).unwrap_or_default();
            tokio::time::sleep(self.delay).await;
            Ok(candles)
        }

        async fn fetch_symbol_rules(&self, _symbol: &Symbol) -> Result<SymbolRules, ExchangeError> {
            unreachable!("a range sync never fetches symbol rules")
        }

        async fn server_time(&self) -> Result<TimestampMs, ExchangeError> {
            unreachable!("a range sync never reads the server time")
        }
    }

    fn store() -> (tempfile::TempDir, SqliteStore) {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let store = SqliteStore::new(tempdir.path().join("market.sqlite"));
        CandleStore::init(&store).expect("init");
        (tempdir, store)
    }

    fn stored(store: &SqliteStore) -> Vec<i64> {
        store
            .load_candles(&market(), CandleQuery::default())
            .expect("load")
            .iter()
            .map(|candle| candle.open_time_ms)
            .collect()
    }

    #[test]
    fn a_candle_settles_a_second_after_its_close() {
        let closed_at = candle(0).close_time_ms;
        for (cutoff_ms, settled) in [
            (closed_at + SETTLE_MS + 1, 1),
            (closed_at + SETTLE_MS, 0),
            (closed_at + 1, 0),
            (closed_at, 0),
        ] {
            assert_eq!(settled_len(&[candle(0)], cutoff_ms), settled, "{cutoff_ms}");
        }
        // Only the leading run counts, whatever follows it.
        let batch = [candle(0), candle(120_000), candle(60_000)];
        assert_eq!(settled_len(&batch, 120_000 + SETTLE_MS), 1);
        let mut far = candle(0);
        far.close_time_ms = i64::MAX;
        assert_eq!(settled_len(&[far], i64::MAX), 0);
    }

    #[tokio::test]
    async fn an_unsettled_tail_is_held_back_and_ends_the_range() {
        let (_tempdir, store) = store();
        // The newest candle opened ten seconds ago and is still forming.
        let source = ScriptedSource::new(vec![page(|now| {
            let forming = now - 10_000;
            vec![
                candle(forming - 120_000),
                candle(forming - 60_000),
                candle(forming),
            ]
        })]);
        let outcome = sync_market_range(&source, &store, &market(), 0, i64::MAX / 2, 1000)
            .await
            .expect("sync");
        assert_eq!(outcome.written, 2);
        let stored = stored(&store);
        assert_eq!(stored.len(), 2);
        assert_eq!(outcome.first_open_time_ms, stored.first().copied());
        assert_eq!(outcome.last_open_time_ms, stored.last().copied());
        assert_eq!(
            outcome.unsettled_from_ms,
            stored.last().map(|last| last + 60_000)
        );
        assert!(source.pages.lock().expect("lock").is_empty());
    }

    #[tokio::test]
    async fn a_page_with_nothing_settled_writes_nothing_and_ends_the_range() {
        let (_tempdir, store) = store();
        let source = ScriptedSource::new(vec![
            page(|now| vec![candle(now - 10_000)]),
            page(|_| unreachable!("paging stops at an unsettled candle")),
        ]);
        let outcome = sync_market_range(&source, &store, &market(), 0, i64::MAX / 2, 1000)
            .await
            .expect("sync");
        assert_eq!(outcome.written, 0);
        assert_eq!(outcome.first_open_time_ms, None);
        assert!(outcome.unsettled_from_ms.is_some());
        assert!(stored(&store).is_empty());
    }

    #[tokio::test]
    async fn the_cutoff_is_the_clock_before_the_request() {
        let (_tempdir, store) = store();
        // The candle settles 100 ms into a 300 ms request: read after the
        // response, the clock would pass it as settled.
        let mut source = ScriptedSource::new(vec![page(|now| {
            let mut candle = candle(0);
            candle.close_time_ms = now - SETTLE_MS + 100;
            vec![candle]
        })]);
        source.delay = Duration::from_millis(300);
        let outcome = sync_market_range(&source, &store, &market(), 0, i64::MAX / 2, 1000)
            .await
            .expect("sync");
        assert_eq!(outcome.written, 0);
        assert_eq!(outcome.unsettled_from_ms, Some(0));
    }
}
