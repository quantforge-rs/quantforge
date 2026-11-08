//! A worker's process: started from this binary with nothing of the
//! server's but three pipes, its frames and the end of its standard error
//! read on threads of their own, and, once it stops, why. A worker is
//! killed with its process group, so that a process it started goes too.

use std::io::{self, BufReader, Read};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use super::config::WorkerProgram;
use super::frames::{self, FromWorker, MOST_FROM_WORKER, ToWorker};
use super::isolation::Posture;
use super::scratch;
use crate::protocol::bundle::Runtime;

/// The most bytes kept of what a worker writes to its standard error: its
/// last, which say why it stopped.
const STDERR_KEPT: usize = 64 << 10;

/// What a Rust process prints as it aborts for want of memory.
#[cfg(unix)]
const ALLOCATION_FAILED: &str = "memory allocation of ";

/// A worker that is ready for its job.
#[derive(Debug)]
pub(super) struct Worker {
    pub(super) process: Process,
    pub(super) posture: Posture,
    pub(super) runtime: Option<Runtime>,
}

/// A worker's process, its pipes and its scratch directory, all of which
/// go once it is retired or dropped.
#[derive(Debug)]
pub(super) struct Process {
    child: Child,
    pub(super) pid: u32,
    /// Open until the job ends: the worker reads its end as the server's.
    pub(super) stdin: Option<ChildStdin>,
    pub(super) frames: Receiver<io::Result<FromWorker>>,
    stderr: Arc<Mutex<Vec<u8>>>,
    stderr_done: Receiver<()>,
    pub(super) scratch: PathBuf,
    retired: bool,
}

/// Starts a worker in `scratch`, sends it `setup`, and waits up to
/// `patience` for it to say it is ready.
pub(super) fn start(
    program: &WorkerProgram,
    setup: &ToWorker,
    scratch: PathBuf,
    patience: Duration,
) -> io::Result<Worker> {
    let mut command = Command::new(&program.path);
    command
        .args(&program.args)
        .env_clear()
        .current_dir(&scratch)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Its scratch directory is its temporary directory too.
    #[cfg(unix)]
    command.env("TMPDIR", &scratch);
    #[cfg(windows)]
    command.env("TMP", &scratch).env("TEMP", &scratch);
    own_group(&mut command);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            scratch::wipe(&scratch);
            return Err(error);
        }
    };
    let pid = child.id();
    let (stdin, stdout, stderr) = (child.stdin.take(), child.stdout.take(), child.stderr.take());
    let kept = Arc::new(Mutex::new(Vec::new()));
    let (stderr_ended, stderr_done) = mpsc::channel();
    let (sender, frames) = mpsc::channel();
    let mut process = Process {
        child,
        pid,
        stdin,
        frames,
        stderr: Arc::clone(&kept),
        stderr_done,
        scratch,
        retired: false,
    };
    let (Some(stdout), Some(stderr)) = (stdout, stderr) else {
        return Err(io::Error::other("the worker's pipes were not opened"));
    };
    read_frames(stdout, sender)?;
    keep_stderr(stderr, kept, stderr_ended)?;
    let Some(stdin) = process.stdin.as_mut() else {
        return Err(io::Error::other("the worker's input was not opened"));
    };
    frames::write(stdin, setup)?;
    match process.frames.recv_timeout(patience) {
        Ok(Ok(FromWorker::Ready {
            build,
            posture,
            runtime,
            ..
        })) => {
            if build != frames::build() {
                return Err(io::Error::other(format!(
                    "the worker is {build}, not {}",
                    frames::build()
                )));
            }
            Ok(Worker {
                process,
                posture,
                runtime,
            })
        }
        Ok(Ok(FromWorker::Finished { .. })) => Err(io::Error::other(
            "the worker said a job finished before it was ready",
        )),
        Ok(Err(error)) => Err(error),
        Err(RecvTimeoutError::Timeout) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!("the worker was not ready within {} s", patience.as_secs()),
        )),
        Err(RecvTimeoutError::Disconnected) => {
            let status = process.stopped(Duration::from_secs(1));
            let said = process.said(Duration::from_millis(500));
            Err(io::Error::other(format!(
                "the worker stopped as it started ({}): {}",
                status.map_or_else(|| "still running".to_string(), |status| status.to_string()),
                said.trim()
            )))
        }
    }
}

/// Its own process group: a signal for the server's, such as the Ctrl-C a
/// terminal sends, never reaches a worker, and a worker is killed with
/// every process it started.
fn own_group(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(CREATE_NEW_PROCESS_GROUP);
    }
}

/// Reads the worker's frames as they come, until its output ends or a
/// frame cannot be read, and sends each on.
fn read_frames(
    stdout: impl Read + Send + 'static,
    sender: mpsc::Sender<io::Result<FromWorker>>,
) -> io::Result<()> {
    std::thread::Builder::new()
        .name("worker-frames".to_string())
        .spawn(move || {
            let mut stdout = BufReader::new(stdout);
            loop {
                match frames::read(&mut stdout, MOST_FROM_WORKER) {
                    Ok(Some(frame)) => {
                        if sender.send(Ok(frame)).is_err() {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(error) => {
                        let _ = sender.send(Err(error));
                        break;
                    }
                }
            }
        })
        .map(drop)
}

/// Keeps the last `STDERR_KEPT` bytes of the worker's standard error, and
/// says when it ends.
fn keep_stderr(
    mut stderr: impl Read + Send + 'static,
    kept: Arc<Mutex<Vec<u8>>>,
    ended: mpsc::Sender<()>,
) -> io::Result<()> {
    std::thread::Builder::new()
        .name("worker-stderr".to_string())
        .spawn(move || {
            let mut chunk = [0_u8; 8192];
            loop {
                match stderr.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(read) => {
                        let mut kept = kept.lock().unwrap_or_else(PoisonError::into_inner);
                        kept.extend_from_slice(&chunk[..read]);
                        let excess = kept.len().saturating_sub(STDERR_KEPT);
                        kept.drain(..excess);
                    }
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
            }
            let _ = ended.send(());
        })
        .map(drop)
}

impl Process {
    /// Kills it, and every process in its group.
    pub(super) fn kill(&mut self) {
        #[cfg(unix)]
        if let Some(group) = i32::try_from(self.pid)
            .ok()
            .and_then(rustix::process::Pid::from_raw)
        {
            let _ = rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
        }
        let _ = self.child.kill();
    }

    /// How it stopped, once it has, waiting up to `patience`; `None` if it
    /// still runs.
    pub(super) fn stopped(&mut self, patience: Duration) -> Option<ExitStatus> {
        let deadline = Instant::now() + patience;
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => return Some(status),
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                _ => return None,
            }
        }
    }

    /// The end of what it wrote to its standard error, once that has ended
    /// or `patience` has passed.
    pub(super) fn said(&self, patience: Duration) -> String {
        let _ = self.stderr_done.recv_timeout(patience);
        self.stderr_tail()
    }

    fn stderr_tail(&self) -> String {
        let kept = self.stderr.lock().unwrap_or_else(PoisonError::into_inner);
        String::from_utf8_lossy(&kept).into_owned()
    }

    /// Kills it, waits for it, and removes its scratch directory.
    pub(super) fn retire(&mut self) {
        if self.retired {
            return;
        }
        self.retired = true;
        self.kill();
        let _ = self.child.wait();
        self.stdin = None;
        scratch::wipe(&self.scratch);
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        self.retire();
    }
}

/// Why a worker stopped before it finished its job, from how it stopped and
/// the end of what it wrote to its standard error.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Stop {
    /// Its CPU time ran out.
    #[cfg_attr(not(unix), allow(dead_code))]
    Cpu,
    /// An allocation failed at its memory limit.
    #[cfg_attr(not(unix), allow(dead_code))]
    Memory,
    /// Anything else, said as a clause.
    Crash(String),
}

pub(super) fn why(status: ExitStatus, said: &str) -> Stop {
    #[cfg(unix)]
    {
        use rustix::process::Signal;
        use std::os::unix::process::ExitStatusExt;

        if let Some(signal) = status.signal() {
            if signal == Signal::XCPU.as_raw() {
                return Stop::Cpu;
            }
            if signal == Signal::ABORT.as_raw() && said.contains(ALLOCATION_FAILED) {
                return Stop::Memory;
            }
            return Stop::Crash(format!(
                "it was stopped by signal {signal}{}",
                signal_name(signal)
            ));
        }
    }
    #[cfg(not(unix))]
    let _ = said;
    match status.code() {
        Some(code) => Stop::Crash(format!("it exited with status {code}")),
        None => Stop::Crash("it stopped".to_string()),
    }
}

#[cfg(unix)]
fn signal_name(signal: i32) -> &'static str {
    use rustix::process::Signal;

    [
        (Signal::KILL, " (SIGKILL)"),
        (Signal::SEGV, " (SIGSEGV)"),
        (Signal::BUS, " (SIGBUS)"),
        (Signal::ILL, " (SIGILL)"),
        (Signal::FPE, " (SIGFPE)"),
        (Signal::ABORT, " (SIGABRT)"),
        (Signal::TERM, " (SIGTERM)"),
        (Signal::SYS, " (SIGSYS)"),
        (Signal::XFSZ, " (SIGXFSZ)"),
    ]
    .into_iter()
    .find_map(|(known, name)| (known.as_raw() == signal).then_some(name))
    .unwrap_or("")
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;

    fn signalled(signal: rustix::process::Signal) -> ExitStatus {
        ExitStatus::from_raw(signal.as_raw())
    }

    #[test]
    fn how_a_worker_stopped_names_the_limit_it_ran_into() {
        use rustix::process::Signal;
        assert_eq!(why(signalled(Signal::XCPU), ""), Stop::Cpu);
        assert_eq!(
            why(
                signalled(Signal::ABORT),
                "memory allocation of 16777216 bytes failed\n"
            ),
            Stop::Memory
        );
        assert_eq!(
            why(signalled(Signal::ABORT), "thread 'main' panicked"),
            Stop::Crash("it was stopped by signal 6 (SIGABRT)".to_string())
        );
        assert_eq!(
            why(signalled(Signal::KILL), ""),
            Stop::Crash("it was stopped by signal 9 (SIGKILL)".to_string())
        );
        assert_eq!(
            why(ExitStatus::from_raw(3 << 8), ""),
            Stop::Crash("it exited with status 3".to_string())
        );
    }
}
