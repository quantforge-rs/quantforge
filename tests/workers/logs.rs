//! A job's log written by a real worker: each line the job writes reaches
//! the log with its level, in order, and so does how far the job has come;
//! a line too long is cut by the worker and again by the log, and both
//! cuts are counted; and a job that writes without end fills no more than
//! the log's limits, and still ends as it would have.

use quantforge::server::{JobLog, LogLimits};
use serde_json::{Value, json};

use crate::support::{Pool, candles, input, report, roomy};

/// What a log holds, as it would be stored: `null` when nothing was ever
/// written to it.
fn held(log: &JobLog) -> Value {
    log.stored_json().map_or(Value::Null, |json| {
        serde_json::from_str(&json).expect("a stored log")
    })
}

/// Each line of a stored log as its number, level and text.
fn lines(log: &Value) -> Vec<(u64, String, String)> {
    log["lines"]
        .as_array()
        .expect("lines")
        .iter()
        .map(|line| {
            (
                line["seq"].as_u64().expect("a number"),
                line["level"].as_str().expect("a level").to_string(),
                line["message"].as_str().expect("text").to_string(),
            )
        })
        .collect()
}

#[test]
fn what_a_job_writes_reaches_its_log_line_by_line_with_its_level_and_its_progress() {
    let pool = Pool::start(1, roomy());
    let (ran, log) = pool.run_logged(
        input("workload = \"log\"\nlines = 6", candles(12)),
        LogLimits::default(),
    );
    assert_eq!(report(&ran), json!({"lines": 6}));
    let log = held(&log);
    assert_eq!(
        lines(&log),
        [
            (1, "debug", "line 1 of 6"),
            (2, "info", "line 2 of 6"),
            (3, "warning", "line 3 of 6"),
            (4, "error", "line 4 of 6"),
            (5, "debug", "line 5 of 6"),
            (6, "info", "line 6 of 6"),
        ]
        .map(|(seq, level, message)| (seq, level.to_string(), message.to_string()))
    );
    assert_eq!(log["dropped_lines"], 0);
    assert_eq!(log["progress"], json!({"bars_done": 12, "bars_total": 12}));
    let times: Vec<i64> = log["lines"]
        .as_array()
        .expect("lines")
        .iter()
        .map(|line| line["at_ms"].as_i64().expect("a time"))
        .collect();
    assert!(times.windows(2).all(|pair| pair[0] <= pair[1]), "{times:?}");
}

#[test]
fn a_job_that_writes_nothing_has_an_empty_log_and_a_backtest_says_how_far_it_came() {
    let pool = Pool::start(1, roomy());
    let (ran, log) = pool.run_logged(
        input(r#"workload = "report""#, candles(5)),
        LogLimits::default(),
    );
    assert_eq!(report(&ran)["bars"], 5);
    assert_eq!(held(&log), Value::Null, "nothing written, nothing to store");

    // A backtest writes no line of its own, and counts its bars to the last.
    let (ran, log) = pool.run_logged(
        input(r#"workload = "backtest""#, candles(1_234)),
        LogLimits::default(),
    );
    assert!(
        matches!(ran.outcome, quantforge::server::WorkerOutcome::Succeeded(_)),
        "{:?}",
        ran.outcome
    );
    assert_eq!(
        held(&log),
        json!({"dropped_lines": 0, "lines": [], "progress": {"bars_done": 1234, "bars_total": 1234}})
    );
}

#[test]
fn a_line_too_long_is_cut_by_the_worker_and_by_the_log_and_both_cuts_are_counted() {
    let pool = Pool::start(1, roomy());
    // Each line is 300,000 bytes: more than a frame carries of one, which is
    // more than a log keeps of one.
    let (ran, log) = pool.run_logged(
        input("workload = \"log\"\nlines = 2\nbytes = 300000", candles(4)),
        LogLimits::default(),
    );
    assert_eq!(report(&ran), json!({"lines": 2}));
    let log = held(&log);
    let kept = quantforge::server::DEFAULT_LOG_LINE_BYTES;
    for (line, start) in log["lines"]
        .as_array()
        .expect("lines")
        .iter()
        .zip(["line 1 of 2....", "line 2 of 2...."])
    {
        let message = line["message"].as_str().expect("text");
        assert_eq!(message.len(), kept);
        assert!(message.starts_with(start), "{}", &message[..20]);
        assert_eq!(
            line["dropped_bytes"],
            u64::try_from(300_000 - kept).expect("a count"),
            "what the worker cut and what the log cut, together"
        );
    }
}

#[test]
fn a_job_that_floods_its_log_fills_no_more_than_the_logs_limits_and_ends_as_any_other() {
    // Several times what a log keeps, by its lines and by its text.
    const WRITTEN: u64 = 50_000;
    let pool = Pool::start(1, roomy());
    let limits = LogLimits::default();
    let (ran, log) = pool.run_logged(
        input(
            &format!("workload = \"log\"\nlines = {WRITTEN}\nbytes = 120"),
            candles(10),
        ),
        limits,
    );
    assert_eq!(
        report(&ran),
        json!({"lines": WRITTEN}),
        "the job ended as it would have"
    );
    let log = held(&log);
    let kept = log["lines"].as_array().expect("lines");
    assert!(kept.len() <= limits.max_lines, "{} lines", kept.len());
    let text: usize = kept
        .iter()
        .map(|line| line["message"].as_str().expect("text").len())
        .sum();
    assert!(text <= limits.max_bytes, "{text} bytes of text");
    // 120 bytes a line: the text fills before the lines do.
    assert!(text > limits.max_bytes - 121, "{text} bytes of text");
    let kept_count = u64::try_from(kept.len()).expect("a count");
    assert_eq!(
        log["dropped_lines"],
        WRITTEN - kept_count,
        "every line dropped is counted"
    );
    let numbers: Vec<u64> = kept
        .iter()
        .map(|line| line["seq"].as_u64().expect("a number"))
        .collect();
    assert_eq!(
        numbers,
        (WRITTEN - kept_count + 1..=WRITTEN).collect::<Vec<u64>>(),
        "the newest lines, each numbered as it was written"
    );
    let last = kept.last().expect("the last line")["message"]
        .as_str()
        .expect("text");
    assert!(
        last.starts_with(&format!("line {WRITTEN} of {WRITTEN}")),
        "{last}"
    );
    assert_eq!(log["progress"], json!({"bars_done": 10, "bars_total": 10}));
}
