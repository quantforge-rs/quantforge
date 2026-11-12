//! The `log` workload: a job that writes to its log and says how far it
//! has come, as much and as fast as its parameters ask, so that the tests
//! can follow a log as it grows and flood one.
//!
//! - `lines`: how many lines it writes, 3 unless given;
//! - `bytes`: how long each line is at least, padded with dots;
//! - `pause_ms`: how long it waits after each line;
//! - `hold`: whether it then waits until its worker is stopped.

use std::time::Duration;

use serde_json::{Value, json};

use super::super::frames::{Finished, JobFrame};
use super::super::report::Reporter;
use super::{report, sleep};
use crate::model::Candle;
use crate::protocol::jobs::LogLevel;

/// The levels its lines take, one after another.
const LEVELS: [LogLevel; 4] = [
    LogLevel::Debug,
    LogLevel::Info,
    LogLevel::Warning,
    LogLevel::Error,
];

pub(super) fn write(job: &JobFrame, candles: &[Candle], reporter: &mut Reporter<'_>) -> Finished {
    let number = |name: &str| job.params.get(name).and_then(Value::as_u64);
    let lines = number("lines").unwrap_or(3);
    let bytes = usize::try_from(number("bytes").unwrap_or(0)).unwrap_or(usize::MAX);
    let pause = Duration::from_millis(number("pause_ms").unwrap_or(0));
    let bars = u64::try_from(candles.len()).unwrap_or(u64::MAX);
    for (n, level) in (1..=lines).zip(LEVELS.into_iter().cycle()) {
        let mut line = format!("line {n} of {lines}");
        if line.len() < bytes {
            line.push_str(&".".repeat(bytes - line.len()));
        }
        reporter.line(level, &line);
        // As many bars as the lines written are of all the lines.
        let done = u128::from(bars) * u128::from(n) / u128::from(lines);
        reporter.progress(u64::try_from(done).unwrap_or(u64::MAX));
        if !pause.is_zero() {
            std::thread::sleep(pause);
        }
    }
    if job.params.get("hold").and_then(Value::as_bool) == Some(true) {
        sleep()
    }
    report(&json!({ "lines": lines }))
}
