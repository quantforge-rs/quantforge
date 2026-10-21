//! `quantforge serve run` — answer HTTP requests on `--listen` until SIGINT
//! or SIGTERM (Ctrl-C on Windows). The server then stops accepting
//! connections and finishes the requests in progress; a second signal stops
//! it without waiting for them.

use anyhow::{Context, Result, bail};
use clap::Args;
use quantforge::server;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::pin::pin;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tracing::info;

#[derive(Args, Debug)]
pub(crate) struct ServeRunArgs {
    /// IP address and port to listen on; port 0 picks a free port
    #[arg(long, env = "QF_LISTEN", default_value = "127.0.0.1:9600")]
    listen: SocketAddr,

    /// Directory for cached candle segments; created if missing
    #[arg(long, env = "QF_SEGMENT_CACHE_DIR", default_value = "data/segments")]
    segment_cache_dir: PathBuf,
}

pub(crate) async fn handle_serve_run(args: ServeRunArgs) -> Result<()> {
    let cache_dir = &args.segment_cache_dir;
    std::fs::create_dir_all(cache_dir).with_context(|| {
        format!(
            "failed to create the segment cache directory {}",
            cache_dir.display()
        )
    })?;
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
    println!("listening: http://{address}");

    let (begin_shutdown, shutdown_begun) = oneshot::channel::<()>();
    let mut serving = pin!(server::serve(listener, async move {
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
