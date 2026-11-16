//! The oracle: the built-in Rust `SmaCrossStrategy`, whose run over the
//! frozen candles wrote `sma_cross.json`, once.
//!
//! While the built-in strategy exists, this module shows on every run that
//! the file is that strategy's output and nothing else: the document made
//! here, from the strategy and the engine as they are now, is the file byte
//! for byte.
//!
//! **This module is deleted with the built-in strategies** (QUA-459), and
//! the file is not: from then on the goldens stand on their own, held by
//! the engine's replay of their decisions (`main.rs`) and by the strategy
//! written in Python (`quantforge-py/tests/test_golden_parity.py`).

use super::{
    CANDLES, ConfigText, Dataset, Document, EXPECTED, KEEP, MarketText, Oracle, Params, ResultText,
    StrategyText, candles, dataset, decided_by, expected, first_difference, lead, read, sha256,
};
use quantforge::{
    BacktestConfig, BacktestEngine, BuiltInStrategyConfig, Candle, ExchangeId, Interval, MarketId,
    SmaCrossStrategy, Strategy, StrategyContext, StrategyError, Symbol, TargetPosition,
    ms_to_rfc3339,
};
use rust_decimal::Decimal;

/// What the file says of itself.
const ABOUT: &str = "The frozen results of QuantForge's built-in Rust SMA cross over candles.csv: \
                     the decision on every bar, every trade, and the equity at every bar, each \
                     amount as the text of its decimal. Written once and never regenerated: \
                     docs/golden-parity.md.";

/// The parameters of the run: the `quantforge backtest` command's defaults.
const PARAMS: Params = Params { fast: 20, slow: 50 };

/// What ran, as the file records it. These are the facts of the day the
/// file was written, and are not read from the build: the engine's version
/// moves on, and the file does not.
fn oracle() -> Oracle {
    Oracle {
        strategy: "quantforge::SmaCrossStrategy".to_string(),
        engine: "quantforge 0.2.0".to_string(),
        commit: "1d38db875ce55953a361af55cd3db076aedcbeb5".to_string(),
        decimal: "rust_decimal 1.40.0".to_string(),
        written_on: "2026-11-16".to_string(),
    }
}

fn market() -> MarketId {
    MarketId::new(
        ExchangeId::BinanceSpot,
        Symbol::new("BTCUSDT").expect("symbol"),
        Interval::M1,
    )
}

fn config() -> BacktestConfig {
    BacktestConfig {
        initial_cash: Decimal::from(10_000),
        fee_bps: Decimal::from(10),
        close_out_at_end: true,
    }
}

/// A strategy with every decision it makes written down.
struct Recorded {
    strategy: Box<dyn Strategy>,
    decisions: Vec<Option<TargetPosition>>,
}

impl Strategy for Recorded {
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
        let decision = self.strategy.on_bar(ctx, bar)?;
        self.decisions.push(decision);
        Ok(decision)
    }

    fn on_finish(&mut self, ctx: &StrategyContext) -> Result<(), StrategyError> {
        self.strategy.on_finish(ctx)
    }
}

/// The document the oracle makes of the frozen candles.
fn document() -> Document {
    let candles = candles();
    let (market, config) = (market(), config());
    let mut strategy = Recorded {
        strategy: BuiltInStrategyConfig::SmaCross {
            fast: PARAMS.fast,
            slow: PARAMS.slow,
        }
        .build()
        .expect("strategy"),
        decisions: Vec::new(),
    };
    let result = BacktestEngine::new(config.clone())
        .run(&market, &candles, &mut strategy)
        .expect("backtest");
    let (first, last) = (
        candles.first().expect("a first candle"),
        candles.last().expect("a last candle"),
    );
    Document {
        format: 1,
        about: ABOUT.to_string(),
        oracle: oracle(),
        dataset: Dataset {
            file: "candles.csv".to_string(),
            sha256: sha256(&read(CANDLES)),
            market: MarketText {
                exchange: market.exchange.as_str().to_string(),
                symbol: market.symbol.as_str().to_string(),
                interval: market.interval.as_str().to_string(),
            },
            candles: candles.len(),
            first_open_ms: first.open_time_ms,
            last_open_ms: last.open_time_ms,
            first_open: ms_to_rfc3339(first.open_time_ms),
            last_open: ms_to_rfc3339(last.open_time_ms),
        },
        strategy: StrategyText {
            name: strategy.name().to_string(),
            params: PARAMS,
        },
        config: ConfigText {
            initial_cash: config.initial_cash.to_string(),
            fee_bps: config.fee_bps.to_string(),
            close_out_at_end: config.close_out_at_end,
        },
        result: ResultText::of(&result),
        decisions: strategy.decisions,
    }
}

#[test]
fn the_rust_sma_cross_makes_the_frozen_results() {
    let mut computed = serde_json::to_string_pretty(&document()).expect("json");
    computed.push('\n');

    let committed = read(EXPECTED);
    assert!(
        computed == committed,
        "the Rust SMA cross no longer makes {EXPECTED}, first at line {}: the goldens are frozen, \
         so the change is in the strategy or the engine (docs/golden-parity.md)",
        first_difference(&committed, &computed)
    );
}

// The run is the engine's own of the built-in strategy, as the command line
// makes it, and the candles are the generator's: nothing else has a part in
// the file.
#[test]
fn the_oracle_is_the_built_in_strategy_over_the_generated_candles() {
    assert_eq!(candles(), dataset::generated());
    let document = document();
    assert_eq!(document.strategy.name, "sma_cross");
    assert_eq!(document.dataset.market.market(), market());
    assert_eq!(document.config.config(), config());
    // A run of it gives the same document every time.
    assert_eq!(document, self::document());
}

/// What comparing the two averages decides on every bar in exact
/// arithmetic, where no division is rounded.
fn exactly(candles: &[Candle], fast: usize, slow: usize) -> Vec<Option<TargetPosition>> {
    (0..candles.len())
        .map(|bar| {
            if bar + 1 < slow {
                return KEEP;
            }
            decided_by(lead(candles, bar, fast, slow))
        })
        .collect()
}

/// Whether every average over `window` of the closes has a decimal form of
/// its own, which no division has to round: the window, with its twos and
/// fives taken out, divides every sum.
fn divides_evenly(candles: &[Candle], window: usize) -> bool {
    let mut rest = window as i128;
    for factor in [2, 5] {
        while rest % factor == 0 {
            rest /= factor;
        }
    }
    candles.windows(window).all(|held| {
        let sum: i128 = held.iter().map(|candle| candle.close.mantissa()).sum();
        sum % rest == 0
    })
}

// The strategy compares two quotients the engine's decimal has rounded, to
// the 28 or 29 digits it holds. Two averages of prices that differ at all
// differ by far more than that rounding, so the comparison comes out as
// exact arithmetic's does. The windows of the goldens divide every sum
// evenly, so the other two pairs are here for the rounding: thirds,
// sevenths and thirteenths. The Python package's tests show the same of the
// strategy written in Python, over the same windows.
#[test]
fn the_rust_strategy_compares_its_averages_as_exact_arithmetic_would() {
    let candles = candles();
    let ctx = StrategyContext {
        market: market(),
        now_ms: 0,
        cash: Decimal::ZERO,
        position_qty: Decimal::ZERO,
    };
    for (fast, slow, even) in [(20, 50, true), (7, 30, false), (3, 13, false)] {
        let mut strategy = SmaCrossStrategy::new(fast, slow).expect("strategy");
        let decided: Vec<Option<TargetPosition>> = candles
            .iter()
            .map(|bar| strategy.on_bar(&ctx, bar).expect("bar"))
            .collect();
        assert_eq!(decided, exactly(&candles, fast, slow), "{fast} and {slow}");
        assert_eq!(
            divides_evenly(&candles, fast) && divides_evenly(&candles, slow),
            even,
            "{fast} and {slow}"
        );
    }
    // At the goldens' own windows, those are the decisions the file records.
    assert_eq!(
        exactly(&candles, PARAMS.fast, PARAMS.slow),
        expected().decisions
    );
}
