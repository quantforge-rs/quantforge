//! The workers a server keeps: as many as it runs jobs at once, each
//! started ahead of its job, so that a job seldom waits for one, and killed
//! once its job ends, so that no job ever runs where another did.

use std::collections::HashMap;
use std::fmt;
use std::io;
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use tracing::error;

use super::config::{Execution, PoolConfig, RunningWorker, WorkerLimits};
use super::frames::{self, ToWorker};
use super::isolation::Posture;
use super::job;
use super::process::{self, Worker};
use super::scratch::ScratchRoot;
use crate::protocol::bundle::Runtime;
use crate::protocol::{ErrorBody, ErrorCode};
use crate::server::jobs::{Cancel, JobInput, JobWorker, WorkerOutcome};

/// How long a worker may take to start and say it is ready.
const READY_PATIENCE: Duration = Duration::from_secs(10);

/// Why a job failed that found no worker to run on.
const NO_WORKER: &str = "the server could not start a worker for the job; submit it again";

/// Why a job failed that came as the pool was stopping.
const STOPPING: &str = "the server is stopping; submit the job again";

/// The workers of a server, which run its jobs. Cloned, it is the same
/// pool.
#[derive(Clone)]
pub struct WorkerPool {
    inner: Arc<Inner>,
}

struct Inner {
    config: PoolConfig,
    root: ScratchRoot,
    posture: Posture,
    runtime: Option<Runtime>,
    state: Mutex<State>,
    changed: Condvar,
}

#[derive(Default)]
struct State {
    idle: Vec<Worker>,
    /// Workers on their way, not yet idle.
    starting: usize,
    /// The jobs running, each with its worker's process.
    running: HashMap<String, u32>,
    stopping: bool,
}

impl WorkerPool {
    /// Starts `config.size` workers at once and waits until each is ready,
    /// or fails as soon as one cannot start. Blocks while they start.
    pub fn start(config: PoolConfig) -> io::Result<Self> {
        let config = PoolConfig {
            size: config.size.max(1),
            ..config
        };
        let root = ScratchRoot::create(&config.scratch_parent)?;
        let started: Vec<io::Result<Worker>> = std::thread::scope(|scope| {
            let starting: Vec<_> = (0..config.size)
                .map(|_| scope.spawn(|| start_worker(&config, &root)))
                .collect();
            starting
                .into_iter()
                .map(|start| {
                    start
                        .join()
                        .unwrap_or_else(|_| Err(io::Error::other("a worker's start panicked")))
                })
                .collect()
        });
        let mut idle = Vec::with_capacity(config.size);
        for worker in started {
            match worker {
                Ok(worker) => idle.push(worker),
                Err(failure) => {
                    drop(idle);
                    root.remove();
                    return Err(failure);
                }
            }
        }
        let Some(first) = idle.first() else {
            root.remove();
            return Err(io::Error::other("no worker started"));
        };
        let (posture, runtime) = (first.posture.clone(), first.runtime.clone());
        Ok(Self {
            inner: Arc::new(Inner {
                config,
                root,
                posture,
                runtime,
                state: Mutex::new(State {
                    idle,
                    ..State::default()
                }),
                changed: Condvar::new(),
            }),
        })
    }

    /// What the workers enforce on themselves.
    pub fn posture(&self) -> &Posture {
        &self.inner.posture
    }

    /// The limits on each job.
    pub fn limits(&self) -> WorkerLimits {
        self.inner.config.limits
    }

    /// The directory that holds the workers' scratch directories.
    pub fn scratch_root(&self) -> &Path {
        self.inner.root.path()
    }

    /// The jobs running on workers now, by job.
    pub fn running(&self) -> Vec<RunningWorker> {
        let mut running: Vec<RunningWorker> = self
            .state()
            .running
            .iter()
            .map(|(job_id, &pid)| RunningWorker {
                job_id: job_id.clone(),
                pid,
            })
            .collect();
        running.sort_by(|a, b| a.job_id.cmp(&b.job_id));
        running
    }

    /// Runs the job on a worker of its own, which is killed once the job
    /// ends, and starts another in its place. Blocks until the job ends, or
    /// soon after `cancel` is sent.
    pub fn execute(&self, input: JobInput, cancel: &Cancel) -> Execution {
        let job = input.job_id.to_string();
        let (running, mut worker) = match self.take(&job, cancel) {
            Ok(taken) => taken,
            Err(outcome) => {
                return Execution {
                    outcome,
                    results: Vec::new(),
                    pid: None,
                };
            }
        };
        let pid = worker.process.pid;
        let ran = job::run(&mut worker, input, cancel, &self.inner.config.limits);
        worker.process.retire();
        drop(worker);
        drop(running);
        self.replenish();
        Execution {
            outcome: ran.outcome,
            results: ran.results,
            pid: Some(pid),
        }
    }

    /// Stops the idle workers and every one that starts from now on, waits
    /// until the jobs that run have ended, and removes the scratch
    /// directories. Stop those jobs first, as [`Jobs::stop`] does.
    ///
    /// [`Jobs::stop`]: crate::server::Jobs::stop
    pub fn stop(&self) {
        let idle = {
            let mut state = self.state();
            state.stopping = true;
            std::mem::take(&mut state.idle)
        };
        drop(idle);
        let mut state = self.state();
        while !state.running.is_empty() || state.starting > 0 {
            state = self
                .inner
                .changed
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
        drop(state);
        self.inner.root.remove();
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// An idle worker, or one started now when none is idle or on its way,
    /// and `job` counted as running under the lock that hands the worker
    /// out: `stop` finds every worker idle, starting, or running a job.
    fn take(&self, job: &str, cancel: &Cancel) -> Result<(Running<'_>, Worker), WorkerOutcome> {
        let deadline = Instant::now() + READY_PATIENCE;
        let mut state = self.state();
        loop {
            if cancel.is_cancelled() {
                return Err(WorkerOutcome::Cancelled);
            }
            if state.stopping {
                return Err(unavailable(STOPPING));
            }
            if let Some(worker) = state.idle.pop() {
                return Ok((self.admit(&mut state, job, &worker), worker));
            }
            if state.starting == 0 {
                state.starting += 1;
                drop(state);
                let started = start_worker(&self.inner.config, &self.inner.root);
                let mut state = self.state();
                state.starting -= 1;
                let taken = match started {
                    Ok(worker) if !state.stopping => {
                        Ok((self.admit(&mut state, job, &worker), worker))
                    }
                    // Retired under the lock, so that `stop` cannot finish
                    // while this worker still runs.
                    Ok(mut worker) => {
                        worker.process.retire();
                        Err(unavailable(STOPPING))
                    }
                    Err(failure) => {
                        error!(%failure, "a worker could not start");
                        Err(unavailable(NO_WORKER))
                    }
                };
                drop(state);
                self.inner.changed.notify_all();
                return taken;
            }
            if Instant::now() >= deadline {
                return Err(unavailable(NO_WORKER));
            }
            state = self
                .inner
                .changed
                .wait_timeout(state, Duration::from_millis(50))
                .map_or_else(|poisoned| poisoned.into_inner().0, |(state, _)| state);
        }
    }

    /// Counts `job` as running on `worker`, under the lock `state` holds.
    fn admit(&self, state: &mut State, job: &str, worker: &Worker) -> Running<'_> {
        state.running.insert(job.to_string(), worker.process.pid);
        Running {
            pool: self,
            job: job.to_string(),
        }
    }

    /// Starts a worker in the background when the pool has fewer than it
    /// keeps.
    fn replenish(&self) {
        {
            let mut state = self.state();
            let alive = state.idle.len() + state.starting + state.running.len();
            if state.stopping || alive >= self.inner.config.size {
                return;
            }
            state.starting += 1;
        }
        let pool = self.clone();
        let started = std::thread::Builder::new()
            .name("worker-start".to_string())
            .spawn(move || {
                let worker = start_worker(&pool.inner.config, &pool.inner.root);
                let mut state = pool.state();
                state.starting -= 1;
                let unwanted = match worker {
                    Ok(worker) if !state.stopping => {
                        state.idle.push(worker);
                        None
                    }
                    Ok(worker) => Some(worker),
                    Err(failure) => {
                        error!(%failure, "a worker could not start; the next job starts one");
                        None
                    }
                };
                drop(state);
                drop(unwanted);
                pool.inner.changed.notify_all();
            });
        if started.is_err() {
            self.state().starting -= 1;
            self.inner.changed.notify_all();
        }
    }
}

/// A job counted as running until this is dropped: by its run once the job
/// has ended, or as a run that panicked unwinds, so that `stop` never waits
/// for a job that has ended.
struct Running<'a> {
    pool: &'a WorkerPool,
    job: String,
}

impl Drop for Running<'_> {
    fn drop(&mut self) {
        self.pool.state().running.remove(&self.job);
        self.pool.inner.changed.notify_all();
    }
}

fn start_worker(config: &PoolConfig, root: &ScratchRoot) -> io::Result<Worker> {
    let setup = ToWorker::Setup {
        build: frames::build(),
        mode: config.mode.clone(),
        cpu_secs: config.limits.cpu_secs,
        memory_bytes: config.limits.memory_mib.saturating_mul(1 << 20),
    };
    process::start(&config.program, &setup, root.new_dir()?, READY_PATIENCE)
}

fn unavailable(message: &str) -> WorkerOutcome {
    WorkerOutcome::Failed(ErrorBody {
        code: ErrorCode::Unavailable,
        message: message.to_string(),
        supported: None,
    })
}

impl JobWorker for WorkerPool {
    fn runtime(&self) -> Option<Runtime> {
        self.inner.runtime.clone()
    }

    fn run(&self, input: JobInput, cancel: &Cancel) -> WorkerOutcome {
        self.execute(input, cancel).outcome
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        let state = self.state.get_mut().unwrap_or_else(PoisonError::into_inner);
        drop(std::mem::take(&mut state.idle));
        self.root.remove();
    }
}

impl fmt::Debug for WorkerPool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WorkerPool")
            .field("size", &self.inner.config.size)
            .field("limits", &self.inner.config.limits)
            .field("posture", &self.inner.posture)
            .finish_non_exhaustive()
    }
}
