//! A backtest in a worker: the engine run over the job's candles with the
//! settings its submission names, the job's progress said as its bars go
//! by, and what it produced as the rows the worker sends its server.
//! Whatever strategy the worker runs is the caller's.

use super::frames::{Finished, JobFrame};
use super::report::Reporter;
use crate::engine::EngineError;
use crate::engine::backtest::{BacktestConfig, BacktestEngine};
use crate::model::{Candle, TargetPosition};
use crate::protocol::{ErrorBody, ErrorCode};
use crate::sdk::{Strategy, StrategyContext, StrategyError};
use crate::server::results::BacktestOutput;
use crate::server::selection::Selection;

/// How a worker's job ended, before the worker says so.
pub(super) enum Ended {
    /// With an outcome to send as it is.
    Said(Finished),
    /// With a backtest's result, to send in its frames.
    Backtest(Box<BacktestOutput>),
}

/// About how many times a job says how far it has come before its end.
const PROGRESS_STEPS: u64 = 100;

/// How many bars go by between two counts said, in a job of `bars` bars.
fn step(bars: u64) -> u64 {
    (bars / PROGRESS_STEPS).max(1)
}

/// A strategy as the engine sees it in a worker: the job's own, with each
/// bar it is handed counted, and the count said every so many bars. It
/// hands on every method of [`Strategy`]: one the trait gains is added
/// here too, or the job's own would never be asked.
struct Followed<'a, 'r> {
    strategy: &'a mut dyn Strategy,
    reporter: &'a mut Reporter<'r>,
    bars_done: u64,
    /// How many bars go by between two counts said.
    every: u64,
}

impl Strategy for Followed<'_, '_> {
    fn name(&self) -> &str {
        self.strategy.name()
    }

    fn on_start(&mut self, ctx: &StrategyContext) -> Result<(), StrategyError> {
        self.strategy.on_start(ctx)
    }

    fn on_bar(
        &mut self,
        ctx: &StrategyContext,
        bar: &Candle,
    ) -> Result<Option<TargetPosition>, StrategyError> {
        let decided = self.strategy.on_bar(ctx, bar)?;
        self.bars_done += 1;
        if self.bars_done % self.every == 0 {
            self.reporter.progress(self.bars_done);
        }
        Ok(decided)
    }

    fn on_finish(&mut self, ctx: &StrategyContext) -> Result<(), StrategyError> {
        self.strategy.on_finish(ctx)
    }
}

/// Runs `strategy` over `candles` under the job's settings, saying through
/// `reporter` how far it has come.
pub(super) fn run(
    job: &JobFrame,
    candles: &[Candle],
    strategy: &mut dyn Strategy,
    reporter: &mut Reporter<'_>,
) -> Ended {
    let bars = u64::try_from(candles.len()).unwrap_or(u64::MAX);
    reporter.progress(0);
    let mut followed = Followed {
        strategy,
        reporter,
        bars_done: 0,
        every: step(bars),
    };
    match backtest(job, candles, &mut followed) {
        Ok(output) => {
            // Every bar went by, whatever the last count said.
            followed.reporter.progress(bars);
            Ended::Backtest(Box::new(output))
        }
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

#[cfg(test)]
mod tests {
    use rust_decimal::Decimal;

    use super::super::frames;
    use super::*;
    use crate::model::{ExchangeId, Interval, MarketId, Symbol};

    /// A strategy that notes every call, and fails on the bar it is told to.
    #[derive(Default)]
    struct Noting {
        calls: Vec<&'static str>,
        fails_on: Option<usize>,
    }

    impl Strategy for Noting {
        fn name(&self) -> &str {
            "noting"
        }

        fn on_start(&mut self, _ctx: &StrategyContext) -> Result<(), StrategyError> {
            self.calls.push("start");
            Ok(())
        }

        fn on_bar(
            &mut self,
            _ctx: &StrategyContext,
            _bar: &Candle,
        ) -> Result<Option<TargetPosition>, StrategyError> {
            self.calls.push("bar");
            let bars = self.calls.iter().filter(|call| **call == "bar").count();
            if self.fails_on == Some(bars) {
                return Err(StrategyError::msg("no"));
            }
            Ok(Some(TargetPosition::LongAllIn))
        }

        fn on_finish(&mut self, _ctx: &StrategyContext) -> Result<(), StrategyError> {
            self.calls.push("finish");
            Ok(())
        }
    }

    fn context() -> StrategyContext {
        StrategyContext {
            market: MarketId::new(
                ExchangeId::BinanceSpot,
                Symbol::new("BTCUSDT").expect("a symbol"),
                Interval::M1,
            ),
            now_ms: 0,
            cash: Decimal::ZERO,
            position_qty: Decimal::ZERO,
        }
    }

    fn bar() -> Candle {
        Candle {
            open_time_ms: 0,
            close_time_ms: 59_999,
            open: Decimal::ONE,
            high: Decimal::ONE,
            low: Decimal::ONE,
            close: Decimal::ONE,
            volume: Decimal::ONE,
            trades: None,
        }
    }

    /// The counts of bars the frames in `output` say, in order.
    fn said(mut output: &[u8]) -> Vec<u64> {
        let mut counts = Vec::new();
        while let Some(frame) =
            frames::read::<_, serde_json::Value>(&mut output, 1 << 20).expect("a frame")
        {
            assert_eq!(frame["type"], "progress", "{frame}");
            counts.push(frame["bars_done"].as_u64().expect("a count"));
        }
        counts
    }

    #[test]
    fn a_job_says_how_far_it_has_come_about_a_hundred_times() {
        for (bars, every) in [
            (0, 1),
            (1, 1),
            (199, 1),
            (200, 2),
            (1_440, 14),
            (527_040, 5_270),
            (u64::MAX, u64::MAX / 100),
        ] {
            assert_eq!(step(bars), every, "{bars} bars");
        }
    }

    #[test]
    fn a_followed_strategy_is_asked_as_the_jobs_own_and_its_bars_are_counted() {
        let mut output = Vec::new();
        let mut own = Noting::default();
        {
            let mut reporter = Reporter::new(&mut output);
            let mut followed = Followed {
                strategy: &mut own,
                reporter: &mut reporter,
                bars_done: 0,
                every: 3,
            };
            assert_eq!(followed.name(), "noting");
            followed.on_start(&context()).expect("a start");
            for _ in 0..7 {
                let decided = followed.on_bar(&context(), &bar()).expect("a bar");
                assert_eq!(
                    decided,
                    Some(TargetPosition::LongAllIn),
                    "the job's own answer"
                );
            }
            followed.on_finish(&context()).expect("a finish");
        }
        let mut calls = vec!["start"];
        calls.extend(["bar"; 7]);
        calls.push("finish");
        assert_eq!(own.calls, calls);
        assert_eq!(said(&output), [3, 6], "every third bar");
    }

    #[test]
    fn a_bar_the_strategy_fails_on_is_not_counted_and_its_failure_is_the_jobs() {
        let mut output = Vec::new();
        let mut own = Noting {
            fails_on: Some(3),
            ..Noting::default()
        };
        {
            let mut reporter = Reporter::new(&mut output);
            let mut followed = Followed {
                strategy: &mut own,
                reporter: &mut reporter,
                bars_done: 0,
                every: 1,
            };
            for _ in 0..2 {
                followed.on_bar(&context(), &bar()).expect("a bar");
            }
            let failed = followed.on_bar(&context(), &bar()).expect_err("a failure");
            assert_eq!(failed.to_string(), StrategyError::msg("no").to_string());
        }
        assert_eq!(said(&output), [1, 2]);
    }
}
