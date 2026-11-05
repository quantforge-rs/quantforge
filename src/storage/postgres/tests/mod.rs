//! The bridge from every context QuantForge calls storage from, its
//! refusals, and its TLS connector: none of these needs a database.
//! [`database`] runs the rest against a real one, [`migrate`] the schema
//! chain, over steps no build has, [`candles`] how a batch of candles
//! becomes one statement, [`segments`] the segment store where only its
//! own read and write-back can show it, and [`durability`] what a commit
//! means and how long a call may take. [`scratch`] gives a test a database
//! of its own.

mod candles;
mod database;
mod durability;
mod migrate;
mod scratch;
mod segments;

use super::bridge::{Bridge, Effect};
use super::runner::{RunError, Runner};
use super::*;
use crate::storage::settings::{
    ConnectionString, PostgresTls, StorageBackend, StorageRequest, StorageSettings, StoreOwner,
    Supplied, TlsMode,
};
use std::panic::{self, AssertUnwindSafe};
use std::sync::Arc;
use std::time::{Duration, Instant};

const PATIENCE: Duration = Duration::from_secs(5);

fn runner() -> Runner {
    Runner::new("qf-runner-test", 2).expect("a runner")
}

/// Work that needs the bridge's own timer.
async fn nap(value: u32) -> u32 {
    tokio::time::sleep(Duration::from_millis(20)).await;
    value
}

#[tokio::test]
async fn a_current_thread_runtime_waits_on_the_bridge_without_starving_it() {
    // This runtime's only thread blocks in `run`; the work still runs,
    // because it runs on the bridge's threads.
    assert_eq!(runner().run(nap(1), PATIENCE).expect("an answer"), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_multi_thread_runtime_waits_on_the_bridge() {
    assert_eq!(runner().run(nap(2), PATIENCE).expect("an answer"), 2);
}

#[tokio::test]
async fn a_blocking_thread_waits_on_the_bridge() {
    let runner = runner();
    let value = tokio::task::spawn_blocking(move || runner.run(nap(3), PATIENCE))
        .await
        .expect("the blocking task")
        .expect("an answer");
    assert_eq!(value, 3);
}

#[test]
fn a_thread_no_runtime_knows_waits_on_the_bridge() {
    assert_eq!(runner().run(nap(4), PATIENCE).expect("an answer"), 4);
}

#[test]
fn a_call_from_the_bridges_own_thread_is_refused() {
    let runner = Arc::new(runner());
    let inner = Arc::clone(&runner);
    let nested = runner
        .run(async move { inner.run(nap(5), PATIENCE) }, PATIENCE)
        .expect("the outer call");
    assert!(matches!(nested, Err(RunError::Reentrant)), "{nested:?}");
}

#[test]
fn a_call_from_another_process_is_refused_instead_of_waiting_forever() {
    let mut runner = runner();
    // A forked child has another pid and none of the bridge's threads.
    runner.built_by = std::process::id().wrapping_add(1);
    let refused = runner.run(nap(6), PATIENCE);
    assert!(
        matches!(refused, Err(RunError::ForeignProcess { .. })),
        "{refused:?}"
    );
}

#[test]
fn a_panic_in_the_work_is_raised_on_the_caller() {
    let runner = runner();
    let outcome = panic::catch_unwind(AssertUnwindSafe(|| {
        runner.run(async { panic!("the work failed") }, PATIENCE)
    }));
    let payload = outcome.expect_err("the panic reaches the caller");
    assert_eq!(payload.downcast_ref::<&str>(), Some(&"the work failed"));
    // And the bridge goes on serving.
    assert_eq!(runner.run(nap(7), PATIENCE).expect("an answer"), 7);
}

#[test]
fn a_wedged_bridge_is_reported_once_the_watchdog_runs_out() {
    let started = Instant::now();
    let outcome = runner().run(std::future::pending::<()>(), Duration::from_millis(100));
    assert!(matches!(outcome, Err(RunError::Watchdog(_))), "{outcome:?}");
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[tokio::test]
async fn the_bridge_can_be_dropped_inside_async_code() {
    let runner = runner();
    assert_eq!(runner.run(nap(8), PATIENCE).expect("an answer"), 8);
    drop(runner);
}

#[tokio::test]
async fn a_runtime_dropped_in_place_inside_async_code_panics() {
    // Why the runner shuts its runtime down in the background instead.
    let runtime = tokio::runtime::Runtime::new().expect("a runtime");
    let outcome = panic::catch_unwind(AssertUnwindSafe(|| drop(runtime)));
    assert!(outcome.is_err());
}

/// The settings for `url`, under an `application_name` of their own.
fn settings(url: &str, name: &str) -> PostgresSettings {
    let request = StorageRequest {
        backend: Some(Supplied::new(StorageBackend::Postgres, "--storage")),
        postgres_url: Some(Supplied::new(ConnectionString::new(url), "test URL")),
        ..StorageRequest::default()
    };
    let StorageSettings::Postgres(settings) =
        StorageSettings::resolve(request, StoreOwner::Command).expect("settings")
    else {
        panic!("not postgres");
    };
    let mut settings = *settings;
    settings.application_name = name.to_string();
    settings
}

/// A password that must never be shown.
const SENTINEL: &str = "pw-sentinel-417";

#[test]
fn the_bridges_work_runs_on_its_own_named_threads() {
    // Building the bridge connects nowhere.
    let bridge = Bridge::new(&settings(
        &format!("postgres://quant:{SENTINEL}@127.0.0.1:9/none"),
        "nowhere",
    ))
    .expect("a bridge");
    let name = bridge
        .shared
        .runner
        .run(
            async { std::thread::current().name().map(str::to_string) },
            PATIENCE,
        )
        .expect("an answer");
    assert_eq!(name.as_deref(), Some("qf-postgres"));
    assert!(!format!("{bridge:?}").contains(SENTINEL));
}

#[test]
fn a_database_that_does_not_answer_is_unavailable_and_never_shown_the_password() {
    let mut settings = settings(
        &format!("postgres://quant:{SENTINEL}@127.0.0.1:9/none"),
        "nowhere",
    );
    settings.connect_timeout = Duration::from_millis(500);
    // The system's trust store takes seconds on a busy Mac, once per process:
    // what is timed here is the answer.
    tls::connector(settings.connection.tls()).expect("the system's roots");
    let started = Instant::now();
    let refused = PostgresStore::connect(&settings).expect_err("nothing listens on port 9");
    assert!(
        matches!(refused, StorageError::Unavailable(_)),
        "{refused:?}"
    );
    assert!(started.elapsed() < PATIENCE, "{:?}", started.elapsed());
    let shown = format!("{refused} {refused:?}");
    assert!(!shown.contains(SENTINEL), "the password leaked");
}

#[test]
fn getting_a_connection_spends_the_calls_deadline_and_running_out_is_unavailable() {
    // Takes connections into its backlog and never answers, so opening one
    // would last the whole connect timeout.
    let silent = std::net::TcpListener::bind("127.0.0.1:0").expect("a listener");
    let port = silent.local_addr().expect("its address").port();
    let mut settings = settings(
        &format!("postgres://quant:{SENTINEL}@127.0.0.1:{port}/none"),
        "silent",
    );
    settings.connect_timeout = Duration::from_secs(60);
    settings.call_deadline = Duration::from_millis(300);
    let bridge = Bridge::new(&settings).expect("a bridge");
    let started = Instant::now();
    // Even a write: nothing was sent, so nothing can have been written.
    let refused = bridge
        .execute(Effect::Write, |client| {
            Box::pin(async move { client.batch_execute("SELECT 1").await })
        })
        .expect_err("no connection in time");
    assert!(
        matches!(&refused, StorageError::Unavailable(what) if what == "no connection within 300 ms"),
        "{refused:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
}

/// A port that accepts and then says nothing, as something other than
/// PostgreSQL may: only the pool's limit ends the wait, in words of ours.
#[test]
fn a_listener_that_never_answers_ends_at_the_connect_timeout_and_says_so() {
    let silent = std::net::TcpListener::bind("127.0.0.1:0").expect("a listener");
    let port = silent.local_addr().expect("its address").port();
    let mut settings = settings(
        &format!("postgres://quant:{SENTINEL}@127.0.0.1:{port}/none?sslmode=disable"),
        "silent",
    );
    settings.connect_timeout = Duration::from_millis(300);
    let refused = PostgresStore::connect(&settings).expect_err("nothing ever answers");
    assert!(
        matches!(&refused, StorageError::Unavailable(message)
            if message.contains("the server did not answer in time")),
        "{refused:?}"
    );
}

#[test]
fn the_tls_connector_takes_the_system_roots_and_refuses_a_ca_file_without_certificates() {
    let verified = |ca_file| PostgresTls {
        mode: TlsMode::Require,
        ca_file,
    };
    tls::connector(&verified(None)).expect("the system roots");
    let dir = tempfile::tempdir().expect("tempdir");
    let empty = dir.path().join("empty.pem");
    std::fs::write(&empty, "no certificate here\n").expect("write");
    let Err(refused) = tls::connector(&verified(Some(empty))) else {
        panic!("a CA file without certificates was accepted");
    };
    assert!(
        refused.to_string().contains("holds no PEM certificate"),
        "{refused}"
    );
    let missing = dir.path().join("missing.pem");
    assert!(tls::connector(&verified(Some(missing))).is_err());
}
