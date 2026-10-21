//! The bridge's runner answers from every kind of thread QuantForge calls
//! storage from, refuses the calls that would hang, and never panics where
//! tokio would. No database needed.

use std::panic::{self, AssertUnwindSafe};
use std::sync::Arc;
use std::time::{Duration, Instant};

use postgres_bridge_spike::runner::{RunError, Runner};

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
fn a_panic_in_the_work_is_raised_on_the_caller() {
    let runner = runner();
    let outcome = panic::catch_unwind(AssertUnwindSafe(|| {
        runner.run(async { panic!("the work failed") }, PATIENCE)
    }));
    let payload = outcome.expect_err("the panic reaches the caller");
    assert_eq!(payload.downcast_ref::<&str>(), Some(&"the work failed"));
    // And the bridge goes on serving.
    assert_eq!(runner.run(nap(6), PATIENCE).expect("an answer"), 6);
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
    assert_eq!(runner.run(nap(7), PATIENCE).expect("an answer"), 7);
    drop(runner);
}

#[tokio::test]
async fn a_runtime_dropped_in_place_inside_async_code_panics() {
    // Why the runner shuts its runtime down in the background instead.
    let runtime = tokio::runtime::Runtime::new().expect("a runtime");
    let outcome = panic::catch_unwind(AssertUnwindSafe(|| drop(runtime)));
    assert!(outcome.is_err());
}
