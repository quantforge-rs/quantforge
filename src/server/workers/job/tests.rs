//! What the server takes from a worker while a job runs: a backtest's
//! rows, kept only while they fit the job and written as the job's three
//! files here, and what the job says, which goes into its log; a frame no
//! worker of this build sends ends the job.

use super::*;
use crate::protocol::results::{decode_equity, decode_trades};
use crate::server::jobs::LogLimits;

/// A frame as a worker's output holds it.
fn frame(json: &str) -> FromWorker {
    serde_json::from_str(json).expect("a frame")
}

/// What `log` holds, as it would be stored.
fn held(log: &JobLog) -> serde_json::Value {
    let stored = log.stored_json().expect("a log something was written to");
    let mut held: serde_json::Value = serde_json::from_str(&stored).expect("json");
    for line in held["lines"].as_array_mut().expect("lines") {
        line.as_object_mut().expect("a line").remove("at_ms");
    }
    held
}

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

#[test]
fn what_a_running_job_says_goes_into_its_log_and_its_bars_are_held_to_the_jobs() {
    let log = JobLog::new(LogLimits {
        max_line_bytes: 8,
        ..LogLimits::default()
    });
    let mut rows = Rows::default();
    for said in [
        r#"{"type":"progress","bars_done":0}"#,
        r#"{"type":"log","level":"warning","message":"short"}"#,
        // Cut by the worker, and again by a log that keeps eight bytes.
        r#"{"type":"log","level":"debug","message":"a long line","dropped_bytes":100}"#,
        r#"{"type":"progress","bars_done":7}"#,
        // A worker cannot say that it ran more bars than the job has.
        r#"{"type":"progress","bars_done":18446744073709551615}"#,
    ] {
        let ended = take(frame(said), &mut rows, 10, &log).expect("a frame of a running job");
        assert!(ended.is_none(), "{said}");
    }
    assert_eq!(
        held(&log),
        serde_json::json!({
            "dropped_lines": 0,
            "lines": [
                {"seq": 1, "level": "warning", "message": "short"},
                {"seq": 2, "level": "debug", "message": "a long l", "dropped_bytes": 103},
            ],
            "progress": {"bars_done": 10, "bars_total": 10},
        })
    );
    assert!(rows.is_empty());

    let finished = r#"{"type":"finished","outcome":{"kind":"succeeded","results":[]}}"#;
    let ended = take(frame(finished), &mut rows, 10, &log).expect("the end");
    assert!(matches!(ended, Some(Finished::Succeeded { .. })));
}

#[test]
fn a_frame_no_worker_of_this_build_sends_ends_the_job_and_leaves_its_log_alone() {
    let log = JobLog::new(LogLimits::default());
    let mut rows = Rows::default();
    for (said, why) in [
        (
            // Read as a level with no name, which no client could be sent.
            r#"{"type":"log","level":"trace","message":"forged"}"#,
            "it wrote a line at a level this build does not know",
        ),
        (
            r#"{"type":"ready","build":"b","pid":1,"posture":{"cpu":{"state":"enforced","by":"x"},"memory":{"state":"enforced","by":"x"},"network":{"state":"enforced","by":"x"},"files":{"state":"enforced","by":"x"}},"runtime":null}"#,
            "it said it was ready a second time",
        ),
        (
            r#"{"type":"equity","rows":[{"open_time_ms":0,"equity":"1"},{"open_time_ms":1,"equity":"1"}]}"#,
            "it sent more than 1 equity rows",
        ),
    ] {
        let refused = take(frame(said), &mut rows, 1, &log).expect_err("no frame of a job");
        assert_eq!(refused, why);
    }
    assert_eq!(log.stored_json(), None, "nothing was written to the log");
    assert!(rows.is_empty());
}
