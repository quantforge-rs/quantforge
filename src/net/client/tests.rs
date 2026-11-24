//! The client against servers of its own, in this process: what it agrees
//! on, what it sends, what it passes on word for word, what it asks again
//! and what it never does, and what it refuses to read.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};

use super::*;
use sha2::{Digest, Sha256};

use crate::protocol::jobs::JobResult;
use crate::protocol::results::{Metrics, RESULT_FORMAT, ResultKind, TradeRow, encode_trades};
use crate::protocol::{ContentHash, ExactDecimal};

/// The headers of every request a fake server took, in order, by path.
pub(super) type Seen = Arc<Mutex<Vec<(String, Option<String>)>>>;

/// Serves `router` on a free loopback port, recording each request's path
/// and `Authorization` header.
pub(super) async fn serve(router: Router) -> (Url, Seen) {
    let seen: Seen = Arc::default();
    let recorder = seen.clone();
    let router = router.layer(axum::middleware::from_fn(
        move |request: axum::extract::Request, next: axum::middleware::Next| {
            let recorder = recorder.clone();
            async move {
                let authorization = request
                    .headers()
                    .get(AUTHORIZATION)
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string);
                recorder
                    .lock()
                    .expect("the record")
                    .push((request.uri().path().to_string(), authorization));
                next.run(request).await
            }
        },
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a listener");
    let address: SocketAddr = listener.local_addr().expect("its address");
    tokio::spawn(async move { axum::serve(listener, router).await });
    let url = server_url(&format!("http://{address}")).expect("a URL");
    (url, seen)
}

pub(super) fn json(status: StatusCode, body: &str) -> Response {
    (
        status,
        [(CONTENT_TYPE, "application/json")],
        body.to_string(),
    )
        .into_response()
}

const VERSION: &str = r#"{"version":"0.2.0","proto_version":1,"min_supported":1}"#;

/// A server that speaks version 1 and answers `rest` as given.
pub(super) fn speaking(rest: Router) -> Router {
    rest.route("/version", get(|| async { json(StatusCode::OK, VERSION) }))
}

pub(super) fn refusal(status: StatusCode, code: &str, message: &str) -> Response {
    json(
        status,
        &format!(r#"{{"error":{{"code":"{code}","message":"{message}"}}}}"#),
    )
}

#[test]
fn a_server_url_is_http_or_https_with_a_host_and_its_path_ends_with_a_slash() {
    for (text, url) in [
        ("http://127.0.0.1:9600", "http://127.0.0.1:9600/"),
        (" https://qf.example/lab ", "https://qf.example/lab/"),
        ("http://qf.example/lab/", "http://qf.example/lab/"),
    ] {
        assert_eq!(server_url(text).expect("a URL").as_str(), url, "{text}");
    }
    for (text, reason) in [
        ("127.0.0.1:9600", "relative URL without a base"),
        ("ws://127.0.0.1:9600", "starts with http:// or https://"),
        ("http://127.0.0.1:9600/?x=1", "no query or fragment"),
        ("http://127.0.0.1:9600/#top", "no query or fragment"),
    ] {
        let error = server_url(text).expect_err("refused");
        assert!(error.to_string().contains(reason), "{text}: {error}");
    }
    let url = server_url("http://ann:secret@qf.example").expect("a URL");
    assert_eq!(shown(&url), "http://qf.example/");
}

#[tokio::test]
async fn a_client_agrees_on_the_newest_version_both_speak_and_refuses_a_server_with_none() {
    let (url, _) = serve(speaking(Router::new())).await;
    let client = RemoteClient::connect(url, None).await.expect("connected");
    assert_eq!(
        (client.version(), client.release()),
        (ProtoVersion(1), "0.2.0")
    );

    let newer = Router::new().route(
        "/version",
        get(|| async {
            json(
                StatusCode::OK,
                r#"{"version":"0.9.0","proto_version":4,"min_supported":3}"#,
            )
        }),
    );
    let (url, _) = serve(newer).await;
    let error = RemoteClient::connect(url.clone(), None)
        .await
        .expect_err("refused");
    assert_eq!(
        error.to_string(),
        format!(
            "the server at {url} runs QuantForge 0.9.0, which speaks protocol versions 3 to 4, \
             and this build speaks 1 to 1; upgrade the older of the two"
        )
    );
}

#[tokio::test]
async fn something_that_is_not_a_quantforge_server_is_named_as_such() {
    let (url, _) = serve(Router::new()).await;
    let error = RemoteClient::connect(url.clone(), None)
        .await
        .expect_err("refused");
    assert_eq!(
        error.to_string(),
        format!(
            "the server at {url} answered 404 Not Found without QuantForge's error envelope; is \
             it a QuantForge server?"
        )
    );
    let html = Router::new().route("/version", get(|| async { "<html>hello</html>" }));
    let (url, _) = serve(html).await;
    let error = RemoteClient::connect(url, None).await.expect_err("refused");
    assert!(
        error
            .to_string()
            .contains("answered /version with a body the protocol does not read"),
        "{error}"
    );
}

#[tokio::test]
async fn a_server_nobody_listens_for_is_unreachable() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a port");
    let address = listener.local_addr().expect("its address");
    drop(listener);
    let url = server_url(&format!("http://user:pw@{address}")).expect("a URL");
    let error = RemoteClient::connect(url, None).await.expect_err("refused");
    let text = error.to_string();
    assert!(
        text.starts_with(&format!(
            "could not reach the server at http://{address}/: "
        )),
        "{text}"
    );
    assert!(!text.contains("pw"), "{text}");
}

#[tokio::test]
async fn a_refusal_is_the_servers_own_message_word_for_word() {
    let message = "the range spans 9 bars of 1m, more than the 5 this server runs one job on; \
                   ask for a shorter range";
    let rest = Router::new().route(
        "/v1/backtests",
        post(move || async move { refusal(StatusCode::BAD_REQUEST, "invalid_request", message) }),
    );
    let (url, _) = serve(speaking(rest)).await;
    let client = RemoteClient::connect(url, None).await.expect("connected");
    let submission = serde_json::from_str(SUBMISSION).expect("a submission");
    let error = client.submit(&submission).await.expect_err("refused");
    assert_eq!(error.to_string(), message);
    assert_eq!(
        error.code(),
        Some(crate::protocol::ErrorCode::InvalidRequest)
    );
}

const SUBMISSION: &str = r#"{"bundle":"sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855","data":{"exchange":"binance","category":"spot","symbol":"BTCUSDT","interval":"1m","start_open_ms":0,"end_open_ms":60000},"backtest":{"initial_cash":"100","fee_bps":"0","close_out_at_end":true}}"#;

/// A route that answers `503` with `Retry-After: 0` `busy` times, then
/// `answer`.
fn busy_then(
    busy: usize,
    answer: &'static str,
) -> impl Fn() -> std::future::Ready<Response> + Clone {
    let left = Arc::new(Mutex::new(busy));
    move || {
        let mut left = left.lock().expect("the count");
        std::future::ready(if *left > 0 {
            *left -= 1;
            let mut response = refusal(StatusCode::SERVICE_UNAVAILABLE, "unavailable", "busy");
            response
                .headers_mut()
                .insert(RETRY_AFTER, HeaderValue::from_static("0"));
            response
        } else {
            json(StatusCode::OK, answer)
        })
    }
}

#[tokio::test]
async fn a_read_is_asked_again_while_the_server_is_busy_and_a_submission_never_is() {
    let rest = Router::new()
        .route("/v1/runtime", get(busy_then(2, "{}")))
        .route("/v1/backtests", post(busy_then(1, "{}")));
    let (url, seen) = serve(speaking(rest)).await;
    let client = RemoteClient::connect(url, None).await.expect("connected");
    assert_eq!(client.runtime().await.expect("a runtime"), None);
    let submission = serde_json::from_str(SUBMISSION).expect("a submission");
    let error = client.submit(&submission).await.expect_err("busy");
    assert_eq!(error.code(), Some(crate::protocol::ErrorCode::Unavailable));
    let paths: Vec<String> = seen
        .lock()
        .expect("the record")
        .iter()
        .map(|(path, _)| path.clone())
        .collect();
    assert_eq!(
        paths,
        [
            "/version",
            "/v1/runtime",
            "/v1/runtime",
            "/v1/runtime",
            "/v1/backtests"
        ]
    );
}

#[tokio::test]
async fn a_token_never_crosses_the_network_in_clear_text() {
    for text in [
        "http://qf.example:9600",
        "http://10.0.0.5:9600",
        "http://[2001:db8::1]",
    ] {
        let url = server_url(text).expect("a URL");
        let error = RemoteClient::connect(url, Some(AuthToken::new("t0k3n-sentinel")))
            .await
            .expect_err("refused");
        assert!(
            matches!(error, ClientError::CleartextToken { .. }),
            "{text}: {error}"
        );
        assert!(
            error.to_string().ends_with("is neither; nothing was sent"),
            "{error}"
        );
        assert!(!error.to_string().contains("t0k3n"), "{error}");
    }
    // Without a token, a host elsewhere is asked; this one is not there.
    let url = server_url("http://127.0.0.1:9").expect("a URL");
    let error = RemoteClient::connect(url, None)
        .await
        .expect_err("unreachable");
    assert!(matches!(error, ClientError::Unreachable { .. }), "{error}");
}

#[tokio::test]
async fn the_token_goes_with_every_request_and_is_never_shown() {
    let (url, seen) = serve(speaking(
        Router::new().route("/v1/runtime", get(|| async { json(StatusCode::OK, "{}") })),
    ))
    .await;
    let client = RemoteClient::connect(url, Some(AuthToken::new("t0k3n-sentinel")))
        .await
        .expect("connected");
    client.runtime().await.expect("a runtime");
    for (path, authorization) in seen.lock().expect("the record").iter() {
        assert_eq!(
            authorization.as_deref(),
            Some("Bearer t0k3n-sentinel"),
            "{path}"
        );
    }
    assert!(!format!("{client:?}").contains("t0k3n"), "{client:?}");

    for token in ["", "with space", "naïve"] {
        let (url, _) = serve(speaking(Router::new())).await;
        let error = RemoteClient::connect(url, Some(AuthToken::new(token)))
            .await
            .expect_err("refused");
        assert!(matches!(error, ClientError::InvalidToken), "{token:?}");
    }
}

fn metrics(trade_count: u64) -> Metrics {
    let amount = |text: &str| text.parse::<ExactDecimal>().expect("a decimal");
    Metrics {
        initial_cash: amount("100"),
        final_equity: amount("101.5"),
        total_return_pct: amount("1.5"),
        max_drawdown_pct: amount("0"),
        trade_count,
    }
}

fn trade() -> TradeRow {
    let amount = |text: &str| text.parse::<ExactDecimal>().expect("a decimal");
    TradeRow {
        symbol: "BTCUSDT".to_string(),
        entry_time_ms: 60_000,
        exit_time_ms: 120_000,
        entry_price: amount("10.00"),
        exit_price: amount("10.15"),
        qty: amount("10"),
        gross_quote_pnl: amount("1.50"),
    }
}

/// A server whose job `j-1` succeeded with one trade, sending `sent` as its
/// `trades.arrow` while listing the bytes of the real file.
async fn result_server(sent: Vec<u8>) -> Url {
    let file = encode_trades(&[trade()]).expect("a file");
    let result = JobResult {
        job_id: JobId::new("j-1").expect("a job id"),
        format: RESULT_FORMAT,
        metrics: metrics(1),
        results: vec![crate::protocol::jobs::ResultLink {
            name: "trades".to_string(),
            media_type: ResultKind::Trades.media_type().to_string(),
            hash: ContentHash::sha256(Sha256::digest(&file).into()),
            size_bytes: u64::try_from(file.len()).expect("a size"),
            path: "/elsewhere/trades.arrow".to_string(),
        }],
    };
    let listed = serde_json::to_string(&result).expect("a result");
    let rest = Router::new()
        .route(
            "/v1/backtests/j-1/result",
            get(move || async move { json(StatusCode::OK, &listed) }),
        )
        .route(
            "/v1/backtests/j-1/result/trades.arrow",
            get(move || async move { sent.clone() }),
        );
    serve(speaking(rest)).await.0
}

#[tokio::test]
async fn a_result_is_read_only_from_the_file_it_lists() {
    let file = encode_trades(&[trade()]).expect("a file");
    let client = RemoteClient::connect(result_server(file.clone()).await, None)
        .await
        .expect("connected");
    let job = JobId::new("j-1").expect("a job id");
    let fetched = client.result(&job).await.expect("a result");
    assert_eq!(fetched.trades, [trade()]);
    assert_eq!(fetched.result.metrics, metrics(1));

    let mut altered = file.clone();
    if let Some(byte) = altered.iter_mut().rev().find(|byte| **byte == b'5') {
        *byte = b'6';
    }
    let client = RemoteClient::connect(result_server(altered).await, None)
        .await
        .expect("connected");
    let error = client.result(&job).await.expect_err("refused");
    assert!(
        error.to_string().contains(&format!(
            "sent trades.arrow of {} bytes hashing to",
            file.len()
        )),
        "{error}"
    );
}
