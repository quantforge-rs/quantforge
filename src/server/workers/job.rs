//! One job's run on a worker: the bundle written into the worker's scratch
//! directory, the job's frames sent on a thread of their own, so that a
//! worker that stops reading never holds the server up, and the worker
//! watched until it says how the job ended, its wall-clock time runs out,
//! the job is cancelled, or it stops.

use std::io;
use std::process::ChildStdin;
use std::sync::mpsc::RecvTimeoutError;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use tracing::{error, warn};

use super::config::{ResultFile, WorkerLimits};
use super::frames::{self, BARS_PER_FRAME, Finished, FromWorker, JobFrame, ResultText, ToWorker};
use super::process::{self, Process, Stop, Worker};
use super::scratch;
use crate::protocol::jobs::ResultRef;
use crate::protocol::{ContentHash, ErrorBody, ErrorCode};
use crate::server::history::bar;
use crate::server::jobs::{Cancel, JobInput, WorkerOutcome};

/// How often a run looks at its cancel and its clock while it waits.
const POLL: Duration = Duration::from_millis(50);

/// How long a worker whose output has ended may take to stop by itself.
const GRACE: Duration = Duration::from_secs(1);

/// How long the end of a stopped worker's standard error may take to come.
const LAST_WORDS: Duration = Duration::from_millis(500);

/// How a run ended, and what it produced.
pub(super) struct Ran {
    pub(super) outcome: WorkerOutcome,
    pub(super) results: Vec<ResultFile>,
}

/// Runs the job on `worker`, which is killed by the time this returns.
pub(super) fn run(
    worker: &mut Worker,
    input: JobInput,
    cancel: &Cancel,
    limits: &WorkerLimits,
) -> Ran {
    let job = input.job_id.to_string();
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
    let ran = match watch(process, cancel, Duration::from_secs(limits.wall_secs)) {
        Watched::Finished(Finished::Succeeded { results }) => succeeded(results),
        Watched::Finished(Finished::Failed { error }) => Ran {
            outcome: WorkerOutcome::Failed(error),
            results: Vec::new(),
        },
        Watched::Cancelled => Ran {
            outcome: WorkerOutcome::Cancelled,
            results: Vec::new(),
        },
        Watched::OutOfTime => failed(
            ErrorCode::InvalidRequest,
            format!(
                "the job ran for the {} s of wall-clock time it may run, and was stopped",
                limits.wall_secs
            ),
        ),
        Watched::Garbled(why) => {
            warn!(job, why, "a worker sent what the server cannot read");
            failed(
                ErrorCode::Internal,
                "the job's worker sent the server something it cannot read",
            )
        }
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

enum Watched {
    Finished(Finished),
    Cancelled,
    OutOfTime,
    Stopped,
    Garbled(String),
}

/// Watches the worker until it says how the job ended, the job is
/// cancelled, the worker stops, or `limit` has passed: measured from now,
/// never as an instant that far ahead, which may not exist.
fn watch(process: &Process, cancel: &Cancel, limit: Duration) -> Watched {
    let started = Instant::now();
    loop {
        let wait = limit.saturating_sub(started.elapsed()).min(POLL);
        match process.frames.recv_timeout(wait) {
            Ok(Ok(FromWorker::Finished { outcome })) => return Watched::Finished(outcome),
            Ok(Ok(FromWorker::Ready { .. })) => {
                return Watched::Garbled("it said it was ready a second time".to_string());
            }
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

/// The end of a job whose worker's output ended before it said how the
/// job did: the limit it ran into, or how it stopped.
fn stopped(process: &mut Process, limits: &WorkerLimits, job: &str) -> Ran {
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

fn succeeded(results: Vec<ResultText>) -> Ran {
    let files: Vec<ResultFile> = results
        .into_iter()
        .map(|result| {
            let bytes = result.text.into_bytes();
            let digest: [u8; 32] = Sha256::digest(&bytes).into();
            ResultFile {
                reference: ResultRef {
                    name: result.name,
                    media_type: result.media_type,
                    hash: ContentHash::sha256(digest),
                    size_bytes: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
                },
                bytes,
            }
        })
        .collect();
    Ran {
        outcome: WorkerOutcome::Succeeded(
            files.iter().map(|file| file.reference.clone()).collect(),
        ),
        results: files,
    }
}

fn failed(code: ErrorCode, message: impl Into<String>) -> Ran {
    Ran {
        outcome: WorkerOutcome::Failed(ErrorBody {
            code,
            message: message.into(),
            supported: None,
        }),
        results: Vec::new(),
    }
}
