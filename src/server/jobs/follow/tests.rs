//! A stream with nothing to say pings its client, while its job waits in
//! the queue and while it runs in silence, so that a quiet connection
//! stays open and a client that has gone is noticed; and a stream its job
//! keeps busy still hears its client.

use std::time::Duration;

use axum::Router;
use axum::routing::get;
use futures_util::{SinkExt, StreamExt};
use tempfile::TempDir;
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message as Said;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use super::*;
use crate::model::{BundleHash, NewJob};
use crate::ports::{CandleStore, JobStore};
use crate::protocol::jobs::LogLevel;
use crate::server::jobs::{JobLimits, LogLimits, NoRuntime};
use crate::server::store::{ServerStore, StoreLimits};
use crate::storage::SqliteStore;

/// How often the streams of these tests ping.
const OFTEN: Duration = Duration::from_millis(40);

/// Job routes over a SQLite store in `dir` whose queue never starts, and a
/// job queued there.
fn queued(dir: &TempDir) -> (Jobs, JobId) {
    queued_keeping(dir, LogLimits::default())
}

/// [`queued`], with logs that keep `log` of what a job writes.
fn queued_keeping(dir: &TempDir, log: LogLimits) -> (Jobs, JobId) {
    let store = SqliteStore::new(dir.path().join("market.sqlite"));
    CandleStore::init(&store).expect("init");
    let bundle = BundleHash::from_digest([7; 32]);
    store
        .store_bundle(&bundle, b"a bundle", 1)
        .expect("a bundle");
    let job_id = JobId::generate();
    store
        .submit_job(&NewJob {
            job_id: job_id.clone(),
            bundle,
            request_json: "{}".to_string(),
            submitted_at_ms: 2,
        })
        .expect("a job");
    let limits = JobLimits {
        concurrency: 1,
        max_span_bars: 1,
        log,
    };
    let store = ServerStore::new(store, StoreLimits::default());
    (Jobs::new(store, NoRuntime, limits), job_id)
}

/// A client's end of a stream of the job's log that pings every [`OFTEN`].
async fn stream(jobs: &Jobs, job_id: &JobId) -> WebSocketStream<MaybeTlsStream<TcpStream>> {
    let (jobs, job_id) = (jobs.clone(), job_id.clone());
    let app = Router::new().route(
        "/",
        get(move |upgrade: WebSocketUpgrade| async move {
            upgrade.on_upgrade(move |socket| follow(socket, jobs, job_id, 0, OFTEN))
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("a listener");
    let address = listener.local_addr().expect("its address");
    tokio::spawn(async move { axum::serve(listener, app).await });
    let (stream, _) = tokio_tungstenite::connect_async(format!("ws://{address}/"))
        .await
        .expect("a stream");
    stream
}

/// What the stream says next, within ten seconds.
async fn next(stream: &mut WebSocketStream<MaybeTlsStream<TcpStream>>) -> Said {
    tokio::time::timeout(Duration::from_secs(10), stream.next())
        .await
        .expect("the stream says something within ten seconds")
        .expect("an open stream")
        .expect("a message")
}

#[tokio::test]
async fn a_stream_that_waits_for_a_queued_job_pings_its_client() {
    let dir = tempfile::tempdir().expect("a directory");
    let (jobs, job_id) = queued(&dir);
    let mut stream = stream(&jobs, &job_id).await;
    for _ in 0..3 {
        let said = next(&mut stream).await;
        assert!(matches!(said, Said::Ping(_)), "{said:?}");
    }
}

#[tokio::test]
async fn a_stream_of_a_running_job_pings_its_client_while_the_job_is_silent() {
    let dir = tempfile::tempdir().expect("a directory");
    let (jobs, job_id) = queued(&dir);
    let (_cancel, log) = jobs.register(&job_id);
    let mut stream = stream(&jobs, &job_id).await;

    log.line(LogLevel::Info, "one");
    // A ping may come first, where the test was slow to write the line.
    let said = loop {
        match next(&mut stream).await {
            Said::Ping(_) => {}
            said => break said,
        }
    };
    assert!(
        matches!(&said, Said::Text(text) if text.contains("one")),
        "{said:?}"
    );
    // Silence: pings, and nothing else.
    for _ in 0..3 {
        let said = next(&mut stream).await;
        assert!(matches!(said, Said::Ping(_)), "{said:?}");
    }
    // The job goes on, and so does the stream, to the job's end.
    log.line(LogLevel::Info, "two");
    log.end();
    let mut texts = Vec::new();
    while texts.len() < 2 {
        match next(&mut stream).await {
            Said::Text(text) => texts.push(text.to_string()),
            Said::Ping(_) => {}
            other => panic!("before a job's end, {other:?}"),
        }
    }
    assert!(texts[0].contains("two"), "{texts:?}");
    assert_eq!(texts[1], r#"{"type":"end"}"#);
}

#[tokio::test]
async fn a_stream_its_job_keeps_busy_still_answers_its_clients_ping() {
    // More text than a connection's buffers hold, so that the stream is
    // busy sending for as long as its client reads.
    const LINES: usize = 3_000;
    let dir = tempfile::tempdir().expect("a directory");
    let roomy = LogLimits {
        max_lines: LINES,
        max_bytes: 64 << 20,
        max_line_bytes: 8 << 10,
    };
    let (jobs, job_id) = queued_keeping(&dir, roomy);
    let (_cancel, log) = jobs.register(&job_id);
    let line = "x".repeat(8 << 10);
    for _ in 0..LINES {
        log.line(LogLevel::Info, &line);
    }
    let mut stream = stream(&jobs, &job_id).await;

    // The client pings before it reads a line.
    stream.send(Said::Ping(vec![7].into())).await.expect("sent");
    let (mut lines, mut answered_after) = (0, None);
    while lines < LINES {
        match next(&mut stream).await {
            Said::Text(_) => lines += 1,
            Said::Pong(payload) if payload.as_ref() == [7] => answered_after = Some(lines),
            Said::Ping(_) => {}
            other => panic!("among a job's lines, {other:?}"),
        }
    }
    assert!(
        answered_after.is_some(),
        "the ping went unanswered until every line was sent"
    );
}
