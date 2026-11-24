//! The report a backtest prints, local or remote alike: both hand it the
//! protocol's rows, a local run through the conversion a server's worker
//! uses, so the same result prints the same text wherever it ran.

use quantforge::protocol::results::{Metrics, TradeRow};

/// How many of the last trades the report lists.
const LAST_TRADES: usize = 5;

/// Prints the report of `strategy`'s run: its summary, then its last
/// trades, oldest first.
pub(super) fn print(strategy: &str, metrics: &Metrics, trades: &[TradeRow]) {
    println!("strategy: {strategy}");
    println!("final_equity: {}", metrics.final_equity);
    println!("total_return_pct: {}", metrics.total_return_pct);
    println!("max_drawdown_pct: {}", metrics.max_drawdown_pct);
    println!("trade_count: {}", metrics.trade_count);
    let first = trades.len().saturating_sub(LAST_TRADES);
    for trade in &trades[first..] {
        println!(
            "trade: entry={} @ {} exit={} @ {} qty={} gross_pnl={}",
            trade.entry_time_ms,
            trade.entry_price,
            trade.exit_time_ms,
            trade.exit_price,
            trade.qty,
            trade.gross_quote_pnl
        );
    }
}
