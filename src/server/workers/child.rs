//! The worker's side: the process this binary becomes when a server starts
//! it with the `worker` command. It reads its setup, puts its limits and
//! sandbox on itself before anything else runs, says it is ready, runs the
//! one job it is handed, says how it ended, and exits. Once it holds its
//! job, the end of its input means that the server has gone or given up on
//! the job, and it exits at once.

use std::io::{self, BufReader, BufWriter, Read, Write};

use super::config::WorkerMode;
use super::frames::{self, Finished, FromWorker, MOST_TO_WORKER, ToWorker};
use super::isolation::{self, Limits};
use super::synthetic::{self, Seen};
use crate::model::Candle;
use crate::protocol::stream::Bar;
use crate::protocol::{ErrorBody, ErrorCode};
use crate::server::jobs::NO_RUNTIME;

/// Why a worker stopped without running a job to its end.
#[derive(Debug, thiserror::Error)]
pub enum WorkerError {
    /// The frames to or from the server failed.
    #[error("the server's frames failed: {0}")]
    Frames(#[from] io::Error),
    /// The server is of another build.
    #[error(
        "the server is {server}, and this worker {worker}; a worker runs only for a server of its own build"
    )]
    Build {
        /// The server's build.
        server: String,
        /// This worker's.
        worker: String,
    },
    /// The server sent a frame out of turn.
    #[error("the server sent a {got} frame where the worker expected {expected}")]
    OutOfTurn {
        /// The frame that came.
        got: &'static str,
        /// The frame the worker waited for.
        expected: &'static str,
    },
}

/// Runs this process as a job's worker, reading frames from `input`, its
/// standard input, and writing them to `output`, its standard output, which
/// nothing else may write to. Returns once it has said how its job ended,
/// or when the server goes away before handing it one.
pub fn run_worker<R, W>(input: R, output: W) -> Result<(), WorkerError>
where
    R: Read + Send + 'static,
    W: Write,
{
    let mut input = BufReader::new(input);
    let mut output = BufWriter::new(output);
    let Some(setup) = frames::read::<_, ToWorker>(&mut input, MOST_TO_WORKER)? else {
        return Ok(());
    };
    let ToWorker::Setup {
        build,
        mode,
        cpu_secs,
        memory_bytes,
    } = setup
    else {
        return Err(out_of_turn(&setup, "setup"));
    };
    if build != frames::build() {
        return Err(WorkerError::Build {
            server: build,
            worker: frames::build(),
        });
    }
    let scratch = std::env::current_dir()?;
    let seen = matches!(mode, WorkerMode::Synthetic { .. }).then(|| Seen::now(&scratch));
    let posture = isolation::isolate(
        Limits {
            cpu_secs,
            memory_bytes,
        },
        &scratch,
    );
    frames::write(
        &mut output,
        &FromWorker::Ready {
            build: frames::build(),
            pid: std::process::id(),
            posture: posture.clone(),
            runtime: mode.runtime(),
        },
    )?;

    let Some(job) = frames::read::<_, ToWorker>(&mut input, MOST_TO_WORKER)? else {
        return Ok(());
    };
    let ToWorker::Job(job) = job else {
        return Err(out_of_turn(&job, "job"));
    };
    let candles = read_candles(&mut input, job.bars)?;
    watch(input)?;
    let outcome = match (&mode, &seen) {
        (WorkerMode::Synthetic { .. }, Some(seen)) => {
            synthetic::run(&job, &candles, seen, &posture)
        }
        _ => Finished::Failed {
            error: ErrorBody {
                code: ErrorCode::Unavailable,
                message: NO_RUNTIME.to_string(),
                supported: None,
            },
        },
    };
    frames::write(&mut output, &FromWorker::Finished { outcome })?;
    Ok(())
}

/// The job's candles, from the `candles` frames that follow it.
fn read_candles<R: Read>(input: &mut R, bars: u64) -> Result<Vec<Candle>, WorkerError> {
    let expected = usize::try_from(bars).unwrap_or(usize::MAX);
    let mut candles = Vec::with_capacity(expected.min(1 << 20));
    while candles.len() < expected {
        let frame = frames::read::<_, ToWorker>(input, MOST_TO_WORKER)?
            .ok_or(io::Error::from(io::ErrorKind::UnexpectedEof))?;
        let ToWorker::Candles { bars } = frame else {
            return Err(out_of_turn(&frame, "candles"));
        };
        candles.extend(bars.into_iter().map(candle));
    }
    if candles.len() > expected {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("the server sent {} bars, not {expected}", candles.len()),
        )
        .into());
    }
    Ok(candles)
}

fn candle(bar: Bar) -> Candle {
    Candle {
        open_time_ms: bar.open_time_ms,
        close_time_ms: bar.close_time_ms,
        open: bar.open.get(),
        high: bar.high.get(),
        low: bar.low.get(),
        close: bar.close.get(),
        volume: bar.volume.get(),
        trades: bar.trades,
    }
}

/// Ends the process once `input` ends: the server closes it only when it
/// has gone, or has given up on the job.
fn watch<R: Read + Send + 'static>(mut input: R) -> io::Result<()> {
    std::thread::Builder::new()
        .name("server-watch".to_string())
        .spawn(move || {
            let mut ignored = [0_u8; 512];
            loop {
                match input.read(&mut ignored) {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
            }
            std::process::exit(0);
        })
        .map(drop)
}

fn out_of_turn(frame: &ToWorker, expected: &'static str) -> WorkerError {
    WorkerError::OutOfTurn {
        got: match frame {
            ToWorker::Setup { .. } => "setup",
            ToWorker::Job(_) => "job",
            ToWorker::Candles { .. } => "candles",
        },
        expected,
    }
}
