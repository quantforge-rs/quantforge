//! Historical and incremental candle ingestion: pulls klines from a
//! [`MarketDataConnector`] in bounded batches, writes the settled ones to a
//! [`CandleStore`], optionally follows the tail, and keeps the market's
//! segments packed in a [`SegmentStore`]. The range fetch is `pub(crate)` —
//! [`live`](super::live) drives it to keep its own poll loop fed.
//!
//! A candle is stored only once it has settled, a second after its close:
//! the one still forming is fetched again on a later pass. Every stored
//! candle is then final, which is what lets a segment pack it for good.

mod range;

pub(crate) use range::{sleep_or_shutdown, sync_market_range};

use crate::EngineError;
use crate::{CandleStore, MarketDataConnector, MarketId, SegmentStore, now_utc_ms};
use std::time::Duration;
use tracing::{info, warn};

#[derive(Clone, Debug)]
pub struct DataSyncConfig {
    pub market: MarketId,
    pub start_time_ms: Option<i64>,
    pub end_time_ms: Option<i64>,
    pub batch_limit: u16,
    pub follow: bool,
    pub poll_interval: Duration,
    pub max_loops: Option<usize>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DataSyncSummary {
    pub iterations: usize,
    pub written: usize,
    /// Open time of the first candle written by this sync, if any. Together
    /// with `last_open_time_ms` this reports the actually covered range, which
    /// can be smaller than the requested one when the exchange has no data.
    pub first_synced_open_time_ms: Option<i64>,
    pub last_open_time_ms: Option<i64>,
    /// Windows packed into a new segment, for the first time or because
    /// their candles changed. Always 0 without [`DataSyncEngine::with_segments`].
    pub segments_built: usize,
}

pub struct DataSyncEngine<'a> {
    source: &'a dyn MarketDataConnector,
    store: &'a dyn CandleStore,
    segments: Option<&'a dyn SegmentStore>,
}

impl<'a> std::fmt::Debug for DataSyncEngine<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DataSyncEngine").finish_non_exhaustive()
    }
}

impl<'a> DataSyncEngine<'a> {
    pub fn new(source: &'a dyn MarketDataConnector, store: &'a dyn CandleStore) -> Self {
        Self {
            source,
            store,
            segments: None,
        }
    }

    /// Packs the market's segments after the first pass and after every
    /// pass that writes candles.
    pub fn with_segments(mut self, segments: &'a dyn SegmentStore) -> Self {
        self.segments = Some(segments);
        self
    }

    pub async fn run(&self, cfg: &DataSyncConfig) -> Result<DataSyncSummary, EngineError> {
        let mut summary = DataSyncSummary::default();
        let mut loops = 0usize;
        let step_ms = cfg.market.interval.step_ms();
        let mut next_start_time_ms = initial_start_time_ms(cfg, now_utc_ms());
        // Assigned on every loop iteration; the loop body runs at least once.
        let mut last_effective_end_ms: i64;
        let mut unsettled_from_ms = None;
        let mut refreshed = false;

        loop {
            let now_ms = now_utc_ms();
            let end_time_ms = effective_end_time_ms(cfg, now_ms);
            last_effective_end_ms = end_time_ms;

            if next_start_time_ms <= end_time_ms {
                let outcome = sync_market_range(
                    self.source,
                    self.store,
                    &cfg.market,
                    next_start_time_ms,
                    end_time_ms,
                    cfg.batch_limit,
                )
                .await?;
                summary.written += outcome.written;
                if summary.first_synced_open_time_ms.is_none() {
                    summary.first_synced_open_time_ms = outcome.first_open_time_ms;
                }
                summary.last_open_time_ms = self.store.max_open_time_ms(&cfg.market)?;
                unsettled_from_ms = outcome.unsettled_from_ms;

                if let Some(segments) = self.segments {
                    if outcome.written > 0 || !refreshed {
                        let refresh = segments.refresh_segments(&cfg.market)?;
                        refreshed = true;
                        summary.segments_built += refresh.built;
                        info!(
                            built = refresh.built,
                            unchanged = refresh.unchanged,
                            blocked = refresh.blocked,
                            "segments refreshed"
                        );
                    }
                }

                if let Some(max_open_time_ms) = summary.last_open_time_ms {
                    next_start_time_ms = next_start_time_ms.max(max_open_time_ms + step_ms);
                }

                info!(
                    written = outcome.written,
                    total_written = summary.written,
                    next_start_time_ms,
                    last_open_time_ms = ?summary.last_open_time_ms,
                    "data sync iteration finished"
                );
            } else {
                info!(
                    next_start_time_ms,
                    end_time_ms, "data sync is waiting for the requested time window"
                );
            }

            loops += 1;
            summary.iterations = loops;

            if !should_continue(cfg, next_start_time_ms, loops) {
                break;
            }
            if sleep_or_shutdown(cfg.poll_interval).await {
                break;
            }
        }

        // A bounded sync that ends before reaching its end boundary leaves a
        // gap the operator would otherwise only discover via `data validate`.
        if is_uncovered(
            cfg,
            next_start_time_ms,
            last_effective_end_ms,
            unsettled_from_ms,
        ) {
            warn!(
                uncovered_start_ms = next_start_time_ms,
                uncovered_end_ms = last_effective_end_ms,
                "bounded sync ended with an uncovered range; the exchange returned \
                 no candles for it"
            );
        }

        Ok(summary)
    }
}

fn initial_start_time_ms(cfg: &DataSyncConfig, now_ms: i64) -> i64 {
    cfg.start_time_ms.unwrap_or(now_ms)
}

fn effective_end_time_ms(cfg: &DataSyncConfig, now_ms: i64) -> i64 {
    cfg.end_time_ms
        .map(|end_time_ms| end_time_ms.min(now_ms))
        .unwrap_or(now_ms)
}

/// Whether a bounded sync ended short of its end with candles missing:
/// not when the range left over starts at a candle held back only because
/// it had not settled yet.
fn is_uncovered(
    cfg: &DataSyncConfig,
    next_start_time_ms: i64,
    last_effective_end_ms: i64,
    unsettled_from_ms: Option<i64>,
) -> bool {
    cfg.end_time_ms.is_some()
        && next_start_time_ms <= last_effective_end_ms
        && unsettled_from_ms != Some(next_start_time_ms)
}

fn should_continue(cfg: &DataSyncConfig, next_start_time_ms: i64, loops: usize) -> bool {
    if cfg.max_loops.map(|max| loops >= max).unwrap_or(false) {
        return false;
    }

    match cfg.end_time_ms {
        None => true,
        Some(end_time_ms) => cfg.follow && next_start_time_ms <= end_time_ms,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ExchangeId, Interval, Symbol};

    fn market() -> MarketId {
        MarketId::new(
            ExchangeId::BinanceSpot,
            Symbol::new("BTCUSDT").expect("symbol"),
            Interval::M1,
        )
    }

    fn config(
        start_time_ms: Option<i64>,
        end_time_ms: Option<i64>,
        follow: bool,
    ) -> DataSyncConfig {
        DataSyncConfig {
            market: market(),
            start_time_ms,
            end_time_ms,
            batch_limit: 1000,
            follow,
            poll_interval: Duration::from_secs(1),
            max_loops: None,
        }
    }

    #[test]
    fn omitted_start_defaults_to_now() {
        let cfg = config(None, Some(5_000), false);
        assert_eq!(initial_start_time_ms(&cfg, 12_345), 12_345);
    }

    #[test]
    fn omitted_end_uses_current_time_for_each_iteration() {
        let cfg = config(Some(1_000), None, false);
        assert_eq!(effective_end_time_ms(&cfg, 9_999), 9_999);
    }

    #[test]
    fn explicit_end_is_capped_at_current_time() {
        let cfg = config(Some(1_000), Some(20_000), false);
        assert_eq!(effective_end_time_ms(&cfg, 9_999), 9_999);
    }

    #[test]
    fn omitted_end_keeps_sync_running_without_follow_flag() {
        let cfg = config(Some(1_000), None, false);
        assert!(should_continue(&cfg, 2_000, 1));
    }

    #[test]
    fn bounded_range_without_follow_stops_after_first_iteration() {
        let cfg = config(Some(1_000), Some(5_000), false);
        assert!(!should_continue(&cfg, 2_000, 1));
    }

    #[test]
    fn bounded_range_with_follow_continues_until_end_is_reached() {
        let cfg = config(Some(1_000), Some(5_000), true);
        assert!(should_continue(&cfg, 5_000, 1));
        assert!(!should_continue(&cfg, 5_001, 1));
    }

    #[test]
    fn a_range_left_over_at_a_candle_still_settling_is_not_uncovered() {
        let bounded = config(Some(0), Some(300_000), false);
        assert!(is_uncovered(&bounded, 120_000, 300_000, None));
        // Candles are missing before the one held back.
        assert!(is_uncovered(&bounded, 120_000, 300_000, Some(180_000)));
        assert!(!is_uncovered(&bounded, 120_000, 300_000, Some(120_000)));
        assert!(!is_uncovered(&bounded, 300_001, 300_000, None));
        let unbounded = config(Some(0), None, false);
        assert!(!is_uncovered(&unbounded, 120_000, 300_000, None));
    }

    use crate::{
        Candle, ExchangeCapabilities, ExchangeConnector, ExchangeError, KlineRequest, SqliteStore,
        Symbol as ModelSymbol, SymbolRules, TimestampMs,
    };
    use rust_decimal::Decimal;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    struct ScriptedSource {
        batches: Mutex<VecDeque<Vec<Candle>>>,
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
            Ok(self
                .batches
                .lock()
                .expect("lock")
                .pop_front()
                .unwrap_or_default())
        }

        async fn fetch_symbol_rules(
            &self,
            _symbol: &ModelSymbol,
        ) -> Result<SymbolRules, ExchangeError> {
            unreachable!("data sync never fetches symbol rules")
        }

        async fn server_time(&self) -> Result<TimestampMs, ExchangeError> {
            unreachable!("data sync never reads the server time")
        }
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

    // A bounded sync where the exchange stops returning data mid-range must
    // still report the range it actually covered (and warns about the rest).
    #[tokio::test]
    async fn bounded_sync_reports_covered_range_when_exchange_runs_dry() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let store = SqliteStore::new(tempdir.path().join("market.sqlite"));
        CandleStore::init(&store).expect("init");

        let source = ScriptedSource {
            batches: Mutex::new(VecDeque::from([vec![candle(0), candle(60_000)]])),
        };
        let engine = DataSyncEngine::new(&source, &store);

        let summary = engine
            .run(&DataSyncConfig {
                market: market(),
                start_time_ms: Some(0),
                end_time_ms: Some(300_000),
                batch_limit: 1000,
                follow: false,
                poll_interval: Duration::from_millis(1),
                max_loops: Some(1),
            })
            .await
            .expect("sync");

        assert_eq!(summary.written, 2);
        assert_eq!(summary.first_synced_open_time_ms, Some(0));
        assert_eq!(summary.last_open_time_ms, Some(60_000));
    }
}
