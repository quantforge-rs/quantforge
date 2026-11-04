//! The candle store's writes against the segment protocol of
//! `docs/storage-schema.md` ("Segments on PostgreSQL"): a batch writes its
//! candles and the marks of the windows they change in one transaction,
//! and every mark is a fresh revision. A refresh's write-back is played
//! here in raw SQL, one statement at a time as the segment store runs it,
//! against the real `upsert_candles`: a correction racing it either waits
//! for it and then marks the window again, or commits first and stops it
//! publishing, so a window is never published as clean over candles it does
//! not pack.
//! Without `QUANTFORGE_TEST_POSTGRES_URL` every test skips.

use quantforge::storage::postgres::PostgresStore;
use quantforge::{Candle, CandleStore, ExchangeId, Interval, MarketId, Symbol};
use rust_decimal::Decimal;
use std::time::{Duration, Instant};
use tokio_postgres::Client;
use tokio_postgres::types::Type;

use super::{IsolatedDatabase, Postgres, VARIABLE, client, query, server_url};
use crate::conformance::{Backend, Fixture};

/// 2026-10-20T00:00:00Z: the start of a `1m` window.
const WINDOW: i64 = 1_792_454_400_000;

fn market() -> MarketId {
    MarketId::new(
        ExchangeId::BybitLinear,
        Symbol::new("BTCUSDT").expect("symbol"),
        Interval::M1,
    )
}

fn candle(open_time_ms: i64, close: &str) -> Candle {
    let close = Decimal::from_str_exact(close).expect("a decimal");
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

/// A database of its own whose store has run `init`, or `None` to skip.
fn fresh() -> Option<(Fixture<PostgresStore>, String)> {
    if server_url().is_none() {
        eprintln!("skipping: {VARIABLE} is unset");
        return None;
    }
    let fixture = Postgres::fresh();
    let url = fixture
        .guard
        .downcast_ref::<IsolatedDatabase>()
        .expect("an isolated database")
        .url
        .clone();
    Some((fixture, url))
}

const MARK_OF_WINDOW: &str = "SELECT id FROM segment_dirty WHERE exchange = 'bybit_linear' \
     AND symbol = 'BTCUSDT' AND interval = '1m' AND window_start_ms = 1792454400000";

/// The window's mark revisions, oldest first: none when it is clean.
fn marks(url: &str) -> Vec<i64> {
    query(url, &format!("{MARK_OF_WINDOW} ORDER BY id"))
        .into_iter()
        .map(|row| {
            row.into_iter()
                .next()
                .flatten()
                .expect("an id")
                .parse()
                .expect("a number")
        })
        .collect()
}

/// The window's mark revision as a write-back reads it: locked until the
/// transaction ends.
async fn lock_mark(client: &Client) -> Option<i64> {
    client
        .query_typed_opt(&format!("{MARK_OF_WINDOW} FOR UPDATE"), &[])
        .await
        .expect("the mark")
        .map(|row| row.try_get(0).expect("an id"))
}

/// Publishes a segment for the window and removes the mark revision the
/// write-back read, as the write-back of a refresh does.
async fn publish(client: &Client, revision: i64) {
    client
        .batch_execute(&format!(
            "INSERT INTO segments(hash, format, payload_len, data) \
             VALUES ('sha256:{0}', 1, 1, '\\x00') ON CONFLICT (hash) DO NOTHING; \
             INSERT INTO segment_index VALUES ('bybit_linear', 'BTCUSDT', '1m', {WINDOW}, \
             {WINDOW}, {WINDOW}, 1, 'sha256:{0}') \
             ON CONFLICT (exchange, symbol, interval, window_start_ms) \
             DO UPDATE SET hash = excluded.hash",
            "ab".repeat(32)
        ))
        .await
        .expect("the segment and its entry");
    let removed = client
        .execute_typed(
            "DELETE FROM segment_dirty WHERE id = $1",
            &[(&revision, Type::INT8)],
        )
        .await
        .expect("the mark removed");
    assert_eq!(removed, 1, "the write-back removes the revision it read");
}

/// Waits until a session of this database waits on a lock, at most ten
/// seconds.
fn wait_for_a_lock(url: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let waiting = query(
            url,
            "SELECT CAST(COUNT(*) AS TEXT) FROM pg_stat_activity \
             WHERE datname = current_database() AND wait_event_type = 'Lock'",
        );
        if waiting != [[Some("0".to_string())]] {
            return;
        }
        assert!(Instant::now() < deadline, "no session waited on the lock");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_correction_during_a_write_back_waits_and_marks_the_window_again() {
    let Some((fixture, url)) = fresh() else {
        return;
    };
    let store = &fixture.store;
    store
        .upsert_candles(&market(), &[candle(WINDOW, "100.10")])
        .expect("the first candles");
    let [read] = marks(&url)[..] else {
        panic!("one mark expected: {:?}", marks(&url));
    };

    let runtime = tokio::runtime::Runtime::new().expect("a runtime");
    let write_back = runtime.block_on(client(&url));
    std::thread::scope(|scope| {
        runtime.block_on(async {
            write_back.batch_execute("BEGIN").await.expect("begin");
            assert_eq!(lock_mark(&write_back).await, Some(read));
        });
        // A correction to the window, while the write-back holds its mark.
        let correction =
            scope.spawn(|| store.upsert_candles(&market(), &[candle(WINDOW, "100.20")]));
        wait_for_a_lock(&url);
        runtime.block_on(async {
            publish(&write_back, read).await;
            write_back.batch_execute("COMMIT").await.expect("commit");
        });
        assert_eq!(
            correction
                .join()
                .expect("the correction's thread")
                .expect("the correction"),
            1
        );
    });
    // The segment is published, but the window is dirty again under a newer
    // revision, so no plan serves it and the next refresh repacks it.
    let after = marks(&url);
    assert!(
        matches!(after[..], [revision] if revision > read),
        "{read} then {after:?}"
    );
}

#[test]
fn a_correction_committed_first_stops_the_write_back_publishing() {
    let Some((fixture, url)) = fresh() else {
        return;
    };
    let store = &fixture.store;
    store
        .upsert_candles(&market(), &[candle(WINDOW, "100.10")])
        .expect("the first candles");
    // The refresh read its window under this revision, then packed it.
    let [read] = marks(&url)[..] else {
        panic!("one mark expected: {:?}", marks(&url));
    };
    store
        .upsert_candles(&market(), &[candle(WINDOW, "100.20")])
        .expect("the correction");
    let runtime = tokio::runtime::Runtime::new().expect("a runtime");
    let found = runtime.block_on(async {
        let write_back = client(&url).await;
        write_back.batch_execute("BEGIN").await.expect("begin");
        let found = lock_mark(&write_back).await;
        write_back
            .batch_execute("ROLLBACK")
            .await
            .expect("rollback");
        found
    });
    // Another revision than the one it read: the write-back publishes nothing.
    assert!(
        matches!(found, Some(revision) if revision > read),
        "{read} then {found:?}"
    );
}

#[test]
fn an_identical_resync_marks_nothing_and_a_change_of_scale_marks_again() {
    let Some((fixture, url)) = fresh() else {
        return;
    };
    let store = &fixture.store;
    let first = [candle(WINDOW, "100.10"), candle(WINDOW + 60_000, "100.20")];
    store
        .upsert_candles(&market(), &first)
        .expect("the candles");
    // A refresh packed the window.
    query(&url, "DELETE FROM segment_dirty");
    assert_eq!(
        store.upsert_candles(&market(), &first).expect("a re-sync"),
        2,
        "every candle given counts"
    );
    assert_eq!(marks(&url), Vec::<i64>::new(), "nothing changed");
    store
        .upsert_candles(&market(), &[candle(WINDOW, "100.1")])
        .expect("a change of scale");
    assert_eq!(marks(&url).len(), 1, "1.1 is not 1.10");
}

#[test]
fn a_batch_whose_marks_cannot_be_written_writes_no_candle_either() {
    let Some((fixture, url)) = fresh() else {
        return;
    };
    let store = &fixture.store;
    // The marks' table out of the way: the second statement of the batch
    // fails after the first wrote its candles.
    query(
        &url,
        "ALTER TABLE segment_dirty RENAME TO segment_dirty_away",
    );
    store
        .upsert_candles(&market(), &[candle(WINDOW, "100.10")])
        .expect_err("no table for the marks");
    query(
        &url,
        "ALTER TABLE segment_dirty_away RENAME TO segment_dirty",
    );
    assert_eq!(
        query(&url, "SELECT CAST(COUNT(*) AS TEXT) FROM candles"),
        [[Some("0".to_string())]],
        "the candles rolled back with the marks"
    );
    store
        .upsert_candles(&market(), &[candle(WINDOW, "100.10")])
        .expect("the same batch, with its table back");
    assert_eq!(marks(&url).len(), 1);
}
