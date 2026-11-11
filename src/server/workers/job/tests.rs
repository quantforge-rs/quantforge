//! A backtest's rows as the server takes them from a worker: kept only
//! while they fit the job, and written as the job's three files here.

use super::*;
use crate::protocol::results::{decode_equity, decode_trades};

fn trade(index: i64) -> TradeRow {
    TradeRow {
        symbol: "BTCUSDT".to_string(),
        entry_time_ms: index * 60_000,
        exit_time_ms: index * 60_000 + 59_999,
        entry_price: "100.10".parse().expect("a price"),
        exit_price: "101".parse().expect("a price"),
        qty: "0.5".parse().expect("a quantity"),
        gross_quote_pnl: "0.45".parse().expect("an amount"),
    }
}

fn point(index: i64) -> EquityRow {
    EquityRow {
        open_time_ms: index * 60_000,
        equity: "10000.00".parse().expect("an amount"),
    }
}

fn metrics(trade_count: u64) -> Metrics {
    Metrics {
        initial_cash: "10000".parse().expect("an amount"),
        final_equity: "10000.45".parse().expect("an amount"),
        total_return_pct: "0.0045".parse().expect("a percentage"),
        max_drawdown_pct: "0".parse().expect("a percentage"),
        trade_count,
    }
}

fn rows(trades: i64, equity: i64) -> Rows {
    Rows {
        trades: (0..trades).map(trade).collect(),
        equity: (0..equity).map(point).collect(),
    }
}

#[test]
fn a_backtests_rows_become_its_three_files_written_here() {
    let outcome = backtest(metrics(2), rows(2, 3), 3).expect("a backtest");
    let WorkerOutcome::Succeeded(files) = outcome else {
        panic!("a succeeded job, not {outcome:?}");
    };
    let names: Vec<&str> = files
        .iter()
        .map(|file| file.reference.name.as_str())
        .collect();
    assert_eq!(names, ["metrics", "trades", "equity"]);
    assert_eq!(
        Metrics::decode(&files[0].bytes).expect("metrics"),
        metrics(2)
    );
    assert_eq!(
        decode_trades(&files[1].bytes).expect("trades"),
        rows(2, 0).trades
    );
    assert_eq!(
        decode_equity(&files[2].bytes).expect("a curve"),
        rows(0, 3).equity
    );
}

#[test]
fn rows_that_are_no_backtest_of_the_jobs_bars_are_refused() {
    for (trade_count, sent, bars, why) in [
        (
            2,
            rows(2, 2),
            3,
            "it sent an equity curve of 2 rows for a job of 3 bars",
        ),
        (
            2,
            rows(2, 4),
            3,
            "it sent an equity curve of 4 rows for a job of 3 bars",
        ),
        (1, rows(2, 3), 3, "it sent 2 trades and a summary of 1"),
        (3, rows(2, 3), 3, "it sent 2 trades and a summary of 3"),
    ] {
        let refused = backtest(metrics(trade_count), sent, bars).expect_err("no backtest");
        assert_eq!(refused, why);
    }
}

#[test]
fn a_worker_cannot_send_more_rows_than_its_job_has_bars() {
    let frames = |count: i64| -> (Vec<TradeFrame>, Vec<EquityFrame>) {
        (
            (0..count).map(|i| TradeFrame::from(&trade(i))).collect(),
            (0..count).map(|i| EquityFrame::from(&point(i))).collect(),
        )
    };
    let mut kept = Rows::default();
    assert!(kept.is_empty());
    let (trades, equity) = frames(2);
    assert_eq!(kept.trades(trades, 3), Ok(()));
    assert_eq!(kept.equity(equity, 3), Ok(()));
    let (trades, equity) = frames(1);
    assert_eq!(kept.trades(trades, 3), Ok(()));
    assert_eq!(kept.equity(equity, 3), Ok(()));
    assert!(!kept.is_empty());

    let (trades, equity) = frames(1);
    assert_eq!(
        kept.trades(trades, 3),
        Err("it sent more than 3 trades".to_string())
    );
    assert_eq!(
        kept.equity(equity, 3),
        Err("it sent more than 3 equity rows".to_string())
    );
    assert_eq!((kept.trades.len(), kept.equity.len()), (3, 3), "none kept");
}
