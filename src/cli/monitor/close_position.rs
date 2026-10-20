//! `quantforge monitor close-position` — market-sell the free base-asset
//! balance, behind the `--yes` confirmation gate.

use crate::cli::common::{ConfirmArgs, SymbolArgs, print_order, round_quantity_for_rules};
use anyhow::{Result, bail};
use clap::Args;
use quantforge::{MarketDataConnector, Side, SignedConnectors, Symbol, now_utc_ms};
use rust_decimal::Decimal;

#[derive(Args, Debug)]
pub(crate) struct MonitorClosePositionArgs {
    #[command(flatten)]
    symbol: SymbolArgs,
    #[command(flatten)]
    confirm: ConfirmArgs,
}

pub(crate) async fn handle_monitor_close_position(
    market_data: &dyn MarketDataConnector,
    signed: SignedConnectors<'_>,
    args: MonitorClosePositionArgs,
) -> Result<()> {
    let symbol = Symbol::new(args.symbol.symbol)?;
    let rules = market_data.fetch_symbol_rules(&symbol).await?;
    let balances = signed.account.account_balances().await?;
    let free_base_qty = balances
        .iter()
        .find(|balance| balance.asset.eq_ignore_ascii_case(&rules.base_asset))
        .map(|balance| balance.free)
        .unwrap_or(Decimal::ZERO);
    let qty = round_quantity_for_rules(free_base_qty, &rules);

    println!("base_asset: {}", rules.base_asset);
    println!("sell_qty: {}", qty);
    if !args.confirm.yes {
        println!("No order sent. Re-run with --yes to execute the market sell.");
        return Ok(());
    }
    if qty <= Decimal::ZERO {
        bail!("no sellable quantity available for {}", rules.base_asset);
    }

    let order = signed
        .trading
        .submit_market_order(&quantforge::MarketOrderRequest {
            symbol,
            side: Side::Sell,
            quantity: Some(qty),
            quote_order_qty: None,
            new_client_order_id: Some(format!("manual-close-{}", now_utc_ms())),
        })
        .await?;
    print_order(&order);
    Ok(())
}
