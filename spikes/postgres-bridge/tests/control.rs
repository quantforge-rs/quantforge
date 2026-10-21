//! Why the blocking `postgres` crate was rejected: each connection owns a
//! tokio runtime and blocks on it for every call, so any call made where a
//! runtime already runs, which is everywhere in QuantForge, panics. It
//! panics before any I/O, so this needs no database.

use std::panic;

fn message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|text| text.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_default()
}

const NOWHERE: &str = "host=127.0.0.1 port=1 user=nobody connect_timeout=1";

#[tokio::test]
async fn the_blocking_client_panics_where_a_runtime_already_runs() {
    let outcome = panic::catch_unwind(|| postgres::Client::connect(NOWHERE, postgres::NoTls));
    let payload = outcome.err().expect("the blocking client panics here");
    let message = message(payload.as_ref());
    assert!(
        message.contains("Cannot start a runtime from within a runtime"),
        "{message}"
    );
}

#[tokio::test]
async fn on_a_blocking_thread_it_only_fails_to_connect() {
    let outcome =
        tokio::task::spawn_blocking(|| postgres::Client::connect(NOWHERE, postgres::NoTls))
            .await
            .expect("no panic on a blocking thread");
    assert!(outcome.is_err(), "nothing listens on port 1");
}
