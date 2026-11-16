//! The goldens: one frozen day of candles, and what the Rust SMA cross made
//! of it, digit for digit. `docs/golden-parity.md` is the contract.
//!
//! - `tests/fixtures/golden/candles.csv` is the dataset: twenty-four hours
//!   of one-minute candles of `binance_spot` `BTCUSDT`, written as the
//!   exchange writes them.
//! - `tests/fixtures/golden/sma_cross.json` is the engine's run of the
//!   built-in `SmaCrossStrategy` over them, at 20 and 50 bars with 10,000 in
//!   cash and a fee of 10 basis points: the decision on every bar, every
//!   trade, the equity at every bar, the final equity, the return and the
//!   drawdown, each amount as the text of its decimal.
//! - `tests/fixtures/golden/sma_cross.py` is the same strategy written in
//!   Python, which the Python package's tests hold to those results through
//!   `run_backtest` (`quantforge-py/tests/test_golden_parity.py`).
//!
//! **The two data files are frozen.** They were written once, from the Rust
//! strategy, and nothing writes them again: no command and no variable
//! refreshes them, `UPDATE_SNAPSHOTS` included. A test that fails here says
//! that the engine, or what crosses its strategy boundary, now computes
//! something else, which is the regression the files exist to catch. The fix
//! is in the code, never in the files, and [`EXPECTED_SHA256`] holds a
//! change to them up where a review sees it.
//!
//! The modules:
//!
//! - this one reads the files and holds the engine to them: the recorded
//!   decisions, played back through `BacktestEngine`, give the recorded
//!   result. It needs no strategy at all, and so outlives the built-in one;
//! - [`dataset`] is how the candles were made, and what they were made to
//!   hold;
//! - [`oracle`] is the Rust SMA cross itself, which has to give the file
//!   byte for byte. **It goes when the built-in strategies go** (QUA-459),
//!   and the files stay;
//! - [`docs`] holds the page to the files.

mod dataset;
mod docs;
mod oracle;

use quantforge::{
    BacktestConfig, BacktestEngine, BacktestResult, Candle, Category, ExchangeId, Interval,
    MarketId, Strategy, StrategyContext, StrategyError, Symbol, TargetPosition, TimestampMs,
    ms_to_rfc3339, validate_candles,
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::path::PathBuf;
use std::str::FromStr;

/// The dataset, the Rust strategy's results over it, and the same strategy
/// in Python, from the repository's root.
const CANDLES: &str = "tests/fixtures/golden/candles.csv";
const EXPECTED: &str = "tests/fixtures/golden/sma_cross.json";
const STRATEGY: &str = "tests/fixtures/golden/sma_cross.py";

/// The SHA-256 of `sma_cross.json` as it was frozen, which names the
/// candles' own in turn: neither file changes without this line changing.
/// **It is never changed to make a test pass.** `docs/golden-parity.md` says
/// what a change to the goldens takes.
const EXPECTED_SHA256: &str = "ea2945bc520657a4a4444e47b573151b3e53eaa3afc346096f657f7002266978";

/// The first line of the dataset: a candle's fields, in the engine's names.
const COLUMNS: &str = "open_time_ms,close_time_ms,open,high,low,close,volume,trades";

const LONG: Option<TargetPosition> = Some(TargetPosition::LongAllIn);
const FLAT: Option<TargetPosition> = Some(TargetPosition::Flat);
const KEEP: Option<TargetPosition> = None;

/// `sma_cross.json`, field for field in the file's order.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    format: u32,
    /// What the file is, for whoever opens it.
    about: String,
    oracle: Oracle,
    dataset: Dataset,
    strategy: StrategyText,
    config: ConfigText,
    result: ResultText,
    /// What the strategy decided on each bar, in the engine's names: `null`
    /// where it decided nothing.
    decisions: Vec<Option<TargetPosition>>,
}

/// What wrote the results, once.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Oracle {
    strategy: String,
    engine: String,
    /// The commit whose engine and strategy ran.
    commit: String,
    decimal: String,
    written_on: String,
}

/// The candles the results are of.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Dataset {
    file: String,
    sha256: String,
    market: MarketText,
    candles: usize,
    first_open_ms: TimestampMs,
    last_open_ms: TimestampMs,
    /// The two times above, as a person reads them.
    first_open: String,
    last_open: String,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MarketText {
    exchange: String,
    symbol: String,
    interval: String,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrategyText {
    name: String,
    params: Params,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Params {
    fast: usize,
    slow: usize,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigText {
    initial_cash: String,
    fee_bps: String,
    close_out_at_end: bool,
}

/// A result as `tests/fixtures/strategy_cases.json` writes one: every
/// amount as the text of its decimal.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResultText {
    initial_cash: String,
    final_equity: String,
    total_return_pct: String,
    trade_count: usize,
    max_drawdown_pct: String,
    trades: Vec<TradeText>,
    equity_curve: Vec<PointText>,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TradeText {
    symbol: String,
    entry_time_ms: TimestampMs,
    exit_time_ms: TimestampMs,
    entry_price: String,
    exit_price: String,
    qty: String,
    gross_quote_pnl: String,
    entry_order_id: Option<String>,
    exit_order_id: Option<String>,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PointText {
    open_time_ms: TimestampMs,
    equity: String,
}

impl ResultText {
    fn of(result: &BacktestResult) -> Self {
        Self {
            initial_cash: result.initial_cash.to_string(),
            final_equity: result.final_equity.to_string(),
            total_return_pct: result.total_return_pct.to_string(),
            trade_count: result.trade_count,
            max_drawdown_pct: result.max_drawdown_pct.to_string(),
            trades: result
                .trades
                .iter()
                .map(|trade| TradeText {
                    symbol: trade.symbol.as_str().to_string(),
                    entry_time_ms: trade.entry_time_ms,
                    exit_time_ms: trade.exit_time_ms,
                    entry_price: trade.entry_price.to_string(),
                    exit_price: trade.exit_price.to_string(),
                    qty: trade.qty.to_string(),
                    gross_quote_pnl: trade.gross_quote_pnl.to_string(),
                    entry_order_id: trade.entry_order_id.as_ref().map(ToString::to_string),
                    exit_order_id: trade.exit_order_id.as_ref().map(ToString::to_string),
                })
                .collect(),
            equity_curve: result
                .equity_curve
                .iter()
                .map(|point| PointText {
                    open_time_ms: point.open_time_ms,
                    equity: point.equity.to_string(),
                })
                .collect(),
        }
    }
}

impl MarketText {
    fn market(&self) -> MarketId {
        MarketId::new(
            ExchangeId::from_str(&self.exchange).expect("exchange"),
            Symbol::new(&self.symbol).expect("symbol"),
            Interval::from_str(&self.interval).expect("interval"),
        )
    }
}

impl ConfigText {
    fn config(&self) -> BacktestConfig {
        BacktestConfig {
            initial_cash: dec(&self.initial_cash),
            fee_bps: dec(&self.fee_bps),
            close_out_at_end: self.close_out_at_end,
        }
    }
}

/// A decimal from its text, with every digit and every decimal place.
fn dec(text: &str) -> Decimal {
    Decimal::from_str_exact(text).unwrap_or_else(|error| panic!("{text:?} is no decimal: {error}"))
}

/// A file of the repository, as text.
fn read(path: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(path);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()))
}

fn sha256(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

/// The candles of a file in the dataset's layout: [`COLUMNS`], then a candle
/// a line. A trade count that was not reported is an empty field.
fn candles_of(text: &str) -> Vec<Candle> {
    let mut lines = text.lines();
    assert_eq!(
        lines.next(),
        Some(COLUMNS),
        "the first line names the columns"
    );
    lines
        .zip(2..)
        .map(|(line, number)| {
            let fields: Vec<&str> = line.split(',').collect();
            let [
                open_time_ms,
                close_time_ms,
                open,
                high,
                low,
                close,
                volume,
                trades,
            ] = fields[..]
            else {
                panic!("line {number} has {} fields, not 8", fields.len());
            };
            let time = |text: &str| -> TimestampMs {
                text.parse()
                    .unwrap_or_else(|error| panic!("line {number}: {text:?} is no time: {error}"))
            };
            Candle {
                open_time_ms: time(open_time_ms),
                close_time_ms: time(close_time_ms),
                open: dec(open),
                high: dec(high),
                low: dec(low),
                close: dec(close),
                volume: dec(volume),
                trades: (!trades.is_empty()).then(|| {
                    trades.parse().unwrap_or_else(|error| {
                        panic!("line {number}: {trades:?} is no count: {error}")
                    })
                }),
            }
        })
        .collect()
}

/// The dataset as its file writes it: the columns, then a candle a line,
/// each decimal with the digits it has.
fn csv(candles: &[Candle]) -> String {
    let mut text = format!("{COLUMNS}\n");
    for candle in candles {
        let trades = candle.trades.map(|count| count.to_string());
        text.push_str(&format!(
            "{},{},{},{},{},{},{},{}\n",
            candle.open_time_ms,
            candle.close_time_ms,
            candle.open,
            candle.high,
            candle.low,
            candle.close,
            candle.volume,
            trades.unwrap_or_default(),
        ));
    }
    text
}

/// The frozen candles.
fn candles() -> Vec<Candle> {
    candles_of(&read(CANDLES))
}

/// The frozen results.
fn expected() -> Document {
    serde_json::from_str(&read(EXPECTED))
        .unwrap_or_else(|error| panic!("{EXPECTED} is not the document: {error}"))
}

/// Plays the recorded decisions back, one a bar: a strategy with no rule of
/// its own, so that a run of it is the engine's work alone.
#[derive(Debug)]
struct Replayed<'a> {
    decisions: &'a [Option<TargetPosition>],
    bars: usize,
}

impl Strategy for Replayed<'_> {
    fn name(&self) -> &str {
        "replayed"
    }

    fn on_bar(
        &mut self,
        _ctx: &StrategyContext,
        _bar: &Candle,
    ) -> Result<Option<TargetPosition>, StrategyError> {
        let decision = self
            .decisions
            .get(self.bars)
            .copied()
            .ok_or_else(|| StrategyError::msg(format!("no decision for bar {}", self.bars)))?;
        self.bars += 1;
        Ok(decision)
    }
}

/// The engine's run of the recorded decisions over the frozen candles, on
/// `market`.
fn replayed(document: &Document, market: &MarketId) -> BacktestResult {
    let candles = candles();
    let mut strategy = Replayed {
        decisions: &document.decisions,
        bars: 0,
    };
    let result = BacktestEngine::new(document.config.config())
        .run(market, &candles, &mut strategy)
        .expect("backtest");
    assert_eq!(strategy.bars, candles.len(), "a decision for every bar");
    result
}

/// Where two texts first differ, for a failure that has to name a line of a
/// long file.
fn first_difference(committed: &str, computed: &str) -> usize {
    committed
        .lines()
        .zip(computed.lines())
        .position(|(old, new)| old != new)
        .unwrap_or_else(|| committed.lines().count().min(computed.lines().count()))
        + 1
}

#[test]
fn the_files_are_the_ones_that_were_frozen() {
    let text = read(EXPECTED);
    assert_eq!(
        sha256(&text),
        EXPECTED_SHA256,
        "{EXPECTED} is not the file that was frozen: the goldens are never edited or \
         regenerated (docs/golden-parity.md)"
    );
    let document = expected();
    assert_eq!(document.format, 1);
    assert_eq!(
        sha256(&read(CANDLES)),
        document.dataset.sha256,
        "{CANDLES} is not the dataset the results are of"
    );
    assert_eq!(
        format!("tests/fixtures/golden/{}", document.dataset.file),
        CANDLES
    );
    // Byte for byte on every platform: the files are checked out with the
    // line ends they were written with (.gitattributes).
    for path in [CANDLES, EXPECTED, STRATEGY] {
        let text = read(path);
        assert!(!text.contains('\r'), "{path} has a carriage return");
        assert!(text.ends_with('\n'), "{path} ends with a line end");
    }
}

#[test]
fn the_dataset_is_read_as_it_is_written() {
    let text = read(CANDLES);
    let candles = candles_of(&text);
    // Nothing is lost on the way in: every digit and every decimal place of
    // the file is in the candles.
    assert_eq!(csv(&candles), text);

    let document = expected();
    let dataset = &document.dataset;
    assert_eq!(candles.len(), dataset.candles);
    let (first, last) = (
        candles.first().expect("a first candle"),
        candles.last().expect("a last candle"),
    );
    assert_eq!(first.open_time_ms, dataset.first_open_ms);
    assert_eq!(last.open_time_ms, dataset.last_open_ms);
    assert_eq!(ms_to_rfc3339(dataset.first_open_ms), dataset.first_open);
    assert_eq!(ms_to_rfc3339(dataset.last_open_ms), dataset.last_open);

    // A valid series of its market: no gap, no duplicate, no impossible bar.
    let market = dataset.market.market();
    assert_eq!(market.category(), Category::Spot);
    let report = validate_candles(&market, &candles);
    assert!(report.is_ok(), "{:?}", report.issues);
}

// The run that needs no strategy: the recorded decisions, played back, give
// the recorded result. It holds the engine itself to the goldens, on every
// platform the crate is tested on, and stays when the built-in strategy is
// gone.
#[test]
fn the_engine_makes_the_recorded_result_of_the_recorded_decisions() {
    let document = expected();
    let market = document.dataset.market.market();
    let result = ResultText::of(&replayed(&document, &market));
    let (computed, committed) = (
        serde_json::to_string_pretty(&result).expect("json"),
        serde_json::to_string_pretty(&document.result).expect("json"),
    );
    assert!(
        computed == committed,
        "the engine's result differs from {EXPECTED}, first at line {} of the result; the goldens \
         are frozen, so the change is in the engine (docs/golden-parity.md)",
        first_difference(&committed, &computed)
    );
    // The three figures the goldens are named for, each as its text.
    assert_eq!(result.final_equity, document.result.final_equity);
    assert_eq!(result.max_drawdown_pct, document.result.max_drawdown_pct);
    assert_eq!(result.trades, document.result.trades);
    assert_eq!(document.result.trade_count, document.result.trades.len());
    assert_eq!(document.decisions.len(), document.dataset.candles);
    assert_eq!(document.result.equity_curve.len(), document.dataset.candles);
}

// A result depends on the candles and on nothing else of the market but its
// symbol, which every trade names: the same candles kept under another
// venue's Spot line give the same figures.
#[test]
fn the_result_is_the_candles_whichever_spot_line_they_are_kept_under() {
    let document = expected();
    let golden = document.dataset.market.market();
    let lines: Vec<ExchangeId> = ExchangeId::all()
        .filter(|line| line.category() == Category::Spot)
        .collect();
    assert!(lines.len() >= 2 && lines.contains(&golden.exchange));
    for line in lines {
        let market = MarketId::new(line, golden.symbol.clone(), golden.interval);
        let result = ResultText::of(&replayed(&document, &market));
        assert_eq!(result, document.result, "{}", line.as_str());
    }
}

/// How far the fast average of the closes stands above the slow one at
/// `bar`, in exact arithmetic: the two sums, each multiplied by the other's
/// window, so that nothing is divided and nothing rounded. It is that
/// difference times the two windows, in the closes' last decimal place,
/// which is the eighth for every close of the dataset.
fn lead(candles: &[Candle], bar: usize, fast: usize, slow: usize) -> i128 {
    let sum = |window: usize| -> i128 {
        candles[bar + 1 - window..=bar]
            .iter()
            .map(|candle| {
                assert_eq!(candle.close.scale(), 8, "{}", candle.close);
                candle.close.mantissa()
            })
            .sum()
    };
    sum(fast) * slow as i128 - sum(slow) * fast as i128
}

/// What the strategy's rule decides of a lead: long above, flat below, and
/// nothing where the two averages are equal.
fn decided_by(lead: i128) -> Option<TargetPosition> {
    match lead.cmp(&0) {
        Ordering::Greater => LONG,
        Ordering::Less => FLAT,
        Ordering::Equal => KEEP,
    }
}

/// The place of the candle that opened at `open_time_ms`.
fn bar_at(candles: &[Candle], open_time_ms: TimestampMs) -> usize {
    candles
        .iter()
        .position(|candle| candle.open_time_ms == open_time_ms)
        .unwrap_or_else(|| panic!("no candle opened at {open_time_ms}"))
}

// What the dataset was made to show of the strategy, read from the frozen
// files: each is a place where a strategy that differs from the Rust one in
// a small way trades differently.
#[test]
fn the_goldens_hold_what_the_dataset_was_made_to_show() {
    let document = expected();
    let candles = candles();
    let Params { fast, slow } = document.strategy.params;
    assert_eq!(
        (document.strategy.name.as_str(), fast, slow),
        ("sma_cross", 20, 50)
    );
    assert_eq!(
        document.config,
        ConfigText {
            initial_cash: "10000".to_string(),
            fee_bps: "10".to_string(),
            close_out_at_end: true,
        }
    );
    let decisions = &document.decisions;
    let trades = &document.result.trades;

    // Nothing is decided while the slow average warms up, on a price that
    // rises at first: a strategy that averaged what it had so far would be
    // in long before bar 50, where this one enters.
    assert!(decisions[..slow - 1].iter().all(|made| *made == KEEP));
    assert_eq!(decisions[slow - 1], LONG);
    let first = trades.first().expect("a first trade");
    assert_eq!(bar_at(&candles, first.entry_time_ms), slow);
    assert!(candles[slow - 1].close > candles[0].close);

    // Twice the market stands still for an hour. Once the slow average holds
    // that one price alone, the two averages are equal and nothing is
    // decided: the position is kept, long the first time and flat the second.
    let [held_long, held_flat] = dataset::STILL;
    for (still, held) in [(&held_long, LONG), (&held_flat, FLAT)] {
        let equal_from = still.start + slow - 2;
        assert!(
            decisions[still.start..equal_from]
                .iter()
                .all(|made| *made == held)
        );
        assert!(
            decisions[equal_from..still.end]
                .iter()
                .all(|made| *made == KEEP)
        );
        // No fill in the hour or at the bar after it: the position at its
        // end is the one it began with.
        assert!(trades.iter().all(|trade| {
            [trade.entry_time_ms, trade.exit_time_ms]
                .into_iter()
                .all(|at| {
                    at < candles[still.start].open_time_ms || at > candles[still.end].open_time_ms
                })
        }));
    }
    let through = |still: &std::ops::Range<usize>| {
        trades
            .iter()
            .filter(|trade| {
                trade.entry_time_ms < candles[still.start].open_time_ms
                    && trade.exit_time_ms > candles[still.end].open_time_ms
            })
            .count()
    };
    assert_eq!((through(&held_long), through(&held_flat)), (1, 0));

    // Every decision is what exact arithmetic decides of the two averages,
    // on each bar from the first the slow one is full at.
    for (bar, made) in decisions.iter().enumerate().skip(slow - 1) {
        assert_eq!(
            *made,
            decided_by(lead(&candles, bar, fast, slow)),
            "bar {bar}"
        );
    }

    // For an hour and a half the price only moves between two prices a tick
    // apart, the lower of them the close before the stretch. So from the
    // 49th bar of it, as in a still hour, both averages hold nothing else,
    // and they are never half a cent apart: the strategy is long or flat by
    // a few ten-thousandths, which averages rounded to the cent seldom
    // show, and decides nothing where the two are equal.
    let quiet = dataset::QUIET;
    let settled = quiet.start + slow - 2..quiet.end;
    assert_eq!(settled.len(), dataset::QUIET_BARS - slow + 2);
    let floor = candles[quiet.start - 1].close;
    assert!(
        candles[settled.start + 1 - slow..quiet.end]
            .iter()
            .all(|candle| candle.close == floor || candle.close == floor + Decimal::new(1, 2))
    );
    // One unit of the closes' last place, times the two windows.
    let unit = (fast * slow) as i128;
    let (tick, half_a_cent) = (1_000_000 * unit, 500_000 * unit);
    let leads: Vec<i128> = settled
        .clone()
        .map(|bar| lead(&candles, bar, fast, slow))
        .collect();
    assert!(leads.iter().all(|lead| lead.abs() < half_a_cent));
    let on = |held: Option<TargetPosition>| {
        settled
            .clone()
            .filter(|bar| decisions[*bar] == held)
            .count()
    };
    assert_eq!((on(LONG), on(FLAT), on(KEEP)), (27, 7, 8));
    // The smallest difference the strategy acts on anywhere is here: a
    // hundredth of a tick.
    let smallest = |bars: std::ops::Range<usize>| {
        bars.map(|bar| lead(&candles, bar, fast, slow).abs())
            .filter(|lead| *lead > 0)
            .min()
    };
    assert_eq!(smallest(settled.clone()), Some(tick / 100));
    assert_eq!(smallest(slow - 1..candles.len()), Some(tick / 100));
    // Where the two averages are equal there, it is not because every bar
    // is: each window holds both prices.
    let equal: Vec<usize> = settled
        .clone()
        .filter(|bar| decisions[*bar] == KEEP)
        .collect();
    for bar in &equal {
        let closes: Vec<_> = candles[bar + 1 - fast..=*bar]
            .iter()
            .map(|candle| candle.close)
            .collect();
        assert!(closes.iter().any(|close| *close != closes[0]), "bar {bar}");
    }
    // Equal averages are met nowhere but in the two still hours and there.
    let undecided: Vec<usize> = (slow - 1..candles.len())
        .filter(|bar| decisions[*bar] == KEEP)
        .collect();
    let expected: Vec<usize> = (held_long.start + slow - 2..held_long.end)
        .chain(equal.iter().copied())
        .chain(held_flat.start + slow - 2..held_flat.end)
        .collect();
    assert_eq!(undecided, expected);
    // The strategy trades on those differences, three times: in and out
    // again, each decided on one of those bars and filled at the bar after.
    let decided_there =
        |filled_at: TimestampMs| settled.contains(&(bar_at(&candles, filled_at) - 1));
    let there = trades
        .iter()
        .filter(|trade| decided_there(trade.entry_time_ms) && decided_there(trade.exit_time_ms))
        .count();
    assert_eq!(there, 3, "trades decided within the quiet stretch");

    // Winners and losers, and a position still open after the last bar,
    // which the run closes out at that bar's close: the last point of the
    // curve leaves the fee of that sale out, and the final equity has paid it.
    assert_eq!(document.result.trade_count, 15);
    let pnl = |trade: &TradeText| dec(&trade.gross_quote_pnl);
    assert!(trades.iter().any(|trade| pnl(trade) > Decimal::ZERO));
    assert!(trades.iter().any(|trade| pnl(trade) < Decimal::ZERO));
    let (last_bar, closed_out) = (
        candles.last().expect("a last candle"),
        trades.last().expect("a last trade"),
    );
    assert_eq!(closed_out.exit_time_ms, last_bar.close_time_ms);
    assert_eq!(dec(&closed_out.exit_price), last_bar.close);
    assert_eq!(decisions.last(), Some(&LONG));
    let last_point = document.result.equity_curve.last().expect("a last point");
    assert!(dec(&last_point.equity) > dec(&document.result.final_equity));
    assert!(dec(&document.result.max_drawdown_pct) > Decimal::ZERO);
}
