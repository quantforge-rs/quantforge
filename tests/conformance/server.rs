//! The local HTTP server the replay and the proxy both run on: HTTP/1.1 on
//! an ephemeral `127.0.0.1` port, on its own thread and runtime so it never
//! competes with the test's, and stopped when dropped.

use http_body_util::{BodyExt, Full};
use hyper::body::{Bytes, Incoming};
use hyper::header::HeaderMap;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use std::convert::Infallible;
use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use std::thread::JoinHandle;
use url::Url;

/// A request as the server received it, body included.
pub(crate) struct Captured {
    pub(crate) method: String,
    pub(crate) path: String,
    /// The query exactly as sent, still percent-encoded.
    pub(crate) query: Option<String>,
    pub(crate) headers: HeaderMap,
    pub(crate) body: Bytes,
}

/// What the server sends back.
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) body: String,
}

/// What a server does with each request.
pub(crate) trait Handle: Send + Sync + 'static {
    fn handle(&self, request: Captured) -> impl Future<Output = Answer> + Send;
}

pub(crate) struct Server {
    addr: SocketAddr,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    join: Option<JoinHandle<()>>,
}

impl Server {
    pub(crate) fn start<H: Handle>(handler: H) -> Self {
        let handler = Arc::new(handler);
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let (addr_tx, addr_rx) = std::sync::mpsc::channel::<SocketAddr>();

        let join = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime");
            runtime.block_on(async move {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                    .await
                    .expect("bind");
                addr_tx
                    .send(listener.local_addr().expect("local addr"))
                    .expect("send addr");
                loop {
                    tokio::select! {
                        _ = &mut shutdown_rx => break,
                        accepted = listener.accept() => {
                            let Ok((stream, _)) = accepted else { break };
                            let handler = Arc::clone(&handler);
                            tokio::spawn(async move {
                                let service = service_fn(move |request| {
                                    let handler = Arc::clone(&handler);
                                    async move {
                                        Ok::<_, Infallible>(serve(handler.as_ref(), request).await)
                                    }
                                });
                                let _ = hyper::server::conn::http1::Builder::new()
                                    .serve_connection(TokioIo::new(stream), service)
                                    .await;
                            });
                        }
                    }
                }
            });
        });

        let addr = addr_rx.recv().expect("server addr");
        Self {
            addr,
            shutdown: Some(shutdown_tx),
            join: Some(join),
        }
    }

    /// The base URL a connector is pointed at, trailing slash included.
    pub(crate) fn base_url(&self) -> Url {
        Url::parse(&format!("http://{}/", self.addr)).expect("server url")
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

async fn serve<H: Handle>(handler: &H, request: Request<Incoming>) -> Response<Full<Bytes>> {
    let (parts, body) = request.into_parts();
    let body = body
        .collect()
        .await
        .map(|collected| collected.to_bytes())
        .unwrap_or_default();
    let answer = handler
        .handle(Captured {
            method: parts.method.to_string(),
            path: parts.uri.path().to_string(),
            query: parts.uri.query().map(str::to_string),
            headers: parts.headers,
            body,
        })
        .await;
    Response::builder()
        .status(StatusCode::from_u16(answer.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR))
        .header("Content-Type", "application/json")
        .body(Full::new(Bytes::from(answer.body)))
        .expect("response")
}
