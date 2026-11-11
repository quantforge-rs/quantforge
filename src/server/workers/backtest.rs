//! A backtest in a worker: the engine run over the job's candles with the
//! settings its submission names, and what it produced as the rows the
//! worker sends its server. Whatever strategy the worker runs is the
//! caller's.

use super::frames::{Finished, JobFrame};
use crate::engine::EngineError;
use crate::engine::backtest::{BacktestConfig, BacktestEngine};
use crate::model::Candle;
use crate::protocol::{ErrorBody, ErrorCode};
use crate::sdk::Strategy;
use crate::server::results::BacktestOutput;
use crate::server::selection::Selection;

/// How a worker's job ended, before the worker says so.
pub(super) enum Ended {
    /// With an outcome to send as it is.
    Said(Finished),
    /// With a backtest's result, to send in its frames.
    Backtest(Box<BacktestOutput>),
}

/// Runs `strategy` over `candles` under the job's settings.
pub(super) fn run(job: &JobFrame, candles: &[Candle], strategy: &mut dyn Strategy) -> Ended {
    match backtest(job, candles, strategy) {
        Ok(output) => Ended::Backtest(Box::new(output)),
        Err(error) => Ended::Said(Finished::Failed { error }),
    }
}

fn backtest(
    job: &JobFrame,
    candles: &[Candle],
    strategy: &mut dyn Strategy,
) -> Result<BacktestOutput, ErrorBody> {
    // The server checked the submission before it queued the job.
    let unreadable = || {
        failure(
            ErrorCode::Internal,
            "the worker cannot read the job's market",
        )
    };
    let selection = Selection::read_data(&job.submission.data).map_err(|_| unreadable())?;
    let market = selection.market(selection.category.ok_or_else(unreadable)?);
    let settings = &job.submission.backtest;
    let engine = BacktestEngine::new(BacktestConfig {
        initial_cash: settings.initial_cash.get(),
        fee_bps: settings.fee_bps.get(),
        close_out_at_end: settings.close_out_at_end,
    });
    let result = engine
        .run(&market, candles, strategy)
        .map_err(|error| match error {
            EngineError::Strategy(error) => failure(
                ErrorCode::InvalidRequest,
                format!("the strategy failed: {error}"),
            ),
            EngineError::NoCandles => failure(
                ErrorCode::InvalidRequest,
                "the job's range holds no candles",
            ),
            EngineError::InvalidConfig(_) | EngineError::Overflow(_) => failure(
                ErrorCode::InvalidRequest,
                format!("the backtest could not be computed: {error}"),
            ),
            other => failure(ErrorCode::Internal, format!("the backtest failed: {other}")),
        })?;
    Ok(BacktestOutput::of(&result))
}

fn failure(code: ErrorCode, message: impl Into<String>) -> ErrorBody {
    ErrorBody {
        code,
        message: message.into(),
        supported: None,
    }
}
