//! Bot-run state: the desired and actual position, execution mode, run
//! status, the realized-trade record, and the journalled run snapshot that
//! `state_json` round-trips.

use super::{MarketId, Symbol, TimestampMs};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TargetPosition {
    Flat,
    LongAllIn,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecutionMode {
    DryRun,
    Live,
}

impl ExecutionMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DryRun => "dry_run",
            Self::Live => "live",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunStatus {
    Starting,
    Running,
    Stopped,
    Failed,
}

impl RunStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Stopped => "stopped",
            Self::Failed => "failed",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClosedTrade {
    pub symbol: Symbol,
    pub entry_time_ms: TimestampMs,
    pub exit_time_ms: TimestampMs,
    pub entry_price: Decimal,
    pub exit_price: Decimal,
    pub qty: Decimal,
    pub gross_quote_pnl: Decimal,
    pub entry_order_id: Option<i64>,
    pub exit_order_id: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PositionState {
    pub qty: Decimal,
    pub entry_price: Option<Decimal>,
    pub entry_time_ms: Option<TimestampMs>,
    pub entry_order_id: Option<i64>,
}

impl PositionState {
    pub fn flat() -> Self {
        Self {
            qty: Decimal::ZERO,
            entry_price: None,
            entry_time_ms: None,
            entry_order_id: None,
        }
    }

    pub fn is_open(&self) -> bool {
        self.qty > Decimal::ZERO
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BotRunState {
    pub run_id: String,
    pub market: MarketId,
    pub strategy_name: String,
    pub strategy_config: serde_json::Value,
    pub execution_mode: ExecutionMode,
    pub status: RunStatus,
    pub last_processed_open_time_ms: Option<TimestampMs>,
    pub started_at_ms: TimestampMs,
    pub updated_at_ms: TimestampMs,
    pub stopped_at_ms: Option<TimestampMs>,
    pub last_error: Option<String>,
    pub position: PositionState,
}

#[cfg(test)]
mod tests {
    use super::*;

    // `execution_mode` is part of a run's identity: state_json without it is
    // from an unsupported schema generation and must fail to load, never
    // default to a mode.
    #[test]
    fn bot_run_state_without_execution_mode_field_is_rejected() {
        let json = serde_json::json!({
            "run_id": "run-legacy",
            "market": {"exchange": "BinanceSpot", "symbol": "BTCUSDT", "interval": "M1"},
            "strategy_name": "sma_cross",
            "strategy_config": {"kind": "sma_cross", "fast": 20, "slow": 50},
            "status": "Running",
            "last_processed_open_time_ms": null,
            "started_at_ms": 0,
            "updated_at_ms": 0,
            "stopped_at_ms": null,
            "last_error": null,
            "position": {"qty": "0", "entry_price": null, "entry_time_ms": null, "entry_order_id": null}
        });
        let error = serde_json::from_value::<BotRunState>(json)
            .expect_err("state without execution_mode must not load");
        assert!(error.to_string().contains("execution_mode"), "got {error}");
    }
}
