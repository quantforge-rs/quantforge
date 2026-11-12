//! One job's run on a worker: the bundle written into the worker's scratch
//! directory, the job's frames sent on a thread of their own, so that a
//! worker that stops reading never holds the server up, and the worker
//! watched until it says how the job ended, its wall-clock time runs out,
//! the job is cancelled, or it stops. What the job says while it runs, the
//! lines of its log and its progress, goes into the job's log as it comes,
//! which keeps no more of it than its limits. A backtest's rows are read
//! as they come, each checked, and written as the job's result files here:
//! a worker runs a job's code, so no file is ever taken from one.

use std::io;
use std::process::ChildStdin;
use std::sync::mpsc::RecvTimeoutError;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use tracing::{error, warn};

use super::config::WorkerLimits;
use super::frames::{
    self, BARS_PER_FRAME, EquityFrame, Finished, FromWorker, JobFrame, ResultText, ToWorker,
    TradeFrame,
};
use super::process::{self, Process, Stop, Worker};
use super::scratch;
use crate::protocol::jobs::LogLevel;
use crate::protocol::results::{EquityRow, Metrics, TradeRow};
use crate::protocol::{ErrorBody, ErrorCode};
use crate::server::history::bar;
use crate::server::jobs::{Cancel, JobInput, JobLog, WorkerOutcome};
use crate::server::results::{BacktestOutput, ResultFile};

/// How often a run looks at its cancel and its clock while it waits.
const POLL: Duration = Duration::from_millis(50);

/// How long a worker whose output has ended may take to stop by itself.
const GRACE: Duration = Duration::from_secs(1);

/// How long the end of a stopped worker's standard error may take to come.
const LAST_WORDS: Duration = Duration::from_millis(500);

/// Runs the job on `worker`, which is killed by the time this returns,
/// writing what the job says into `log`.
pub(super) fn run(
    worker: &mut Worker,
    input: JobInput,
    cancel: &Cancel,
    limits: &WorkerLimits,
    log: &JobLog,
) -> WorkerOutcome {
    let job = input.job_id.to_string();
    let bars = input.candles.len();
    let process = &mut worker.process;
    if let Err(failure) = scratch::write_bundle(&process.scratch, &input.bundle) {
        error!(job, %failure, "a job's bundle could not be written for its worker");
        return failed(
            ErrorCode::Internal,
            "the server could not hand the job's bundle to its worker",
        );
    }
    let Some(stdin) = process.stdin.take() else {
        return failed(ErrorCode::Internal, "the job's worker has no input open");
    };
    let sending = match send(stdin, input) {
        Ok(sending) => sending,
        Err(failure) => {
            error!(job, %failure, "a job's frames could not be sent to its worker");
            return failed(
                ErrorCode::Internal,
                "the server could not hand the job to its worker",
            );
        }
    };
    let wall = Duration::from_secs(limits.wall_secs);
    let ran = match watch(process, cancel, wall, bars, log) {
        Watched::Finished(Finished::Backtest { metrics }, rows) => {
            backtest(metrics, rows, bars).unwrap_or_else(|why| garbled(&job, &why))
        }
        Watched::Finished(Finished::Succeeded { results }, rows) if rows.is_empty() => {
            succeeded(results)
        }
        Watched::Finished(Finished::Succeeded { .. }, _) => {
            garbled(&job, "it sent a backtest's rows and then a report")
        }
        Watched::Finished(Finished::Failed { error }, _) => WorkerOutcome::Failed(error),
        Watched::Cancelled => WorkerOutcome::Cancelled,
        Watched::OutOfTime => failed(
            ErrorCode::InvalidRequest,
            format!(
                "the job ran for the {} s of wall-clock time it may run, and was stopped",
                limits.wall_secs
            ),
        ),
        Watched::Garbled(why) => garbled(&job, &why),
        Watched::Stopped => stopped(process, limits, &job),
    };
    // Killed before its input closes, which the worker reads as the server
    // giving up, and before the sending ends, which a killed worker makes
    // sure of.
    process.kill();
    drop(sending.join());
    ran
}

/// Sends the job and its candles, and answers the input once they are
/// sent, to be kept open until the job ends.
fn send(mut stdin: ChildStdin, input: JobInput) -> io::Result<JoinHandle<io::Result<ChildStdin>>> {
    let manifest = input.bundle.manifest();
    let job = JobFrame {
        job_id: input.job_id.to_string(),
        submission: input.submission,
        entry: manifest.entry().to_string(),
        params: manifest.params().as_map().clone(),
        bars: u64::try_from(input.candles.len()).unwrap_or(u64::MAX),
    };
    let candles = input.candles;
    std::thread::Builder::new()
        .name("worker-job".to_string())
        .spawn(move || {
            frames::write(&mut stdin, &ToWorker::Job(Box::new(job)))?;
            for run in candles.chunks(BARS_PER_FRAME) {
                let bars = run.iter().map(bar).collect();
                frames::write(&mut stdin, &ToWorker::Candles { bars })?;
            }
            Ok(stdin)
        })
}

/// The rows a backtest sent before it finished.
#[derive(Default)]
struct Rows {
    trades: Vec<TradeRow>,
    equity: Vec<EquityRow>,
}

impl Rows {
    fn is_empty(&self) -> bool {
        self.trades.is_empty() && self.equity.is_empty()
    }

    /// Keeps the next run of trades, or says why a job of `bars` bars
    /// cannot have that many: a fill takes a bar.
    fn trades(&mut self, more: Vec<TradeFrame>, bars: usize) -> Result<(), String> {
        if self.trades.len() + more.len() > bars {
            return Err(format!("it sent more than {bars} trades"));
        }
        self.trades.extend(more.into_iter().map(TradeRow::from));
        Ok(())
    }

    /// Keeps the next run of the equity curve, or says why a job of `bars`
    /// bars cannot have that many rows: the curve has one per bar.
    fn equity(&mut self, more: Vec<EquityFrame>, bars: usize) -> Result<(), String> {
        if self.equity.len() + more.len() > bars {
            return Err(format!("it sent more than {bars} equity rows"));
        }
        self.equity.extend(more.into_iter().map(EquityRow::from));
        Ok(())
    }
}

enum Watched {
    Finished(Finished, Rows),
    Cancelled,
    OutOfTime,
    Stopped,
    Garbled(String),
}

/// Watches the worker until it says how the job ended, the job is
/// cancelled, the worker stops, or `limit` has passed: measured from now,
/// never as an instant that far ahead, which may not exist. A backtest's
/// rows are kept as they come, never more of either kind than the job has
/// bars, and what the job says goes into `log`, which bounds it: the
/// worker's count of bars is held to the job's.
fn watch(
    process: &Process,
    cancel: &Cancel,
    limit: Duration,
    bars: usize,
    log: &JobLog,
) -> Watched {
    let started = Instant::now();
    let mut rows = Rows::default();
    loop {
        let wait = limit.saturating_sub(started.elapsed()).min(POLL);
        match process.frames.recv_timeout(wait) {
            Ok(Ok(frame)) => match take(frame, &mut rows, bars, log) {
                Ok(Some(outcome)) => return Watched::Finished(outcome, rows),
                Ok(None) => {}
                Err(why) => return Watched::Garbled(why),
            },
            Ok(Err(error)) => return Watched::Garbled(error.to_string()),
            Err(RecvTimeoutError::Disconnected) => return Watched::Stopped,
            Err(RecvTimeoutError::Timeout) => {}
        }
        if cancel.is_cancelled() {
            return Watched::Cancelled;
        }
        if started.elapsed() >= limit {
            return Watched::OutOfTime;
        }
    }
}

/// Takes one frame of a running job of `bars` bars: a line or the job's
/// progress into `log`, a backtest's rows into `rows`. Answers how the job
/// ended, once a frame says, or why the frame is none a worker of this
/// build sends.
fn take(
    frame: FromWorker,
    rows: &mut Rows,
    bars: usize,
    log: &JobLog,
) -> Result<Option<Finished>, String> {
    match frame {
        FromWorker::Finished { outcome } => return Ok(Some(outcome)),
        // A level with no name could never be sent on to a client.
        FromWorker::Log {
            level: LogLevel::Unrecognized,
            ..
        } => return Err("it wrote a line at a level this build does not know".to_string()),
        FromWorker::Log {
            level,
            message,
            dropped_bytes,
        } => log.line_cut(level, &message, dropped_bytes),
        FromWorker::Progress { bars_done } => {
            log.progress(bars_done, u64::try_from(bars).unwrap_or(u64::MAX));
        }
        FromWorker::Trades { rows: more } => rows.trades(more, bars)?,
        FromWorker::Equity { rows: more } => rows.equity(more, bars)?,
        FromWorker::Ready { .. } => return Err("it said it was ready a second time".to_string()),
    }
    Ok(None)
}

/// The end of a job whose worker's output ended before it said how the
/// job did: the limit it ran into, or how it stopped.
fn stopped(process: &mut Process, limits: &WorkerLimits, job: &str) -> WorkerOutcome {
    let Some(status) = process.stopped(GRACE) else {
        return failed(
            ErrorCode::Internal,
            "the job's worker closed its output before the job finished",
        );
    };
    let said = process.said(LAST_WORDS);
    match process::why(status, &said) {
        Stop::Cpu => failed(
            ErrorCode::InvalidRequest,
            format!(
                "the job used the {} s of CPU time it may use, and was stopped",
                limits.cpu_secs
            ),
        ),
        Stop::Memory => failed(
            ErrorCode::InvalidRequest,
            format!(
                "the job needed more than the {} MiB of memory it may use, and was stopped",
                limits.memory_mib
            ),
        ),
        Stop::Crash(how) => {
            warn!(job, %status, said = said.trim(), "a worker stopped before its job finished");
            failed(
                ErrorCode::Internal,
                format!("the job's worker stopped before the job finished: {how}"),
            )
        }
    }
}

/// A backtest's three result files, written here from the rows its worker
/// sent, or why what it sent is no backtest of the job's `bars` bars.
fn backtest(metrics: Metrics, rows: Rows, bars: usize) -> Result<WorkerOutcome, String> {
    if rows.equity.len() != bars {
        return Err(format!(
            "it sent an equity curve of {} rows for a job of {bars} bars",
            rows.equity.len()
        ));
    }
    if u64::try_from(rows.trades.len()).ok() != Some(metrics.trade_count) {
        return Err(format!(
            "it sent {} trades and a summary of {}",
            rows.trades.len(),
            metrics.trade_count
        ));
    }
    let output = BacktestOutput {
        metrics,
        trades: rows.trades,
        equity: rows.equity,
    };
    Ok(match output.files() {
        Ok(files) => WorkerOutcome::Succeeded(files),
        // A table has room for more rows than any range a server runs by
        // default, so a job this long was asked for.
        Err(refusal) => failed(
            ErrorCode::InvalidRequest,
            format!("the job's result cannot be written: {refusal}"),
        ),
    })
}

/// A synthetic workload's report: each text a result file.
fn succeeded(results: Vec<ResultText>) -> WorkerOutcome {
    WorkerOutcome::Succeeded(
        results
            .into_iter()
            .map(|result| ResultFile::new(result.name, result.media_type, result.text.into_bytes()))
            .collect(),
    )
}

/// The end of a job whose worker sent what no worker of this build sends.
fn garbled(job: &str, why: &str) -> WorkerOutcome {
    warn!(job, why, "a worker sent what the server cannot read");
    failed(
        ErrorCode::Internal,
        "the job's worker sent the server something it cannot read",
    )
}

fn failed(code: ErrorCode, message: impl Into<String>) -> WorkerOutcome {
    WorkerOutcome::Failed(ErrorBody {
        code,
        message: message.into(),
        supported: None,
    })
}

#[cfg(test)]
mod tests;
