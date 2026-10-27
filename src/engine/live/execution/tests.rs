//! Unit tests of the order placement and bookkeeping in `execution.rs`,
//! kept beside it so the module stays within its line budget.

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
            entry_order_id: Some(VenueOrderId::new("7").expect("order id")),
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
