//! The poll loop: bootstrap replay, bar fetching, closed-bar detection,
//! and the per-bar strategy evaluation cadence.

use super::state::current_target;
use super::{LiveTradeConfig, LiveTradeEngine, LiveTradeSummary};
use crate::EngineError;
use crate::engine::data_sync::{sleep_or_shutdown, sync_market_range};
use crate::{
    BotRunState, Candle, CandleQuery, RunStatus, Strategy, StrategyContext, SymbolRules, now_utc_ms,
};
use rust_decimal::Decimal;

impl<'a> LiveTradeEngine<'a> {
    pub(super) async fn run_inner(
        &self,
        cfg: &LiveTradeConfig,
        rules: &SymbolRules,
        run_state: &mut BotRunState,
        strategy: &mut dyn Strategy,
        summary: &mut LiveTradeSummary,
    ) -> Result<(), EngineError> {
        self.journal_store.save_run_state(run_state)?;

        let now = now_utc_ms();
        let bootstrap_start = match run_state.last_processed_open_time_ms {
            Some(value) => Some(value + cfg.market.interval.step_ms()),
            None => {
                let bars = i64::try_from(cfg.bootstrap_bars).map_err(|_| {
                    EngineError::InvalidConfig(format!(
                        "bootstrap_bars {} does not fit into the timestamp range",
                        cfg.bootstrap_bars
                    ))
                })?;
                let window_start = cfg
                    .market
                    .interval
                    .step_ms()
                    .checked_mul(bars)
                    .and_then(|window_ms| now.checked_sub(window_ms))
                    .ok_or_else(|| {
                        EngineError::InvalidConfig(format!(
                            "bootstrap window overflows: {} bars of {}",
                            cfg.bootstrap_bars, cfg.market.interval
                        ))
                    })?;
                Some(window_start)
            }
        };

        if let Some(start_ms) = bootstrap_start {
            sync_market_range(
                self.market_data,
                self.candle_store,
                &cfg.market,
                start_ms,
                now,
                cfg.batch_limit,
            )
            .await?;
        }

        let mut ctx = StrategyContext {
            market: cfg.market.clone(),
            now_ms: now_utc_ms(),
            // Live trading does not track quote-asset cash: `ctx.cash` is
            // always zero here, while the backtest context reports real cash.
            // Cash-based position sizing is a backtest-only feature today;
            // live sizing comes from `LiveTradeConfig::quote_order_qty`.
            cash: Decimal::ZERO,
            position_qty: run_state.position.qty,
        };
        strategy.on_start(&ctx)?;

        let bootstrap_candles = self
            .candle_store
            .load_recent_candles(&cfg.market, cfg.bootstrap_bars)?;
        let closed_bootstrap = filter_closed_candles(bootstrap_candles);

        let mut last_bootstrap_target = current_target(&run_state.position);
        for candle in &closed_bootstrap {
            ctx.now_ms = candle.close_time_ms;
            ctx.position_qty = run_state.position.qty;
            if let Some(target) = strategy.on_bar(&ctx, candle)? {
                last_bootstrap_target = target;
            }
        }

        if run_state.last_processed_open_time_ms.is_none() {
            run_state.last_processed_open_time_ms = closed_bootstrap.last().map(|c| c.open_time_ms);
            run_state.status = RunStatus::Running;
            run_state.updated_at_ms = now_utc_ms();

            if cfg.bootstrap_enter && last_bootstrap_target != current_target(&run_state.position) {
                if let Some(reference_bar) = closed_bootstrap.last() {
                    self.execute_target(
                        cfg,
                        rules,
                        run_state,
                        last_bootstrap_target,
                        reference_bar,
                        summary,
                    )
                    .await?;
                }
            }
            self.journal_store.save_run_state(run_state)?;
        }

        let mut loops = 0usize;
        loop {
            let end_ms = now_utc_ms();
            let start_ms = run_state
                .last_processed_open_time_ms
                .map(|value| value + cfg.market.interval.step_ms())
                .unwrap_or_else(|| end_ms - cfg.market.interval.step_ms());

            if start_ms <= end_ms {
                sync_market_range(
                    self.market_data,
                    self.candle_store,
                    &cfg.market,
                    start_ms,
                    end_ms,
                    cfg.batch_limit,
                )
                .await?;
            }

            let new_candles = self.candle_store.load_candles(
                &cfg.market,
                CandleQuery {
                    start_time_ms: run_state
                        .last_processed_open_time_ms
                        .map(|value| value + cfg.market.interval.step_ms()),
                    end_time_ms: None,
                    limit: None,
                },
            )?;
            let closed_new_candles = filter_closed_candles(new_candles);

            for candle in closed_new_candles {
                if run_state
                    .last_processed_open_time_ms
                    .map(|value| candle.open_time_ms <= value)
                    .unwrap_or(false)
                {
                    continue;
                }

                ctx.now_ms = candle.close_time_ms;
                ctx.position_qty = run_state.position.qty;

                let desired = strategy
                    .on_bar(&ctx, &candle)?
                    .unwrap_or_else(|| current_target(&run_state.position));

                if desired != current_target(&run_state.position) {
                    self.execute_target(cfg, rules, run_state, desired, &candle, summary)
                        .await?;
                }

                run_state.last_processed_open_time_ms = Some(candle.open_time_ms);
                run_state.status = RunStatus::Running;
                run_state.updated_at_ms = now_utc_ms();
                self.journal_store.save_run_state(run_state)?;
                summary.processed_bars += 1;
                summary.last_processed_open_time_ms = run_state.last_processed_open_time_ms;
            }

            loops += 1;
            if cfg.max_loops.map(|max| loops >= max).unwrap_or(false) {
                break;
            }
            if sleep_or_shutdown(cfg.poll_interval).await {
                break;
            }
        }

        strategy.on_finish(&ctx)?;
        Ok(())
    }
}

fn filter_closed_candles(candles: Vec<Candle>) -> Vec<Candle> {
    let now_ms = now_utc_ms();
    candles
        .into_iter()
        .filter(|candle| candle.close_time_ms <= now_ms)
        .collect()
}
