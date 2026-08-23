//! Order execution: the dry-run vs live branch, exchange interaction,
//! exchange-rule gates, and the journal/position writes that follow a
//! filled order.

use super::{LiveTradeConfig, LiveTradeEngine, LiveTradeSummary};
use crate::EngineError;
use crate::{
    BotRunState, Candle, ClosedTrade, ExchangeOrder, ExecutionMode, MarketOrderRequest,
    PositionState, RunStatus, Side, SymbolRules, TargetPosition, now_utc_ms, round_down_to_step,
};
use rust_decimal::Decimal;
use tracing::{info, warn};
use uuid::Uuid;

impl<'a> LiveTradeEngine<'a> {
    pub(super) async fn execute_target(
        &self,
        cfg: &LiveTradeConfig,
        rules: &SymbolRules,
        run_state: &mut BotRunState,
        target: TargetPosition,
        reference_bar: &Candle,
        summary: &mut LiveTradeSummary,
    ) -> Result<(), EngineError> {
        // The min-notional entry check runs for BOTH execution modes, so a
        // clean dry run implies the same notional validation a live entry
        // performs. Exit-side quantity checks (balance, min/max lot rules)
        // depend on live balances and still run only in live mode.
        if target == TargetPosition::LongAllIn {
            ensure_entry_notional(rules, cfg.quote_order_qty)?;
        }

        let order = match cfg.execution_mode {
            ExecutionMode::DryRun => synthetic_market_order(
                rules,
                run_state,
                target,
                cfg.quote_order_qty,
                reference_bar,
            )?,
            ExecutionMode::Live => {
                let venue = self.trading_venue.ok_or_else(|| {
                    EngineError::InvalidConfig("live mode requires a trading venue".to_string())
                })?;
                match target {
                    TargetPosition::LongAllIn => {
                        venue
                            .submit_market_order(&MarketOrderRequest {
                                symbol: cfg.market.symbol.clone(),
                                side: Side::Buy,
                                quantity: None,
                                quote_order_qty: Some(cfg.quote_order_qty),
                                new_client_order_id: Some(new_client_order_id(
                                    "entry",
                                    &run_state.run_id,
                                )),
                            })
                            .await?
                    }
                    TargetPosition::Flat => {
                        let balances = venue.account_balances().await?;
                        let free_base_qty = balances
                            .into_iter()
                            .find(|balance| balance.asset.eq_ignore_ascii_case(&rules.base_asset))
                            .map(|balance| balance.free)
                            .unwrap_or(Decimal::ZERO);

                        let requested_qty = free_base_qty.min(run_state.position.qty);
                        let requested_qty = maybe_round_qty(requested_qty, rules);

                        if requested_qty <= Decimal::ZERO {
                            warn!(
                                requested_qty = %requested_qty,
                                run_position_qty = %run_state.position.qty,
                                "ignoring flat target because no sellable quantity remained"
                            );
                            return Ok(());
                        }
                        if let Some(min_qty) = sell_qty_below_market_min(requested_qty, rules) {
                            warn!(
                                requested_qty = %requested_qty,
                                min_qty = %min_qty,
                                "ignoring flat target: quantity is below the exchange minimum \
                                 (dust); the exchange would reject the sell"
                            );
                            return Ok(());
                        }
                        if let Some(max_qty) = sell_qty_above_market_max(requested_qty, rules) {
                            return Err(EngineError::InvalidState(format!(
                                "sell quantity {requested_qty} exceeds exchange maximum {max_qty}"
                            )));
                        }

                        venue
                            .submit_market_order(&MarketOrderRequest {
                                symbol: cfg.market.symbol.clone(),
                                side: Side::Sell,
                                quantity: Some(requested_qty),
                                quote_order_qty: None,
                                new_client_order_id: Some(new_client_order_id(
                                    "exit",
                                    &run_state.run_id,
                                )),
                            })
                            .await?
                    }
                }
            }
        };

        info!(
            run_id = %run_state.run_id,
            side = %order.side,
            status = %order.status.as_str(),
            executed_qty = ?order.executed_qty,
            avg_price = ?order.average_price(),
            "order submitted"
        );

        // Journal the order event, but defer any journaling failure until the
        // position mutation below is persisted: an executed order must be
        // reflected in local state even when the journal write fails,
        // otherwise a restart sees a flat position and doubles the exposure.
        let order_event_result = self
            .journal_store
            .append_order_event(&run_state.run_id, &order)
            .map_err(|err| {
                EngineError::InvalidState(format!(
                    "order executed but journaling the order event failed ({err}); \
                     reconcile manually with `monitor status`"
                ))
            });
        summary.submitted_orders += 1;

        match target {
            TargetPosition::LongAllIn => {
                let executed_qty = order.executed_qty.ok_or_else(|| {
                    EngineError::InvalidState(format!(
                        "exchange did not report an executed quantity for entry order {:?}; \
                         reconcile the position manually with `monitor status`",
                        order.order_id
                    ))
                })?;
                if executed_qty <= Decimal::ZERO {
                    warn!("entry order had zero executed quantity");
                    order_event_result?;
                    return Ok(());
                }

                // The buy has executed: the position must be recorded even
                // when parts of the response are missing. Missing fills mean
                // the gross quantity is recorded (no fee deduction can be
                // computed); a missing price is recorded as None and will
                // fail the exit explicitly instead of fabricating PnL.
                let qty = match order.net_base_qty_after_base_fees(&rules.base_asset) {
                    Some(net) => net,
                    None => {
                        warn!(
                            order_id = ?order.order_id,
                            "exchange did not report fills; recording gross executed \
                             quantity without fee deduction"
                        );
                        executed_qty
                    }
                };
                if qty <= Decimal::ZERO {
                    warn!("entry order netted zero quantity after fees");
                    order_event_result?;
                    return Ok(());
                }
                if order.average_price().is_none() {
                    warn!(
                        order_id = ?order.order_id,
                        "exchange did not report a fill price for the entry; closing \
                         this position will not produce a trade record"
                    );
                }

                run_state.position = PositionState {
                    qty,
                    entry_price: order.average_price(),
                    entry_time_ms: order.transact_time_ms.or(Some(reference_bar.close_time_ms)),
                    entry_order_id: order.order_id,
                };

                run_state.updated_at_ms = now_utc_ms();
                run_state.status = RunStatus::Running;
                run_state.last_error = None;
                self.journal_store.save_run_state(run_state)?;
                order_event_result?;
                Ok(())
            }
            TargetPosition::Flat => {
                let executed_qty = order.executed_qty.ok_or_else(|| {
                    EngineError::InvalidState(format!(
                        "exchange did not report an executed quantity for exit order {:?}; \
                         run state left unchanged — reconcile manually with `monitor status`",
                        order.order_id
                    ))
                })?;
                let closed_qty = executed_qty.min(run_state.position.qty);
                if closed_qty <= Decimal::ZERO {
                    warn!("exit order had zero executed quantity");
                    order_event_result?;
                    return Ok(());
                }

                // The sell has executed on the exchange, so the local
                // position is updated and persisted before anything below
                // can fail. A missing price then aborts with a clear error
                // instead of writing a closed-trade row with fabricated PnL.
                let position_before = run_state.position.clone();
                let remaining_qty = (position_before.qty - closed_qty).max(Decimal::ZERO);
                if remaining_qty > Decimal::ZERO && !is_dust_remnant(remaining_qty, rules) {
                    run_state.position.qty = remaining_qty;
                } else {
                    if remaining_qty > Decimal::ZERO {
                        warn!(
                            written_off_qty = %remaining_qty,
                            "position remnant after exit is below the tradeable minimum; \
                             writing it off so the run does not wedge on unsellable dust"
                        );
                    }
                    run_state.position = PositionState::flat();
                }
                run_state.updated_at_ms = now_utc_ms();
                run_state.status = RunStatus::Running;
                run_state.last_error = None;
                self.journal_store.save_run_state(run_state)?;
                order_event_result?;

                let entry_price = position_before.entry_price.ok_or_else(|| {
                    EngineError::InvalidState(
                        "position had no recorded entry price; the exit was executed and \
                         position state updated, but no closed-trade row was written \
                         because its PnL would be fabricated"
                            .to_string(),
                    )
                })?;
                let exit_price = order.average_price().ok_or_else(|| {
                    EngineError::InvalidState(format!(
                        "exchange did not report a fill price for exit order {:?}; the exit \
                         was executed and position state updated, but no closed-trade row \
                         was written because its PnL would be fabricated",
                        order.order_id
                    ))
                })?;

                let closed_trade = ClosedTrade {
                    symbol: cfg.market.symbol.clone(),
                    entry_time_ms: position_before
                        .entry_time_ms
                        .unwrap_or(reference_bar.open_time_ms),
                    exit_time_ms: order
                        .transact_time_ms
                        .unwrap_or(reference_bar.close_time_ms),
                    entry_price,
                    exit_price,
                    qty: closed_qty,
                    gross_quote_pnl: (exit_price - entry_price) * closed_qty,
                    entry_order_id: position_before.entry_order_id,
                    exit_order_id: order.order_id,
                };
                self.journal_store
                    .append_closed_trade(&run_state.run_id, &closed_trade)?;
                summary.closed_trades += 1;
                Ok(())
            }
        }
    }
}

fn maybe_round_qty(qty: Decimal, rules: &SymbolRules) -> Decimal {
    if let Some(step_size) = rules.effective_market_step_size() {
        round_down_to_step(qty, step_size)
    } else {
        qty
    }
}

fn ensure_entry_notional(rules: &SymbolRules, quote_order_qty: Decimal) -> Result<(), EngineError> {
    if let Some(min_notional) = rules.min_notional {
        if quote_order_qty < min_notional {
            return Err(EngineError::InvalidConfig(format!(
                "quote_order_qty {quote_order_qty} is below exchange min_notional {min_notional}"
            )));
        }
    }
    Ok(())
}

/// Returns the exchange market-order minimum when `qty` is below it.
fn sell_qty_below_market_min(qty: Decimal, rules: &SymbolRules) -> Option<Decimal> {
    rules
        .effective_market_min_qty()
        .filter(|min_qty| qty < *min_qty)
}

/// Returns the exchange market-order maximum when `qty` exceeds it.
fn sell_qty_above_market_max(qty: Decimal, rules: &SymbolRules) -> Option<Decimal> {
    rules
        .effective_market_max_qty()
        .filter(|max_qty| qty > *max_qty)
}

/// A position remnant is dust when it cannot be sold: it rounds to zero at
/// the exchange step size or falls below the market minimum quantity.
/// Keeping dust as an open position wedges the run — exits skip it as
/// unsellable while entries are no-ops because the position reads as open.
fn is_dust_remnant(qty: Decimal, rules: &SymbolRules) -> bool {
    if qty <= Decimal::ZERO {
        return false;
    }
    let tradeable = maybe_round_qty(qty, rules);
    tradeable <= Decimal::ZERO || sell_qty_below_market_min(tradeable, rules).is_some()
}

fn synthetic_market_order(
    rules: &SymbolRules,
    run_state: &BotRunState,
    target: TargetPosition,
    quote_order_qty: Decimal,
    reference_bar: &Candle,
) -> Result<ExchangeOrder, EngineError> {
    let side = match target {
        TargetPosition::LongAllIn => Side::Buy,
        TargetPosition::Flat => Side::Sell,
    };

    let (requested_qty, requested_quote_qty, executed_qty, cumulative_quote_qty) = match target {
        TargetPosition::LongAllIn => {
            if reference_bar.close <= Decimal::ZERO {
                return Err(EngineError::InvalidState(
                    "cannot simulate market buy with non-positive reference price".to_string(),
                ));
            }
            let raw_qty = quote_order_qty / reference_bar.close;
            let qty = maybe_round_qty(raw_qty, rules);
            (None, Some(quote_order_qty), qty, qty * reference_bar.close)
        }
        TargetPosition::Flat => {
            let qty = maybe_round_qty(run_state.position.qty, rules);
            (Some(qty), None, qty, qty * reference_bar.close)
        }
    };

    Ok(ExchangeOrder {
        symbol: run_state.market.symbol.clone(),
        side,
        order_type: "MARKET".to_string(),
        status: crate::OrderStatus::Filled,
        order_id: None,
        client_order_id: Some(new_client_order_id("dry", &run_state.run_id)),
        requested_qty,
        requested_quote_qty,
        executed_qty: Some(executed_qty),
        cumulative_quote_qty: Some(cumulative_quote_qty),
        avg_price: Some(reference_bar.close),
        transact_time_ms: Some(reference_bar.close_time_ms),
        // Synthetic fills model no fees: dry-run reports the gross quantity
        // as net, which is optimistic relative to a real fill.
        fills: Some(Vec::new()),
        raw: serde_json::json!({
            "execution_mode": "dry_run",
            "reference_open_time_ms": reference_bar.open_time_ms,
            "reference_close_time_ms": reference_bar.close_time_ms
        }),
    })
}

fn sanitize_client_order_id_fragment(input: &str, max_len: usize) -> String {
    let out: String = input
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(max_len)
        .collect();

    if out.is_empty() {
        "run".to_string()
    } else {
        out
    }
}

fn new_client_order_id(tag: &str, run_id: &str) -> String {
    let tag = sanitize_client_order_id_fragment(tag, 5);
    let prefix = sanitize_client_order_id_fragment(run_id, 8);

    let nonce = Uuid::new_v4().simple().to_string();
    let nonce = &nonce[..8];

    // keep timestamp short so total length stays <= 36
    let ts = (now_utc_ms() % 100_000_000).to_string();

    // qf-<tag>-<prefix>-<ts>-<nonce>
    let id = format!("qf-{tag}-{prefix}-{ts}-{nonce}");
    debug_assert!(id.len() <= 36);
    id
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ExchangeId, Interval, MarketId, Symbol};
    use std::str::FromStr;

    fn market() -> MarketId {
        MarketId::new(
            ExchangeId::BinanceSpot,
            Symbol::new("BTCUSDT").expect("symbol"),
            Interval::M1,
        )
    }

    fn rules() -> SymbolRules {
        SymbolRules {
            symbol: Symbol::new("BTCUSDT").expect("symbol"),
            base_asset: "BTC".to_string(),
            quote_asset: "USDT".to_string(),
            min_qty: Some(Decimal::from_str("0.001").expect("decimal")),
            max_qty: None,
            step_size: Some(Decimal::from_str("0.001").expect("decimal")),
            market_min_qty: Some(Decimal::from_str("0.001").expect("decimal")),
            market_max_qty: None,
            market_step_size: Some(Decimal::from_str("0.001").expect("decimal")),
            min_notional: Some(Decimal::from(10)),
            tick_size: Some(Decimal::from_str("0.01").expect("decimal")),
        }
    }

    fn reference_bar() -> Candle {
        Candle {
            open_time_ms: 0,
            close_time_ms: 59_999,
            open: Decimal::from(10_000),
            high: Decimal::from(10_000),
            low: Decimal::from(10_000),
            close: Decimal::from(10_000),
            volume: Decimal::ONE,
            trades: Some(1),
        }
    }

    fn run_state() -> BotRunState {
        BotRunState {
            run_id: "run-1".to_string(),
            market: market(),
            strategy_name: "sma_cross".to_string(),
            strategy_config: serde_json::json!({"kind":"sma_cross","fast":20,"slow":50}),
            execution_mode: ExecutionMode::DryRun,
            status: RunStatus::Running,
            last_processed_open_time_ms: None,
            started_at_ms: 0,
            updated_at_ms: 0,
            stopped_at_ms: None,
            last_error: None,
            position: PositionState {
                qty: Decimal::from_str("0.0254").expect("decimal"),
                entry_price: Some(Decimal::from(9_900)),
                entry_time_ms: Some(0),
                entry_order_id: Some(7),
            },
        }
    }

    #[test]
    fn synthetic_buy_uses_quote_order_qty_and_rounds_down() {
        let order = synthetic_market_order(
            &rules(),
            &run_state(),
            TargetPosition::LongAllIn,
            Decimal::from(123),
            &reference_bar(),
        )
        .expect("order");

        assert_eq!(order.side, Side::Buy);
        assert_eq!(order.requested_quote_qty, Some(Decimal::from(123)));
        assert_eq!(
            order.executed_qty,
            Some(Decimal::from_str("0.012").expect("decimal"))
        );
    }

    #[test]
    fn synthetic_sell_uses_position_qty_and_rounds_down() {
        let order = synthetic_market_order(
            &rules(),
            &run_state(),
            TargetPosition::Flat,
            Decimal::from(123),
            &reference_bar(),
        )
        .expect("order");

        assert_eq!(order.side, Side::Sell);
        assert_eq!(
            order.requested_qty,
            Some(Decimal::from_str("0.025").expect("decimal"))
        );
        assert_eq!(
            order.executed_qty,
            Some(Decimal::from_str("0.025").expect("decimal"))
        );
    }

    #[test]
    fn dust_remnants_are_detected_only_below_the_tradeable_minimum() {
        // step 0.001, market min 0.001 (from rules()).
        let sub_step = Decimal::from_str("0.0004").expect("decimal");
        let at_min = Decimal::from_str("0.001").expect("decimal");
        let sellable = Decimal::from_str("0.5").expect("decimal");

        assert!(is_dust_remnant(sub_step, &rules()));
        assert!(!is_dust_remnant(at_min, &rules()));
        assert!(!is_dust_remnant(sellable, &rules()));
        assert!(!is_dust_remnant(Decimal::ZERO, &rules()));
    }

    #[test]
    fn entry_notional_below_exchange_minimum_is_rejected() {
        let error = ensure_entry_notional(&rules(), Decimal::from(9)).expect_err("notional error");
        assert!(matches!(error, EngineError::InvalidConfig(_)));
        assert!(
            error.to_string().contains("below exchange min_notional 10"),
            "got {error}"
        );
    }

    #[test]
    fn entry_notional_at_or_above_exchange_minimum_is_accepted() {
        ensure_entry_notional(&rules(), Decimal::from(10)).expect("at minimum");
        ensure_entry_notional(&rules(), Decimal::from(100)).expect("above minimum");
    }

    #[test]
    fn sell_qty_rule_helpers_flag_only_out_of_range_quantities() {
        let mut rules = rules();
        rules.market_max_qty = Some(Decimal::from(1));

        let below = Decimal::from_str("0.0001").expect("decimal");
        let within = Decimal::from_str("0.5").expect("decimal");
        let above = Decimal::from(2);

        assert_eq!(
            sell_qty_below_market_min(below, &rules),
            Some(Decimal::from_str("0.001").expect("decimal"))
        );
        assert_eq!(sell_qty_below_market_min(within, &rules), None);
        assert_eq!(
            sell_qty_above_market_max(above, &rules),
            Some(Decimal::from(1))
        );
        assert_eq!(sell_qty_above_market_max(within, &rules), None);
    }

    #[test]
    fn sell_qty_rule_helpers_pass_everything_when_rules_are_absent() {
        let mut rules = rules();
        rules.min_qty = None;
        rules.max_qty = None;
        rules.market_min_qty = None;
        rules.market_max_qty = None;

        let qty = Decimal::from_str("0.0000001").expect("decimal");
        assert_eq!(sell_qty_below_market_min(qty, &rules), None);
        assert_eq!(
            sell_qty_above_market_max(Decimal::from(1_000_000), &rules),
            None
        );
    }

    #[test]
    fn synthetic_market_order_rejects_non_positive_reference_price() {
        let mut reference = reference_bar();
        reference.close = Decimal::ZERO;

        let error = synthetic_market_order(
            &rules(),
            &run_state(),
            TargetPosition::LongAllIn,
            Decimal::from(100),
            &reference,
        )
        .expect_err("non-positive price");

        assert!(matches!(error, EngineError::InvalidState(_)));
        assert!(
            error.to_string().contains("non-positive reference price"),
            "got {error}"
        );
    }
}
