//! `quantforge monitor orders` — list open orders for a symbol.

use crate::cli::common::{SymbolArgs, print_order};
use anyhow::Result;
use clap::Args;
use quantforge::{Symbol, TradingConnector};

#[derive(Args, Debug)]
pub(crate) struct MonitorOrdersArgs {
    #[command(flatten)]
    symbol: SymbolArgs,
}

pub(crate) async fn handle_monitor_orders(
    trading: &dyn TradingConnector,
    args: MonitorOrdersArgs,
) -> Result<()> {
    let symbol = Symbol::new(args.symbol.symbol)?;
    let orders = trading.open_orders(Some(&symbol)).await?;
    println!("open_orders: {}", orders.len());
    for order in orders {
        print_order(&order);
    }
    Ok(())
}
