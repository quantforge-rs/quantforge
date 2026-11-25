//! What the server takes from a worker while a job runs: a backtest's
//! rows, kept only while they fit the job and written as the job's three
//! files here, and what the job says, which goes into its log; a frame no
//! worker of this build sends ends the job, and so does one that only a
//! worker of another kind sends. A bundle runs in its worker's process, so
//! it can write any of these frames itself.

use super::*;
use crate::protocol::bundle::Runtime;
use crate::protocol::results::{decode_equity, decode_trades};
use crate::server::jobs::LogLimits;

/// The symbol the jobs here trade.
const SYMBOL: &str = "BTCUSDT";

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
        symbol: SYMBOL.to_string(),
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

/// The rows of a job of `bars` bars: `trades` trades and `equity` points.
fn rows(trades: i64, equity: i64, bars: usize) -> Rows {
    Rows {
        trades: (0..trades).map(trade).collect(),
        equity: (0..equity).map(point).collect(),
        ..Rows::of(bars, SYMBOL)
    }
}

#[test]
fn a_backtests_rows_become_its_three_files_written_here() {
    let outcome = backtest(metrics(2), rows(2, 3, 3)).expect("a backtest");
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
        rows(2, 0, 3).trades
    );
    assert_eq!(
        decode_equity(&files[2].bytes).expect("a curve"),
        rows(0, 3, 3).equity
    );
}

#[test]
fn rows_that_are_no_backtest_of_the_jobs_bars_are_refused() {
    for (trade_count, sent, why) in [
        (
            2,
            rows(2, 2, 3),
            "it sent an equity curve of 2 rows for a job of 3 bars",
        ),
        (
            2,
            rows(2, 4, 3),
            "it sent an equity curve of 4 rows for a job of 3 bars",
        ),
        (1, rows(2, 3, 3), "it sent 2 trades and a summary of 1"),
        (3, rows(2, 3, 3), "it sent 2 trades and a summary of 3"),
    ] {
        let refused = backtest(metrics(trade_count), sent).expect_err("no backtest");
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
    let mut kept = Rows::of(3, SYMBOL);
    assert!(kept.is_empty());
    let (trades, equity) = frames(2);
    assert_eq!(kept.trades(trades), Ok(()));
    assert_eq!(kept.equity(equity), Ok(()));
    let (trades, equity) = frames(1);
    assert_eq!(kept.trades(trades), Ok(()));
    assert_eq!(kept.equity(equity), Ok(()));
    assert!(!kept.is_empty());

    let (trades, equity) = frames(1);
    assert_eq!(
        kept.trades(trades),
        Err("it sent more than 3 trades".to_string())
    );
    assert_eq!(
        kept.equity(equity),
        Err("it sent more than 3 equity rows".to_string())
    );
    assert_eq!((kept.trades.len(), kept.equity.len()), (3, 3), "none kept");
}

#[test]
fn a_trade_of_another_symbol_than_the_jobs_is_refused_and_its_symbol_never_said() {
    // A symbol is the one text in a row, and a forged one is as long as
    // its frame: a megabyte here, and every row of a job could hold one.
    let forged = "S".repeat(1 << 20);
    let mut kept = Rows::of(1_000, SYMBOL);
    for symbol in [forged.as_str(), "ETHUSDT", "btcusdt", ""] {
        let sent = vec![
            TradeFrame::from(&trade(0)),
            TradeFrame::from(&TradeRow {
                symbol: symbol.to_string(),
                ..trade(1)
            }),
        ];
        let refused = kept.trades(sent).expect_err("another symbol");
        assert_eq!(
            refused,
            "it sent a trade of another symbol than the job's BTCUSDT"
        );
    }
    assert!(kept.is_empty(), "no row of a refused frame is kept");
}

/// A worker's failure, as the server keeps it.
fn kept_of(code: ErrorCode, message: String) -> Result<(ErrorCode, String), String> {
    let error = ErrorBody {
        code,
        message,
        supported: None,
    };
    said(error).map(|outcome| match outcome {
        WorkerOutcome::Failed(error) => (error.code, error.message),
        other => panic!("a failure, not {other:?}"),
    })
}

#[test]
fn a_failure_keeps_the_start_of_what_its_worker_said_and_says_how_much_it_cut() {
    let short = "the strategy \"x\" failed in on_bar: ValueError: no".to_string();
    assert_eq!(
        kept_of(ErrorCode::InvalidRequest, short.clone()),
        Ok((ErrorCode::InvalidRequest, short))
    );
    // Exactly as long as a failure may be: kept whole.
    let most = "m".repeat(FAILURE_BYTES);
    assert_eq!(
        kept_of(ErrorCode::Internal, most.clone()),
        Ok((ErrorCode::Internal, most))
    );
    // As long as a frame may be: a strategy raises it in one line.
    let long = format!("ValueError: {}", "x".repeat(15 << 20));
    let (code, kept) = kept_of(ErrorCode::InvalidRequest, long.clone()).expect("a failure");
    assert_eq!(code, ErrorCode::InvalidRequest);
    let cut = long.len() - FAILURE_BYTES;
    assert_eq!(
        kept,
        format!("{}… [{cut} more bytes left out]", &long[..FAILURE_BYTES])
    );
    assert!(kept.len() < FAILURE_BYTES + 64, "{}", kept.len());
    // Cut between two characters, never inside one.
    let wide = format!("{}é and more", "x".repeat(FAILURE_BYTES - 1));
    let (_, kept) = kept_of(ErrorCode::Unavailable, wide).expect("a failure");
    assert!(
        kept.starts_with(&"x".repeat(FAILURE_BYTES - 1))
            && kept.ends_with("x… [11 more bytes left out]"),
        "{}",
        &kept[FAILURE_BYTES - 8..]
    );
}

#[test]
fn a_failure_no_job_fails_as_is_refused() {
    for code in [
        ErrorCode::Unauthorized,
        ErrorCode::NotFound,
        ErrorCode::MethodNotAllowed,
        ErrorCode::Conflict,
        ErrorCode::UnsupportedVersion,
        ErrorCode::LimitExceeded,
        ErrorCode::RateLimited,
        ErrorCode::Unrecognized,
    ] {
        let refused = kept_of(code, "forged".to_string()).expect_err("no job's failure");
        assert_eq!(
            refused,
            format!("it said the job failed as {code:?}, which no job does")
        );
    }
    // What a worker says its server supports is no part of a job's failure.
    let forged: FromWorker = frame(
        r#"{"type":"finished","outcome":{"kind":"failed","error":{"code":"invalid_request","message":"m","supported":{"min_supported":1,"proto_version":9}}}}"#,
    );
    let FromWorker::Finished {
        outcome: Finished::Failed { error },
    } = forged
    else {
        panic!("a failure");
    };
    assert_eq!(
        said(error),
        Ok(WorkerOutcome::Failed(ErrorBody {
            code: ErrorCode::InvalidRequest,
            message: "m".to_string(),
            supported: None,
        }))
    );
}

#[test]
fn only_a_worker_started_for_the_synthetic_workloads_reports() {
    let results = || {
        vec![ResultText {
            name: "metrics".to_string(),
            media_type: "application/json".to_string(),
            text: "{}".to_string(),
        }]
    };
    let runtime: Runtime =
        serde_json::from_str(r#"{"engine":"0.3.0","python":"3.12","packages":{}}"#)
            .expect("a runtime");
    let none = Rows::of(3, SYMBOL);
    let reported = report(&WorkerMode::Synthetic { runtime }, results(), &none);
    let Ok(WorkerOutcome::Succeeded(files)) = reported else {
        panic!("a report, not {reported:?}");
    };
    assert_eq!(files[0].reference.name, "metrics");
    // A bundle that writes its worker's frames itself: no result of its
    // own naming is kept, whatever the worker runs.
    for mode in [WorkerMode::python(), WorkerMode::NoRuntime] {
        assert_eq!(
            report(&mode, results(), &none),
            Err("it sent a report, which only a synthetic workload does")
        );
    }
    let synthetic = WorkerMode::Synthetic {
        runtime: serde_json::from_str(r#"{"engine":"0.3.0","python":"3.12","packages":{}}"#)
            .expect("a runtime"),
    };
    assert_eq!(
        report(&synthetic, results(), &rows(1, 0, 3)),
        Err("it sent a backtest's rows and then a report")
    );
}

#[test]
fn what_a_running_job_says_goes_into_its_log_and_its_bars_are_held_to_the_jobs() {
    let log = JobLog::new(LogLimits {
        max_line_bytes: 8,
        ..LogLimits::default()
    });
    let mut rows = Rows::of(10, SYMBOL);
    for said in [
        r#"{"type":"progress","bars_done":0}"#,
        r#"{"type":"log","level":"warning","message":"short"}"#,
        // Cut by the worker, and again by a log that keeps eight bytes.
        r#"{"type":"log","level":"debug","message":"a long line","dropped_bytes":100}"#,
        r#"{"type":"progress","bars_done":7}"#,
        // A worker cannot say that it ran more bars than the job has.
        r#"{"type":"progress","bars_done":18446744073709551615}"#,
    ] {
        let ended = take(frame(said), &mut rows, &log).expect("a frame of a running job");
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
    let ended = take(frame(finished), &mut rows, &log).expect("the end");
    assert!(matches!(ended, Some(Finished::Succeeded { .. })));
}

#[test]
fn a_frame_no_worker_of_this_build_sends_ends_the_job_and_leaves_its_log_alone() {
    let log = JobLog::new(LogLimits::default());
    let mut rows = Rows::of(1, SYMBOL);
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
        (
            r#"{"type":"trades","rows":[{"symbol":"ETHUSDT","entry_time_ms":0,"exit_time_ms":1,"entry_price":"1","exit_price":"1","qty":"1","gross_quote_pnl":"0"}]}"#,
            "it sent a trade of another symbol than the job's BTCUSDT",
        ),
    ] {
        let refused = take(frame(said), &mut rows, &log).expect_err("no frame of a job");
        assert_eq!(refused, why);
    }
    assert_eq!(log.stored_json(), None, "nothing was written to the log");
    assert!(rows.is_empty());
}
