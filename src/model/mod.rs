//! Normalized domain types shared by every layer: market identity with its
//! venues, product families and product lines, the candle grid and its
//! validation, the columnar candle batch, the candle segments a store packs,
//! exchange orders and the exchange-agnostic requests that produce them,
//! bot-run state, and the timestamp and decimal helpers they are built
//! from, including the decimal ↔ float64 conversions. Execution identity,
//! units and refusals are shared too; the futures contract of ADR 0002
//! lives in [`futures`].

pub mod candle;
pub mod candle_batch;
pub mod decimal;
pub mod futures;
pub mod identity;
pub mod interval;
pub mod market;
pub mod money;
pub mod order;
pub mod refusal;
pub mod request;
pub mod rules;
pub mod run;
pub mod segment;
pub mod timestamp;
pub mod units;

use thiserror::Error;

pub use candle::{Candle, ValidationIssue, ValidationReport, validate_candles};
pub use candle_batch::{CandleBatch, CandleColumns};
pub use decimal::{decimal_from_f64, decimal_to_f64, round_down_to_step};
pub use identity::{
    AccountRef, Environment, InstrumentId, OperationId, VenueAccountId, VenueAlgoId, VenueOrderId,
    VenueTradeId,
};
pub use interval::Interval;
pub use market::{Category, ExchangeId, MarketId, Symbol, Venue};
pub use money::{Asset, Money};
pub use order::{AccountTrade, ExchangeOrder, Fill, OrderStatus, Side};
pub use refusal::Refusal;
pub use request::{CancelOrderRequest, KlineRequest, MarketOrderRequest, OrderQueryRequest};
pub use rules::{AssetBalance, SymbolRules};
pub use run::{BotRunState, ClosedTrade, ExecutionMode, PositionState, RunStatus, TargetPosition};
pub use segment::{SegmentHash, SegmentPlan, SegmentRef, SegmentRefresh, SegmentTip};
pub use timestamp::{TimestampMs, ms_to_rfc3339, now_utc_ms, parse_rfc3339_to_ms};
pub use units::{BaseQty, Contracts, FaceValue, Leverage, PositionSide, Price, Quantity};

#[derive(Error, Debug)]
pub enum ModelError {
    #[error("invalid symbol: {0}")]
    InvalidSymbol(String),

    #[error("invalid interval: {0}")]
    InvalidInterval(String),

    #[error("invalid side: {0}")]
    InvalidSide(String),

    #[error("invalid segment hash {0:?}: expected sha256: and 64 lowercase hex digits")]
    InvalidSegmentHash(String),

    #[error("invalid rfc3339 timestamp: {0}")]
    InvalidTimestamp(String),

    #[error("time parse error: {0}")]
    TimeParse(#[from] time::error::Parse),

    #[error("candle column `{column}` has {found} rows, expected {expected}")]
    CandleColumnLength {
        column: &'static str,
        found: usize,
        expected: usize,
    },

    // Debug formatting keeps the float short: Display would write f64::MAX
    // as 309 digits.
    #[error("cannot convert {value:?} to a decimal: {reason}")]
    FloatToDecimal { value: f64, reason: &'static str },

    #[error("invalid decimal {0:?}: expected exact text such as 0.10 or -1.5")]
    InvalidDecimal(String),

    #[error("invalid asset {0:?}: expected a code without spaces or control characters")]
    InvalidAsset(String),

    #[error("cannot combine {left} with {right}: amounts in different assets are never summed")]
    AssetMismatch { left: String, right: String },

    #[error("invalid {what} {value}: must be {rule}")]
    InvalidAmount {
        what: &'static str,
        value: String,
        rule: &'static str,
    },

    #[error("{subject} must be in {expected}, not {found}")]
    UnitMismatch {
        subject: String,
        expected: &'static str,
        found: &'static str,
    },

    #[error("invalid {what} {value:?}: {rule}")]
    InvalidIdentifier {
        what: &'static str,
        value: String,
        rule: &'static str,
    },

    #[error("unsupported instrument {instrument}: {reason}")]
    UnsupportedInstrument { instrument: String, reason: String },

    #[error("{machine} cannot move from {from} to {to}")]
    InvalidTransition {
        machine: &'static str,
        from: &'static str,
        to: &'static str,
    },

    #[error("{machine} reaches {to} only on {evidence}")]
    MissingEvidence {
        machine: &'static str,
        to: &'static str,
        evidence: String,
    },

    #[error("{record} format {found} is not supported; this build reads format {expected}")]
    UnsupportedFormat {
        record: &'static str,
        found: u32,
        expected: u32,
    },
}
