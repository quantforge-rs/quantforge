//! How large a job's result is, and what a store keeps of it: a year of
//! one-minute bars, the longest range a server runs by default. Run on
//! demand:
//!
//! ```bash
//! cargo test --release --test results measure -- --ignored --nocapture
//! ```

use std::time::Instant;

use quantforge::protocol::results::{decode_equity, decode_trades};
use quantforge::server::{BacktestOutput, DEFAULT_MAX_SPAN_BARS};
use quantforge::{
    BacktestConfig, BacktestEngine, BuiltInStrategyConfig, Candle, ExchangeId, Interval, MarketId,
    Symbol,
};
use rust_decimal::Decimal;

/// One-minute candles with prices of two decimal places that drift and
/// wave, as a liquid Spot market's do.
fn candles(count: i64) -> Vec<Candle> {
    let close = |i: i64| {
        let phase = i % 360;
        let height = if phase < 180 { phase } else { 360 - phase };
        Decimal::new(6_701_234 + 37 * height + (i * 7_919) % 1_009 + i / 50, 2)
    };
    (0..count)
        .map(|i| {
            let (open, close) = (close((i - 1).max(0)), close(i));
            Candle {
                open_time_ms: 1_792_454_400_000 + i * 60_000,
                close_time_ms: 1_792_454_400_000 + (i + 1) * 60_000 - 1,
                open,
                high: open.max(close),
                low: open.min(close),
                close,
                volume: Decimal::new(50_000_000, 8),
                trades: None,
            }
        })
        .collect()
}

#[test]
#[ignore = "a measurement, run on demand"]
fn measure_a_year_of_one_minute_bars() {
    let bars = i64::try_from(DEFAULT_MAX_SPAN_BARS).expect("a count");
    let candles = candles(bars);
    let market = MarketId::new(
        ExchangeId::BinanceSpot,
        Symbol::new("BTCUSDT").expect("a symbol"),
        Interval::M1,
    );
    let mut strategy = BuiltInStrategyConfig::SmaCross { fast: 20, slow: 50 }
        .build()
        .expect("a strategy");
    let started = Instant::now();
    let result = BacktestEngine::new(BacktestConfig {
        initial_cash: Decimal::from(10_000),
        fee_bps: Decimal::from(10),
        close_out_at_end: true,
    })
    .run(&market, &candles, strategy.as_mut())
    .expect("a backtest");
    println!(
        "{bars} bars, {} trades: the engine took {:?}",
        result.trade_count,
        started.elapsed()
    );

    let started = Instant::now();
    let files = BacktestOutput::of(&result).files().expect("the files");
    println!("the three files were written in {:?}", started.elapsed());
    for file in &files {
        let started = Instant::now();
        let packed = zstd::bulk::compress(&file.bytes, 3).expect("zstd");
        let took = started.elapsed();
        println!(
            "{:<8} {:>10} bytes, {:>9} as a store keeps it ({:.1}x), packed in {took:?}",
            file.reference.name,
            file.bytes.len(),
            packed.len(),
            file.bytes.len() as f64 / packed.len() as f64
        );
    }
    let started = Instant::now();
    let trades = decode_trades(&files[1].bytes).expect("the trades");
    let curve = decode_equity(&files[2].bytes).expect("the curve");
    println!(
        "{} trades and {} equity rows read back strictly in {:?}",
        trades.len(),
        curve.len(),
        started.elapsed()
    );
}
