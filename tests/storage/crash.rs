//! Acknowledged writes outlive the process that made them: a writer reports
//! each candle once the store answered, and is killed, or the server under
//! it is; every candle it reported must be there. This exercises an
//! application crash and a database crash, and says nothing about the
//! operating system or the power: killing a process leaves the page cache
//! standing, so these tests pass at any `synchronous` setting
//! (`docs/storage-durability.md`, "What was exercised").

use quantforge::{
    Candle, CandleQuery, CandleStore, ExchangeId, Interval, MarketId, SqliteStore, Symbol,
};
use rust_decimal::Decimal;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};

/// Set for the child: `sqlite:<path>` or `postgres:<url>`.
const WRITER: &str = "QF_TEST_CRASH_WRITER";

/// What the child prints once a write is answered.
const ACKNOWLEDGED: &str = "acknowledged ";

/// How many acknowledgements a crash waits for.
const ACKNOWLEDGEMENTS: usize = 200;

/// 2026-10-20T00:00:00Z.
const OPEN: i64 = 1_792_454_400_000;

fn market() -> MarketId {
    MarketId::new(
        ExchangeId::BybitSpot,
        Symbol::new("BTCUSDT").expect("symbol"),
        Interval::M1,
    )
}

fn candle(index: i64) -> Candle {
    let close = Decimal::from_str_exact("100.10").expect("a decimal");
    Candle {
        open_time_ms: OPEN + index * 60_000,
        close_time_ms: OPEN + index * 60_000 + 59_999,
        open: close,
        high: close,
        low: close,
        close,
        volume: Decimal::ONE,
        trades: Some(3),
    }
}

/// Writes candles and reports each one, until it is stopped from outside.
fn write_until_stopped(store: &impl CandleStore) {
    let mut index = 0;
    loop {
        store
            .upsert_candles(&market(), &[candle(index)])
            .expect("a write");
        println!("{ACKNOWLEDGED}{index}");
        index += 1;
    }
}

/// Not a test: the child the tests below start, which returns at once
/// everywhere else.
#[test]
fn writer() {
    let Ok(target) = std::env::var(WRITER) else {
        return;
    };
    if let Some(path) = target.strip_prefix("sqlite:") {
        write_until_stopped(&SqliteStore::new(path));
    }
    #[cfg(feature = "postgres")]
    if let Some(url) = target.strip_prefix("postgres:") {
        let store =
            quantforge::storage::postgres::PostgresStore::connect(&crate::postgres::settings(url))
                .expect("the database");
        write_until_stopped(&store);
    }
}

/// Starts this test binary as the writer.
fn start_writer(target: &str) -> Child {
    Command::new(std::env::current_exe().expect("this test binary"))
        .args(["crash::writer", "--exact", "--nocapture"])
        .env(WRITER, target)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("the writer")
}

/// Reads the writer's acknowledgements, crashes it with `crash` once there
/// are enough, and returns all it reported.
fn acknowledged_until_crash(mut child: Child, crash: impl FnOnce(&mut Child)) -> Vec<i64> {
    let stdout = child.stdout.take().expect("the writer's stdout");
    let mut lines = BufReader::new(stdout).lines();
    let mut acknowledged = Vec::new();
    let read = |line: String, acknowledged: &mut Vec<i64>| {
        if let Some(index) = line.strip_prefix(ACKNOWLEDGED) {
            acknowledged.push(index.parse().expect("an index"));
        }
    };
    for line in lines.by_ref() {
        read(line.expect("a line"), &mut acknowledged);
        if acknowledged.len() == ACKNOWLEDGEMENTS {
            break;
        }
    }
    assert_eq!(acknowledged.len(), ACKNOWLEDGEMENTS, "the writer stopped");
    crash(&mut child);
    // What it had reported by the time it died counts too.
    for line in lines.map_while(Result::ok) {
        read(line, &mut acknowledged);
    }
    child.wait().expect("the writer's end");
    acknowledged
}

fn every_acknowledged_candle_is_stored(store: &impl CandleStore, acknowledged: &[i64]) {
    let stored: Vec<i64> = store
        .load_candles(&market(), CandleQuery::default())
        .expect("the candles")
        .iter()
        .map(|candle| (candle.open_time_ms - OPEN) / 60_000)
        .collect();
    let missing: Vec<&i64> = acknowledged
        .iter()
        .filter(|index| !stored.contains(index))
        .collect();
    assert!(missing.is_empty(), "lost: {missing:?} of {stored:?}");
}

#[test]
fn acknowledged_writes_survive_killing_the_process_on_sqlite() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let path = tempdir.path().join("market.sqlite");
    let store = SqliteStore::new(&path);
    CandleStore::init(&store).expect("init");
    let child = start_writer(&format!("sqlite:{}", path.display()));
    let acknowledged = acknowledged_until_crash(child, |child| {
        child.kill().expect("killed");
    });
    every_acknowledged_candle_is_stored(&store, &acknowledged);
}

#[cfg(feature = "postgres")]
mod postgres {
    use super::*;
    use crate::conformance::Backend;
    use crate::postgres::{IsolatedDatabase, Postgres, VARIABLE, server_url};

    /// The cluster's data directory, for the test that stops it.
    const DATA_DIRECTORY: &str = "QUANTFORGE_TEST_POSTGRES_DATA_DIR";

    fn url_of(fixture: &crate::conformance::Fixture<impl Sized>) -> String {
        fixture
            .guard
            .downcast_ref::<IsolatedDatabase>()
            .expect("an isolated database")
            .url
            .clone()
    }

    #[test]
    fn acknowledged_writes_survive_killing_the_process_on_postgres() {
        if server_url().is_none() {
            eprintln!("skipping: {VARIABLE} is unset");
            return;
        }
        let fixture = Postgres::fresh();
        let child = start_writer(&format!("postgres:{}", url_of(&fixture)));
        let acknowledged = acknowledged_until_crash(child, |child| {
            child.kill().expect("killed");
        });
        every_acknowledged_candle_is_stored(&fixture.store, &acknowledged);
    }

    fn pg_ctl(directory: &str, arguments: &[&str]) {
        let status = Command::new("pg_ctl")
            .args(["-D", directory, "-w"])
            .args(arguments)
            .stdout(Stdio::null())
            .status()
            .expect("pg_ctl on the PATH");
        assert!(status.success(), "pg_ctl {arguments:?}: {status}");
    }

    /// An immediate shutdown ends every server process without a
    /// checkpoint, as a crash does, and the restart recovers from the
    /// write-ahead log. It stops the server, so it runs alone and on demand.
    #[test]
    #[ignore = "stops the server: run it alone, with QUANTFORGE_TEST_POSTGRES_DATA_DIR"]
    fn acknowledged_writes_survive_the_server_crashing() {
        let directory = std::env::var(DATA_DIRECTORY).expect("the data directory");
        let fixture = Postgres::fresh();
        let child = start_writer(&format!("postgres:{}", url_of(&fixture)));
        let acknowledged = acknowledged_until_crash(child, |_| {
            pg_ctl(&directory, &["stop", "-m", "immediate"]);
        });
        pg_ctl(&directory, &["start", "-l", &format!("{directory}.log")]);
        every_acknowledged_candle_is_stored(&fixture.store, &acknowledged);
    }
}
