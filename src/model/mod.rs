//! Normalized domain types shared by every layer: market identity, the
//! candle grid and its validation, exchange orders, bot-run state, and the
//! timestamp and decimal helpers they are built from.

pub mod candle;
pub mod decimal;
pub mod interval;
pub mod market;
pub mod order;
pub mod rules;
pub mod run;
pub mod timestamp;

use thiserror::Error;

pub use candle::{Candle, ValidationIssue, ValidationReport, validate_candles};
pub use decimal::round_down_to_step;
pub use interval::Interval;
pub use market::{ExchangeId, MarketId, Symbol};
pub use order::{AccountTrade, ExchangeOrder, Fill, OrderStatus, Side};
pub use rules::{AssetBalance, SymbolRules};
pub use run::{BotRunState, ClosedTrade, ExecutionMode, PositionState, RunStatus, TargetPosition};
pub use timestamp::{TimestampMs, ms_to_rfc3339, now_utc_ms, parse_rfc3339_to_ms};

#[derive(Error, Debug)]
pub enum ModelError {
    #[error("invalid symbol: {0}")]
    InvalidSymbol(String),

    #[error("invalid interval: {0}")]
    InvalidInterval(String),

    #[error("invalid side: {0}")]
    InvalidSide(String),

    #[error("invalid rfc3339 timestamp: {0}")]
    InvalidTimestamp(String),

    #[error("time parse error: {0}")]
    TimeParse(#[from] time::error::Parse),
}
