//! Run-state identity: creating or resuming a run, the resume identity
//! checks, and the position → target mapping the loop compares against.

use super::{LiveTradeConfig, LiveTradeEngine};
use crate::EngineError;
use crate::{
    BotRunState, ExecutionMode, PositionState, RunIdentity, RunStatus, TargetPosition, now_utc_ms,
};
use uuid::Uuid;

impl<'a> LiveTradeEngine<'a> {
    pub(super) fn load_or_create_run_state(
        &self,
        cfg: &LiveTradeConfig,
    ) -> Result<BotRunState, EngineError> {
        require_account_for_live(cfg)?;
        if let Some(run_id) = &cfg.run_id {
            if let Some(existing) = self.journal_store.load_run_state(run_id)? {
                apply_resume_checks(&existing, cfg)?;
                let recorded = self.journal_store.run_identity(run_id)?.ok_or_else(|| {
                    EngineError::InvalidState(format!(
                        "run {run_id} has no identity record; refusing to resume"
                    ))
                })?;
                apply_identity_check(run_id, &recorded, &cfg.identity)?;
                return Ok(existing);
            }
        }

        let now_ms = now_utc_ms();
        let state = BotRunState {
            run_id: cfg
                .run_id
                .clone()
                .unwrap_or_else(|| format!("qf-{}", Uuid::new_v4().simple())),
            market: cfg.market.clone(),
            strategy_name: cfg.strategy.strategy_name().to_string(),
            strategy_config: serde_json::to_value(&cfg.strategy).map_err(|err| {
                EngineError::InvalidConfig(format!("failed to serialize strategy config: {err}"))
            })?,
            execution_mode: cfg.execution_mode,
            status: RunStatus::Starting,
            last_processed_open_time_ms: None,
            started_at_ms: now_ms,
            updated_at_ms: now_ms,
            stopped_at_ms: None,
            last_error: None,
            position: PositionState::flat(),
        };
        self.journal_store.create_run(&state, &cfg.identity)?;
        Ok(state)
    }
}

/// A live run is recorded with the account behind its credentials, so that
/// only that key resumes or closes it; an identity without one would let any
/// key do so. A dry run has no account, and keeps none.
fn require_account_for_live(cfg: &LiveTradeConfig) -> Result<(), EngineError> {
    if cfg.execution_mode == ExecutionMode::Live && cfg.identity.account.is_none() {
        return Err(EngineError::InvalidConfig(
            "a live run records the account behind its credentials, but this identity \
             carries none; refusing to start or resume it"
                .to_string(),
        ));
    }
    Ok(())
}

/// A run is driven only under the identity that recorded it: a legacy run
/// (no verified environment or account) is never resumed, and neither is a
/// run recorded under another environment or API key.
fn apply_identity_check(
    run_id: &str,
    recorded: &RunIdentity,
    current: &RunIdentity,
) -> Result<(), EngineError> {
    recorded.matches(current).map_err(|err| {
        EngineError::InvalidConfig(format!("run {run_id}: {err}; refusing to resume"))
    })
}

/// Validates that a resumed run matches the current invocation's identity.
///
/// A run's market, strategy, and execution mode are part of its identity: a
/// position accumulated under one must never be silently adopted by another
/// (a dry-run position leaking into live trading being the worst case).
fn apply_resume_checks(existing: &BotRunState, cfg: &LiveTradeConfig) -> Result<(), EngineError> {
    if existing.market != cfg.market {
        return Err(EngineError::InvalidConfig(format!(
            "run {} was recorded for market {} {} {}, but this invocation targets {} {} {}; \
             refusing to resume",
            existing.run_id,
            existing.market.exchange,
            existing.market.symbol,
            existing.market.interval,
            cfg.market.exchange,
            cfg.market.symbol,
            cfg.market.interval
        )));
    }

    let strategy_name = cfg.strategy.strategy_name();
    if existing.strategy_name != strategy_name {
        return Err(EngineError::InvalidConfig(format!(
            "run {} was recorded with strategy {}, but this invocation uses {}; \
             refusing to resume",
            existing.run_id, existing.strategy_name, strategy_name
        )));
    }

    let strategy_config = serde_json::to_value(&cfg.strategy).map_err(|err| {
        EngineError::InvalidConfig(format!("failed to serialize strategy config: {err}"))
    })?;
    if existing.strategy_config != strategy_config {
        return Err(EngineError::InvalidConfig(format!(
            "run {} was recorded with strategy config {}, but this invocation uses {}; \
             refusing to resume so its position is not driven by different parameters",
            existing.run_id, existing.strategy_config, strategy_config
        )));
    }

    if existing.execution_mode != cfg.execution_mode {
        return Err(EngineError::InvalidConfig(format!(
            "run {} was recorded in {} mode, but this invocation is {} mode; refusing to \
             resume so a {} position cannot leak into {} trading",
            existing.run_id,
            existing.execution_mode.as_str(),
            cfg.execution_mode.as_str(),
            existing.execution_mode.as_str(),
            cfg.execution_mode.as_str()
        )));
    }

    Ok(())
}

pub(super) fn current_target(position: &PositionState) -> TargetPosition {
    if position.is_open() {
        TargetPosition::LongAllIn
    } else {
        TargetPosition::Flat
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        BuiltInStrategyConfig, Environment, ExchangeId, ExecutionMode, Interval, KeyFingerprint,
        MarketId, Symbol, VenueAccountId, VenueOrderId,
    };
    use rust_decimal::Decimal;
    use std::str::FromStr;
    use std::time::Duration;

    fn market() -> MarketId {
        MarketId::new(
            ExchangeId::BinanceSpot,
            Symbol::new("BTCUSDT").expect("symbol"),
            Interval::M1,
        )
    }

    fn run_state() -> BotRunState {
        BotRunState {
            run_id: "run-1".to_string(),
            market: market(),
            strategy_name: "sma_cross".to_string(),
            strategy_config: serde_json::json!({"kind":"sma_cross","fast":20,"slow":50}),
            execution_mode: ExecutionMode::DryRun,
            status: RunStatus::Running,
            last_processed_open_time_ms: None,
            started_at_ms: 0,
            updated_at_ms: 0,
            stopped_at_ms: None,
            last_error: None,
            position: PositionState {
                qty: Decimal::from_str("0.0254").expect("decimal"),
                entry_price: Some(Decimal::from(9_900)),
                entry_time_ms: Some(0),
                entry_order_id: Some(VenueOrderId::new("7").expect("order id")),
            },
        }
    }

    fn config(execution_mode: ExecutionMode) -> LiveTradeConfig {
        LiveTradeConfig {
            market: market(),
            strategy: BuiltInStrategyConfig::SmaCross { fast: 20, slow: 50 },
            execution_mode,
            identity: RunIdentity::recorded(None, None),
            quote_order_qty: Decimal::from(100),
            poll_interval: Duration::from_secs(1),
            bootstrap_bars: 10,
            bootstrap_enter: false,
            batch_limit: 1000,
            run_id: Some("run-1".to_string()),
            max_loops: Some(1),
        }
    }

    #[test]
    fn resume_refuses_execution_mode_mismatch() {
        let existing = run_state();
        let error = apply_resume_checks(&existing, &config(ExecutionMode::Live)).expect_err("mode");
        assert!(matches!(error, EngineError::InvalidConfig(_)));
        assert!(
            error.to_string().contains("recorded in dry_run mode"),
            "got {error}"
        );
        assert!(
            error.to_string().contains("cannot leak into live trading"),
            "got {error}"
        );
    }

    #[test]
    fn resume_refuses_market_mismatch() {
        let mut existing = run_state();
        existing.market = MarketId::new(
            ExchangeId::BinanceSpot,
            Symbol::new("ETHUSDT").expect("symbol"),
            Interval::M1,
        );
        let error =
            apply_resume_checks(&existing, &config(ExecutionMode::DryRun)).expect_err("market");
        assert!(matches!(error, EngineError::InvalidConfig(_)));
        assert!(error.to_string().contains("ETHUSDT"), "got {error}");
        assert!(
            error.to_string().contains("refusing to resume"),
            "got {error}"
        );
    }

    #[test]
    fn resume_refuses_strategy_mismatch() {
        let mut existing = run_state();
        existing.strategy_name = "other_strategy".to_string();
        let error =
            apply_resume_checks(&existing, &config(ExecutionMode::DryRun)).expect_err("strategy");
        assert!(matches!(error, EngineError::InvalidConfig(_)));
        assert!(error.to_string().contains("other_strategy"), "got {error}");
    }

    #[test]
    fn resume_refuses_strategy_parameter_mismatch() {
        let mut existing = run_state();
        existing.strategy_config = serde_json::json!({"kind":"sma_cross","fast":5,"slow":200});
        let error = apply_resume_checks(&existing, &config(ExecutionMode::DryRun))
            .expect_err("parameter mismatch");
        assert!(matches!(error, EngineError::InvalidConfig(_)));
        assert!(error.to_string().contains("strategy config"), "got {error}");
    }

    #[test]
    fn resume_accepts_matching_identity() {
        let existing = run_state();
        apply_resume_checks(&existing, &config(ExecutionMode::DryRun)).expect("matching");
    }

    #[test]
    fn resume_refuses_a_legacy_run_and_another_environment_or_key() {
        let current = RunIdentity::recorded(Some(Environment::Testnet), None);
        let error =
            apply_identity_check("run-1", &RunIdentity::legacy(), &current).expect_err("legacy");
        assert!(matches!(error, EngineError::InvalidConfig(_)));
        assert!(
            error
                .to_string()
                .contains("run run-1: the run's environment and account were never verified"),
            "got {error}"
        );
        assert!(
            error.to_string().ends_with("refusing to resume"),
            "got {error}"
        );
        let unbound = RunIdentity::recorded(None, None);
        let error = apply_identity_check("run-1", &unbound, &current).expect_err("environment");
        assert!(
            error.to_string().contains("recorded as undeclared"),
            "got {error}"
        );
        assert!(apply_identity_check("run-1", &current, &current).is_ok());
    }

    #[test]
    fn a_live_run_needs_an_account_in_its_identity_and_a_dry_run_none() {
        let live = config(ExecutionMode::Live);
        let error = require_account_for_live(&live).expect_err("no account");
        assert!(matches!(error, EngineError::InvalidConfig(_)));
        assert!(
            error
                .to_string()
                .contains("carries none; refusing to start or resume"),
            "got {error}"
        );
        let with_account = LiveTradeConfig {
            identity: RunIdentity::recorded(
                None,
                Some(VenueAccountId::KeyFingerprint(KeyFingerprint::of("key"))),
            ),
            ..live
        };
        assert!(require_account_for_live(&with_account).is_ok());
        assert!(require_account_for_live(&config(ExecutionMode::DryRun)).is_ok());
    }
}
