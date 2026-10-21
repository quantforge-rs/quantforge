//! Trusted Python embedding: Python, embedded in a process where a tokio
//! runtime runs as in the CLI or the server, reads candles through the
//! bridge. The calls work from Python's main thread and from a tokio worker,
//! and a call blocked on the database releases the GIL, so other Python
//! threads run meanwhile.
//!
//! A test without the libtest harness (`harness = false`), so that Python's
//! main thread is this process's main thread. Build with the `python` feature
//! and `PYO3_PYTHON` naming the interpreter to embed.

mod common;

use std::ffi::CString;
use std::sync::OnceLock;
use std::time::Duration;

use postgres_bridge_spike::bridge::{BridgeConfig, Effect};
use postgres_bridge_spike::store::PgCandleStore;
use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use quantforge::{CandleQuery, CandleStore};

use common::{market, tricky_candles, unique};

static STORE: OnceLock<PgCandleStore> = OnceLock::new();

fn store() -> PyResult<&'static PgCandleStore> {
    STORE
        .get()
        .ok_or_else(|| PyRuntimeError::new_err("no store"))
}

/// How many candles the store holds; blocks with the GIL released.
#[pyfunction]
fn count_candles(py: Python<'_>) -> PyResult<usize> {
    let store = store()?;
    py.detach(|| store.load_candles(&market(), CandleQuery::default()))
        .map(|candles| candles.len())
        .map_err(|error| PyRuntimeError::new_err(error.to_string()))
}

fn pg_sleep(store: &PgCandleStore, seconds: f64) -> PyResult<()> {
    store
        .bridge()
        .execute(Effect::Read, move |client| {
            Box::pin(async move {
                client
                    .batch_execute(&format!("SELECT pg_sleep({seconds})"))
                    .await
            })
        })
        .map_err(|error| PyRuntimeError::new_err(error.to_string()))
}

/// Waits `seconds` in the database with the GIL released.
#[pyfunction]
fn sleep(py: Python<'_>, seconds: f64) -> PyResult<()> {
    let store = store()?;
    py.detach(|| pg_sleep(store, seconds))
}

/// The control: the same wait, holding the GIL.
#[pyfunction]
fn sleep_holding_the_gil(seconds: f64) -> PyResult<()> {
    pg_sleep(store()?, seconds)
}

const CHECKS: &str = r#"
import sys, threading, time
import qfspike

assert sys.version_info >= (3, 9), sys.version_info
assert qfspike.count_candles() == 5

def four_at_once(sleep):
    threads = [threading.Thread(target=sleep, args=(0.3,)) for _ in range(4)]
    started = time.monotonic()
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join()
    return time.monotonic() - started

released = four_at_once(qfspike.sleep)
held = four_at_once(qfspike.sleep_holding_the_gil)
print(f"python {sys.version.split()[0]}: four 0.3 s waits took {released:.2f} s with the GIL released, {held:.2f} s holding it", flush=True)
assert released < 1.0, released
assert held >= 1.15, held
"#;

fn register(py: Python<'_>) -> PyResult<()> {
    let module = PyModule::new(py, "qfspike")?;
    module.add_function(wrap_pyfunction!(count_candles, &module)?)?;
    module.add_function(wrap_pyfunction!(sleep, &module)?)?;
    module.add_function(wrap_pyfunction!(sleep_holding_the_gil, &module)?)?;
    py.import("sys")?
        .getattr("modules")?
        .set_item("qfspike", module)
}

fn main() {
    let Some(url) = common::database_url() else {
        eprintln!("skipping: SPIKE_DATABASE_URL is unset");
        return;
    };
    // As in the CLI and the server, a tokio runtime runs in this process.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("a runtime");

    let mut config = BridgeConfig::new(&url, unique("python"));
    config.max_size = 4;
    let store = PgCandleStore::connect(&config, &unique("python")).expect("a store");
    CandleStore::init(&store).expect("init");
    store
        .upsert_candles(&market(), &tricky_candles())
        .expect("candles");
    let _ = STORE.set(store);

    let checks = CString::new(CHECKS).expect("a script");
    Python::attach(|py| {
        register(py)?;
        py.run(&checks, None, None)
    })
    .expect("the checks on Python's main thread");

    // Python called from a tokio worker, as a server handler would.
    let on_worker = runtime.block_on(async {
        tokio::spawn(async {
            Python::attach(|py| {
                py.run(
                    c"import qfspike; assert qfspike.count_candles() == 5",
                    None,
                    None,
                )
            })
        })
        .await
    });
    on_worker
        .expect("the worker task")
        .expect("the checks on a tokio worker");

    // Dropping the runtime here is fine: this is not async code.
    drop(runtime);
    std::thread::sleep(Duration::from_millis(10));
    println!("python spike: ok");
}
