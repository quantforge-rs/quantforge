//! The strategy boundary's shared cases: what a strategy is called with,
//! callback by callback, and what its decisions come to, for runs scripted
//! here and written to `tests/fixtures/strategy_cases.json`.
//!
//! The engine is the oracle. For every case the file holds the run's
//! inputs, the decision the script gives on each bar, every callback in the
//! order the engine made it with the state it was handed, and the result or
//! the error. The Python package's tests read it and hold a strategy
//! written in Python to it, call for call and digit for digit
//! (`quantforge-py/tests/test_strategy_cases.py`), so a strategy behind the
//! Python adapter is driven exactly as one written in Rust is.
//!
//! The file is generated, never edited. After an intended change to a case,
//! or to the engine's execution model, refresh it with
//!
//! ```text
//! UPDATE_SNAPSHOTS=1 cargo test --test strategy_cases
//! ```
//!
//! and review the diff. `tests/backtest_engine.rs` holds the model itself to
//! hand-worked figures; this file adds only the anchors at its end.

use quantforge::{
    BacktestConfig, BacktestEngine, BacktestResult, Candle, ExchangeId, Interval, MarketId,
    Strategy, StrategyContext, StrategyError, Symbol, TargetPosition, TimestampMs,
};
use rust_decimal::Decimal;
use serde::Serialize;
use std::path::PathBuf;

const FIXTURE: &str = "tests/fixtures/strategy_cases.json";

/// What a scripted failure says: the whole of the engine's error for it.
const FAILURE: &str = "the script fails here";

const LONG: Option<TargetPosition> = Some(TargetPosition::LongAllIn);
const FLAT: Option<TargetPosition> = Some(TargetPosition::Flat);
const KEEP: Option<TargetPosition> = None;

/// One run: its inputs, and what its strategy does.
struct Script {
    id: &'static str,
    doc: &'static str,
    market: MarketId,
    config: BacktestConfig,
    candles: Vec<Candle>,
    /// The decision on each bar; a bar past the list's end keeps the
    /// position.
    decisions: Vec<Option<TargetPosition>>,
    fails: Option<Fails>,
}

/// The callback a script fails in, and for `on_bar` the bar, by its place
/// among the candles.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
struct Fails {
    callback: &'static str,
    bar: Option<usize>,
}

#[derive(Debug, Serialize)]
struct Document {
    format: u32,
    failure: &'static str,
    cases: Vec<Case>,
}

#[derive(Debug, Serialize)]
struct Case {
    id: &'static str,
    doc: &'static str,
    market: MarketText,
    config: ConfigText,
    candles: Vec<CandleText>,
    decisions: Vec<Option<TargetPosition>>,
    fails: Option<Fails>,
    calls: Vec<Call>,
    result: Option<ResultText>,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct MarketText {
    exchange: &'static str,
    symbol: String,
    interval: &'static str,
}

#[derive(Debug, Serialize)]
struct ConfigText {
    initial_cash: String,
    fee_bps: String,
    close_out_at_end: bool,
}

#[derive(Debug, Serialize)]
struct CandleText {
    open_time_ms: TimestampMs,
    close_time_ms: TimestampMs,
    open: String,
    high: String,
    low: String,
    close: String,
    volume: String,
    trades: Option<u64>,
}

/// One callback as the engine made it: which, for `on_bar` the bar by its
/// place among the candles, and the context it was handed.
#[derive(Debug, PartialEq, Serialize)]
struct Call {
    callback: &'static str,
    bar: Option<usize>,
    now_ms: TimestampMs,
    cash: String,
    position_qty: String,
}

#[derive(Debug, Serialize)]
struct ResultText {
    initial_cash: String,
    final_equity: String,
    total_return_pct: String,
    trade_count: usize,
    max_drawdown_pct: String,
    trades: Vec<TradeText>,
    equity_curve: Vec<PointText>,
}

#[derive(Debug, Serialize)]
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

#[derive(Debug, Serialize)]
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

/// Plays a script back and writes down every callback. It holds the engine
/// to what the file leaves unsaid: the market of every context, and each
/// bar being the candle at its place.
#[derive(Debug)]
struct Scripted<'a> {
    market: &'a MarketId,
    candles: &'a [Candle],
    decisions: &'a [Option<TargetPosition>],
    fails: Option<Fails>,
    bars: usize,
    calls: Vec<Call>,
}

impl Scripted<'_> {
    fn called(
        &mut self,
        callback: &'static str,
        bar: Option<usize>,
        ctx: &StrategyContext,
    ) -> Result<(), StrategyError> {
        assert_eq!(&ctx.market, self.market, "{callback} is handed the market");
        self.calls.push(Call {
            callback,
            bar,
            now_ms: ctx.now_ms,
            cash: ctx.cash.to_string(),
            position_qty: ctx.position_qty.to_string(),
        });
        if self.fails == Some(Fails { callback, bar }) {
            return Err(StrategyError::msg(FAILURE));
        }
        Ok(())
    }
}

impl Strategy for Scripted<'_> {
    fn name(&self) -> &str {
        "scripted"
    }

    fn on_start(&mut self, ctx: &StrategyContext) -> Result<(), StrategyError> {
        self.called("on_start", None, ctx)
    }

    fn on_bar(
        &mut self,
        ctx: &StrategyContext,
        bar: &Candle,
    ) -> Result<Option<TargetPosition>, StrategyError> {
        let index = self.bars;
        self.bars += 1;
        assert_eq!(Some(bar), self.candles.get(index), "bar {index} in order");
        self.called("on_bar", Some(index), ctx)?;
        Ok(self.decisions.get(index).copied().flatten())
    }

    fn on_finish(&mut self, ctx: &StrategyContext) -> Result<(), StrategyError> {
        self.called("on_finish", None, ctx)
    }
}

fn played(script: &Script) -> Case {
    let mut strategy = Scripted {
        market: &script.market,
        candles: &script.candles,
        decisions: &script.decisions,
        fails: script.fails,
        bars: 0,
        calls: Vec::new(),
    };
    let outcome = BacktestEngine::new(script.config.clone()).run(
        &script.market,
        &script.candles,
        &mut strategy,
    );
    let (result, error) = match outcome {
        Ok(result) => (Some(ResultText::of(&result)), None),
        Err(error) => (None, Some(error.to_string())),
    };
    let mut decisions = script.decisions.clone();
    decisions.resize(script.candles.len(), KEEP);
    Case {
        id: script.id,
        doc: script.doc,
        market: MarketText {
            exchange: script.market.exchange.as_str(),
            symbol: script.market.symbol.as_str().to_string(),
            interval: script.market.interval.as_str(),
        },
        config: ConfigText {
            initial_cash: script.config.initial_cash.to_string(),
            fee_bps: script.config.fee_bps.to_string(),
            close_out_at_end: script.config.close_out_at_end,
        },
        candles: script
            .candles
            .iter()
            .map(|candle| CandleText {
                open_time_ms: candle.open_time_ms,
                close_time_ms: candle.close_time_ms,
                open: candle.open.to_string(),
                high: candle.high.to_string(),
                low: candle.low.to_string(),
                close: candle.close.to_string(),
                volume: candle.volume.to_string(),
                trades: candle.trades,
            })
            .collect(),
        decisions,
        fails: script.fails,
        calls: strategy.calls,
        result,
        error,
    }
}

fn dec(value: &str) -> Decimal {
    Decimal::from_str_exact(value).expect("decimal")
}

fn market(exchange: ExchangeId, symbol: &str, interval: Interval) -> MarketId {
    MarketId::new(exchange, Symbol::new(symbol).expect("symbol"), interval)
}

fn btcusdt() -> MarketId {
    market(ExchangeId::BinanceSpot, "BTCUSDT", Interval::M1)
}

fn config(initial_cash: &str, fee_bps: &str, close_out_at_end: bool) -> BacktestConfig {
    BacktestConfig {
        initial_cash: dec(initial_cash),
        fee_bps: dec(fee_bps),
        close_out_at_end,
    }
}

/// A candle that opens at `open_time_ms` and runs for `step_ms`, with the
/// high and the low its open and close make.
fn candle(open_time_ms: TimestampMs, step_ms: i64, open: &str, close: &str) -> Candle {
    let (open, close) = (dec(open), dec(close));
    Candle {
        open_time_ms,
        close_time_ms: open_time_ms + step_ms - 1,
        open,
        high: open.max(close),
        low: open.min(close),
        close,
        volume: Decimal::ONE,
        trades: Some(1),
    }
}

/// The one-minute candle at `index` minutes.
fn minute(index: i64, open: &str, close: &str) -> Candle {
    candle(index * 60_000, 60_000, open, close)
}

fn minutes(prices: &[(&str, &str)]) -> Vec<Candle> {
    prices
        .iter()
        .zip(0..)
        .map(|((open, close), index)| minute(index, open, close))
        .collect()
}

fn failing(callback: &'static str, bar: Option<usize>) -> Option<Fails> {
    Some(Fails { callback, bar })
}

/// Sixty bars of a price that swings, entered and left every nine bars.
fn sixty_bars() -> (Vec<Candle>, Vec<Option<TargetPosition>>) {
    let price = |index: i64| format!("{}.{:02}", 100 + (index * 7) % 23, (index * 37) % 100);
    let candles = (0..60)
        .map(|index| minute(index, &price(index), &price(index + 1)))
        .collect();
    let decisions = (0..60)
        .map(|index| match index % 9 {
            1 => LONG,
            6 => FLAT,
            _ => KEEP,
        })
        .collect();
    (candles, decisions)
}

fn scripts() -> Vec<Script> {
    let (sixty_candles, sixty_decisions) = sixty_bars();
    let flat_market = [("100", "100"), ("100", "100"), ("100", "100")];
    vec![
        Script {
            id: "enter_then_exit",
            doc: "The engine's documented example: long on the first bar and flat on the \
                  third, each filled at the next bar's open.",
            market: btcusdt(),
            config: config("1000", "0", true),
            candles: minutes(&[
                ("100", "100"),
                ("100", "110"),
                ("110", "120"),
                ("125", "125"),
            ]),
            decisions: vec![LONG, KEEP, FLAT, KEEP],
            fails: None,
        },
        Script {
            id: "fees_on_both_fills",
            doc: "Ten basis points on the entry and on the exit: the quantity bought is a \
                  division the engine rounds, and crosses with every digit it has.",
            market: btcusdt(),
            config: config("10000", "10", true),
            candles: minutes(&[
                ("100", "100.1"),
                ("100.1", "103.37"),
                ("103.37", "107.5"),
                ("107.9", "106"),
            ]),
            decisions: vec![LONG, KEEP, FLAT, KEEP],
            fails: None,
        },
        Script {
            id: "closed_out_at_the_end",
            doc: "A position still open after the last bar is sold at that bar's close, \
                  fee included, before on_finish is called.",
            market: btcusdt(),
            config: config("5000", "25", true),
            candles: minutes(&[("50", "50"), ("50", "55"), ("55", "60.5")]),
            decisions: vec![LONG, KEEP, KEEP],
            fails: None,
        },
        Script {
            id: "left_open_at_the_end",
            doc: "Without close-out the position is only valued at the last close: \
                  on_finish is handed it still held, and no trade is recorded.",
            market: btcusdt(),
            config: config("5000", "25", false),
            candles: minutes(&[("50", "50"), ("50", "55"), ("55", "60.5")]),
            decisions: vec![LONG, KEEP, KEEP],
            fails: None,
        },
        Script {
            id: "no_decision",
            doc: "A strategy that never decides is called for every bar and stays flat.",
            market: btcusdt(),
            config: config("1000", "10", true),
            candles: minutes(&[("100", "101"), ("101", "99"), ("99", "104")]),
            decisions: vec![KEEP, KEEP, KEEP],
            fails: None,
        },
        Script {
            id: "the_held_position_asked_for_again",
            doc: "Asking for the position already held changes nothing: long while long, \
                  and flat while flat.",
            market: btcusdt(),
            config: config("1000", "0", true),
            candles: minutes(&[
                ("10", "10"),
                ("10", "11"),
                ("11", "12"),
                ("12", "13"),
                ("13", "14"),
                ("14", "15"),
            ]),
            decisions: vec![FLAT, LONG, LONG, FLAT, FLAT, KEEP],
            fails: None,
        },
        Script {
            id: "a_decision_on_the_last_bar",
            doc: "A decision on the last bar has no next open to fill at, and never fills.",
            market: btcusdt(),
            config: config("1000", "0", true),
            candles: minutes(&[("100", "100"), ("100", "110")]),
            decisions: vec![KEEP, LONG],
            fails: None,
        },
        Script {
            id: "one_bar",
            doc: "One candle: on_start, one on_bar and on_finish, all at its open time.",
            market: btcusdt(),
            config: config("1000", "0", true),
            candles: minutes(&[("100", "105")]),
            decisions: vec![LONG],
            fails: None,
        },
        Script {
            id: "two_round_trips",
            doc: "In, out, in and out again: the second entry spends what the first trade \
                  left.",
            market: btcusdt(),
            config: config("1000", "5", true),
            candles: minutes(&[
                ("20", "20"),
                ("20", "25"),
                ("25", "24"),
                ("24", "30"),
                ("32", "31"),
                ("31", "31"),
            ]),
            decisions: vec![LONG, FLAT, LONG, FLAT, KEEP, KEEP],
            fails: None,
        },
        Script {
            id: "decimals_as_they_are_written",
            doc: "Prices and a volume with trailing zeros, eight decimal places and none, \
                  and a trade count the exchange did not report: each reaches the strategy \
                  as it was given.",
            market: btcusdt(),
            config: config("10000.00", "7.5", true),
            candles: vec![
                Candle {
                    open_time_ms: 1_700_000_000_000,
                    close_time_ms: 1_700_000_059_999,
                    open: dec("81940.01000000"),
                    high: dec("81990.5"),
                    low: dec("81900"),
                    close: dec("81955.25"),
                    volume: dec("0.17235000"),
                    trades: None,
                },
                Candle {
                    open_time_ms: 1_700_000_060_000,
                    close_time_ms: 1_700_000_119_999,
                    open: dec("81955.25000000"),
                    high: dec("82010.00000001"),
                    low: dec("81955.25"),
                    close: dec("82001.10"),
                    volume: dec("12"),
                    trades: Some(412),
                },
                Candle {
                    open_time_ms: 1_700_000_120_000,
                    close_time_ms: 1_700_000_179_999,
                    open: dec("82001.1"),
                    high: dec("82001.1"),
                    low: dec("81700.00"),
                    close: dec("81750.50000000"),
                    volume: dec("0"),
                    trades: Some(0),
                },
                Candle {
                    open_time_ms: 1_700_000_180_000,
                    close_time_ms: 1_700_000_239_999,
                    open: dec("81750.5"),
                    high: dec("81800"),
                    low: dec("81749.99999999"),
                    close: dec("81777.77777777"),
                    volume: dec("3.5"),
                    trades: Some(18_446_744_073_709_551_615),
                },
            ],
            decisions: vec![LONG, KEEP, FLAT, KEEP],
            fails: None,
        },
        Script {
            id: "bars_in_the_order_given",
            doc: "The candles are not checked: out of order and one of them twice, they are \
                  replayed as given, and now_ms follows each bar's own open time.",
            market: btcusdt(),
            config: config("1000", "0", true),
            candles: vec![
                minute(2, "100", "100"),
                minute(0, "50", "50"),
                minute(0, "50", "50"),
                minute(1, "200", "200"),
            ],
            decisions: vec![LONG, KEEP, FLAT, KEEP],
            fails: None,
        },
        Script {
            id: "another_market",
            doc: "Another venue's Spot line, another symbol and eight-hour bars: every \
                  context names the market of the run, and a trade its symbol.",
            market: market(ExchangeId::BybitSpot, "ETHUSDT", Interval::H8),
            config: config("2500", "10", true),
            candles: vec![
                candle(0, 28_800_000, "2000", "2010"),
                candle(28_800_000, 28_800_000, "2010", "2100"),
                candle(57_600_000, 28_800_000, "2100", "2050"),
            ],
            decisions: vec![LONG, FLAT, KEEP],
            fails: None,
        },
        Script {
            id: "bars_before_1970",
            doc: "Open times below zero are times like any other.",
            market: btcusdt(),
            config: config("1000", "0", true),
            candles: vec![
                minute(-2, "4", "4"),
                minute(-1, "4", "5"),
                minute(0, "5", "8"),
            ],
            decisions: vec![LONG, FLAT, KEEP],
            fails: None,
        },
        Script {
            id: "an_entry_at_no_price",
            doc: "An entry that would fill at an open of zero is skipped, and the strategy \
                  is handed its cash untouched.",
            market: btcusdt(),
            config: config("1000", "10", true),
            candles: minutes(&[("100", "100"), ("0", "50"), ("50", "50")]),
            decisions: vec![LONG, KEEP, KEEP],
            fails: None,
        },
        Script {
            id: "sixty_bars",
            doc: "Sixty bars of a price that swings, entered and left every nine bars at \
                  7.5 basis points: six trades a decision ends, and a seventh closed out at \
                  the end.",
            market: btcusdt(),
            config: config("2500", "7.5", true),
            candles: sixty_candles,
            decisions: sixty_decisions,
            fails: None,
        },
        Script {
            id: "fails_in_on_start",
            doc: "A failure in on_start ends the run before any bar.",
            market: btcusdt(),
            config: config("1000", "0", true),
            candles: minutes(&flat_market),
            decisions: vec![LONG, KEEP, KEEP],
            fails: failing("on_start", None),
        },
        Script {
            id: "fails_in_on_bar",
            doc: "A failure on the second bar, with a position just entered, ends the run \
                  there: no bar follows, nothing is closed out, and on_finish is not called.",
            market: btcusdt(),
            config: config("1000", "0", true),
            candles: minutes(&flat_market),
            decisions: vec![LONG, KEEP, KEEP],
            fails: failing("on_bar", Some(1)),
        },
        Script {
            id: "fails_in_on_finish",
            doc: "A failure in on_finish, after every bar and the close-out, still fails \
                  the run: there is no result.",
            market: btcusdt(),
            config: config("1000", "0", true),
            candles: minutes(&flat_market),
            decisions: vec![LONG, KEEP, KEEP],
            fails: failing("on_finish", None),
        },
    ]
}

fn document() -> Document {
    Document {
        format: 1,
        failure: FAILURE,
        cases: scripts().iter().map(played).collect(),
    }
}

/// Whether this run rewrites the file. By value on purpose: a leftover
/// `UPDATE_SNAPSHOTS=0` in the shell must not switch the comparison off.
fn updating() -> bool {
    matches!(
        std::env::var("UPDATE_SNAPSHOTS").ok().as_deref(),
        Some("1") | Some("true")
    )
}

#[test]
fn the_fixture_is_every_case_as_the_engine_runs_it() {
    let mut written = serde_json::to_string_pretty(&document()).expect("json");
    written.push('\n');
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(FIXTURE);
    if updating() {
        std::fs::write(&path, &written)
            .unwrap_or_else(|error| panic!("cannot write {}: {error}", path.display()));
        return;
    }
    let committed = std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "cannot read {}: {error}; run `UPDATE_SNAPSHOTS=1 cargo test --test strategy_cases`",
            path.display()
        )
    });
    if committed != written {
        let line = committed
            .lines()
            .zip(written.lines())
            .position(|(old, new)| old != new)
            .unwrap_or_else(|| committed.lines().count().min(written.lines().count()));
        panic!(
            "{FIXTURE} differs from line {}; run `UPDATE_SNAPSHOTS=1 cargo test --test \
             strategy_cases` and review the diff",
            line + 1
        );
    }
}

fn call(
    callback: &'static str,
    bar: Option<usize>,
    now_ms: TimestampMs,
    cash: &str,
    position_qty: &str,
) -> Call {
    Call {
        callback,
        bar,
        now_ms,
        cash: cash.to_string(),
        position_qty: position_qty.to_string(),
    }
}

// The one case worked by hand here: the example of `BacktestEngine`'s own
// documentation, which fills the entry at 100 and the exit at 125.
#[test]
fn the_first_case_is_the_engines_documented_example() {
    let cases = document().cases;
    let example = cases.first().expect("a first case");
    assert_eq!(example.id, "enter_then_exit");
    assert_eq!(
        example.calls,
        [
            call("on_start", None, 0, "1000", "0"),
            call("on_bar", Some(0), 0, "1000", "0"),
            call("on_bar", Some(1), 60_000, "0", "10"),
            call("on_bar", Some(2), 120_000, "0", "10"),
            call("on_bar", Some(3), 180_000, "1250", "0"),
            call("on_finish", None, 180_000, "1250", "0"),
        ]
    );
    let result = example.result.as_ref().expect("a result");
    assert_eq!(result.final_equity, "1250");
    assert_eq!(result.trade_count, 1);
    let trade = result.trades.first().expect("a trade");
    assert_eq!(
        (trade.entry_price.as_str(), trade.exit_price.as_str()),
        ("100", "125")
    );
    assert_eq!((trade.entry_time_ms, trade.exit_time_ms), (60_000, 180_000));
}

// What the Python package's tests count on finding among the cases.
#[test]
fn the_cases_cover_every_callback_decision_and_ending() {
    let cases = document().cases;
    let mut ids: Vec<&str> = cases.iter().map(|case| case.id).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), cases.len(), "every case has an id of its own");

    for decision in [LONG, FLAT, KEEP] {
        assert!(
            cases.iter().any(|case| case.decisions.contains(&decision)),
            "{decision:?} is decided somewhere"
        );
    }
    for fails in [
        failing("on_start", None),
        failing("on_bar", Some(1)),
        failing("on_finish", None),
    ] {
        let failed: Vec<&Case> = cases.iter().filter(|case| case.fails == fails).collect();
        assert_eq!(failed.len(), 1, "one case fails as {fails:?}");
        assert_eq!(failed[0].error.as_deref(), Some(FAILURE));
        assert!(failed[0].result.is_none());
    }
    for case in &cases {
        assert_eq!(case.decisions.len(), case.candles.len(), "{}", case.id);
        assert_eq!(case.result.is_some(), case.fails.is_none(), "{}", case.id);
        let callbacks: Vec<&str> = case.calls.iter().map(|call| call.callback).collect();
        assert_eq!(callbacks.first(), Some(&"on_start"), "{}", case.id);
        if case.fails.is_none() {
            let bars = vec!["on_bar"; case.candles.len()];
            let whole = [&["on_start"][..], &bars, &["on_finish"]].concat();
            assert_eq!(callbacks, whole, "{}", case.id);
        }
    }

    // on_finish is handed a position still held, and one closed out.
    let finish = |id: &str| {
        let case = cases.iter().find(|case| case.id == id).expect("the case");
        let last = case.calls.last().expect("a last call");
        assert_eq!(last.callback, "on_finish");
        (
            last.position_qty.clone(),
            case.result.as_ref().map(|result| result.trade_count),
        )
    };
    assert_eq!(finish("closed_out_at_the_end"), ("0".to_string(), Some(1)));
    let (held, trades) = finish("left_open_at_the_end");
    assert!(dec(&held) > Decimal::ZERO, "held {held}");
    assert_eq!(trades, Some(0));

    // The run that fails on its second bar ends there.
    let failed = cases
        .iter()
        .find(|case| case.id == "fails_in_on_bar")
        .expect("the case");
    let callbacks: Vec<&str> = failed.calls.iter().map(|call| call.callback).collect();
    assert_eq!(callbacks, ["on_start", "on_bar", "on_bar"]);
}
