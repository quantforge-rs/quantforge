//! The server other QuantForge instances connect to: the HTTP routes it
//! answers, and how it stops. `quantforge serve run` opens the store the
//! settings select, binds the address, handles the shutdown signals, and
//! hands the listener and the store's [`History`] to [`serve`].
//!
//! | Route | Answer |
//! | --- | --- |
//! | `GET /healthz` | `200`, the plain text `ok`, while the server runs |
//! | `GET /version` | `200`, a [`ServerVersion`]: this release and the protocol versions it speaks |
//! | `GET /v1/markets` | `200`, the markets the store holds candles of, and the open times each spans |
//! | `GET /v1/candles` | `200`, one market's bars in a range: the segments that hold them, and the tip after them |
//! | `GET /v1/segments/{hash}` | `200`, a segment's stored bytes, which any cache may keep for a year; or `304` |
//!
//! [`protocol::segments`](crate::protocol::segments) specifies the three
//! history routes: their parameters, their answers and their refusals. The
//! first two paths name no protocol version: a client asks them before it
//! knows which version to send its requests under (see
//! [`protocol`](crate::protocol)). `HEAD` works wherever `GET` does. Any
//! other path is answered `404 not_found`, a path under a version the server
//! does not speak `400 unsupported_version`, and a known path asked with
//! another method `405 method_not_allowed`, each as the protocol's error
//! envelope.
//!
//! The history routes read the store through the storage ports, as a
//! [`HistoryStore`], and never open one, so the server reads the store the
//! operator selected and nothing else. A request waits for one of
//! [`HistoryLimits::concurrency`] turns at the store, as many as its pool
//! has connections, and makes its store calls on a blocking thread, so a
//! slow store never stalls the server's other requests; one that waits
//! longer than [`HistoryLimits::wait`] is answered `503` with `Retry-After`.
//!
//! - `history`: the markets and candles routes, and the turns at the store.
//! - `selection`: a candles query read into a market and a range, before
//!   the store is asked anything.
//! - `segment`: the segment route, its caching headers, and the check of
//!   the bytes it sends.
//! - `refusal`: every failed answer, and what a storage failure tells a
//!   client.

mod history;
mod refusal;
mod segment;
mod selection;
#[cfg(test)]
mod tests;

use std::fmt::Debug;
use std::io;

use axum::routing::get;
use axum::serve::Listener;
use axum::{Json, Router};
use tokio::net::TcpListener;

use crate::protocol::{MIN_SUPPORTED, PROTO_VERSION, ProtoVersion, ServerVersion};

pub use history::{DEFAULT_MAX_SPAN_BARS, History, HistoryLimits, HistoryStore};

/// Answers every route on `listener` until `shutdown` resolves, the history
/// routes from `history`. Then it stops accepting connections at once,
/// closes the idle ones, and returns once every request already in progress
/// has been answered, however long that takes. A caller that cannot wait
/// drops this future and ends the runtime, which cuts those requests off.
pub async fn serve<F>(listener: TcpListener, history: History, shutdown: F) -> io::Result<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    serve_on(listener, router(history), shutdown).await
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

/// Every route, the history ones under each protocol version the server
/// speaks.
fn router(history: History) -> Router {
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
            );
    }
    router
        .fallback(refusal::no_route)
        .method_not_allowed_fallback(refusal::wrong_method)
        .with_state(history)
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
