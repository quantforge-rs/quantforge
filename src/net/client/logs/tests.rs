//! A job's log followed against servers of its own: a page at a time to
//! its end, and the stream that breaks off taken up by the pages.

use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::ws::rejection::WebSocketUpgradeRejection;
use axum::extract::ws::{Message as WsMessage, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::routing::get;

use super::super::tests::{json, refusal, serve, speaking};
use super::super::{JobId, RemoteClient};
use super::LogEvent;

const LINE: &str = r#"{"seq":SEQ,"at_ms":1,"level":"info","message":"line SEQ"}"#;

fn line(seq: u64) -> String {
    LINE.replace("SEQ", &seq.to_string())
}

fn seqs(events: &[LogEvent]) -> Vec<String> {
    events
        .iter()
        .map(|event| match event {
            LogEvent::Line(line) => format!("line {}", line.seq),
            LogEvent::Progress(progress) => {
                format!("progress {}/{}", progress.bars_done, progress.bars_total)
            }
            LogEvent::Dropped(count) => format!("dropped {count}"),
        })
        .collect()
}

#[tokio::test]
async fn a_log_without_a_stream_is_read_a_page_at_a_time_to_its_end() {
    let pages = Arc::new(Mutex::new(vec![
        format!(
            r#"{{"job_id":"j-1","lines":[{},{}],"dropped_lines":3,"next_after":5,"progress":{{"bars_done":1,"bars_total":2}},"complete":false}}"#,
            line(4),
            line(5)
        ),
        r#"{"job_id":"j-1","lines":[],"dropped_lines":0,"next_after":5,"progress":{"bars_done":1,"bars_total":2},"complete":false}"#.to_string(),
        format!(
            r#"{{"job_id":"j-1","lines":[{}],"dropped_lines":0,"next_after":6,"progress":{{"bars_done":2,"bars_total":2}},"complete":true}}"#,
            line(6)
        ),
    ]));
    let asked = Arc::new(Mutex::new(Vec::new()));
    let (pages_route, asked_route) = (pages.clone(), asked.clone());
    let rest = Router::new().route(
        "/v1/backtests/j-1/logs",
        get(
            move |uri: axum::http::Uri, headers: axum::http::HeaderMap| async move {
                // A server behind a proxy that takes no WebSocket.
                if headers.contains_key("upgrade") {
                    return refusal(StatusCode::BAD_REQUEST, "invalid_request", "no upgrade");
                }
                asked_route
                    .lock()
                    .expect("the record")
                    .push(uri.query().unwrap_or_default().to_string());
                let page = pages_route.lock().expect("the pages").remove(0);
                json(StatusCode::OK, &page)
            },
        ),
    );
    let (url, _) = serve(speaking(rest)).await;
    let client = RemoteClient::connect(url, None).await.expect("connected");
    let mut events = Vec::new();
    client
        .follow(&JobId::new("j-1").expect("a job id"), 0, |event| {
            events.push(event)
        })
        .await
        .expect("followed");
    assert_eq!(
        seqs(&events),
        [
            "dropped 3",
            "line 4",
            "line 5",
            "progress 1/2",
            "line 6",
            "progress 2/2"
        ]
    );
    // Each page from the last line held.
    assert_eq!(
        *asked.lock().expect("the record"),
        ["after=0", "after=5", "after=5"]
    );
}

#[tokio::test]
async fn a_stream_that_breaks_off_is_taken_up_by_the_pages_from_its_last_line() {
    let rest = Router::new().route(
        "/v1/backtests/j-1/logs",
        get(
            |uri: axum::http::Uri, upgrade: Result<WebSocketUpgrade, WebSocketUpgradeRejection>| async move {
                match upgrade {
                    Ok(upgrade) => upgrade.on_upgrade(|mut socket| async move {
                        for frame in [
                            format!(r#"{{"type":"line",{}"#, &line(1)[1..]),
                            r#"{"type":"progress","bars_done":1,"bars_total":3}"#.to_string(),
                            format!(r#"{{"type":"line",{}"#, &line(2)[1..]),
                        ] {
                            let _ = socket.send(WsMessage::Text(frame.into())).await;
                        }
                        // Gone without `end`.
                    }),
                    Err(_) => {
                        assert_eq!(uri.query(), Some("after=2"));
                        json(
                            StatusCode::OK,
                            &format!(
                                r#"{{"job_id":"j-1","lines":[{},{}],"dropped_lines":0,"next_after":3,"progress":{{"bars_done":3,"bars_total":3}},"complete":true}}"#,
                                line(2),
                                line(3)
                            ),
                        )
                    }
                }
            },
        ),
    );
    let (url, _) = serve(speaking(rest)).await;
    let client = RemoteClient::connect(url, None).await.expect("connected");
    let mut events = Vec::new();
    client
        .follow(&JobId::new("j-1").expect("a job id"), 0, |event| {
            events.push(event)
        })
        .await
        .expect("followed");
    assert_eq!(
        seqs(&events),
        ["line 1", "progress 1/3", "line 2", "line 3", "progress 3/3"]
    );
}
