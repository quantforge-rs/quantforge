//! A shutdown that lets go of the listener at once and returns only once
//! the request in progress has been answered.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::routing::get;
use tokio::net::TcpStream;
use tokio::sync::{Notify, oneshot};

use super::*;

/// A listener that reports when the server lets go of it, which is when it
/// stops accepting connections.
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
    let history = Bed::new().history();
    let jobs = idle(&history);
    let server = tokio::spawn(serve_on(
        listener,
        router(history, jobs).route("/slow", get(slow)),
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
