//! Deterministic bar-by-bar backtest: replays a candle slice through a
//! [`Strategy`] and fills each decision at the next bar's open, so a fill
//! never uses information the strategy had not seen.
//!
//! [`BacktestEngine::run`] is the whole API. It takes the market, the
//! candles, and any [`Strategy`]: the built-in
//! [`SmaCrossStrategy`](crate::SmaCrossStrategy) or one the caller writes
//! (see the example on [`BacktestEngine`]). It touches no CLI code,
//! database, network, or clock.
//!
//! # Execution model
//!
//! - **Bar-close signal.** For each candle, in the order given,
//!   [`Strategy::on_bar`] is called once with the complete bar, close
//!   included. Its [`StrategyContext`] has the bar's open time as `now_ms`,
//!   and `cash` and `position_qty` already include any fill at that bar's
//!   open.
//! - **Next-open execution.** A decision returned for bar N fills at bar
//!   N + 1's open price, stamped with that bar's open time. A decision
//!   returned for the last bar never fills; `None` keeps the position.
//! - **One position, long or flat.** [`TargetPosition::LongAllIn`] spends all
//!   cash on the base asset and [`TargetPosition::Flat`] sells all of it.
//!   Requesting the position already held does nothing, and an entry with
//!   no cash or at a non-positive price is skipped. There is no shorting,
//!   leverage, partial sizing, slippage, or exchange rule such as a step
//!   size or minimum notional.
//! - **Fees.** Every fill pays `fee_bps` basis points of its notional
//!   (quantity × price) in quote currency. An entry buys
//!   `cash / (price × (1 + fee_bps / 10_000))`, so that notional plus fee
//!   spends the cash; an exit receives the notional minus the fee. A
//!   [`ClosedTrade`]'s `gross_quote_pnl` is the change in cash from before
//!   the entry to after the exit, so both fees are already deducted.
//! - **Close-out.** With `close_out_at_end`, a position still open after the
//!   last bar is sold at that bar's close, stamped with its close time, fee
//!   included. Without it, the position is valued at that close.
//! - **Equity.** Equity is `cash + position × close`, marked at every bar's
//!   close after that bar's fill. `max_drawdown_pct` is the largest fall of
//!   that series from an earlier peak, in percent; it does not include the
//!   close-out fee, but `final_equity` does.
//! - **Lifecycle.** [`Strategy::on_start`] runs once before the first bar,
//!   with the first bar's open time, the initial cash, and no position.
//!   [`Strategy::on_finish`] runs once after close-out, with the last bar's
//!   open time and the final cash and position. An error from any callback
//!   ends the run with that error, and `on_finish` is then not called.
//!
//! # Determinism
//!
//! A run is a pure function of its config, its candles, and its strategy.
//! The engine reads no clock, draws no random numbers, and does no I/O, and
//! all money math is decimal arithmetic. Division rounds to the 28
//! significant digits a [`Decimal`] holds, the same way every time, so an
//! entry can leave a residue of cash in the last digits. Given a strategy
//! that is itself deterministic, as the [`Strategy`] contract requires, the
//! same inputs produce an identical [`BacktestResult`] on every run and
//! every machine.
//!
//! # Errors
//!
//! - no candles: [`EngineError::NoCandles`];
//! - `initial_cash` not above zero, or `fee_bps` below zero:
//!   [`EngineError::InvalidConfig`];
//! - a result outside the decimal range, for example an entry at a price so
//!   small that the quantity cannot be represented: [`EngineError::Overflow`],
//!   never a panic;
//! - a strategy error: [`EngineError::Strategy`].
//!
//! The candles themselves are not checked; they are replayed in the order
//! given. Run [`validate_candles`](crate::validate_candles) first to catch
//! gaps, duplicates, and impossible prices.

use crate::{
    Candle, ClosedTrade, MarketId, Strategy, StrategyContext, TargetPosition, TimestampMs,
};
use rust_decimal::Decimal;
use tracing::info;

use crate::EngineError;

/// How a backtest starts and what it charges.
#[derive(Clone, Debug)]
pub struct BacktestConfig {
    /// Quote currency to start with; must be above zero.
    pub initial_cash: Decimal,
    /// The fee on every fill, in basis points of its notional (10 is 0.1%);
    /// must be zero or more.
    pub fee_bps: Decimal,
    /// Sell a position still open after the last bar at that bar's close,
    /// recording the trade; otherwise it is only valued at that close.
    pub close_out_at_end: bool,
}

/// What a backtest produced. Amounts are in quote currency.
#[derive(Clone, Debug, PartialEq)]
pub struct BacktestResult {
    /// The configured starting cash.
    pub initial_cash: Decimal,
    /// Cash plus any open position valued at the last close, after close-out.
    pub final_equity: Decimal,
    /// `(final_equity - initial_cash) / initial_cash`, in percent.
    pub total_return_pct: Decimal,
    /// The number of closed trades.
    pub trade_count: usize,
    /// The largest fall of the close-marked equity from an earlier peak, in
    /// percent.
    pub max_drawdown_pct: Decimal,
    /// Every round trip, in order. `gross_quote_pnl` is net of both fees,
    /// and the order ids are `None`: nothing reached an exchange.
    pub trades: Vec<ClosedTrade>,
}

/// Runs a [`Strategy`] over a candle slice, under the execution model in the
/// [module docs](self).
///
/// # Example
///
/// A strategy written outside the crate, run from library code only:
///
/// ```
/// use quantforge::{
///     BacktestConfig, BacktestEngine, Candle, ExchangeId, Interval, MarketId, Strategy,
///     StrategyContext, StrategyError, Symbol, TargetPosition,
/// };
/// use rust_decimal::Decimal;
///
/// /// Goes long on the first bar and flat on the third.
/// struct EnterThenExit {
///     bars: usize,
/// }
///
/// impl Strategy for EnterThenExit {
///     fn name(&self) -> &str {
///         "enter_then_exit"
///     }
///
///     fn on_bar(
///         &mut self,
///         _ctx: &StrategyContext,
///         _bar: &Candle,
///     ) -> Result<Option<TargetPosition>, StrategyError> {
///         self.bars += 1;
///         Ok(match self.bars {
///             1 => Some(TargetPosition::LongAllIn),
///             3 => Some(TargetPosition::Flat),
///             _ => None,
///         })
///     }
/// }
///
/// fn candle(minute: i64, open: i64, close: i64) -> Candle {
///     Candle {
///         open_time_ms: minute * 60_000,
///         close_time_ms: minute * 60_000 + 59_999,
///         open: Decimal::from(open),
///         high: Decimal::from(open.max(close)),
///         low: Decimal::from(open.min(close)),
///         close: Decimal::from(close),
///         volume: Decimal::ONE,
///         trades: None,
///     }
/// }
///
/// let market = MarketId::new(ExchangeId::BinanceSpot, Symbol::new("BTCUSDT")?, Interval::M1);
/// let candles = [
///     candle(0, 100, 100),
///     candle(1, 100, 110),
///     candle(2, 110, 120),
///     candle(3, 125, 125),
/// ];
/// let engine = BacktestEngine::new(BacktestConfig {
///     initial_cash: Decimal::from(1_000),
///     fee_bps: Decimal::ZERO,
///     close_out_at_end: true,
/// });
/// let result = engine.run(&market, &candles, &mut EnterThenExit { bars: 0 })?;
///
/// // The long decision on bar 0 fills at bar 1's open (100); the flat
/// // decision on bar 2 fills at bar 3's open (125).
/// assert_eq!(result.trade_count, 1);
/// assert_eq!(result.trades[0].entry_price, Decimal::from(100));
/// assert_eq!(result.trades[0].exit_price, Decimal::from(125));
/// assert_eq!(result.final_equity, Decimal::from(1_250));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Debug)]
pub struct BacktestEngine {
    cfg: BacktestConfig,
}

impl BacktestEngine {
    pub fn new(cfg: BacktestConfig) -> Self {
        Self { cfg }
    }

    /// Replays `candles`, in the order given, through `strategy`, which can
    /// be any [`Strategy`], including one defined outside the crate. The
    /// [module docs](self) state the execution model and the errors.
    pub fn run(
        &self,
        market: &MarketId,
        candles: &[Candle],
        strategy: &mut dyn Strategy,
    ) -> Result<BacktestResult, EngineError> {
        if candles.is_empty() {
            return Err(EngineError::NoCandles);
        }
        if self.cfg.initial_cash <= Decimal::ZERO {
            return Err(EngineError::InvalidConfig(format!(
                "initial_cash must be greater than 0, got {}",
                self.cfg.initial_cash
            )));
        }
        if self.cfg.fee_bps < Decimal::ZERO {
            return Err(EngineError::InvalidConfig(format!(
                "fee_bps must be zero or greater, got {}",
                self.cfg.fee_bps
            )));
        }

        let fee_rate = checked(
            self.cfg.fee_bps.checked_div(Decimal::from(10_000)),
            "the fee rate",
        )?;
        let mut cash = self.cfg.initial_cash;
        let mut qty = Decimal::ZERO;
        let mut open_trade: Option<OpenTrade> = None;
        let mut trades = Vec::new();
        let mut pending_target: Option<TargetPosition> = None;
        let mut peak_equity = Decimal::ZERO;
        let mut max_drawdown = Decimal::ZERO;

        let mut ctx = StrategyContext {
            market: market.clone(),
            now_ms: candles[0].open_time_ms,
            cash,
            position_qty: qty,
        };

        strategy.on_start(&ctx)?;

        for (index, candle) in candles.iter().enumerate() {
            ctx.now_ms = candle.open_time_ms;

            if index > 0 {
                if let Some(target) = pending_target.take() {
                    execute_target(
                        market,
                        target,
                        candle.open,
                        candle.open_time_ms,
                        fee_rate,
                        &mut cash,
                        &mut qty,
                        &mut open_trade,
                        &mut trades,
                    )?;
                }
            }

            ctx.cash = cash;
            ctx.position_qty = qty;

            let equity = mark(cash, qty, candle.close)?;
            if equity > peak_equity {
                peak_equity = equity;
            }
            if peak_equity > Decimal::ZERO {
                let drawdown = checked(
                    peak_equity
                        .checked_sub(equity)
                        .and_then(|fall| fall.checked_div(peak_equity)),
                    "the drawdown",
                )?;
                if drawdown > max_drawdown {
                    max_drawdown = drawdown;
                }
            }

            pending_target = strategy.on_bar(&ctx, candle)?;
        }

        if self.cfg.close_out_at_end && qty > Decimal::ZERO {
            if let Some(last) = candles.last() {
                execute_target(
                    market,
                    TargetPosition::Flat,
                    last.close,
                    last.close_time_ms,
                    fee_rate,
                    &mut cash,
                    &mut qty,
                    &mut open_trade,
                    &mut trades,
                )?;
            }
        }

        ctx.cash = cash;
        ctx.position_qty = qty;
        strategy.on_finish(&ctx)?;

        let last_close = candles
            .last()
            .map(|c| c.close)
            .ok_or_else(|| EngineError::InvalidState("no candles at close-out".to_string()))?;
        let final_equity = mark(cash, qty, last_close)?;
        let total_return_pct = checked(
            final_equity
                .checked_sub(self.cfg.initial_cash)
                .and_then(|gain| gain.checked_div(self.cfg.initial_cash))
                .and_then(|ratio| ratio.checked_mul(Decimal::from(100))),
            "the return",
        )?;
        let max_drawdown_pct =
            checked(max_drawdown.checked_mul(Decimal::from(100)), "the drawdown")?;

        info!(
            strategy = strategy.name(),
            final_equity = %final_equity,
            total_return_pct = %total_return_pct,
            trades = trades.len(),
            "backtest completed"
        );

        Ok(BacktestResult {
            initial_cash: self.cfg.initial_cash,
            final_equity,
            total_return_pct,
            trade_count: trades.len(),
            max_drawdown_pct,
            trades,
        })
    }
}

#[derive(Debug)]
struct OpenTrade {
    entry_time_ms: TimestampMs,
    entry_price: Decimal,
    qty: Decimal,
    cash_before: Decimal,
}

/// rust_decimal's operators panic on overflow; every money step goes
/// through a `checked_*` call and this, which reports it instead.
fn checked(value: Option<Decimal>, what: &'static str) -> Result<Decimal, EngineError> {
    value.ok_or(EngineError::Overflow(what))
}

/// Equity at `price`: `cash + qty × price`.
fn mark(cash: Decimal, qty: Decimal, price: Decimal) -> Result<Decimal, EngineError> {
    checked(
        qty.checked_mul(price)
            .and_then(|value| cash.checked_add(value)),
        "the equity mark",
    )
}

/// Fills `target` at `price`. Every new value is computed before any state
/// changes, so an overflow leaves the account as it was.
#[allow(clippy::too_many_arguments)]
fn execute_target(
    market: &MarketId,
    target: TargetPosition,
    price: Decimal,
    timestamp_ms: TimestampMs,
    fee_rate: Decimal,
    cash: &mut Decimal,
    qty: &mut Decimal,
    open_trade: &mut Option<OpenTrade>,
    trades: &mut Vec<ClosedTrade>,
) -> Result<(), EngineError> {
    match target {
        TargetPosition::Flat => {
            if *qty <= Decimal::ZERO {
                return Ok(());
            }

            let notional = checked(qty.checked_mul(price), "the exit")?;
            let fee = checked(notional.checked_mul(fee_rate), "the exit")?;
            let cash_after = checked(
                cash.checked_add(notional)
                    .and_then(|value| value.checked_sub(fee)),
                "the exit",
            )?;
            let gross_quote_pnl = open_trade
                .as_ref()
                .map(|open| checked(cash_after.checked_sub(open.cash_before), "the exit"))
                .transpose()?;

            if let (Some(open), Some(gross_quote_pnl)) = (open_trade.take(), gross_quote_pnl) {
                trades.push(ClosedTrade {
                    symbol: market.symbol.clone(),
                    entry_time_ms: open.entry_time_ms,
                    exit_time_ms: timestamp_ms,
                    entry_price: open.entry_price,
                    exit_price: price,
                    qty: open.qty,
                    gross_quote_pnl,
                    entry_order_id: None,
                    exit_order_id: None,
                });
            }

            *cash = cash_after;
            *qty = Decimal::ZERO;
        }
        TargetPosition::LongAllIn => {
            if *qty > Decimal::ZERO || *cash <= Decimal::ZERO {
                return Ok(());
            }

            let denominator = checked(
                Decimal::ONE
                    .checked_add(fee_rate)
                    .and_then(|factor| price.checked_mul(factor)),
                "the entry",
            )?;
            if denominator <= Decimal::ZERO {
                return Ok(());
            }

            let cash_before = *cash;
            let buy_qty = checked(cash.checked_div(denominator), "the entry")?;
            let notional = checked(buy_qty.checked_mul(price), "the entry")?;
            let fee = checked(notional.checked_mul(fee_rate), "the entry")?;
            let cash_after = checked(
                cash.checked_sub(notional)
                    .and_then(|value| value.checked_sub(fee)),
                "the entry",
            )?;

            *cash = cash_after;
            *qty = buy_qty;
            *open_trade = Some(OpenTrade {
                entry_time_ms: timestamp_ms,
                entry_price: price,
                qty: buy_qty,
                cash_before,
            });
        }
    }
    Ok(())
}
