//! The bridge's runtime: a small multi-thread tokio runtime the store owns,
//! and the one way synchronous code waits on it.
//!
//! [`Runner::run`] spawns a future onto the bridge and blocks the calling
//! thread on a `std::sync::mpsc` reply. It never calls `block_on` on the
//! caller's thread, which panics on any thread that drives a runtime, so it
//! works from an async CLI handler, a current-thread `#[tokio::test]`, a
//! `spawn_blocking` thread, a Python thread, or a thread no runtime knows.
//! The future runs on the bridge's own workers, so a caller that blocks a
//! current-thread runtime cannot starve it.

use std::fmt;
use std::future::Future;
use std::io;
use std::panic;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

use tokio::runtime::{Builder, Handle, Runtime};

/// Why [`Runner::run`] could not wait for an answer.
#[derive(Debug)]
pub enum RunError {
    /// Called from one of the bridge's own threads, which would wait on
    /// itself: with every worker blocked like this one, nothing would run.
    Reentrant,
    /// Called from another process than the one that built the bridge, such
    /// as a forked child, where the bridge's threads do not exist.
    ForeignProcess { built_by: u32, called_from: u32 },
    /// No answer within the watchdog's time, so the bridge is wedged. The
    /// work itself carries a deadline; this only bounds the waiting.
    Watchdog(Duration),
    /// The bridge shut down, or cancelled the work, before answering.
    ShutDown,
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Reentrant => f.write_str(
                "a storage call was made from the storage bridge's own thread, which would wait \
                 on itself",
            ),
            Self::ForeignProcess {
                built_by,
                called_from,
            } => write!(
                f,
                "the storage bridge was built by process {built_by} and cannot serve process \
                 {called_from}; build a store in each process"
            ),
            Self::Watchdog(waited) => write!(
                f,
                "the storage bridge did not answer within {} ms",
                waited.as_millis()
            ),
            Self::ShutDown => f.write_str("the storage bridge shut down before answering"),
        }
    }
}

impl std::error::Error for RunError {}

/// A runtime of its own, and the process that built it.
#[derive(Debug)]
pub struct Runner {
    // An `Option` only so that `Drop` can take it out: a runtime dropped in
    // place inside async code panics, so it is shut down in the background.
    runtime: Option<Runtime>,
    handle: Handle,
    built_by: u32,
}

impl Runner {
    /// Starts `workers` named threads; `enable_all` gives the work timers
    /// and sockets.
    pub fn new(name: &str, workers: usize) -> io::Result<Self> {
        let runtime = Builder::new_multi_thread()
            .worker_threads(workers)
            .thread_name(name)
            .enable_all()
            .build()?;
        Ok(Self {
            handle: runtime.handle().clone(),
            runtime: Some(runtime),
            built_by: std::process::id(),
        })
    }

    /// The bridge's runtime, for work that must live on it.
    pub fn handle(&self) -> &Handle {
        &self.handle
    }

    /// Runs `future` on the bridge and waits for its output, at most
    /// `watchdog`. A panic in `future` is raised again on the caller.
    pub fn run<F>(&self, future: F, watchdog: Duration) -> Result<F::Output, RunError>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let called_from = std::process::id();
        if called_from != self.built_by {
            return Err(RunError::ForeignProcess {
                built_by: self.built_by,
                called_from,
            });
        }
        if Handle::try_current().is_ok_and(|current| current.id() == self.handle.id()) {
            return Err(RunError::Reentrant);
        }

        let (reply, answer) = mpsc::sync_channel(1);
        let task = self.handle.spawn(future);
        // A second task awaits the first, so that a panic arrives here as a
        // `JoinError` instead of silently dropping the reply.
        self.handle.spawn(async move {
            let _ = reply.send(task.await);
        });
        match answer.recv_timeout(watchdog) {
            Ok(Ok(output)) => Ok(output),
            Ok(Err(error)) if error.is_panic() => panic::resume_unwind(error.into_panic()),
            Ok(Err(_)) | Err(RecvTimeoutError::Disconnected) => Err(RunError::ShutDown),
            Err(RecvTimeoutError::Timeout) => Err(RunError::Watchdog(watchdog)),
        }
    }
}

impl Drop for Runner {
    fn drop(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_forked_child_is_refused_instead_of_waiting_forever() {
        let mut runner = Runner::new("qf-runner-test", 1).expect("a runner");
        // A forked child has another pid and none of the bridge's threads.
        runner.built_by = std::process::id().wrapping_add(1);
        assert!(matches!(
            runner.run(async { 1 }, Duration::from_secs(5)),
            Err(RunError::ForeignProcess { .. })
        ));
    }
}
