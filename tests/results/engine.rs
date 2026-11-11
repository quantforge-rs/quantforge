//! An engine's result written as its three files and read back: every
//! decimal the engine computed comes back digit for digit, compared as
//! text, where `==` on decimals would take `1.10` for `1.1`.

use quantforge::protocol::results::{Metrics, decode_equity, decode_trades};
use quantforge::server::BacktestOutput;
use quantforge::{
    BacktestConfig, BacktestEngine, BacktestResult, BuiltInStrategyConfig, Candle, ExchangeId,
    Interval, MarketId, Symbol,
};
use rust_decimal::Decimal;
use std::str::FromStr;

fn market() -> MarketId {
    MarketId::new(
        ExchangeId::BinanceSpot,
        Symbol::new("BTCUSDT").expect("a symbol"),
        Interval::M1,
    )
}

fn dec(text: &str) -> Decimal {
    Decimal::from_str(text).expect("a decimal")
}

/// `count` one-minute candles whose closes rise and fall in waves of 24
/// bars around a slow drift, with prices of eight decimal places, so that
/// an entry's quantity and every equity mark after it use all the digits a
/// decimal holds.
fn waves(count: i64) -> Vec<Candle> {
    let close = |i: i64| {
        let phase = i % 24;
        let height = if phase < 12 { phase } else { 24 - phase };
        Decimal::new(
            6_701_234_567_891 + 13_700_000_123 * height + 1_000_007 * i,
            8,
        )
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

fn run(candles: &[Candle], cash: &str, fee_bps: &str, close_out_at_end: bool) -> BacktestResult {
    let mut strategy = BuiltInStrategyConfig::SmaCross { fast: 2, slow: 3 }
        .build()
        .expect("a strategy");
    BacktestEngine::new(BacktestConfig {
        initial_cash: dec(cash),
        fee_bps: dec(fee_bps),
        close_out_at_end,
    })
    .run(&market(), candles, strategy.as_mut())
    .expect("a backtest")
}

/// Every value of a result as text, in the order the files hold them.
fn digits(result: &BacktestResult) -> Vec<String> {
    let mut texts = vec![
        result.initial_cash.to_string(),
        result.final_equity.to_string(),
        result.total_return_pct.to_string(),
        result.max_drawdown_pct.to_string(),
        result.trade_count.to_string(),
    ];
    for trade in &result.trades {
        assert_eq!(
            (&trade.entry_order_id, &trade.exit_order_id),
            (&None, &None)
        );
        texts.push(format!(
            "{} {} {} {} {} {} {}",
            trade.symbol,
            trade.entry_time_ms,
            trade.exit_time_ms,
            trade.entry_price,
            trade.exit_price,
            trade.qty,
            trade.gross_quote_pnl
        ));
    }
    for point in &result.equity_curve {
        texts.push(format!("{} {}", point.open_time_ms, point.equity));
    }
    texts
}

/// The same texts, read back from the three files.
fn read_back(files: &[Vec<u8>]) -> Vec<String> {
    let metrics = Metrics::decode(&files[0]).expect("the metrics");
    let mut texts = vec![
        metrics.initial_cash.to_string(),
        metrics.final_equity.to_string(),
        metrics.total_return_pct.to_string(),
        metrics.max_drawdown_pct.to_string(),
        metrics.trade_count.to_string(),
    ];
    for trade in decode_trades(&files[1]).expect("the trades") {
        texts.push(format!(
            "{} {} {} {} {} {} {}",
            trade.symbol,
            trade.entry_time_ms,
            trade.exit_time_ms,
            trade.entry_price,
            trade.exit_price,
            trade.qty,
            trade.gross_quote_pnl
        ));
    }
    for row in decode_equity(&files[2]).expect("the curve") {
        texts.push(format!("{} {}", row.open_time_ms, row.equity));
    }
    texts
}

fn files(result: &BacktestResult) -> Vec<Vec<u8>> {
    BacktestOutput::of(result)
        .files()
        .expect("the files")
        .into_iter()
        .map(|file| file.bytes)
        .collect()
}

#[test]
fn an_engines_result_reads_back_from_its_files_digit_for_digit() {
    let candles = waves(1_500);
    for (cash, fee_bps, close_out) in [
        ("10000", "10", true),
        ("10000.00", "7.5", false),
        ("0.00000001", "0", true),
        ("79228162514264.337593543950335", "0.0001", true),
    ] {
        let result = run(&candles, cash, fee_bps, close_out);
        let written = files(&result);
        assert_eq!(
            read_back(&written),
            digits(&result),
            "cash {cash}, fee {fee_bps}"
        );
        assert_eq!(result.equity_curve.len(), candles.len());
        assert_eq!(files(&result), written, "the same result, the same files");
    }
}

// The test above would pass on values that never needed their digits: this
// holds the fixture to trading, to quantities of 28 digits, and to equity
// marks that end in zeros a number compare would drop.
#[test]
fn the_fixture_trades_and_uses_every_digit_a_decimal_holds() {
    let result = run(&waves(1_500), "10000.00", "7.5", false);
    assert!(result.trade_count >= 50, "{} trades", result.trade_count);
    let digits_of = |value: Decimal| value.to_string().replace(['.', '-'], "").len();
    assert!(
        result.trades.iter().any(|trade| digits_of(trade.qty) >= 28),
        "a quantity of 28 digits"
    );
    assert!(
        result
            .equity_curve
            .iter()
            .any(|point| digits_of(point.equity) >= 28),
        "an equity mark of 28 digits"
    );
    let text = result.equity_curve[0].equity.to_string();
    assert_eq!(text, "10000.00", "the first mark keeps the cash's scale");
    assert_eq!(
        read_back(&files(&result))[5 + result.trades.len()],
        format!("{} 10000.00", result.equity_curve[0].open_time_ms)
    );
}

#[test]
fn a_result_of_no_trades_and_one_bar_has_its_three_files() {
    let candles = waves(1);
    let result = run(&candles, "250", "10", true);
    assert_eq!((result.trade_count, result.equity_curve.len()), (0, 1));
    let written = files(&result);
    assert_eq!(read_back(&written), digits(&result));
    assert_eq!(
        String::from_utf8(written[0].clone()).expect("JSON"),
        r#"{"initial_cash":"250","final_equity":"250","total_return_pct":"0","max_drawdown_pct":"0","trade_count":0}"#
    );
}
