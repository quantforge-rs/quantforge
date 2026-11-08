//! `quantforge serve run` — answer HTTP requests on `--listen` until SIGINT
//! or SIGTERM (Ctrl-C on Windows). The server then stops accepting
//! connections and finishes the requests in progress; a second signal stops
//! it without waiting for them. Its history routes read the store the
//! settings selected, each call bounded by the storage route budget, with
//! as many requests at the store at once as its pool has connections.

use crate::cli::context::AppContext;
use anyhow::{Context, Result, bail};
use clap::Args;
use quantforge::server::{self, History, HistoryLimits};
use quantforge::{StorageSettings, StoreOwner};
use std::net::SocketAddr;
use std::pin::pin;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tracing::info;

#[derive(Args, Debug)]
pub(crate) struct ServeRunArgs {
    /// IP address and port to listen on; port 0 picks a free port
    #[arg(long, env = "QF_LISTEN", default_value = "127.0.0.1:9600")]
    listen: SocketAddr,

    /// Most bars one /v1/candles answer may span; a longer range is refused
    #[arg(
        long,
        env = "QF_MAX_SPAN_BARS",
        default_value_t = server::DEFAULT_MAX_SPAN_BARS,
        value_parser = clap::value_parser!(u64).range(1..)
    )]
    max_span_bars: u64,
}

pub(crate) async fn handle_serve_run(ctx: &AppContext, args: ServeRunArgs) -> Result<()> {
    // The pool holds as many connections as there are turns at the store;
    // SQLite opens one per call, and takes the server's default.
    let concurrency = match &ctx.storage {
        StorageSettings::Postgres(settings) => settings.pool_size,
        _ => StoreOwner::Server.default_pool_size(),
    };
    let budget = ctx.storage.limits().route_budget;
    let history = History::new(
        ctx.store.within(budget),
        HistoryLimits {
            max_span_bars: args.max_span_bars,
            concurrency: usize::try_from(concurrency).unwrap_or(usize::MAX),
            wait: budget,
            ..HistoryLimits::default()
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
    println!("storage: {}", ctx.storage);
    println!("listening: http://{address}");

    let (begin_shutdown, shutdown_begun) = oneshot::channel::<()>();
    let mut serving = pin!(server::serve(listener, history, async move {
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
    info!("finishing the requests in progress; a second signal stops without waiting for them");
    // The receiver lives in the server until its shutdown begins.
    let _ = begin_shutdown.send(());
    tokio::select! {
        served = &mut serving => served.context("the server failed while shutting down")?,
        again = signals.recv() => bail!(
            "{again} received while finishing requests in progress; stopped without waiting \
             for them"
        ),
    }
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
