//! `quantforge serve run` — answer HTTP requests on `--listen` until SIGINT
//! or SIGTERM (Ctrl-C on Windows). The server then stops accepting
//! connections, finishes the requests in progress and stops the jobs that
//! run, failing them so that their clients submit them again; a second
//! signal stops it without waiting. Its routes and job queue read and write
//! the store the settings selected, each call bounded by the storage route
//! budget, with as many calls at the store at once as its pool has
//! connections. Jobs run `--job-concurrency` at a time, each in a worker
//! process of its own, started ahead of it and held to `--job-cpu-secs`,
//! `--job-memory-mib` and `--job-wall-secs`; this build's workers have no
//! runtime, so each job fails as it starts.

use crate::cli::context::AppContext;
use anyhow::{Context, Result, bail};
use clap::Args;
use quantforge::server::{
    self, Guard, History, HistoryLimits, JobLimits, Jobs, PoolConfig, ServerStore, StoreLimits,
    WorkerLimits, WorkerMode, WorkerPool, WorkerProgram,
};
use quantforge::{StorageSettings, StoreOwner};
use std::net::SocketAddr;
use std::num::NonZeroUsize;
use std::pin::pin;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tracing::info;

/// Help section for the limits on each job.
const JOB_LIMITS_HEADING: &str = "Job limits";

#[derive(Args, Debug)]
pub(crate) struct ServeRunArgs {
    /// IP address and port to listen on; port 0 picks a free port
    #[arg(long, env = "QF_LISTEN", default_value = "127.0.0.1:9600")]
    listen: SocketAddr,

    /// Most bars one /v1/candles answer or backtest job may span; a longer range is refused
    #[arg(
        long,
        env = "QF_MAX_SPAN_BARS",
        default_value_t = server::DEFAULT_MAX_SPAN_BARS,
        value_parser = clap::value_parser!(u64).range(1..)
    )]
    max_span_bars: u64,

    /// Backtest jobs that run at once, the rest waiting their turn; omitted, half the CPU cores
    #[arg(
        long,
        env = "QF_JOB_CONCURRENCY",
        value_name = "COUNT",
        value_parser = clap::value_parser!(u64).range(1..)
    )]
    job_concurrency: Option<u64>,

    /// CPU time one backtest job may use, in seconds; a job that uses more is stopped
    #[arg(
        long,
        env = "QF_JOB_CPU_SECS",
        help_heading = JOB_LIMITS_HEADING,
        value_name = "SECS",
        default_value_t = server::DEFAULT_JOB_CPU_SECS,
        value_parser = clap::value_parser!(u64).range(1..)
    )]
    job_cpu_secs: u64,

    /// Memory one backtest job may use, in MiB of address space; enforced on Linux
    #[arg(
        long,
        env = "QF_JOB_MEMORY_MIB",
        help_heading = JOB_LIMITS_HEADING,
        value_name = "MIB",
        default_value_t = server::DEFAULT_JOB_MEMORY_MIB,
        value_parser = clap::value_parser!(u64).range(1..)
    )]
    job_memory_mib: u64,

    /// Wall-clock time one backtest job may run, in seconds, before it is stopped
    #[arg(
        long,
        env = "QF_JOB_WALL_SECS",
        help_heading = JOB_LIMITS_HEADING,
        value_name = "SECS",
        default_value_t = server::DEFAULT_JOB_WALL_SECS,
        value_parser = clap::value_parser!(u64).range(1..)
    )]
    job_wall_secs: u64,
}

pub(crate) async fn handle_serve_run(ctx: &AppContext, args: ServeRunArgs) -> Result<()> {
    let job_concurrency = match args.job_concurrency {
        Some(count) => usize::try_from(count).unwrap_or(usize::MAX),
        None => std::thread::available_parallelism().map_or(1, NonZeroUsize::get) / 2,
    }
    .max(1);
    // The pool holds as many connections as there are turns at the store;
    // SQLite opens one per call, and takes the server's default.
    let concurrency = match &ctx.storage {
        StorageSettings::Postgres(settings) => settings.pool_size,
        _ => StoreOwner::Server.default_pool_size(),
    };
    let budget = ctx.storage.limits().route_budget;
    let store = ServerStore::new(
        ctx.store.within(budget),
        StoreLimits {
            concurrency: usize::try_from(concurrency).unwrap_or(usize::MAX),
            wait: budget,
        },
    );
    let history = History::new(
        store.clone(),
        HistoryLimits {
            max_span_bars: args.max_span_bars,
            ..HistoryLimits::default()
        },
    );
    let limits = WorkerLimits {
        cpu_secs: args.job_cpu_secs,
        memory_mib: args.job_memory_mib,
        wall_secs: args.job_wall_secs,
    };
    // Before any worker starts: a worker of the same user must not read the
    // store's URL or credentials out of this process.
    server::protect_server().context("failed to close the server to its job workers")?;
    let program = WorkerProgram::this_binary()
        .context("failed to find this binary to start the job workers from")?;
    let pool = tokio::task::spawn_blocking(move || {
        WorkerPool::start(PoolConfig {
            program,
            size: job_concurrency,
            limits,
            mode: WorkerMode::NoRuntime,
            scratch_parent: std::env::temp_dir(),
        })
    })
    .await
    .context("failed to start the job workers")?
    .context("failed to start the job workers")?;
    let posture = pool.posture().clone();
    for (guard, held) in [
        ("cpu", &posture.cpu),
        ("memory", &posture.memory),
        ("network", &posture.network),
        ("files", &posture.files),
    ] {
        if let Guard::Unenforced { why } = held {
            info!(guard, why, "job workers run without this guard");
        }
    }
    let jobs = Jobs::new(
        store,
        pool.clone(),
        JobLimits {
            concurrency: job_concurrency,
            max_span_bars: args.max_span_bars,
        },
    );
    // Installed before the address is printed: a signal sent as soon as it
    // appears must reach this handler, not the default action, which ends
    // the process on the spot.
    let mut signals = Signals::register().context("failed to watch for shutdown signals")?;
    let listener = TcpListener::bind(args.listen)
        .await
        .with_context(|| format!("failed to listen on {}", args.listen))?;
    let address = listener
        .local_addr()
        .context("failed to read the address the server listens on")?;
    jobs.start()
        .await
        .context("failed to start the job queue")?;
    println!("storage: {}", ctx.storage);
    println!("job_concurrency: {}", jobs.concurrency());
    println!("job_limits: {limits}");
    println!("worker_isolation: {posture}");
    println!("listening: http://{address}");

    let (begin_shutdown, shutdown_begun) = oneshot::channel::<()>();
    let mut serving = pin!(server::serve(listener, history, jobs.clone(), async move {
        // A dropped sender begins the shutdown as well.
        let _ = shutdown_begun.await;
    }));
    let signal = tokio::select! {
        served = &mut serving => {
            served.context("the server failed")?;
            bail!("the server stopped before any shutdown signal");
        }
        signal = signals.recv() => signal,
    };
    println!("shutdown: {signal}");
    info!(
        "finishing the requests in progress and stopping the jobs that run; a second signal \
         stops without waiting for them"
    );
    // The receiver lives in the server until its shutdown begins.
    let _ = begin_shutdown.send(());
    tokio::select! {
        (served, stopped) = async { tokio::join!(&mut serving, jobs.stop()) } => {
            served.context("the server failed while shutting down")?;
            if stopped > 0 {
                info!(stopped, "stopped the jobs that ran; each failed, to be submitted again");
            }
        }
        again = signals.recv() => bail!(
            "{again} received while finishing requests in progress; stopped without waiting \
             for them"
        ),
    }
    // Every job has ended: the idle workers go, and the scratch directories.
    tokio::task::spawn_blocking(move || pool.stop())
        .await
        .context("failed to stop the job workers")?;
    println!("stopped: requests in progress finished");
    Ok(())
}

/// The signals that stop the server: SIGINT, which Ctrl-C sends, and
/// SIGTERM, which service managers send.
#[cfg(unix)]
struct Signals {
    interrupt: tokio::signal::unix::Signal,
    terminate: tokio::signal::unix::Signal,
}

#[cfg(unix)]
impl Signals {
    /// Installs the handlers now, not at the first wait.
    fn register() -> std::io::Result<Self> {
        use tokio::signal::unix::{SignalKind, signal};
        Ok(Self {
            interrupt: signal(SignalKind::interrupt())?,
            terminate: signal(SignalKind::terminate())?,
        })
    }

    /// The name of the next signal. A stream that has ended never answers.
    async fn recv(&mut self) -> &'static str {
        tokio::select! {
            Some(()) = self.interrupt.recv() => "SIGINT",
            Some(()) = self.terminate.recv() => "SIGTERM",
            else => std::future::pending().await,
        }
    }
}

/// The signal that stops the server on Windows, which has no SIGTERM:
/// Ctrl-C.
#[cfg(windows)]
struct Signals {
    ctrl_c: tokio::signal::windows::CtrlC,
}

#[cfg(windows)]
impl Signals {
    /// Installs the handler now, not at the first wait.
    fn register() -> std::io::Result<Self> {
        Ok(Self {
            ctrl_c: tokio::signal::windows::ctrl_c()?,
        })
    }

    /// The name of the next signal. A stream that has ended never answers.
    async fn recv(&mut self) -> &'static str {
        match self.ctrl_c.recv().await {
            Some(()) => "Ctrl-C",
            None => std::future::pending().await,
        }
    }
}
