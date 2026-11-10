//! The server other QuantForge instances connect to: the HTTP routes it
//! answers, the job queue behind them, and how it stops. `quantforge serve
//! run` opens the store the settings select, binds the address, handles the
//! shutdown signals, and hands the listener, the store's [`History`] and its
//! [`Jobs`] to [`serve`].
//!
//! | Route | Answer |
//! | --- | --- |
//! | `GET /healthz` | `200`, the plain text `ok`, while the server runs |
//! | `GET /version` | `200`, a [`ServerVersion`]: this release and the protocol versions it speaks |
//! | `GET /v1/markets` | `200`, the markets the store holds candles of, and the open times each spans |
//! | `GET /v1/candles` | `200`, one market's bars in a range: the segments that hold them, and the tip after them |
//! | `GET /v1/segments/{hash}` | `200`, a segment's stored bytes, which any cache may keep for a year; or `304` |
//! | `PUT /v1/bundles/{code_hash}` | `201` or `200`, a bundle stored under its code hash |
//! | `POST /v1/backtests` | `201`, a backtest job, queued |
//! | `GET /v1/backtests/{job_id}` | `200`, where a job stands |
//! | `GET /v1/backtests` | `200`, the jobs, the last submitted first |
//! | `POST /v1/backtests/{job_id}/cancel` | `200`, a job, cancelled |
//!
//! [`protocol::segments`](crate::protocol::segments) specifies the three
//! history routes and [`protocol::jobs`](crate::protocol::jobs) the five
//! job routes: their parameters, their answers and their refusals. The
//! first two paths name no protocol version: a client asks them before it
//! knows which version to send its requests under (see
//! [`protocol`](crate::protocol)). `HEAD` works wherever `GET` does. Any
//! other path is answered `404 not_found`, a path under a version the server
//! does not speak `400 unsupported_version`, and a known path asked with
//! another method `405 method_not_allowed`, each as the protocol's error
//! envelope.
//!
//! The routes and the queue read and write the store through the storage
//! ports, as a [`ServedStore`] behind one [`ServerStore`], and never open
//! one, so the server uses the store the operator selected and nothing
//! else. A call waits for one of [`StoreLimits::concurrency`] turns at the
//! store, as many as its pool has connections, and runs on a blocking
//! thread, so a slow store never stalls the server's other requests; a
//! request that waits longer than [`StoreLimits::wait`] is answered `503`
//! with `Retry-After`. A job's worker never sees the store: the server reads
//! everything the job runs on and writes how it ended. `serve run` runs each
//! job in a worker process of its own, from a [`WorkerPool`], within the
//! limits of [`WorkerLimits`].
//!
//! - `store`: the store and the turns at it.
//! - `history`: the markets and candles routes.
//! - `selection`: a candles query or a job's data read into a market and a
//!   range, before the store is asked anything.
//! - `segment`: the segment route, its caching headers, and the check of
//!   the bytes it sends.
//! - `jobs`: the job routes, the queue, and what runs a job.
//! - `workers`: the processes that run jobs, one per job, and their limits.
//! - `refusal`: every failed answer, and what a storage failure tells a
//!   client.

mod history;
mod jobs;
mod refusal;
mod segment;
mod selection;
mod store;
#[cfg(test)]
mod tests;
mod workers;

use std::fmt::Debug;
use std::io;

use axum::extract::{DefaultBodyLimit, FromRef};
use axum::routing::{get, post, put};
use axum::serve::Listener;
use axum::{Json, Router};
use tokio::net::TcpListener;

use crate::protocol::bundle::BundleLimits;
use crate::protocol::{MIN_SUPPORTED, PROTO_VERSION, ProtoVersion, ServerVersion};

pub use history::{DEFAULT_MAX_SPAN_BARS, DEFAULT_MAX_TIP_BARS, History, HistoryLimits};
pub use jobs::{Cancel, JobInput, JobLimits, JobWorker, Jobs, NoRuntime, WorkerOutcome};
pub use store::{ServedStore, ServerStore, StoreLimits};
pub use workers::{
    DEFAULT_JOB_CPU_SECS, DEFAULT_JOB_MEMORY_MIB, DEFAULT_JOB_WALL_SECS, Execution, Guard,
    PoolConfig, Posture, ResultFile, RunningWorker, WORKER_COMMAND, WorkerError, WorkerLimits,
    WorkerMode, WorkerPool, WorkerProgram, protect_server, run_worker,
};

/// Answers every route on `listener` until `shutdown` resolves, the history
/// routes from `history` and the job routes from `jobs`. Then it stops
/// accepting connections at once, closes the idle ones, and returns once
/// every request already in progress has been answered, however long that
/// takes. A caller that cannot wait drops this future and ends the runtime,
/// which cuts those requests off. The jobs keep running until
/// [`Jobs::stop`].
pub async fn serve<F>(
    listener: TcpListener,
    history: History,
    jobs: Jobs,
    shutdown: F,
) -> io::Result<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    serve_on(listener, router(history, jobs), shutdown).await
}

/// [`serve`] over any listener and router, so that a test can see the
/// listener being let go and add a route that takes its time.
async fn serve_on<L, F>(listener: L, router: Router, shutdown: F) -> io::Result<()>
where
    L: Listener,
    L::Addr: Debug,
    F: Future<Output = ()> + Send + 'static,
{
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown)
        .await
}

/// What the routes read: the store, the history routes' limits, and the
/// jobs.
#[derive(Clone)]
struct ServerState {
    store: ServerStore,
    history: History,
    jobs: Jobs,
}

impl FromRef<ServerState> for ServerStore {
    fn from_ref(state: &ServerState) -> Self {
        state.store.clone()
    }
}

impl FromRef<ServerState> for History {
    fn from_ref(state: &ServerState) -> Self {
        state.history.clone()
    }
}

impl FromRef<ServerState> for Jobs {
    fn from_ref(state: &ServerState) -> Self {
        state.jobs.clone()
    }
}

/// Every route, the history and job ones under each protocol version the
/// server speaks.
fn router(history: History, jobs: Jobs) -> Router {
    // An upload larger than any bundle within the limits is refused unread.
    let upload_bytes =
        usize::try_from(BundleLimits::default().max_archive_bytes()).unwrap_or(usize::MAX);
    let mut router = Router::new()
        .route("/healthz", get(healthz))
        .route("/version", get(version));
    for number in MIN_SUPPORTED.0..=PROTO_VERSION.0 {
        let version = ProtoVersion(number).path_segment();
        router = router
            .route(&format!("/{version}/markets"), get(history::markets))
            .route(&format!("/{version}/candles"), get(history::candles))
            .route(
                &format!("/{version}/segments/{{hash}}"),
                get(segment::segment),
            )
            .route(
                &format!("/{version}/bundles/{{code_hash}}"),
                put(jobs::upload).layer(DefaultBodyLimit::max(upload_bytes)),
            )
            .route(
                &format!("/{version}/backtests"),
                get(jobs::list).post(jobs::submit),
            )
            .route(&format!("/{version}/backtests/{{job_id}}"), get(jobs::job))
            .route(
                &format!("/{version}/backtests/{{job_id}}/cancel"),
                post(jobs::cancel),
            );
    }
    let state = ServerState {
        store: history.store().clone(),
        history,
        jobs,
    };
    router
        .fallback(refusal::no_route)
        .method_not_allowed_fallback(refusal::wrong_method)
        .with_state(state)
}

async fn healthz() -> &'static str {
    "ok"
}

async fn version() -> Json<ServerVersion> {
    Json(ServerVersion {
        version: env!("CARGO_PKG_VERSION").to_string(),
        proto_version: PROTO_VERSION,
        min_supported: MIN_SUPPORTED,
    })
}
