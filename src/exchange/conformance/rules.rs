//! The symbol's trading rules: what the connector reports, and what the
//! exchange enforces on orders.

use rust_decimal::Decimal;

use super::support::{Ctx, Fault, Verdict, ensure};
use crate::{ExchangeErrorKind, MarketOrderRequest, Side, SymbolRules};

pub(super) async fn symbol_rules(ctx: &Ctx<'_>) -> Verdict {
    let rules = ctx.rules().await?;
    // Destructured whole, so a field added to the rules does not compile
    // here until the check covers it.
    let SymbolRules {
        symbol,
        base_asset,
        quote_asset,
        min_qty,
        max_qty,
        step_size,
        market_min_qty,
        market_max_qty,
        market_step_size,
        min_notional,
        tick_size,
    } = &rules;

    ensure!(
        *symbol == ctx.target.symbol,
        "the rules for {} came back for {symbol}",
        ctx.target.symbol
    );
    ensure!(
        !base_asset.is_empty() && !quote_asset.is_empty(),
        "the rules name no base or quote asset: base {base_asset:?}, quote {quote_asset:?}"
    );
    for (name, limit) in [
        ("min_qty", min_qty),
        ("max_qty", max_qty),
        ("step_size", step_size),
        ("market_min_qty", market_min_qty),
        ("market_max_qty", market_max_qty),
        ("market_step_size", market_step_size),
        ("min_notional", min_notional),
        ("tick_size", tick_size),
    ] {
        if let Some(limit) = limit {
            ensure!(
                *limit > Decimal::ZERO,
                "{name} is {limit}; a limit the exchange does not set must be absent, not zero"
            );
        }
    }
    for (min_name, min, max_name, max) in [
        ("min_qty", min_qty, "max_qty", max_qty),
        (
            "market_min_qty",
            market_min_qty,
            "market_max_qty",
            market_max_qty,
        ),
    ] {
        if let (Some(min), Some(max)) = (min, max) {
            ensure!(min <= max, "{min_name} {min} exceeds {max_name} {max}");
        }
    }
    ensure!(
        rules.effective_market_step_size().is_some(),
        "the rules report no quantity step, so an order cannot be rounded to the lot size"
    );
    ensure!(tick_size.is_some(), "the rules report no price tick");
    Ok(None)
}

/// What the order checks size their orders from.
pub(super) struct Sizing {
    pub(super) min_notional: Decimal,
    pub(super) step: Decimal,
    pub(super) min_qty: Option<Decimal>,
}

impl Sizing {
    pub(super) fn from_rules(rules: &SymbolRules) -> Result<Self, Fault> {
        let min_notional = rules
            .min_notional
            .filter(|value| *value > Decimal::ZERO)
            .ok_or_else(|| {
                Fault::failed(
                    "the symbol rules report no minimum notional, which the order checks size \
                     their orders from",
                )
            })?;
        let step = rules
            .effective_market_step_size()
            .filter(|value| *value > Decimal::ZERO)
            .ok_or_else(|| Fault::failed("the symbol rules report no quantity step"))?;
        Ok(Self {
            min_notional,
            step,
            min_qty: rules.effective_market_min_qty(),
        })
    }
}

pub(super) async fn rule_violations(ctx: &Ctx<'_>) -> Verdict {
    let signed = ctx.signed()?;
    let rules = ctx.rules().await?;
    let sizing = Sizing::from_rules(&rules)?;
    let symbol = &ctx.target.symbol;

    let half = sizing
        .min_notional
        .checked_div(Decimal::TWO)
        .ok_or_else(|| Fault::failed("the minimum notional cannot be halved"))?;
    let below_notional = signed
        .trading
        .submit_market_order(&MarketOrderRequest {
            symbol: symbol.clone(),
            side: Side::Buy,
            quantity: None,
            quote_order_qty: Some(half),
            new_client_order_id: None,
        })
        .await;
    ctx.expect_kind(
        &format!("a market buy for {half}, half the minimum notional"),
        below_notional,
        ExchangeErrorKind::RuleViolation,
    )?;

    let off_step = sizing
        .step
        .checked_div(Decimal::TEN)
        .ok_or_else(|| Fault::failed("the quantity step cannot be divided"))?;
    let off_the_grid = signed
        .trading
        .submit_market_order(&MarketOrderRequest {
            symbol: symbol.clone(),
            side: Side::Sell,
            quantity: Some(off_step),
            quote_order_qty: None,
            new_client_order_id: None,
        })
        .await;
    ctx.expect_kind(
        &format!("a market sell of {off_step}, a tenth of the quantity step"),
        off_the_grid,
        ExchangeErrorKind::RuleViolation,
    )?;
    Ok(None)
}
