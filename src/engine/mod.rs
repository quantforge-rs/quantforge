//! Deterministic execution engines: backtest simulation, market-data
//! ingestion, and the polling live/dry-run trader.

pub mod backtest;
pub mod data_sync;
pub mod live;

use thiserror::Error;

use crate::{ExchangeError, StorageError, StrategyError};

#[derive(Error, Debug)]
pub enum EngineError {
    #[error("no candles provided")]
    NoCandles,

    #[error("invalid configuration: {0}")]
    InvalidConfig(String),

    #[error("invalid state: {0}")]
    InvalidState(String),

    /// A decimal result fell outside the representable range; the payload
    /// names the step, such as "the entry". Reported instead of the panic
    /// rust_decimal's operators would raise.
    #[error("decimal overflow computing {0}")]
    Overflow(&'static str),

    #[error(transparent)]
    Strategy(#[from] StrategyError),

    #[error(transparent)]
    Exchange(#[from] ExchangeError),

    #[error(transparent)]
    Storage(#[from] StorageError),
}
