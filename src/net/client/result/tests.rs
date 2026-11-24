//! A result read only from the file it lists: a file of another size or
//! hash is refused before it is decoded.

use axum::Router;
use axum::http::StatusCode;
use axum::routing::get;
use sha2::{Digest, Sha256};
use url::Url;

use super::super::tests::{json, serve, speaking};
use super::super::{JobId, RemoteClient};
use crate::protocol::jobs::{JobResult, ResultLink};
use crate::protocol::results::{Metrics, RESULT_FORMAT, ResultKind, TradeRow, encode_trades};
use crate::protocol::{ContentHash, ExactDecimal};

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
        results: vec![ResultLink {
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
