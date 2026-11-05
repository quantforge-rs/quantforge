//! The segment store against a real database, at the points the ports
//! cannot pause it: the races of the shared cases through its own read and
//! write-back; a write-back that waits on a correction's lock; and a window
//! and a plan read while a commit lands, each still one snapshot. A
//! transaction held open from a thread makes the other side wait at a known
//! statement. Each test works in a scratch database of its own on the server
//! `QUANTFORGE_TEST_POSTGRES_URL` names, and skips without the variable.

use super::database::{DATABASE, database_url};
use super::scratch::{Scratch, rows};
use super::*;
use crate::storage::races;
use crate::storage::refresh::{DirtyWindows, Written, pack};
use crate::storage::segments as format;
use crate::{Candle, CandleStore, ExchangeId, Interval, MarketId, SegmentStore, Symbol};
use rust_decimal::Decimal;
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use tokio::sync::oneshot;

/// 2026-10-20T00:00:00Z: the start of a `1m` window, one day long.
const WINDOW: i64 = 1_792_454_400_000;
const DAY: i64 = 86_400_000;

const MARKET: &str = "exchange = 'bybit_linear' AND symbol = 'BTCUSDT' AND interval = '1m'";

/// What `upsert_candles` does to the window's first candle, as raw SQL that
/// can stay uncommitted: its close changes, and its mark gets a revision.
fn correction() -> String {
    format!(
        "UPDATE candles SET close = '7' WHERE {MARKET} AND open_time_ms = {WINDOW}; \
         INSERT INTO segment_dirty(exchange, symbol, interval, window_start_ms) \
         VALUES ('bybit_linear', 'BTCUSDT', '1m', {WINDOW}) \
         ON CONFLICT (exchange, symbol, interval, window_start_ms) DO UPDATE SET id = DEFAULT;"
    )
}

fn market() -> MarketId {
    MarketId::new(
        ExchangeId::BybitLinear,
        Symbol::new("BTCUSDT").expect("symbol"),
        Interval::M1,
    )
}

fn candle(open_time_ms: i64) -> Candle {
    let close = Decimal::from_str_exact("100.10").expect("a decimal");
    Candle {
        open_time_ms,
        close_time_ms: open_time_ms + 59_999,
        open: close,
        high: close,
        low: close,
        close,
        volume: Decimal::ONE,
        trades: Some(3),
    }
}

/// A scratch database whose store holds one dirty window, closed by a
/// candle of the next one, or `None` to skip.
fn seeded() -> Option<(Scratch, PostgresStore)> {
    let server = database_url(DATABASE)?;
    let scratch = Scratch::create(&server);
    let store = scratch.store();
    store.init().expect("init");
    store
        .upsert_candles(&market(), &[candle(WINDOW), candle(WINDOW + DAY)])
        .expect("candles");
    Some((scratch, store))
}

/// A transaction on a store of its own, held open until [`Held::finish`].
pub(super) struct Held {
    finish: oneshot::Sender<()>,
    ended: JoinHandle<()>,
}

impl Held {
    /// Runs `first` in a new transaction and returns with it open.
    /// [`Held::finish`] then runs `last` and commits it.
    pub(super) fn start(scratch: &Scratch, first: String, last: String) -> Self {
        let store = scratch.store();
        let (started, running) = mpsc::channel();
        let (finish, finishing) = oneshot::channel();
        let ended = thread::spawn(move || {
            store
                .bridge
                .execute(Effect::Write, move |client| {
                    Box::pin(async move {
                        let transaction = client.transaction().await?;
                        transaction.batch_execute(&first).await?;
                        let _ = started.send(());
                        let _ = finishing.await;
                        transaction.batch_execute(&last).await?;
                        transaction.commit().await
                    })
                })
                .expect("the held transaction");
        });
        running.recv().expect("the transaction started");
        Self { finish, ended }
    }

    pub(super) fn finish(self) {
        let _ = self.finish.send(());
        self.ended.join().expect("the held transaction ended");
    }
}

/// Waits until a session of the store's database waits on a lock.
pub(super) fn wait_for_a_lock(store: &PostgresStore) {
    let deadline = Instant::now() + 2 * PATIENCE;
    loop {
        let waiting = rows(
            store,
            "SELECT COUNT(*) FROM pg_stat_activity \
             WHERE datname = current_database() AND wait_event_type = 'Lock'",
        );
        if waiting != [[Some("0".to_string())]] {
            return;
        }
        assert!(Instant::now() < deadline, "no session waited on a lock");
        thread::sleep(Duration::from_millis(10));
    }
}

fn window() -> std::ops::Range<i64> {
    format::window(Interval::M1, WINDOW).expect("a window")
}

fn count(store: &PostgresStore, table: &str) -> String {
    rows(store, &format!("SELECT COUNT(*) FROM {table}"))[0][0]
        .clone()
        .expect("a count")
}

#[test]
fn every_race_of_the_shared_cases_ends_as_the_cases_say() {
    let Some(server) = database_url(DATABASE) else {
        return;
    };
    races::every_race(|cases, race| {
        let scratch = Scratch::create(&server);
        let store = scratch.store();
        store.init().expect("init");
        races::step_through(cases, race, &store, || &store);
    });
}

#[test]
fn a_write_back_waiting_on_a_correction_finds_its_revision_and_publishes_nothing() {
    let Some((scratch, store)) = seeded() else {
        return;
    };
    let snapshot = (&store)
        .read_window(&market(), &window())
        .expect("read")
        .expect("a dirty window");
    let packed = pack(&snapshot).expect("pack");

    // The correction holds the mark, so the write-back's lock waits for it,
    // and then reads the revision it left.
    let correction = Held::start(&scratch, correction(), String::new());
    let writer = scratch.store();
    let written = thread::spawn(move || {
        (&writer)
            .write_window(&market(), WINDOW, snapshot.id, &packed)
            .expect("write back")
    });
    wait_for_a_lock(&store);
    correction.finish();
    assert!(matches!(
        written.join().expect("the write-back"),
        Written::Marked
    ));
    assert_eq!(count(&store, "segments"), "0");
    assert_eq!(count(&store, "segment_index"), "0");

    // The next refresh packs the correction.
    assert_eq!(store.refresh_segments(&market()).expect("refresh").built, 1);
    let plan = store
        .resolve_segments(&market(), WINDOW, WINDOW)
        .expect("a plan");
    let bytes = store
        .load_segment(&plan.segments[0].hash)
        .expect("load")
        .expect("stored bytes");
    let candles = format::open(&bytes, &plan.segments[0].hash).expect("open");
    assert_eq!(candles[0].close.to_string(), "7");
}

#[test]
fn a_window_is_one_snapshot_whatever_commits_while_it_is_read() {
    let Some((scratch, store)) = seeded() else {
        return;
    };
    let revision = rows(
        &store,
        &format!("SELECT id FROM segment_dirty WHERE {MARKET} AND window_start_ms = {WINDOW}"),
    )[0][0]
        .clone()
        .expect("a revision");

    // The read takes its snapshot on the mark, then waits for the candles
    // while a correction commits both.
    let correction = Held::start(
        &scratch,
        "LOCK TABLE candles IN ACCESS EXCLUSIVE MODE".to_string(),
        correction(),
    );
    let reader = scratch.store();
    let read = thread::spawn(move || (&reader).read_window(&market(), &window()));
    wait_for_a_lock(&store);
    correction.finish();
    let snapshot = read
        .join()
        .expect("the read")
        .expect("read")
        .expect("a dirty window");

    assert_eq!(snapshot.id.to_string(), revision, "the mark it began with");
    assert_eq!(
        snapshot.candles[0].decimals[3], "100.10",
        "the candles of that same moment"
    );
    // So what it packed is refused.
    let packed = pack(&snapshot).expect("pack");
    let written = (&store)
        .write_window(&market(), WINDOW, snapshot.id, &packed)
        .expect("write back");
    assert!(matches!(written, Written::Marked));
}

#[test]
fn a_plan_is_one_snapshot_whatever_commits_while_it_is_read() {
    let Some((scratch, store)) = seeded() else {
        return;
    };
    assert_eq!(store.refresh_segments(&market()).expect("refresh").built, 1);
    let before = store
        .resolve_segments(&market(), WINDOW, WINDOW)
        .expect("a plan");
    let replacement = format!("sha256:{}", "ab".repeat(32));

    // The plan takes its snapshot on the tip, then waits for the index
    // while the entry moves to another segment.
    let publish = Held::start(
        &scratch,
        "LOCK TABLE segment_index IN ACCESS EXCLUSIVE MODE".to_string(),
        format!(
            "INSERT INTO segments(hash, format, payload_len, data) \
             VALUES ('{replacement}', 1, 1, '\\x00'); \
             UPDATE segment_index SET hash = '{replacement}' \
             WHERE {MARKET} AND window_start_ms = {WINDOW};"
        ),
    );
    let reader = scratch.store();
    let plan = thread::spawn(move || reader.resolve_segments(&market(), WINDOW, WINDOW));
    wait_for_a_lock(&store);
    publish.finish();
    assert_eq!(plan.join().expect("the plan").expect("a plan"), before);

    let after = store
        .resolve_segments(&market(), WINDOW, WINDOW)
        .expect("a plan");
    assert_eq!(after.segments[0].hash.to_string(), replacement);
}
