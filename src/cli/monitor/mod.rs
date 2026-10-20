//! `quantforge monitor` — observe exchange state and manage orders
//! manually when needed. Every subcommand requires credentials, gated by
//! the caller before dispatch.

use anyhow::Result;
use clap::Subcommand;
use quantforge::{MarketDataConnector, SignedConnectors, SqliteStore};

mod cancel_order;
mod close_position;
mod orders;
mod status;
mod trades;
mod watch;

use cancel_order::{MonitorCancelOrderArgs, handle_monitor_cancel_order};
use close_position::{MonitorClosePositionArgs, handle_monitor_close_position};
use orders::{MonitorOrdersArgs, handle_monitor_orders};
use status::{MonitorStatusArgs, handle_monitor_status};
use trades::{MonitorTradesArgs, handle_monitor_trades};
use watch::{MonitorWatchArgs, handle_monitor_watch};

#[derive(Subcommand, Debug)]
pub(crate) enum MonitorCommand {
    /// Print balances, latest run state, open orders, and recent trades.
    Status(MonitorStatusArgs),

    /// Poll status repeatedly.
    Watch(MonitorWatchArgs),

    /// List current open orders on the symbol.
    Orders(MonitorOrdersArgs),

    /// List the account's recent trades on the symbol.
    Trades(MonitorTradesArgs),

    /// Cancel a specific order manually.
    CancelOrder(MonitorCancelOrderArgs),

    /// Close the current free base-asset balance for the symbol.
    ClosePosition(MonitorClosePositionArgs),
}

pub(crate) async fn dispatch(
    store: &SqliteStore,
    market_data: &dyn MarketDataConnector,
    signed: SignedConnectors<'_>,
    command: MonitorCommand,
) -> Result<()> {
    match command {
        MonitorCommand::Status(args) => {
            handle_monitor_status(store, market_data, signed, args).await
        }
        MonitorCommand::Watch(args) => handle_monitor_watch(store, market_data, signed, args).await,
        MonitorCommand::Orders(args) => handle_monitor_orders(signed.trading, args).await,
        MonitorCommand::Trades(args) => handle_monitor_trades(signed.account, args).await,
        MonitorCommand::CancelOrder(args) => {
            handle_monitor_cancel_order(signed.trading, args).await
        }
        MonitorCommand::ClosePosition(args) => {
            handle_monitor_close_position(market_data, signed, args).await
        }
    }
}
