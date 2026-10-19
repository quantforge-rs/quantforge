//! Black-box tests for `BacktestEngine`, driven through the crate's public
//! API the way the Python bindings will drive it. Moved out of
//! `src/engine/backtest.rs`, together with one test per guarantee its module
//! docs state: fill timing, the fee model, close-out, equity marking, the
//! strategy lifecycle, determinism, and the errors.

use quantforge::{
    BacktestConfig, BacktestEngine, BacktestResult, BuiltInStrategyConfig, Candle, ClosedTrade,
    EngineError, ExchangeId, Interval, MarketId, Strategy, StrategyContext, StrategyError, Symbol,
    TargetPosition, TimestampMs, validate_candles,
};
use rust_decimal::Decimal;
use std::str::FromStr;

#[derive(Debug)]
struct ScriptedStrategy {
    targets: Vec<(TimestampMs, TargetPosition)>,
}

impl Strategy for ScriptedStrategy {
    fn name(&self) -> &str {
        "scripted"
    }

    fn on_bar(
        &mut self,
        _ctx: &StrategyContext,
        bar: &Candle,
    ) -> Result<Option<TargetPosition>, StrategyError> {
        Ok(self
            .targets
            .iter()
            .find(|(ts, _)| *ts == bar.open_time_ms)
            .map(|(_, target)| *target))
    }
}

fn market() -> MarketId {
    MarketId::new(
        ExchangeId::BinanceSpot,
        Symbol::new("BTCUSDT").expect("symbol"),
        Interval::M1,
    )
}

fn candle(open_time_ms: i64, open: &str, close: &str) -> Candle {
    Candle {
        open_time_ms,
        close_time_ms: open_time_ms + 59_999,
        open: Decimal::from_str(open).expect("decimal"),
        high: Decimal::from_str(close).expect("decimal"),
        low: Decimal::from_str(open).expect("decimal"),
        close: Decimal::from_str(close).expect("decimal"),
        volume: Decimal::ONE,
        trades: Some(1),
    }
}

fn dec(value: &str) -> Decimal {
    Decimal::from_str(value).expect("decimal")
}

fn ohlc_candle(index: i64, open: &str, close: &str) -> Candle {
    let open = dec(open);
    let close = dec(close);
    let open_time_ms = index * 60_000;
    Candle {
        open_time_ms,
        close_time_ms: open_time_ms + 59_999,
        open,
        high: open.max(close),
        low: open.min(close),
        close,
        volume: Decimal::ONE,
        trades: Some(1),
    }
}

// Regression fixture for the full SMA-cross pipeline (fast=2, slow=3,
// cash=10010, fee=10 bps). Prices are chosen so every division behind the
// pinned values is exact in `Decimal` and each entry spends cash to
// exactly zero, keeping the pinned trades, equity, return, and drawdown
// hand-verifiable (the few rounded divisions only feed comparisons whose
// outcome is unambiguous):
// - closes rise: fast crosses above slow on bar 2, buy fills at bar 3
//   open 100 (10010 / (100 * 1.001) = qty 100), equity peaks at 11000 on
//   bar 5, then closes fall to 9900 on bar 7 (the 10% max drawdown) where
//   fast drops below slow;
// - the sell fills at bar 8 open 100.1 (cash 100 * 100.1 * 0.999 =
//   9999.99, a fee-driven losing trade);
// - bar 10 has fast == slow, which must produce no signal;
// - fast crosses back above slow on bar 11, the buy fills at bar 12 open
//   99.9 (9999.99 / (99.9 * 1.001) = qty 100), and the run ends long, so
//   close-out sells at the last close 110.11 (cash 10999.989).
fn sma_cross_fixture() -> Vec<Candle> {
    [
        ("96", "97"),
        ("97", "99"),
        ("99", "101"),
        ("100", "103"),
        ("103", "105"),
        ("105", "110"),
        ("110", "103"),
        ("103", "99"),
        ("100.1", "98"),
        ("98", "97"),
        ("97", "99"),
        ("99", "101"),
        ("99.9", "103"),
        ("103", "105"),
        ("105", "110.11"),
    ]
    .iter()
    .enumerate()
    .map(|(index, (open, close))| ohlc_candle(index as i64, open, close))
    .collect()
}

#[test]
fn backtest_executes_on_next_bar_open_and_records_trade() {
    let candles = vec![
        candle(0, "100", "100"),
        candle(60_000, "110", "110"),
        candle(120_000, "120", "120"),
    ];
    let mut strategy = ScriptedStrategy {
        targets: vec![
            (0, TargetPosition::LongAllIn),
            (60_000, TargetPosition::Flat),
        ],
    };

    let result = BacktestEngine::new(BacktestConfig {
        initial_cash: Decimal::from(1_000),
        fee_bps: Decimal::ZERO,
        close_out_at_end: true,
    })
    .run(&market(), &candles, &mut strategy)
    .expect("backtest");

    assert_eq!(result.trade_count, 1);
    assert_eq!(result.trades[0].entry_price, Decimal::from(110));
    assert_eq!(result.trades[0].exit_price, Decimal::from(120));
    assert!(result.final_equity > Decimal::from(1_000));
    assert!(result.trades[0].gross_quote_pnl > Decimal::ZERO);
}

#[test]
fn backtest_rejects_non_positive_initial_cash() {
    let candles = vec![candle(0, "100", "100")];
    let mut strategy = ScriptedStrategy { targets: vec![] };

    for cash in [Decimal::ZERO, Decimal::from(-100)] {
        let error = BacktestEngine::new(BacktestConfig {
            initial_cash: cash,
            fee_bps: Decimal::ZERO,
            close_out_at_end: true,
        })
        .run(&market(), &candles, &mut strategy)
        .expect_err("config error");
        assert!(
            matches!(error, EngineError::InvalidConfig(_)),
            "for cash {cash}"
        );
        assert!(
            error.to_string().contains("initial_cash must be greater"),
            "for cash {cash}, got {error}"
        );
    }
}

#[test]
fn backtest_rejects_negative_fee_bps() {
    let candles = vec![candle(0, "100", "100")];
    let mut strategy = ScriptedStrategy { targets: vec![] };

    let error = BacktestEngine::new(BacktestConfig {
        initial_cash: Decimal::from(1_000),
        fee_bps: Decimal::from(-1),
        close_out_at_end: true,
    })
    .run(&market(), &candles, &mut strategy)
    .expect_err("config error");
    assert!(matches!(error, EngineError::InvalidConfig(_)));
    assert!(
        error
            .to_string()
            .contains("fee_bps must be zero or greater")
    );
}

#[test]
fn backtest_sma_cross_on_fixed_fixture_reproduces_pinned_result() {
    let market = market();
    let candles = sma_cross_fixture();
    let report = validate_candles(&market, &candles);
    assert!(
        report.is_ok(),
        "fixture must stay a valid candle series: {:?}",
        report.issues
    );

    let run = || {
        let mut strategy = BuiltInStrategyConfig::SmaCross { fast: 2, slow: 3 }
            .build()
            .expect("strategy");
        BacktestEngine::new(BacktestConfig {
            initial_cash: dec("10010"),
            fee_bps: dec("10"),
            close_out_at_end: true,
        })
        .run(&market, &candles, strategy.as_mut())
        .expect("backtest")
    };

    let result = run();
    let expected = BacktestResult {
        initial_cash: dec("10010"),
        final_equity: dec("10999.989"),
        total_return_pct: dec("9.89"),
        trade_count: 2,
        max_drawdown_pct: dec("10"),
        trades: vec![
            ClosedTrade {
                symbol: Symbol::new("BTCUSDT").expect("symbol"),
                entry_time_ms: 180_000,
                exit_time_ms: 480_000,
                entry_price: dec("100"),
                exit_price: dec("100.1"),
                qty: dec("100"),
                gross_quote_pnl: dec("-10.01"),
                entry_order_id: None,
                exit_order_id: None,
            },
            ClosedTrade {
                symbol: Symbol::new("BTCUSDT").expect("symbol"),
                entry_time_ms: 720_000,
                exit_time_ms: 899_999,
                entry_price: dec("99.9"),
                exit_price: dec("110.11"),
                qty: dec("100"),
                gross_quote_pnl: dec("999.999"),
                entry_order_id: None,
                exit_order_id: None,
            },
        ],
    };
    assert_eq!(result, expected);

    // A fresh strategy over the same fixture must reproduce the result
    // bit for bit: the backtest is a pure function of its inputs.
    assert_eq!(run(), result);
}

// ---------------------------------------------------------------------------
// One test per guarantee in the `engine::backtest` module docs.
// ---------------------------------------------------------------------------

/// What a callback saw: its label and the context's time, cash, and
/// position.
type Snapshot = (String, TimestampMs, Decimal, Decimal);

/// Plays back one scripted decision per bar, records every callback with
/// the context it received, and can fail on a chosen bar.
#[derive(Debug, Default)]
struct Recorder {
    decisions: Vec<Option<TargetPosition>>,
    fail_on_bar: Option<usize>,
    bars: usize,
    seen: Vec<Snapshot>,
}

impl Recorder {
    fn deciding(decisions: Vec<Option<TargetPosition>>) -> Self {
        Self {
            decisions,
            ..Self::default()
        }
    }

    fn record(&mut self, label: String, ctx: &StrategyContext) {
        self.seen
            .push((label, ctx.now_ms, ctx.cash, ctx.position_qty));
    }

    fn labels(&self) -> Vec<&str> {
        self.seen.iter().map(|(label, ..)| label.as_str()).collect()
    }
}

impl Strategy for Recorder {
    fn name(&self) -> &str {
        "recorder"
    }

    fn on_start(&mut self, ctx: &StrategyContext) -> Result<(), StrategyError> {
        self.record("start".to_string(), ctx);
        Ok(())
    }

    fn on_bar(
        &mut self,
        ctx: &StrategyContext,
        _bar: &Candle,
    ) -> Result<Option<TargetPosition>, StrategyError> {
        let index = self.bars;
        self.bars += 1;
        self.record(format!("bar {index}"), ctx);
        if self.fail_on_bar == Some(index) {
            return Err(StrategyError::msg(format!("refused bar {index}")));
        }
        Ok(self.decisions.get(index).copied().flatten())
    }

    fn on_finish(&mut self, ctx: &StrategyContext) -> Result<(), StrategyError> {
        self.record("finish".to_string(), ctx);
        Ok(())
    }
}

fn engine(initial_cash: &str, fee_bps: &str, close_out_at_end: bool) -> BacktestEngine {
    BacktestEngine::new(BacktestConfig {
        initial_cash: dec(initial_cash),
        fee_bps: dec(fee_bps),
        close_out_at_end,
    })
}

fn snapshot(label: &str, now_ms: TimestampMs, cash: &str, position_qty: &str) -> Snapshot {
    (label.to_string(), now_ms, dec(cash), dec(position_qty))
}

#[test]
fn a_decision_on_the_last_bar_never_fills() {
    let candles = vec![ohlc_candle(0, "100", "100"), ohlc_candle(1, "100", "110")];
    for close_out_at_end in [true, false] {
        let mut strategy = Recorder::deciding(vec![None, Some(TargetPosition::LongAllIn)]);
        let result = engine("1000", "0", close_out_at_end)
            .run(&market(), &candles, &mut strategy)
            .expect("backtest");
        assert_eq!(result.trade_count, 0, "close_out_at_end {close_out_at_end}");
        assert_eq!(
            result.final_equity,
            dec("1000"),
            "close_out_at_end {close_out_at_end}"
        );
    }
}

// cash 10010 at 10 bps: the entry at 100 buys 10010 / (100 * 1.001) = 100
// and spends 10000 + a 10 fee; the exit at 110 receives 11000 - an 11 fee.
#[test]
fn fees_charge_basis_points_of_notional_on_both_fills() {
    let candles = vec![
        ohlc_candle(0, "100", "100"),
        ohlc_candle(1, "100", "105"),
        ohlc_candle(2, "105", "108"),
        ohlc_candle(3, "110", "110"),
    ];
    let mut strategy = Recorder::deciding(vec![
        Some(TargetPosition::LongAllIn),
        None,
        Some(TargetPosition::Flat),
    ]);
    let result = engine("10010", "10", true)
        .run(&market(), &candles, &mut strategy)
        .expect("backtest");

    assert_eq!(result.trade_count, 1);
    let trade = &result.trades[0];
    assert_eq!(
        (trade.entry_time_ms, trade.entry_price, trade.qty),
        (60_000, dec("100"), dec("100"))
    );
    assert_eq!(
        (trade.exit_time_ms, trade.exit_price),
        (180_000, dec("110"))
    );
    assert_eq!(trade.gross_quote_pnl, dec("979"), "net of both fees");
    assert_eq!(result.final_equity, dec("10989"));
}

#[test]
fn close_out_sells_at_the_last_close_with_its_fee() {
    let candles = vec![
        ohlc_candle(0, "100", "100"),
        ohlc_candle(1, "100", "105"),
        ohlc_candle(2, "105", "120"),
    ];
    let decisions = vec![Some(TargetPosition::LongAllIn)];

    let mut strategy = Recorder::deciding(decisions.clone());
    let closed = engine("10010", "10", true)
        .run(&market(), &candles, &mut strategy)
        .expect("backtest");
    assert_eq!(closed.trade_count, 1);
    let trade = &closed.trades[0];
    assert_eq!(
        (trade.exit_time_ms, trade.exit_price),
        (179_999, dec("120")),
        "the last bar's close, at its close time"
    );
    // 100 * 120 = 12000, less a 12 fee.
    assert_eq!(closed.final_equity, dec("11988"));
    assert_eq!(trade.gross_quote_pnl, dec("1978"));

    let mut strategy = Recorder::deciding(decisions);
    let held = engine("10010", "10", false)
        .run(&market(), &candles, &mut strategy)
        .expect("backtest");
    assert_eq!(held.trade_count, 0);
    assert_eq!(held.final_equity, dec("12000"), "marked at the last close");
}

// Bar 1 opens with the entry decided on bar 0 already filled; bar 2 opens
// with the exit decided on bar 1 already filled.
fn lifecycle_run() -> Recorder {
    let candles = vec![
        ohlc_candle(0, "100", "100"),
        ohlc_candle(1, "100", "110"),
        ohlc_candle(2, "120", "120"),
    ];
    let mut strategy = Recorder::deciding(vec![
        Some(TargetPosition::LongAllIn),
        Some(TargetPosition::Flat),
    ]);
    engine("1000", "0", true)
        .run(&market(), &candles, &mut strategy)
        .expect("backtest");
    strategy
}

#[test]
fn the_strategy_sees_the_bar_open_time_and_the_state_after_that_bars_fill() {
    let strategy = lifecycle_run();
    let bars: Vec<&Snapshot> = strategy
        .seen
        .iter()
        .filter(|(label, ..)| label.starts_with("bar"))
        .collect();
    assert_eq!(
        bars,
        [
            &snapshot("bar 0", 0, "1000", "0"),
            &snapshot("bar 1", 60_000, "0", "10"),
            &snapshot("bar 2", 120_000, "1200", "0"),
        ]
    );
}

#[test]
fn callbacks_run_on_start_then_every_bar_then_on_finish() {
    let strategy = lifecycle_run();
    assert_eq!(
        strategy.labels(),
        ["start", "bar 0", "bar 1", "bar 2", "finish"]
    );
    assert_eq!(strategy.seen[0], snapshot("start", 0, "1000", "0"));
    assert_eq!(
        strategy.seen[4],
        snapshot("finish", 120_000, "1200", "0"),
        "the last bar's open time, the final cash and position"
    );
}

#[test]
fn a_strategy_error_ends_the_run_without_on_finish() {
    let candles = vec![
        ohlc_candle(0, "100", "100"),
        ohlc_candle(1, "100", "100"),
        ohlc_candle(2, "100", "100"),
    ];
    let mut strategy = Recorder {
        fail_on_bar: Some(1),
        ..Recorder::default()
    };
    let error = engine("1000", "0", true)
        .run(&market(), &candles, &mut strategy)
        .expect_err("strategy error");

    assert!(matches!(error, EngineError::Strategy(_)), "got {error:?}");
    assert_eq!(error.to_string(), "refused bar 1");
    assert_eq!(strategy.labels(), ["start", "bar 0", "bar 1"]);
}

#[test]
fn requesting_the_held_position_does_nothing() {
    let candles = vec![
        ohlc_candle(0, "100", "100"),
        ohlc_candle(1, "100", "100"),
        ohlc_candle(2, "110", "110"),
        ohlc_candle(3, "120", "120"),
        ohlc_candle(4, "130", "130"),
    ];
    // Flat while flat, then a long that is requested twice, then the exit.
    let mut strategy = Recorder::deciding(vec![
        Some(TargetPosition::Flat),
        Some(TargetPosition::LongAllIn),
        Some(TargetPosition::LongAllIn),
        Some(TargetPosition::Flat),
    ]);
    let result = engine("1000", "0", true)
        .run(&market(), &candles, &mut strategy)
        .expect("backtest");

    assert_eq!(result.trade_count, 1);
    let trade = &result.trades[0];
    assert_eq!(
        (trade.entry_time_ms, trade.entry_price),
        (120_000, dec("110"))
    );
    assert_eq!(
        (trade.exit_time_ms, trade.exit_price),
        (240_000, dec("130"))
    );
}

// Entry at 100 with 1000 cash: equity 1000, then 1200 at close 120, 900 at
// close 90 (a 25% fall from the 1200 peak), then 1500. Each open differs
// from the close before it, so marking at the opens would find a 13.6% fall
// (1100 to 950) instead.
#[test]
fn max_drawdown_follows_close_marked_equity() {
    let candles = vec![
        ohlc_candle(0, "100", "100"),
        ohlc_candle(1, "100", "120"),
        ohlc_candle(2, "110", "90"),
        ohlc_candle(3, "95", "150"),
    ];
    let mut strategy = Recorder::deciding(vec![Some(TargetPosition::LongAllIn)]);
    let result = engine("1000", "0", false)
        .run(&market(), &candles, &mut strategy)
        .expect("backtest");

    assert_eq!(result.max_drawdown_pct, dec("25"));
    assert_eq!(result.final_equity, dec("1500"));
    assert_eq!(result.total_return_pct, dec("50"));
}

// 1000 / 1e-28 is about 1e31, beyond the decimal maximum of about 7.9e28.
#[test]
fn an_overflowing_entry_is_an_error_not_a_panic() {
    let tiny = "0.0000000000000000000000000001";
    let candles = vec![ohlc_candle(0, "1", "1"), ohlc_candle(1, tiny, tiny)];
    let mut strategy = Recorder::deciding(vec![Some(TargetPosition::LongAllIn)]);
    let error = engine("1000", "0", true)
        .run(&market(), &candles, &mut strategy)
        .expect_err("overflow");

    assert!(
        matches!(error, EngineError::Overflow("the entry")),
        "got {error:?}"
    );
    assert_eq!(error.to_string(), "decimal overflow computing the entry");
}

// 1e20 cash buys 1e28 at 1e-8, which fits; marking it at a close of 100
// gives 1e30, which does not.
#[test]
fn an_overflowing_equity_mark_is_an_error_not_a_panic() {
    let candles = vec![
        ohlc_candle(0, "1", "1"),
        ohlc_candle(1, "0.00000001", "100"),
    ];
    let mut strategy = Recorder::deciding(vec![Some(TargetPosition::LongAllIn)]);
    let error = engine("100000000000000000000", "0", true)
        .run(&market(), &candles, &mut strategy)
        .expect_err("overflow");

    assert!(
        matches!(error, EngineError::Overflow("the equity mark")),
        "got {error:?}"
    );
}

// `==` on decimals compares values; the Debug form also shows each
// decimal's scale, so equal strings mean bit-identical results.
#[test]
fn the_same_inputs_give_bit_identical_results() {
    let candles = sma_cross_fixture();
    let run = || {
        let mut strategy = BuiltInStrategyConfig::SmaCross { fast: 2, slow: 3 }
            .build()
            .expect("strategy");
        engine("10000", "7", true)
            .run(&market(), &candles, strategy.as_mut())
            .expect("backtest")
    };

    let first = run();
    assert!(first.trade_count > 0, "the fixture trades");
    assert_eq!(format!("{first:?}"), format!("{:?}", run()));
}

#[test]
fn no_candles_is_an_error() {
    let mut strategy = Recorder::default();
    let error = engine("1000", "0", true)
        .run(&market(), &[], &mut strategy)
        .expect_err("no candles");
    assert!(matches!(error, EngineError::NoCandles), "got {error:?}");
    assert!(strategy.seen.is_empty(), "no callback ran");
}
