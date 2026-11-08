//! The store behind the routes: a failure tells a client whether to retry
//! and nothing of the store itself, and a request waits for a turn at the
//! store only so long, keeping it until its store calls end.

use std::sync::mpsc;

use reqwest::header::RETRY_AFTER;

use super::*;

/// Words a store's failure might hold, such as its host and database, that
/// no client may read.
const SENTINEL: &str = "sentinel-db-417 at sentinel-host:5432";

/// Every route that reads the store, each asked something it would read.
fn routes() -> Vec<String> {
    let candles = query(VENUE, Some(Category::Spot), "BTCUSDT", T, T + DAY)
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("&");
    vec![
        "/v1/markets".to_string(),
        format!("/v1/candles?{candles}"),
        format!("/v1/segments/sha256:{}", "0".repeat(64)),
        "/v1/backtests".to_string(),
        format!("/v1/backtests/{}", "0".repeat(32)),
    ]
}

async fn get(address: SocketAddr, path: &str) -> reqwest::Response {
    client()
        .get(format!("http://{address}{path}"))
        .send()
        .await
        .expect("a response")
}

/// A failure, and the status, code and message a client gets for it.
type Answered = (fn() -> StorageError, u16, &'static str, &'static str);

#[tokio::test]
async fn a_store_out_of_time_is_unavailable_and_a_failed_one_internal_and_neither_named() {
    let failures: [Answered; 6] = [
        (
            || StorageError::Unavailable(SENTINEL.into()),
            503,
            "unavailable",
            "the server's store could not answer in time; retry later",
        ),
        (
            || StorageError::Contention(SENTINEL.into()),
            503,
            "unavailable",
            "the server's store could not answer in time; retry later",
        ),
        (
            || StorageError::Conflict(SENTINEL.into()),
            503,
            "unavailable",
            "the server's store could not answer in time; retry later",
        ),
        (
            || StorageError::Other(SENTINEL.into()),
            500,
            "internal",
            "the server's store failed to answer",
        ),
        (
            || StorageError::InvalidArgument(SENTINEL.into()),
            500,
            "internal",
            "the server's store failed to answer",
        ),
        (
            || StorageError::OutcomeUnknown(SENTINEL.into()),
            500,
            "internal",
            "the server's store failed to answer",
        ),
    ];
    for (failure, status, code, message) in failures {
        let bed = Bed::new();
        let store = Faulty::failing(bed.store.within(Duration::from_secs(2)), failure);
        let address = start(History::new(served(store), HistoryLimits::default())).await;
        for path in routes() {
            let response = get(address, &path).await;
            let retry = header(&response, RETRY_AFTER).map(str::to_string);
            assert_eq!(retry.as_deref(), (status == 503).then_some("1"), "{path}");
            let answer = refusal(response).await;
            assert_eq!(
                answer,
                (status, code.to_string(), message.to_string()),
                "{path}"
            );
        }
    }
}

#[tokio::test]
async fn a_stored_decimal_the_store_cannot_read_exactly_is_internal() {
    let bed = Bed::new();
    let spot = market(Category::Spot);
    bed.store
        .upsert_candles(
            &spot,
            &[candle(T, "1", None), candle(T + MINUTE, "2", None)],
        )
        .expect("the candles");
    bed.connection()
        .execute(
            "UPDATE candles SET close = '1.00000000000000000000000000001' WHERE open_time_ms = ?1",
            [T + MINUTE],
        )
        .expect("the damage");
    let address = start(bed.history()).await;
    let path = &routes()[1];
    let (status, code, _) = refusal(get(address, path).await).await;
    assert_eq!((status, code.as_str()), (500, "internal"));
}

/// The history routes over a store whose calls each wait, once begun,
/// until `release` is dropped, with one turn at the store and a short wait
/// for it.
fn one_turn(bed: &Bed) -> (History, mpsc::Receiver<()>, mpsc::Sender<()>) {
    let (began, begun) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let store = ServerStore::new(
        Faulty::stalling(bed.store.within(Duration::from_secs(2)), began, released),
        StoreLimits {
            concurrency: 1,
            wait: Duration::from_millis(200),
        },
    );
    (
        History::new(store, HistoryLimits::default()),
        begun,
        release,
    )
}

/// Waits, off the runtime's thread, until a store call has begun.
async fn begun(calls: mpsc::Receiver<()>) -> mpsc::Receiver<()> {
    tokio::task::spawn_blocking(move || {
        calls
            .recv_timeout(Duration::from_secs(10))
            .expect("a store call began");
        calls
    })
    .await
    .expect("the wait")
}

async fn refused_for_want_of_a_turn(address: SocketAddr) {
    let response = within("a refusal", get(address, "/v1/markets")).await;
    assert_eq!(header(&response, RETRY_AFTER), Some("1"));
    let (status, code, message) = refusal(response).await;
    assert_eq!((status, code.as_str()), (503, "unavailable"));
    assert_eq!(
        message,
        "the server is answering as many requests as its store can take; retry later"
    );
}

#[tokio::test]
async fn a_request_that_finds_no_turn_at_the_store_in_time_is_unavailable() {
    let bed = Bed::new();
    let (history, calls, release) = one_turn(&bed);
    let address = start(history).await;
    let first = tokio::spawn(get(address, "/v1/markets"));
    let _calls = begun(calls).await;

    refused_for_want_of_a_turn(address).await;
    // Other routes never wait for the store.
    let health = within("health", get(address, "/healthz")).await;
    assert_eq!(health.status(), 200);

    drop(release);
    let first = within("the first answer", first)
        .await
        .expect("the request");
    assert_eq!(first.status(), 200);
}

#[tokio::test]
async fn a_request_whose_client_hangs_up_keeps_its_turn_until_its_store_calls_end() {
    let bed = Bed::new();
    let (history, calls, release) = one_turn(&bed);
    let address = start(history).await;
    let first = tokio::spawn(get(address, "/v1/markets"));
    let _calls = begun(calls).await;
    first.abort();
    let _ = first.await;

    refused_for_want_of_a_turn(address).await;
    drop(release);
    // The store call ends, and its turn frees up.
    let next = within("an answer", async {
        loop {
            let response = get(address, "/v1/markets").await;
            if response.status() == 200 {
                return response;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    assert_eq!(next.text().await.expect("a body"), r#"{"markets":[]}"#);
}
