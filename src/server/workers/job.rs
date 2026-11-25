//! One job's run on a worker: the bundle written into the worker's scratch
//! directory, the job's frames sent on a thread of their own, so that a
//! worker that stops reading never holds the server up, and the worker
//! watched until it says how the job ended, its wall-clock time runs out,
//! the job is cancelled, or it stops. What the job says while it runs, the
//! lines of its log and its progress, goes into the job's log as it comes,
//! which keeps no more of it than its limits. A backtest's rows are read
//! as they come, each checked, and written as the job's result files here:
//! a worker runs a job's code, so no file is ever taken from one, and
//! nothing it says is trusted: its rows are held to the job's bars and
//! symbol, its failure's words are cut short, and a frame no worker of
//! this build sends for such a job ends the job.

use std::io;
use std::process::ChildStdin;
use std::sync::mpsc::RecvTimeoutError;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use tracing::{error, warn};

use super::config::{PoolConfig, WorkerLimits, WorkerMode};
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
use crate::server::jobs::{Cancel, JobInput, JobLog, WorkerOutcome, start_of};
use crate::server::results::{BacktestOutput, ResultFile};

/// How often a run looks at its cancel and its clock while it waits.
const POLL: Duration = Duration::from_millis(50);

/// How long a worker whose output has ended may take to stop by itself.
const GRACE: Duration = Duration::from_secs(1);

/// How long the end of a stopped worker's standard error may take to come.
const LAST_WORDS: Duration = Duration::from_millis(500);

/// The most bytes a job's failure keeps of the words its own code chose.
const FAILURE_BYTES: usize = 8 << 10;

/// Runs the job on `worker`, one of a pool started with `config`, which is
/// killed by the time this returns, writing what the job says into `log`.
pub(super) fn run(
    worker: &mut Worker,
    input: JobInput,
    cancel: &Cancel,
    config: &PoolConfig,
    log: &JobLog,
) -> WorkerOutcome {
    let job = input.job_id.to_string();
    let limits = &config.limits;
    let rows = Rows::of(input.candles.len(), &input.submission.data.symbol);
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
    let ran = match watch(process, cancel, wall, rows, log) {
        Watched::Finished(Finished::Backtest { metrics }, rows) => {
            backtest(metrics, rows).unwrap_or_else(|why| garbled(&job, &why))
        }
        Watched::Finished(Finished::Succeeded { results }, rows) => {
            report(&config.mode, results, &rows).unwrap_or_else(|why| garbled(&job, why))
        }
        Watched::Finished(Finished::Failed { error }, _) => {
            said(error).unwrap_or_else(|why| garbled(&job, &why))
        }
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
    let entry = manifest.entry();
    let job = JobFrame {
        job_id: input.job_id.to_string(),
        submission: input.submission,
        entry: entry.to_string(),
        module: entry.module().to_string(),
        class: entry.class().to_string(),
        params: manifest.params().as_map().clone(),
        deps: manifest
            .deps()
            .iter()
            .map(|(name, version)| (name.to_string(), version.to_string()))
            .collect(),
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

/// The rows a backtest sent before it finished, held to its job of `bars`
/// bars on `symbol`: a row is a few numbers and that symbol, so a worker
/// makes its server hold no more than the job's bars, whatever it sends.
struct Rows {
    bars: usize,
    symbol: String,
    trades: Vec<TradeRow>,
    equity: Vec<EquityRow>,
}

impl Rows {
    fn of(bars: usize, symbol: &str) -> Self {
        Self {
            bars,
            symbol: symbol.to_string(),
            trades: Vec::new(),
            equity: Vec::new(),
        }
    }

    fn is_empty(&self) -> bool {
        self.trades.is_empty() && self.equity.is_empty()
    }

    /// Keeps the next run of trades, or says why the job cannot have them:
    /// a fill takes a bar, and the job trades one symbol. The symbol a
    /// refused row names is never said: it is as long as its sender likes.
    fn trades(&mut self, more: Vec<TradeFrame>) -> Result<(), String> {
        if self.trades.len() + more.len() > self.bars {
            return Err(format!("it sent more than {} trades", self.bars));
        }
        let more: Vec<TradeRow> = more.into_iter().map(TradeRow::from).collect();
        if more.iter().any(|trade| trade.symbol != self.symbol) {
            return Err(format!(
                "it sent a trade of another symbol than the job's {}",
                self.symbol
            ));
        }
        self.trades.extend(more);
        Ok(())
    }

    /// Keeps the next run of the equity curve, or says why the job cannot
    /// have that many rows: the curve has one per bar.
    fn equity(&mut self, more: Vec<EquityFrame>) -> Result<(), String> {
        if self.equity.len() + more.len() > self.bars {
            return Err(format!("it sent more than {} equity rows", self.bars));
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
/// rows are kept in `rows` as they come, which holds them to the job, and
/// what the job says goes into `log`, which bounds it: the worker's count
/// of bars is held to the job's.
fn watch(
    process: &Process,
    cancel: &Cancel,
    limit: Duration,
    mut rows: Rows,
    log: &JobLog,
) -> Watched {
    let started = Instant::now();
    loop {
        let wait = limit.saturating_sub(started.elapsed()).min(POLL);
        match process.frames.recv_timeout(wait) {
            Ok(Ok(frame)) => match take(frame, &mut rows, log) {
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

/// Takes one frame of a running job: a line or the job's progress into
/// `log`, a backtest's rows into `rows`. Answers how the job ended, once a
/// frame says, or why the frame is none a worker of this build sends.
fn take(frame: FromWorker, rows: &mut Rows, log: &JobLog) -> Result<Option<Finished>, String> {
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
            log.progress(bars_done, u64::try_from(rows.bars).unwrap_or(u64::MAX));
        }
        FromWorker::Trades { rows: more } => rows.trades(more)?,
        FromWorker::Equity { rows: more } => rows.equity(more)?,
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
/// sent, or why what it sent is no backtest of the job's bars.
fn backtest(metrics: Metrics, rows: Rows) -> Result<WorkerOutcome, String> {
    if rows.equity.len() != rows.bars {
        return Err(format!(
            "it sent an equity curve of {} rows for a job of {} bars",
            rows.equity.len(),
            rows.bars
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

/// A synthetic workload's report, each text a result file, or why the
/// worker could not have sent one: only a worker started for the tests'
/// workloads reports, and a report comes without a backtest's rows. A
/// bundle that writes its worker's frames itself names no result this way.
fn report(
    mode: &WorkerMode,
    results: Vec<ResultText>,
    rows: &Rows,
) -> Result<WorkerOutcome, &'static str> {
    if !matches!(mode, WorkerMode::Synthetic { .. }) {
        return Err("it sent a report, which only a synthetic workload does");
    }
    if !rows.is_empty() {
        return Err("it sent a backtest's rows and then a report");
    }
    Ok(WorkerOutcome::Succeeded(
        results
            .into_iter()
            .map(|result| ResultFile::new(result.name, result.media_type, result.text.into_bytes()))
            .collect(),
    ))
}

/// A failure as its worker worded it, or why no worker of this build said
/// it: a job fails as its request's fault, the worker's, or for want of a
/// runtime, and as nothing else. The message is cut to its first
/// `FAILURE_BYTES` bytes, between two characters, and says by how much.
fn said(error: ErrorBody) -> Result<WorkerOutcome, String> {
    let known = [
        ErrorCode::InvalidRequest,
        ErrorCode::Internal,
        ErrorCode::Unavailable,
    ];
    if !known.contains(&error.code) {
        return Err(format!(
            "it said the job failed as {:?}, which no job does",
            error.code
        ));
    }
    let kept = start_of(&error.message, FAILURE_BYTES);
    let message = match error.message.len() - kept.len() {
        0 => error.message,
        cut => format!("{kept}… [{cut} more bytes left out]"),
    };
    Ok(failed(error.code, message))
}

/// The end of a job whose worker sent what no worker of this build sends,
/// logged with the start of why: a frame's refusal can quote the frame.
fn garbled(job: &str, why: &str) -> WorkerOutcome {
    let why = start_of(why, 256);
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
