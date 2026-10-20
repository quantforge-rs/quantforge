//! The lifecycle of a filled market order, from the buy to the sell-back.

use rust_decimal::Decimal;

use super::Environment;
use super::rules::Sizing;
use super::support::{Anchor, Ctx, Fault, POLL_ATTEMPTS, Verdict, client_order_id, ensure, free};
use crate::ports::SignedConnectors;
use crate::{
    AssetBalance, CancelOrderRequest, ExchangeErrorKind, ExchangeOrder, MarketOrderRequest,
    OrderQueryRequest, OrderStatus, Side, SymbolRules, round_down_to_step,
};

/// The lifecycle buys this many times the minimum notional. An exchange
/// may judge the sell-back's notional at an average price rather than the
/// fill's, so the order keeps a margin above the minimum.
const NOTIONAL_MULTIPLE: u32 = 4;

/// Trades the lifecycle asks for beyond the buy's own fills.
const TRADE_SLACK: usize = 5;

pub(super) async fn lifecycle(ctx: &Ctx<'_>) -> Verdict {
    let signed = ctx.signed()?;
    let rules = ctx.rules().await?;
    let sizing = Sizing::from_rules(&rules)?;
    let anchor = ctx.anchor().await?;
    let before = ctx.balances().await?;
    let spend = sizing
        .min_notional
        .checked_mul(Decimal::from(NOTIONAL_MULTIPLE))
        .ok_or_else(|| Fault::failed("the minimum notional overflows when multiplied"))?;
    let free_quote = free(&before, &rules.quote_asset);
    if free_quote < spend {
        return Err(Fault::Inconclusive(format!(
            "the account holds {free_quote} {}; the check spends {spend}",
            rules.quote_asset
        )));
    }

    let buy_id = ctx
        .capabilities
        .orders
        .client_order_ids
        .then(|| client_order_id(&anchor, "buy"));
    let buy = signed
        .trading
        .submit_market_order(&MarketOrderRequest {
            symbol: ctx.target.symbol.clone(),
            side: Side::Buy,
            quantity: None,
            quote_order_qty: Some(spend),
            new_client_order_id: buy_id.clone(),
        })
        .await
        .map_err(|err| ctx.unexpected("the market buy", err))?;

    // From here on the buy is sold back, whatever the checks in between
    // find.
    let checked = after_buy(ctx, signed, &rules, &before, &buy, buy_id.as_deref()).await;
    let sold = sell_back(ctx, signed, &rules, &sizing, &anchor, &buy).await;
    match (checked, sold) {
        (Ok(steps), Ok(())) => Ok(Some(format!("ran: {steps}, sell back"))),
        (Err(fault), Ok(())) | (Ok(_), Err(fault)) => Err(fault),
        (Err(first), Err(then)) => Err(both(first, then)),
    }
}

/// Everything the lifecycle checks about a placed buy, returning the steps
/// it ran.
async fn after_buy(
    ctx: &Ctx<'_>,
    signed: SignedConnectors<'_>,
    rules: &SymbolRules,
    before: &[AssetBalance],
    buy: &ExchangeOrder,
    buy_id: Option<&str>,
) -> Result<String, Fault> {
    let mut steps = vec!["buy"];
    filled(ctx, buy, Side::Buy)?;
    let executed = buy
        .executed_qty
        .filter(|qty| *qty > Decimal::ZERO)
        .ok_or_else(|| Fault::failed("the buy reported no executed quantity"))?;
    ensure!(
        buy.cumulative_quote_qty
            .is_some_and(|qty| qty > Decimal::ZERO),
        "the buy reported no quote amount: {:?}",
        buy.cumulative_quote_qty
    );
    let fills = buy
        .fills
        .as_deref()
        .filter(|fills| !fills.is_empty())
        .ok_or_else(|| Fault::failed("the buy reported no fills"))?;
    let filled_qty = fills
        .iter()
        .try_fold(Decimal::ZERO, |sum, fill| sum.checked_add(fill.qty))
        .ok_or_else(|| Fault::failed("the buy's fill quantities overflow when added"))?;
    ensure!(
        filled_qty == executed,
        "the buy's fills add up to {filled_qty}, not its executed quantity {executed}"
    );
    let trade_ids: Vec<i64> = fills
        .iter()
        .map(|fill| fill.trade_id)
        .collect::<Option<_>>()
        .ok_or_else(|| Fault::failed("a fill of the buy reported no trade id"))?;
    let order_id = buy
        .order_id
        .ok_or_else(|| Fault::failed("the buy reported no order id"))?;
    ensure!(
        buy.transact_time_ms.is_some(),
        "the buy reported no transaction time"
    );
    if let Some(id) = buy_id {
        ensure!(
            buy.client_order_id.as_deref() == Some(id),
            "the buy came back with client order id {:?}, not {id}",
            buy.client_order_id
        );
    }

    let symbol = &ctx.target.symbol;
    let by_order_id = signed
        .trading
        .query_order(&OrderQueryRequest {
            symbol: symbol.clone(),
            order_id: Some(order_id),
            client_order_id: None,
        })
        .await
        .map_err(|err| ctx.unexpected("looking the buy up by order id", err))?;
    same_order(buy, &by_order_id, "looked up by order id")?;
    steps.push("query by order id");
    if let Some(id) = buy_id {
        let by_client_id = signed
            .trading
            .query_order(&OrderQueryRequest {
                symbol: symbol.clone(),
                order_id: None,
                client_order_id: Some(id.to_string()),
            })
            .await
            .map_err(|err| ctx.unexpected("looking the buy up by client order id", err))?;
        same_order(buy, &by_client_id, "looked up by client order id")?;
        steps.push("query by client order id");
    }

    let open = signed
        .trading
        .open_orders(Some(symbol))
        .await
        .map_err(|err| ctx.unexpected("listing the open orders", err))?;
    ensure!(
        open.iter().all(|order| order.order_id != Some(order_id)),
        "the filled buy {order_id} is listed among the open orders"
    );
    steps.push("open orders");

    let cancel = signed
        .trading
        .cancel_order(&CancelOrderRequest {
            symbol: symbol.clone(),
            order_id: Some(order_id),
            client_order_id: None,
        })
        .await;
    ctx.expect_kind(
        "cancelling the filled buy",
        cancel,
        ExchangeErrorKind::UnknownOrder,
    )?;
    steps.push("cancel");

    let limit = trade_ids.len() + TRADE_SLACK;
    let mut traded = false;
    for attempt in 0..POLL_ATTEMPTS {
        if attempt > 0 {
            ctx.pause().await;
        }
        let trades = signed
            .account
            .recent_trades(symbol, limit)
            .await
            .map_err(|err| ctx.unexpected("listing the recent trades", err))?;
        traded = trade_ids.iter().all(|id| {
            trades
                .iter()
                .any(|trade| trade.trade_id == *id && trade.order_id == order_id)
        });
        if traded {
            break;
        }
    }
    ensure!(
        traded,
        "the buy's trades {trade_ids:?} never appeared among the account's {limit} most recent"
    );
    steps.push("trades");

    let net = buy
        .net_base_qty_after_base_fees(&rules.base_asset)
        .ok_or_else(|| Fault::failed("the buy's net base quantity is unknown"))?;
    let base_before = free(before, &rules.base_asset);
    let expected = base_before
        .checked_add(net)
        .ok_or_else(|| Fault::failed("the base balance overflows"))?;
    let mut base_after = base_before;
    for attempt in 0..POLL_ATTEMPTS {
        if attempt > 0 {
            ctx.pause().await;
        }
        base_after = free(&ctx.balances().await?, &rules.base_asset);
        if base_after == expected {
            break;
        }
    }
    ensure!(
        base_after == expected,
        "the free {} balance went from {base_before} to {base_after}; the buy's net fill of \
         {net} makes {expected}",
        rules.base_asset
    );
    steps.push("balances");
    Ok(steps.join(", "))
}

/// Sells back what the buy bought, rounded down to the quantity step.
async fn sell_back(
    ctx: &Ctx<'_>,
    signed: SignedConnectors<'_>,
    rules: &SymbolRules,
    sizing: &Sizing,
    anchor: &Anchor,
    buy: &ExchangeOrder,
) -> Result<(), Fault> {
    let Some(bought) = buy
        .net_base_qty_after_base_fees(&rules.base_asset)
        .or(buy.executed_qty)
    else {
        return Ok(());
    };
    let qty = round_down_to_step(bought, sizing.step);
    if qty <= Decimal::ZERO {
        return Ok(());
    }
    if let Some(min_qty) = sizing.min_qty {
        ensure!(
            qty >= min_qty,
            "the bought {bought} rounds down to {qty}, below the minimum quantity {min_qty}, so \
             it cannot be sold back"
        );
    }
    let sell_id = ctx
        .capabilities
        .orders
        .client_order_ids
        .then(|| client_order_id(anchor, "sell"));
    let sell = signed
        .trading
        .submit_market_order(&MarketOrderRequest {
            symbol: ctx.target.symbol.clone(),
            side: Side::Sell,
            quantity: Some(qty),
            quote_order_qty: None,
            new_client_order_id: sell_id,
        })
        .await
        .map_err(|err| ctx.unexpected("selling the buy back", err))?;
    filled(ctx, &sell, Side::Sell)?;
    ensure!(
        sell.executed_qty == Some(qty),
        "the sell-back of {qty} executed {:?}",
        sell.executed_qty
    );
    Ok(())
}

/// Passes when `order` is a filled market order on the target's symbol.
fn filled(ctx: &Ctx<'_>, order: &ExchangeOrder, side: Side) -> Result<(), Fault> {
    ensure!(
        order.symbol == ctx.target.symbol && order.side == side,
        "a market {} on {} came back as a {} on {}",
        side.as_str(),
        ctx.target.symbol,
        order.side.as_str(),
        order.symbol
    );
    if order.status == OrderStatus::Filled {
        return Ok(());
    }
    let reason = format!(
        "the market {} came back {:?}, not filled",
        side.as_str(),
        order.status
    );
    // A live book too thin to fill the order says nothing about the
    // connector.
    let thin_book = matches!(
        order.status,
        OrderStatus::Expired | OrderStatus::PartiallyFilled
    );
    Err(
        if ctx.target.environment == Environment::Live && thin_book {
            Fault::Inconclusive(reason)
        } else {
            Fault::Failed(reason)
        },
    )
}

/// Passes when `found`, the buy read back, agrees with the buy as placed.
fn same_order(placed: &ExchangeOrder, found: &ExchangeOrder, how: &str) -> Result<(), Fault> {
    ensure!(
        found.order_id == placed.order_id && found.client_order_id == placed.client_order_id,
        "the buy {how} came back as order {:?} ({:?}), not {:?} ({:?})",
        found.order_id,
        found.client_order_id,
        placed.order_id,
        placed.client_order_id
    );
    ensure!(
        found.symbol == placed.symbol && found.side == placed.side,
        "the buy {how} came back as a {} on {}",
        found.side.as_str(),
        found.symbol
    );
    ensure!(
        found.status == OrderStatus::Filled,
        "the buy {how} reads {:?}, not filled",
        found.status
    );
    ensure!(
        found.executed_qty == placed.executed_qty
            && found.cumulative_quote_qty == placed.cumulative_quote_qty,
        "the buy {how} reports {:?} executed for {:?}, not {:?} for {:?}",
        found.executed_qty,
        found.cumulative_quote_qty,
        placed.executed_qty,
        placed.cumulative_quote_qty
    );
    Ok(())
}

/// One fault for two: failed if either failed, both reasons told in order.
fn both(first: Fault, then: Fault) -> Fault {
    match (first, then) {
        (Fault::Inconclusive(first), Fault::Inconclusive(then)) => {
            Fault::Inconclusive(format!("{first}; then {then}"))
        }
        (
            Fault::Failed(first) | Fault::Inconclusive(first),
            Fault::Failed(then) | Fault::Inconclusive(then),
        ) => Fault::Failed(format!("{first}; then {then}")),
    }
}
