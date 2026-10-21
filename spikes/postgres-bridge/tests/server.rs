//! The async server: store calls run under `spawn_blocking` behind a
//! semaphore sized to the pool, so the executor keeps running, the database
//! never sees more connections than the pool holds, and a request that cannot
//! get one in time is answered 503 instead of piling up. The router is called
//! in process, so a burst does not run into the open-file limit.
//!
//! Run with `--test-threads=1`: the heartbeat measures timing.

mod common;

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{Request, StatusCode, header::RETRY_AFTER};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use postgres_bridge_spike::bridge::{BridgeConfig, Effect, PgError};
use postgres_bridge_spike::store::PgCandleStore;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tower::ServiceExt;

use common::{Observer, unique};

#[derive(Clone)]
struct App {
    store: Arc<PgCandleStore>,
    gate: Arc<Semaphore>,
    gate_timeout: Duration,
}

/// `SELECT pg_sleep(ms)` through the store's bridge.
fn sleep(store: &PgCandleStore, ms: u64) -> Result<(), PgError> {
    store.bridge().execute(Effect::Read, move |client| {
        Box::pin(async move {
            client
                .batch_execute(&format!("SELECT pg_sleep({})", ms as f64 / 1000.0))
                .await
        })
    })
}

fn unavailable() -> Response {
    (StatusCode::SERVICE_UNAVAILABLE, [(RETRY_AFTER, "1")]).into_response()
}

/// The pattern the server follows: a permit first, then the blocking call.
async fn gated(State(app): State<App>, Path(ms): Path<u64>) -> Response {
    let Ok(Ok(permit)) =
        tokio::time::timeout(app.gate_timeout, app.gate.clone().acquire_owned()).await
    else {
        return unavailable();
    };
    let store = Arc::clone(&app.store);
    match tokio::task::spawn_blocking(move || {
        let _permit = permit;
        sleep(&store, ms)
    })
    .await
    {
        Ok(Ok(())) => StatusCode::OK.into_response(),
        Ok(Err(PgError::Unavailable(_))) => unavailable(),
        _ => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// The control: the blocking call made on the executor itself.
async fn direct(State(app): State<App>, Path(ms): Path<u64>) -> StatusCode {
    match sleep(&app.store, ms) {
        Ok(()) => StatusCode::OK,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

fn router(app: App) -> Router {
    Router::new()
        .route("/gated/{ms}", get(gated))
        .route("/direct/{ms}", get(direct))
        .with_state(app)
}

fn app(
    url: &str,
    name: &str,
    max_size: usize,
    wait_timeout: Duration,
    gate_timeout: Duration,
) -> App {
    let mut config = BridgeConfig::new(url, name);
    config.max_size = max_size;
    config.wait_timeout = wait_timeout;
    App {
        store: Arc::new(PgCandleStore::connect(&config, &unique("server")).expect("a store")),
        gate: Arc::new(Semaphore::new(max_size)),
        gate_timeout,
    }
}

/// `requests` concurrent GETs of `uri`, with their statuses.
async fn burst(router: &Router, uri: &str, requests: usize) -> Vec<StatusCode> {
    let mut calls = JoinSet::new();
    for _ in 0..requests {
        let call = router
            .clone()
            .oneshot(Request::get(uri).body(Body::empty()).expect("a request"));
        calls.spawn(async move { call.await.expect("a response").status() });
    }
    calls.join_all().await
}

/// How late a 5 ms heartbeat on this runtime ran at worst while `work` ran.
async fn worst_lateness<F: Future>(work: F) -> (F::Output, Duration) {
    let stop = Arc::new(AtomicBool::new(false));
    let beat = {
        let stop = Arc::clone(&stop);
        tokio::spawn(async move {
            let mut worst = Duration::ZERO;
            while !stop.load(Ordering::Relaxed) {
                let asked = Instant::now();
                tokio::time::sleep(Duration::from_millis(5)).await;
                worst = worst.max(asked.elapsed().saturating_sub(Duration::from_millis(5)));
            }
            worst
        })
    };
    let output = work.await;
    stop.store(true, Ordering::Relaxed);
    (output, beat.await.expect("the heartbeat"))
}

/// Samples the connections open under `name` until stopped; the peak.
fn sample_peak(url: &str, name: &str) -> (Arc<AtomicBool>, thread::JoinHandle<i64>) {
    let stop = Arc::new(AtomicBool::new(false));
    let observer = Observer::connect(url);
    let (name, flag) = (name.to_string(), Arc::clone(&stop));
    let peak = Arc::new(AtomicI64::new(0));
    let sampler = thread::spawn(move || {
        while !flag.load(Ordering::Relaxed) {
            peak.fetch_max(observer.connections(&name), Ordering::Relaxed);
            thread::sleep(Duration::from_millis(5));
        }
        peak.load(Ordering::Relaxed)
    });
    (stop, sampler)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_burst_keeps_the_executor_running_and_the_connections_within_the_pool() {
    let url = database_or_skip!();
    let name = unique("burst");
    let app = app(
        &url,
        &name,
        8,
        Duration::from_secs(10),
        Duration::from_secs(30),
    );
    let router = router(app);

    let (stop, sampler) = sample_peak(&url, &name);
    let started = Instant::now();
    let (statuses, gated_worst) = worst_lateness(burst(&router, "/gated/50", 200)).await;
    let elapsed = started.elapsed();
    stop.store(true, Ordering::Relaxed);
    let peak = sampler.join().expect("the sampler");
    assert!(
        statuses.iter().all(|status| *status == StatusCode::OK),
        "{statuses:?}"
    );
    assert_eq!(
        peak, 8,
        "the peak must be the pool's size: never more, and reached"
    );

    // The control: the same store called on the executor, four requests on
    // two workers, each holding a worker for 200 ms.
    let (statuses, direct_worst) = worst_lateness(burst(&router, "/direct/200", 4)).await;
    assert!(
        statuses.iter().all(|status| *status == StatusCode::OK),
        "{statuses:?}"
    );

    eprintln!(
        "200 gated requests of 50 ms on 8 connections took {elapsed:?}; heartbeat worst \
         lateness {gated_worst:?} gated, {direct_worst:?} direct"
    );
    assert!(
        direct_worst >= Duration::from_millis(150),
        "{direct_worst:?}"
    );
    assert!(
        gated_worst * 4 < direct_worst,
        "{gated_worst:?} vs {direct_worst:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_request_that_cannot_get_a_connection_in_time_is_answered_503() {
    let url = database_or_skip!();
    let name = unique("exhausted");
    // Two connections, but four permits: the pool, not the gate, runs out.
    let mut app = app(
        &url,
        &name,
        2,
        Duration::from_millis(300),
        Duration::from_secs(30),
    );
    app.gate = Arc::new(Semaphore::new(4));
    let router = router(app.clone());
    let observer = Observer::connect(&url);

    let holders: Vec<_> = (0..2)
        .map(|_| {
            let store = Arc::clone(&app.store);
            thread::spawn(move || sleep(&store, 2_000))
        })
        .collect();
    assert_eq!(observer.wait_for(2, |observer| observer.sleeping(&name)), 2);

    let started = Instant::now();
    let response = router
        .clone()
        .oneshot(
            Request::get("/gated/0")
                .body(Body::empty())
                .expect("a request"),
        )
        .await
        .expect("a response");
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.headers()[RETRY_AFTER], "1");
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(
        observer.connections(&name),
        2,
        "no third connection was opened"
    );

    for holder in holders {
        holder.join().expect("a holder").expect("its sleep");
    }
}
