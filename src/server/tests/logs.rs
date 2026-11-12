//! The log route's refusals: a job it cannot name or does not hold, a query
//! or an upgrade that is none, a stored log no build can answer, which is
//! never sent on, as a page or as a stream, a store too busy to answer a
//! stream, which the stream waits out, and a client that says more than a
//! log stream's client has to say.

use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

use super::jobs::{Admitting, jobs_of, runtime, serve_jobs};
use super::queue::queued;
use super::*;
use crate::model::{JobId, JobOutcome};
use crate::ports::JobStore;

/// The nonce of RFC 6455's own example handshake: 16 bytes in base64 that
/// say "the sample nonce".
const NONCE: &str = "dGhlIHNhbXBsZSBub25jZQ==";

/// A stored log of one line.
const ONE_LINE: &str =
    r#"{"dropped_lines":0,"lines":[{"seq":1,"at_ms":5,"level":"info","message":"one"}]}"#;

/// The headers of a WebSocket handshake.
const HANDSHAKE: [(&str, &str); 4] = [
    ("connection", "Upgrade"),
    ("upgrade", "websocket"),
    ("sec-websocket-version", "13"),
    ("sec-websocket-key", NONCE),
];

/// `GET path` with `headers`: a page, or a handshake the test wrote.
async fn get(address: SocketAddr, path: &str, headers: &[(&str, &str)]) -> reqwest::Response {
    let mut request = client().get(format!("http://{address}{path}"));
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    request.send().await.expect("a response")
}

/// A job that failed, with `log_json` stored as its log.
fn ended(bed: &Bed, log_json: &str) -> JobId {
    let job_id = queued(bed);
    bed.store
        .claim_job(T + 1)
        .expect("a claim")
        .expect("the job");
    let outcome = JobOutcome::Failed {
        failure_json: r#"{"code":"internal","message":"it failed"}"#.to_string(),
    };
    assert!(
        bed.store
            .finish_job(&job_id, &outcome, T + 2)
            .expect("an end")
    );
    assert!(
        bed.store
            .store_job_log(&job_id, log_json, T + 3)
            .expect("a stored log")
    );
    job_id
}

#[tokio::test]
async fn the_log_route_reads_its_job_as_every_job_route_does_and_takes_only_get() {
    let bed = Bed::new();
    let address = serve_jobs(&bed, jobs_of(&bed, Admitting(runtime("3.12")), 100)).await;
    // Asked for a page, and asked as a WebSocket: refused the same, before
    // any upgrade.
    for headers in [&[][..], &HANDSHAKE[..]] {
        let (status, code, message) =
            refusal(get(address, "/v1/backtests/j%20x/logs", headers).await).await;
        assert_eq!((status, code.as_str()), (400, "invalid_request"));
        assert!(message.starts_with("invalid job id"), "{message}");
        let (status, code, message) =
            refusal(get(address, "/v1/backtests/job-1/logs", headers).await).await;
        assert_eq!((status, code.as_str()), (404, "not_found"));
        assert_eq!(message, "the server holds no job job-1");
    }

    // A version the server does not speak is refused before any upgrade.
    let (status, code, _) =
        refusal(get(address, "/v2/backtests/job-1/logs", &HANDSHAKE).await).await;
    assert_eq!((status, code.as_str()), (400, "unsupported_version"));

    let posted = client()
        .post(format!("http://{address}/v1/backtests/job-1/logs"))
        .send()
        .await
        .expect("a response");
    assert_eq!(header(&posted, ALLOW), Some("GET,HEAD"));
    let (status, code, _) = refusal(posted).await;
    assert_eq!((status, code.as_str()), (405, "method_not_allowed"));
    let (status, code, _) = refusal(get(address, "/v1/backtests/job-1/logs/x", &[]).await).await;
    assert_eq!((status, code.as_str()), (404, "not_found"));
}

#[tokio::test]
async fn a_log_query_is_refused_unless_its_cursor_and_limit_are_numbers_in_range() {
    let bed = Bed::new();
    let job = ended(&bed, ONE_LINE);
    let address = serve_jobs(&bed, jobs_of(&bed, Admitting(runtime("3.12")), 100)).await;
    let path = |query: &str| format!("/v1/backtests/{job}/logs{query}");

    for query in [
        "?after=x",
        "?after=-1",
        "?after=",
        "?after=1.5",
        "?after=18446744073709551616",
        "?limit=many",
        "?after=1&after=2",
    ] {
        for headers in [&[][..], &HANDSHAKE[..]] {
            let (status, code, message) = refusal(get(address, &path(query), headers).await).await;
            assert_eq!((status, code.as_str()), (400, "invalid_request"), "{query}");
            assert!(
                message.starts_with("the query string is not a log query: "),
                "{query}: {message}"
            );
        }
    }
    for limit in ["0", "1001", "18446744073709551615"] {
        let (status, code, message) =
            refusal(get(address, &path(&format!("?limit={limit}")), &[]).await).await;
        assert_eq!((status, code.as_str()), (400, "invalid_request"), "{limit}");
        assert_eq!(message, format!("limit {limit} is not between 1 and 1000"));
    }

    // In range, and a parameter the server does not know, which it ignores.
    for query in ["", "?limit=1", "?limit=1000", "?after=0&later=1"] {
        let response = get(address, &path(query), &[]).await;
        assert_eq!(response.status(), 200, "{query}");
        assert_eq!(header(&response, CACHE_CONTROL), Some("no-cache"));
        let page: Value = response.json().await.expect("a page");
        assert_eq!(
            page,
            serde_json::json!({
                "job_id": job.as_str(),
                "lines": [{"seq": 1, "at_ms": 5, "level": "info", "message": "one"}],
                "dropped_lines": 0,
                "next_after": 1,
                "complete": true,
            }),
            "{query}"
        );
    }
    // Nothing follows a cursor at the log's end, or past it.
    for after in [1, 2, u64::MAX] {
        let page: Value = get(address, &path(&format!("?after={after}")), &[])
            .await
            .json()
            .await
            .expect("a page");
        assert_eq!(page["lines"], serde_json::json!([]), "after {after}");
        assert_eq!(page["dropped_lines"], 0);
        assert_eq!(page["next_after"], after);
        assert_eq!(page["complete"], true);
    }
}

#[tokio::test]
async fn a_request_that_names_an_upgrade_and_is_no_websocket_handshake_is_refused() {
    let bed = Bed::new();
    let job = ended(&bed, r#"{"dropped_lines":0,"lines":[]}"#);
    let address = serve_jobs(&bed, jobs_of(&bed, Admitting(runtime("3.12")), 100)).await;
    let path = format!("/v1/backtests/{job}/logs");
    let without = |name: &str| -> Vec<(&str, &str)> {
        HANDSHAKE
            .into_iter()
            .filter(|(header, _)| *header != name)
            .collect()
    };
    let mut another_upgrade = without("upgrade");
    another_upgrade.push(("upgrade", "h2c"));
    let mut another_version = without("sec-websocket-version");
    another_version.push(("sec-websocket-version", "8"));
    for (case, headers, names) in [
        ("another upgrade", another_upgrade, "`Upgrade`"),
        ("no connection header", without("connection"), "Connection"),
        (
            "no key",
            without("sec-websocket-key"),
            "`Sec-WebSocket-Key`",
        ),
        (
            "another version",
            another_version,
            "`Sec-WebSocket-Version`",
        ),
        (
            "no version",
            without("sec-websocket-version"),
            "`Sec-WebSocket-Version`",
        ),
    ] {
        let (status, code, message) = refusal(get(address, &path, &headers).await).await;
        assert_eq!((status, code.as_str()), (400, "invalid_request"), "{case}");
        let why = message
            .strip_prefix("the request is not a WebSocket upgrade: ")
            .unwrap_or_else(|| panic!("{case}: {message}"));
        assert!(why.contains(names), "{case}: {message}");
    }
    // The handshake as a whole is taken.
    let upgraded = get(address, &path, &HANDSHAKE).await;
    assert_eq!(upgraded.status(), 101);
    assert_eq!(header(&upgraded, "upgrade"), Some("websocket"));
}

#[tokio::test]
async fn a_stored_log_no_build_can_answer_is_never_sent_on() {
    let bed = Bed::new();
    let job = ended(&bed, ONE_LINE);
    let address = serve_jobs(&bed, jobs_of(&bed, Admitting(runtime("3.12")), 100)).await;
    let path = format!("/v1/backtests/{job}/logs");
    assert_eq!(get(address, &path, &[]).await.status(), 200);

    for damage in [
        "",
        "not json",
        "[]",
        r#"{"lines":[]}"#,
        // A line numbered out of turn, and a level only a later build names.
        r#"{"dropped_lines":0,"lines":[{"seq":2,"at_ms":5,"level":"info","message":"one"}]}"#,
        r#"{"dropped_lines":0,"lines":[{"seq":1,"at_ms":5,"level":"trace","message":"one"}]}"#,
    ] {
        bed.connection()
            .execute(
                "UPDATE job_logs SET log_json = ?1 WHERE job_id = ?2",
                rusqlite::params![damage, job.as_str()],
            )
            .expect("the damage");
        for headers in [&[][..], &HANDSHAKE[..]] {
            let (status, code, message) = refusal(get(address, &path, headers).await).await;
            assert_eq!((status, code.as_str()), (500, "internal"), "{damage}");
            assert_eq!(
                message,
                format!("the server cannot read the log of job {job}"),
                "{damage}"
            );
        }
    }
    // The job's status still reads.
    let status = get(address, &format!("/v1/backtests/{job}"), &[]).await;
    assert_eq!(status.status(), 200);
}

#[tokio::test]
async fn a_stream_the_server_cannot_go_on_with_ends_with_an_error_frame() {
    let bed = Bed::new();
    let job = queued(&bed);
    let address = serve_jobs(&bed, jobs_of(&bed, Admitting(runtime("3.12")), 100)).await;
    let url = format!("ws://{address}/v1/backtests/{job}/logs");
    let (mut stream, _) = tokio_tungstenite::connect_async(url)
        .await
        .expect("a stream of a queued job");

    // The job ends while the stream waits for it, with a log that reads as
    // none: the stream says why it stops, and closes as a server that
    // failed does.
    bed.connection()
        .execute(
            "INSERT INTO job_logs (job_id, log_json, stored_at_ms) VALUES (?1, 'not json', 0)",
            [job.as_str()],
        )
        .expect("the damage");
    let cancelled = client()
        .post(format!("http://{address}/v1/backtests/{job}/cancel"))
        .send()
        .await
        .expect("a response");
    assert_eq!(cancelled.status(), 200);
    let said = within("an error frame", stream.next()).await;
    let Some(Ok(Message::Text(text))) = said else {
        panic!("an error frame, not {said:?}");
    };
    let frame: Value = serde_json::from_str(text.as_str()).expect("a frame");
    assert_eq!(
        frame,
        serde_json::json!({
            "type": "error",
            "code": "internal",
            "message": format!("the server cannot read the log of job {job}"),
        })
    );
    let closed = within("the close", stream.next()).await;
    let Some(Ok(Message::Close(Some(close)))) = closed else {
        panic!("a close, not {closed:?}");
    };
    assert_eq!(close.code, CloseCode::Error);
}

#[tokio::test]
async fn a_stream_waits_out_a_store_too_busy_to_say_where_its_job_stands() {
    let bed = Bed::new();
    let job = ended(&bed, ONE_LINE);
    // The store answers as the stream is opened, and is then out of time
    // once: the stream asks again, and says nothing of it.
    let busy = Faulty::busy_jobs(bed.store.within(Duration::from_secs(2)), 1, 1);
    let jobs = Jobs::new(
        served(busy),
        Admitting(runtime("3.12")),
        JobLimits {
            concurrency: 1,
            max_span_bars: 100,
            log: LogLimits::default(),
        },
    );
    let address = start_with(bed.history(), jobs).await;
    let url = format!("ws://{address}/v1/backtests/{job}/logs");
    let (mut stream, _) = tokio_tungstenite::connect_async(url)
        .await
        .expect("a stream");
    let mut frames = Vec::new();
    let closed = loop {
        match within("the stream's next word", stream.next()).await {
            Some(Ok(Message::Text(text))) => {
                frames.push(serde_json::from_str::<Value>(text.as_str()).expect("a frame"));
            }
            Some(Ok(Message::Close(close))) => break close.map(|close| close.code),
            Some(Ok(_)) => {}
            other => panic!("the stream ended with {other:?}"),
        }
    };
    assert_eq!(
        frames,
        [
            serde_json::json!({"type": "line", "seq": 1, "at_ms": 5, "level": "info", "message": "one"}),
            serde_json::json!({"type": "end"}),
        ]
    );
    assert_eq!(closed, Some(CloseCode::Normal));
}

#[tokio::test]
async fn a_client_that_talks_is_not_answered_and_one_that_sends_too_much_is_dropped() {
    let bed = Bed::new();
    let job = queued(&bed);
    let jobs = jobs_of(&bed, Admitting(runtime("3.12")), 100);
    let address = serve_jobs(&bed, jobs.clone()).await;
    let url = format!("ws://{address}/v1/backtests/{job}/logs");
    let (mut stream, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("a stream of a queued job");

    // What a client says is read and dropped: only its ping is answered,
    // as WebSocket asks.
    for message in [
        Message::Text("after=7".into()),
        Message::Binary(vec![0; 1_000].into()),
        Message::Ping(vec![1, 2, 3].into()),
    ] {
        stream.send(message).await.expect("sent");
    }
    let answered = within("a pong", stream.next()).await;
    assert!(
        matches!(&answered, Some(Ok(Message::Pong(payload))) if payload.as_ref() == [1, 2, 3]),
        "{answered:?}"
    );
    let quiet = tokio::time::timeout(Duration::from_millis(300), stream.next()).await;
    assert!(quiet.is_err(), "nothing is said to a client that talks");

    // More than a client of a log stream has to say: the stream is given
    // up, with no end, since the job has none.
    stream
        .send(Message::Text("x".repeat(4_096).into()))
        .await
        .expect("sent");
    let dropped = within("the stream's end", async {
        loop {
            match stream.next().await {
                None | Some(Err(_)) => return true,
                Some(Ok(Message::Close(close))) => {
                    return close.is_none_or(|close| close.code != CloseCode::Normal);
                }
                Some(Ok(Message::Text(text))) => panic!("a frame: {text}"),
                Some(Ok(_)) => {}
            }
        }
    })
    .await;
    assert!(dropped, "a normal close is a job's end");

    // The server goes on: the next client is served.
    let (mut next, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("another stream");
    // And a stopping server closes it, the job not having ended.
    assert_eq!(within("the stop", jobs.stop()).await, 0);
    let closed = within("the close", next.next()).await;
    let Some(Ok(Message::Close(Some(close)))) = closed else {
        panic!("a close, not {closed:?}");
    };
    assert_eq!(close.code, CloseCode::Away);
    assert_eq!(
        close.reason.as_str(),
        "the server is stopping; ask again once it is back"
    );
}
