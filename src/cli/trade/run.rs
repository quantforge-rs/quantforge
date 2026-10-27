//! `quantforge trade run` — the polling strategy bot, dry-run by default;
//! live mode sits behind the confirmation and credential gates.

use crate::cli::common::{
    CliExecutionMode, ConfirmArgs, MarketArgs, parse_market, parse_positive_decimal,
    strategy_config,
};
use crate::cli::context::{self, AppContext, display_url};
use anyhow::Result;
use clap::Args;
use quantforge::{LiveTradeConfig, LiveTradeEngine, exchange, ms_to_rfc3339};
use std::time::Duration;
use tracing::warn;

#[derive(Args, Debug)]
pub(crate) struct TradeRunArgs {
    #[command(flatten)]
    market: MarketArgs,
    #[arg(long, default_value_t = 20)]
    fast: usize,
    #[arg(long, default_value_t = 50)]
    slow: usize,
    #[arg(long, default_value = "100")]
    quote_order_qty: String,
    #[arg(long, value_enum, default_value_t = CliExecutionMode::DryRun)]
    mode: CliExecutionMode,
    #[command(flatten)]
    confirm: ConfirmArgs,
    // poll_secs/max_loops stay raw here: they are not adjacent in this
    // struct, and flattening PollArgs would reorder `trade run --help`.
    #[arg(long, default_value_t = 5, value_parser = clap::value_parser!(u64).range(1..))]
    poll_secs: u64,
    #[arg(long, default_value_t = 300)]
    bootstrap_bars: usize,
    #[arg(long, default_value_t = false)]
    bootstrap_enter: bool,
    #[arg(long, default_value_t = 1000)]
    limit: u16,
    #[arg(long)]
    run_id: Option<String>,
    #[arg(long)]
    max_loops: Option<usize>,
}

pub(crate) async fn handle_trade_run(ctx: &AppContext, args: TradeRunArgs) -> Result<()> {
    let store = &ctx.store;
    let market_data = ctx.exchange.market_data.as_ref();
    let base_url = &ctx.base_url;
    let market = parse_market(ctx.exchange_id(), args.market.symbol, args.market.interval)?;
    let quote_order_qty = parse_positive_decimal("--quote-order-qty", &args.quote_order_qty)?;

    if matches!(args.mode, CliExecutionMode::Live) {
        if !args.confirm.yes {
            let strategy_name = strategy_config(args.fast, args.slow).strategy_name();
            let url_note = if market_data.is_production() {
                " (PRODUCTION)"
            } else {
                ""
            };
            let interactive = context::is_interactive();
            if !interactive {
                println!("refusing to start live trading without --yes");
                println!(
                    "would run strategy {} on {} {} {}",
                    strategy_name, market.exchange, market.symbol, market.interval
                );
                println!("with REAL orders via {}{}", display_url(base_url), url_note);
                if args.bootstrap_enter {
                    println!("note: --bootstrap-enter may place an order on the first loop");
                }
                println!("re-run with --yes to confirm, or use --mode dry-run");
                return Ok(());
            }
            println!("about to start live trading:");
            println!(
                "strategy {} on {} {} {}",
                strategy_name, market.exchange, market.symbol, market.interval
            );
            println!("REAL orders via {}{}", display_url(base_url), url_note);
            if args.bootstrap_enter {
                println!("note: --bootstrap-enter may place an order on the first loop");
            }
            if !context::prompt_confirmation("type 'yes' to confirm, anything else aborts: ")? {
                println!("aborted; no orders sent");
                return Ok(());
            }
        }
        if market_data.is_production() {
            warn!(
                exchange = exchange::name(market.exchange),
                base_url = %display_url(base_url),
                "live trading against a PRODUCTION endpoint; real funds at risk"
            );
        }
        if ctx.identity.environment.is_none() {
            warn!(
                "the store is not bound to an environment, so this run records none; bind \
                 it with `quantforge store bind --environment <mainnet|testnet|demo> --yes`"
            );
        }
    }

    let engine = LiveTradeEngine::new(
        market_data,
        store,
        store,
        if matches!(args.mode, CliExecutionMode::Live) {
            Some(ctx.require_signed("trade run --mode live requires exchange credentials")?)
        } else {
            None
        },
    );

    let summary = engine
        .run(&LiveTradeConfig {
            market,
            strategy: strategy_config(args.fast, args.slow),
            execution_mode: args.mode.into(),
            identity: ctx.run_identity(args.mode.into()),
            quote_order_qty,
            poll_interval: Duration::from_secs(args.poll_secs),
            bootstrap_bars: args.bootstrap_bars,
            bootstrap_enter: args.bootstrap_enter,
            batch_limit: args.limit,
            run_id: args.run_id,
            max_loops: args.max_loops,
        })
        .await?;

    println!("run_id: {}", summary.run_id);
    println!("processed_bars: {}", summary.processed_bars);
    println!("submitted_orders: {}", summary.submitted_orders);
    println!("closed_trades: {}", summary.closed_trades);
    println!(
        "last_processed_open_time: {}",
        summary
            .last_processed_open_time_ms
            .map(ms_to_rfc3339)
            .unwrap_or_else(|| "none".to_string())
    );
    Ok(())
}
