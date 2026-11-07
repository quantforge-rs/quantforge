//! Deterministic execution engines: backtest simulation, market-data
//! ingestion, the polling live/dry-run trader, and the transfer of one
//! store into another.

pub mod backtest;
pub mod data_sync;
pub mod live;
pub mod transfer;

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

    /// A transfer refused before it wrote anything, or a copy that cannot
    /// be trusted: the reason says what to do.
    #[error("transfer refused: {0}")]
    TransferRefused(String),

    /// A copy its store failed in `table`. Every batch stands alone, so the
    /// rows it wrote stay, and the copy run again finishes it.
    #[error(
        "the copy stopped in {table}; the rows it wrote stay, and running it again finishes it"
    )]
    CopyStopped {
        table: &'static str,
        #[source]
        source: StorageError,
    },
}
