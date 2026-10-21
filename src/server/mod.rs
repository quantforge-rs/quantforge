//! The server other QuantForge instances connect to: the HTTP routes it
//! answers, and how it stops. `quantforge serve run` binds the address,
//! handles the shutdown signals, and hands the listener to [`serve`].
//!
//! | Route | Answer |
//! | --- | --- |
//! | `GET /healthz` | `200`, the plain text `ok`, while the server runs |
//! | `GET /version` | `200`, a [`ServerVersion`]: this release and the protocol versions it speaks |
//!
//! Neither path names a protocol version: a client asks them before it knows
//! which version to send its requests under (see [`protocol`](crate::protocol)).
//! Any other request gets `404`, or `405` on a known path, with an empty
//! body. `HEAD` works wherever `GET` does.
//!
//! The server speaks [`protocol`](crate::protocol), the only other module it
//! uses.

use std::fmt::Debug;
use std::io;

use axum::routing::get;
use axum::serve::Listener;
use axum::{Json, Router};
use tokio::net::TcpListener;

use crate::protocol::{MIN_SUPPORTED, PROTO_VERSION, ServerVersion};

/// Answers every route on `listener` until `shutdown` resolves. Then it
/// stops accepting connections at once, closes the idle ones, and returns
/// once every request already in progress has been answered, however long
/// that takes. A caller that cannot wait drops this future and ends the
/// runtime, which cuts those requests off.
pub async fn serve<F>(listener: TcpListener, shutdown: F) -> io::Result<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    serve_on(listener, router(), shutdown).await
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

fn router() -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/version", get(version))
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

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::sync::Arc;
    use std::time::Duration;

    use reqwest::header::CONTENT_TYPE;
    use tokio::net::TcpStream;
    use tokio::sync::{Notify, oneshot};

    use super::*;

    /// A client that never routes loopback requests through a proxy from
    /// the environment.
    fn client() -> reqwest::Client {
        reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("an HTTP client")
    }

    /// Serves the routes on a free loopback port for the rest of the test.
    async fn start() -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("a listener");
        let address = listener.local_addr().expect("its address");
        tokio::spawn(serve(listener, std::future::pending()));
        address
    }

    /// `future`'s output, or a failed test after ten seconds, so that a
    /// server that never lets go fails the test instead of hanging it.
    async fn within<F: Future>(what: &str, future: F) -> F::Output {
        tokio::time::timeout(Duration::from_secs(10), future)
            .await
            .unwrap_or_else(|_| panic!("no {what} within ten seconds"))
    }

    fn content_type(response: &reqwest::Response) -> Option<&str> {
        response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
    }

    #[tokio::test]
    async fn healthz_answers_ok_as_plain_text() {
        let address = start().await;
        let response = client()
            .get(format!("http://{address}/healthz"))
            .send()
            .await
            .expect("a response");
        assert_eq!(response.status(), 200);
        assert_eq!(content_type(&response), Some("text/plain; charset=utf-8"));
        assert_eq!(response.text().await.expect("a body"), "ok");

        let head = client()
            .head(format!("http://{address}/healthz"))
            .send()
            .await
            .expect("a response");
        assert_eq!(head.status(), 200);
    }

    #[tokio::test]
    async fn version_answers_this_release_and_the_protocol_versions_it_speaks() {
        let address = start().await;
        let response = client()
            .get(format!("http://{address}/version"))
            .send()
            .await
            .expect("a response");
        assert_eq!(response.status(), 200);
        assert_eq!(content_type(&response), Some("application/json"));
        let expected = format!(
            r#"{{"version":"{}","proto_version":{},"min_supported":{}}}"#,
            env!("CARGO_PKG_VERSION"),
            PROTO_VERSION.0,
            MIN_SUPPORTED.0
        );
        assert_eq!(response.text().await.expect("a body"), expected);
    }

    #[tokio::test]
    async fn any_other_request_is_refused_with_an_empty_body() {
        let address = start().await;
        let unknown = client()
            .get(format!("http://{address}/v1/jobs"))
            .send()
            .await
            .expect("a response");
        assert_eq!(unknown.status(), 404);
        assert_eq!(unknown.text().await.expect("a body"), "");

        let wrong_method = client()
            .post(format!("http://{address}/healthz"))
            .send()
            .await
            .expect("a response");
        assert_eq!(wrong_method.status(), 405);
        assert_eq!(wrong_method.text().await.expect("a body"), "");
    }

    /// A listener that reports when the server lets go of it, which is when
    /// it stops accepting connections.
    struct Watched {
        inner: TcpListener,
        dropped: Option<oneshot::Sender<()>>,
    }

    impl Listener for Watched {
        type Io = TcpStream;
        type Addr = SocketAddr;

        async fn accept(&mut self) -> (TcpStream, SocketAddr) {
            Listener::accept(&mut self.inner).await
        }

        fn local_addr(&self) -> io::Result<SocketAddr> {
            Listener::local_addr(&self.inner)
        }
    }

    impl Drop for Watched {
        fn drop(&mut self) {
            if let Some(dropped) = self.dropped.take() {
                let _ = dropped.send(());
            }
        }
    }

    #[tokio::test]
    async fn a_request_in_progress_is_answered_before_the_server_returns() {
        // The slow route says when it has the request, then answers once
        // released, so the shutdown starts while the request is in progress.
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let slow = {
            let (entered, release) = (entered.clone(), release.clone());
            move || {
                let (entered, release) = (entered.clone(), release.clone());
                async move {
                    entered.notify_one();
                    release.notified().await;
                    "done"
                }
            }
        };
        let inner = TcpListener::bind("127.0.0.1:0").await.expect("a listener");
        let address = inner.local_addr().expect("its address");
        let (dropped, listener_dropped) = oneshot::channel();
        let listener = Watched {
            inner,
            dropped: Some(dropped),
        };
        let (stop, stopping) = oneshot::channel::<()>();
        let server = tokio::spawn(serve_on(
            listener,
            router().route("/slow", get(slow)),
            async move {
                let _ = stopping.await;
            },
        ));

        let request = tokio::spawn(client().get(format!("http://{address}/slow")).send());
        within("request", entered.notified()).await;
        stop.send(()).expect("the server waits for the signal");
        within("end of accepting", listener_dropped)
            .await
            .expect("the listener is dropped, not leaked");
        assert!(
            !server.is_finished(),
            "the server returned with a request in progress"
        );

        release.notify_one();
        let response = within("response", request)
            .await
            .expect("the request task")
            .expect("a response");
        assert_eq!(response.status(), 200);
        assert_eq!(response.text().await.expect("a body"), "done");
        let served = within("return once the request is answered", server)
            .await
            .expect("the server task");
        assert!(served.is_ok(), "{served:?}");
    }
}
