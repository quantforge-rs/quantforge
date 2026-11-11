//! A backtest run in a real worker: the rows it sends its server become
//! exactly the three files a run of the same engine writes in this process,
//! however many frames they took, and a backtest that cannot run fails its
//! job in words a client can act on.

use quantforge::protocol::ErrorCode;
use quantforge::protocol::results::{Metrics, decode_equity, decode_trades};
use quantforge::server::{BacktestOutput, ResultFile, WorkerOutcome};
use quantforge::{
    BacktestConfig, BacktestEngine, BuiltInStrategyConfig, Candle, ExchangeId, Interval, MarketId,
    Symbol,
};
use rust_decimal::Decimal;

use crate::support::{Pool, candles, failure, input, roomy};

/// What the SMA cross of `fast` and `slow` bars answers on `candles` in
/// this process, under the settings every job here submits, as the three
/// files it is written as.
pub(crate) fn local_files(candles: &[Candle], fast: usize, slow: usize) -> Vec<ResultFile> {
    let market = MarketId::new(
        ExchangeId::BinanceSpot,
        Symbol::new("BTCUSDT").expect("a symbol"),
        Interval::M1,
    );
    let mut strategy = BuiltInStrategyConfig::SmaCross { fast, slow }
        .build()
        .expect("a strategy");
    let result = BacktestEngine::new(BacktestConfig {
        initial_cash: Decimal::from(10_000),
        fee_bps: Decimal::from(10),
        close_out_at_end: true,
    })
    .run(&market, candles, strategy.as_mut())
    .expect("a backtest");
    BacktestOutput::of(&result).files().expect("the files")
}

fn succeeded(outcome: WorkerOutcome) -> Vec<ResultFile> {
    match outcome {
        WorkerOutcome::Succeeded(files) => files,
        other => panic!("a succeeded job, not {other:?}"),
    }
}

#[test]
fn a_backtest_in_a_worker_answers_the_files_a_local_run_writes() {
    let pool = Pool::start(1, roomy());
    // More bars than one frame of rows carries, so the curve comes in three.
    let stored = candles(25_000);
    let expected = local_files(&stored, 2, 3);
    let files = succeeded(
        pool.run(input(r#"workload = "backtest""#, stored.clone()))
            .outcome,
    );
    let names: Vec<&str> = files
        .iter()
        .map(|file| file.reference.name.as_str())
        .collect();
    assert_eq!(names, ["metrics", "trades", "equity"]);
    for (file, local) in files.iter().zip(&expected) {
        assert_eq!(file.reference, local.reference, "{}", local.reference.name);
        assert!(file.bytes == local.bytes, "{}", local.reference.name);
    }
    let metrics = Metrics::decode(&files[0].bytes).expect("the metrics");
    let trades = decode_trades(&files[1].bytes).expect("the trades");
    let curve = decode_equity(&files[2].bytes).expect("the curve");
    assert!(trades.len() >= 10, "the fixture trades: {}", trades.len());
    assert_eq!(
        metrics.trade_count,
        u64::try_from(trades.len()).expect("a count")
    );
    assert_eq!(curve.len(), stored.len());

    // Other periods are another result, from a fresh worker.
    let slower = succeeded(
        pool.run(input(
            "workload = \"backtest\"\nfast = 5\nslow = 20",
            stored.clone(),
        ))
        .outcome,
    );
    assert_ne!(slower[1].reference.hash, files[1].reference.hash);
    for (file, local) in slower.iter().zip(&local_files(&stored, 5, 20)) {
        assert_eq!(file.reference, local.reference, "{}", local.reference.name);
    }
}

#[test]
fn a_backtest_of_one_bar_has_a_curve_of_one_row_and_no_trades() {
    let pool = Pool::start(1, roomy());
    let stored = candles(1);
    let files = succeeded(
        pool.run(input(r#"workload = "backtest""#, stored.clone()))
            .outcome,
    );
    assert!(
        decode_trades(&files[1].bytes)
            .expect("the trades")
            .is_empty()
    );
    assert_eq!(decode_equity(&files[2].bytes).expect("the curve").len(), 1);
    for (file, local) in files.iter().zip(&local_files(&stored, 2, 3)) {
        assert_eq!(file.reference, local.reference);
    }
}

#[test]
fn a_backtest_that_cannot_run_fails_its_job_as_the_requests_fault() {
    let pool = Pool::start(1, roomy());
    // Periods no SMA cross takes.
    let unbuilt = pool.run(input(
        "workload = \"backtest\"\nfast = 3\nslow = 3",
        candles(10),
    ));
    let (code, message) = failure(&unbuilt);
    assert_eq!(code, ErrorCode::InvalidRequest);
    assert!(
        message.starts_with("the strategy cannot be built: "),
        "{message}"
    );
}
