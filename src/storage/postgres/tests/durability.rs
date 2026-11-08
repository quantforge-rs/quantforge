//! What a commit means on a real server, and how long a call may take: each
//! category from a failure the server really reports, a real deadlock, a
//! call cut off mid-way, bounded calls against a full pool and a slow
//! statement, and the settings every connection gets. Each test works on
//! the server `QUANTFORGE_TEST_POSTGRES_URL` names, and skips without the
//! variable. A server with `fsync` off, named by
//! `QUANTFORGE_TEST_POSTGRES_WEAK_URL`, shows the refusal of a weak one.

use super::database::{DATABASE, SCHEDULING, WEAK_DATABASE, database_url, sleep, unique};
use super::scratch::{Scratch, rows};
use super::segments::{Held, wait_for_a_lock};
use super::*;
use crate::storage::retry_conflicts;
use crate::storage::settings::Durability;
use crate::{
    Candle, CandleQuery, CandleStore, ExchangeId, Interval, MarketId, StorageErrorKind,
    StoreControl, Symbol,
};
use rust_decimal::Decimal;
use std::sync::atomic::{AtomicUsize, Ordering};

/// 2026-10-20T00:00:00Z.
const OPEN: i64 = 1_792_454_400_000;

const THE_CANDLE: &str = "exchange = 'bybit_linear' AND symbol = 'BTCUSDT' AND interval = '1m' \
     AND open_time_ms = 1792454400000";

const THE_MARK: &str = "exchange = 'bybit_linear' AND symbol = 'BTCUSDT' AND interval = '1m' \
     AND window_start_ms = 1792454400000";

/// Ends the sessions of the store's database that wait on a lock.
const END_THE_WAITERS: &str = "SELECT pg_terminate_backend(pid) FROM pg_stat_activity \
     WHERE datname = current_database() AND wait_event_type = 'Lock'";

fn market() -> MarketId {
    MarketId::new(
        ExchangeId::BybitLinear,
        Symbol::new("BTCUSDT").expect("symbol"),
        Interval::M1,
    )
}

fn candle(close: &str) -> Candle {
    let close = Decimal::from_str_exact(close).expect("a decimal");
    Candle {
        open_time_ms: OPEN,
        close_time_ms: OPEN + 59_999,
        open: close,
        high: close,
        low: close,
        close,
        volume: Decimal::ONE,
        trades: Some(3),
    }
}

/// A scratch database whose store has run `init`, or `None` to skip.
fn fresh() -> Option<(Scratch, PostgresStore)> {
    let url = database_url(DATABASE)?;
    let scratch = Scratch::create(&url);
    let store = scratch.store();
    store.init().expect("init");
    Some((scratch, store))
}

fn setting(store: &PostgresStore, name: &str) -> String {
    let report = store.durability().expect("the report");
    report
        .settings()
        .iter()
        .find(|setting| setting.name == name)
        .map(|setting| setting.effective.clone())
        .unwrap_or_else(|| panic!("no {name} in {report}"))
}

#[test]
fn each_failure_the_server_reports_falls_in_one_category() {
    let Some(url) = database_url(DATABASE) else {
        return;
    };
    use StorageErrorKind::{Conflict, Contention, Other, OutcomeUnknown, Unavailable};
    let cases = [
        // SQLSTATE, then what it is for a read and for a write.
        ("40001", Conflict, Conflict),
        ("40P01", Conflict, Conflict),
        ("55P03", Contention, Contention),
        ("53100", Unavailable, Unavailable),
        ("53300", Unavailable, Unavailable),
        ("57014", Unavailable, OutcomeUnknown),
        ("57P01", Unavailable, OutcomeUnknown),
        ("57P02", Unavailable, OutcomeUnknown),
        ("08006", Unavailable, OutcomeUnknown),
        ("23505", Other, Other),
        ("42P01", Other, Other),
    ];
    let store = PostgresStore::connect(&settings(&url, &unique("kinds"))).expect("a store");
    for (code, as_read, as_write) in cases {
        for (effect, kind) in [(Effect::Read, as_read), (Effect::Write, as_write)] {
            let raise =
                format!("DO $$ BEGIN RAISE EXCEPTION 'raised' USING ERRCODE = '{code}'; END $$");
            let failure = store
                .bridge
                .execute(effect, move |client| {
                    Box::pin(async move { client.batch_execute(&raise).await })
                })
                .expect_err("raised");
            assert_eq!(failure.kind(), kind, "{code} as a {effect:?}: {failure}");
        }
    }
}

#[test]
fn a_real_deadlock_is_a_conflict_and_running_the_transaction_again_goes_through() {
    let Some((scratch, store)) = fresh() else {
        return;
    };
    store
        .upsert_candles(&market(), &[candle("100.10")])
        .expect("a candle and its mark");
    // Room for the other session to join the cycle on a loaded machine: the
    // store looks for a deadlock once, this long after it starts waiting.
    scratch.set_for_new_connections("deadlock_timeout", "'3s'");

    // The other session holds the mark, and takes the candle only once the
    // store holds it and waits for the mark. The session that looks for the
    // deadlock first is the one the server rolls back, so the other looks
    // only after a minute: waiting first is not enough, since a store held
    // up for a moment on a loaded machine would look second.
    let other = Held::start(
        &scratch,
        format!(
            "SET LOCAL deadlock_timeout = '1min'; \
             SELECT id FROM segment_dirty WHERE {THE_MARK} FOR UPDATE"
        ),
        format!("UPDATE candles SET close = '8' WHERE {THE_CANDLE}"),
    );
    let attempts = AtomicUsize::new(0);
    let writer = scratch.store();
    let written = std::thread::scope(|scope| {
        let attempts = &attempts;
        let writing = scope.spawn(move || {
            retry_conflicts(Duration::from_secs(20), |left| {
                attempts.fetch_add(1, Ordering::Relaxed);
                writer
                    .within(left)
                    .upsert_candles(&market(), &[candle("9")])
            })
        });
        wait_for_a_lock(&store);
        other.finish();
        writing.join().expect("the writer")
    });

    assert_eq!(written.expect("the second attempt"), 1);
    assert_eq!(
        attempts.load(Ordering::Relaxed),
        2,
        "one conflict, one retry"
    );
    let stored = store
        .load_candles(&market(), CandleQuery::default())
        .expect("the candles");
    assert_eq!(stored[0].close.to_string(), "9");
}

#[test]
fn a_write_cut_off_while_it_waits_has_an_unknown_outcome_and_a_read_is_unavailable() {
    let Some((scratch, store)) = fresh() else {
        return;
    };
    let held = Held::start(
        &scratch,
        "LOCK TABLE candles IN ACCESS EXCLUSIVE MODE".to_string(),
        String::new(),
    );
    let (writer, reader) = (scratch.store(), scratch.store());
    let write = std::thread::spawn(move || writer.upsert_candles(&market(), &[candle("1")]));
    wait_for_a_lock(&store);
    rows(&store, END_THE_WAITERS);
    let refused = write.join().expect("the write").expect_err("cut off");
    assert_eq!(
        refused.kind(),
        StorageErrorKind::OutcomeUnknown,
        "{refused}"
    );

    let read = std::thread::spawn(move || reader.load_candles(&market(), CandleQuery::default()));
    wait_for_a_lock(&store);
    rows(&store, END_THE_WAITERS);
    let refused = read.join().expect("the read").expect_err("cut off");
    assert_eq!(refused.kind(), StorageErrorKind::Unavailable, "{refused}");

    // The session is gone, and the store goes on without it.
    held.finish();
    assert_eq!(
        store
            .upsert_candles(&market(), &[candle("1")])
            .expect("a later write"),
        1
    );
}

#[test]
fn a_bounded_call_refuses_a_full_pool_inside_its_budget() {
    let Some(url) = database_url(DATABASE) else {
        return;
    };
    let mut settings = settings(&url, &unique("bounded_pool"));
    settings.pool_size = 1;
    let store = Arc::new(PostgresStore::connect(&settings).expect("a store"));
    let holder = Arc::clone(&store);
    let held = std::thread::spawn(move || sleep(&holder, Effect::Read, 2.0));
    // The holder is on the only connection once a ping cannot get one.
    let bounded = store.within(Duration::from_millis(500));
    let mut refused = None;
    let deadline = Instant::now() + PATIENCE;
    while refused.is_none() && Instant::now() < deadline {
        let started = Instant::now();
        refused = bounded.ping().err().map(|error| (error, started.elapsed()));
    }
    let (refused, took) = refused.expect("the pool filled up");
    assert_eq!(refused.kind(), StorageErrorKind::Unavailable, "{refused}");
    assert!(took < Duration::from_millis(500) + SCHEDULING, "{took:?}");
    held.join().expect("the holder").expect("its sleep");
}

#[test]
fn a_bounded_call_is_stopped_by_the_server_and_leaves_the_session_as_it_found_it() {
    let Some(url) = database_url(DATABASE) else {
        return;
    };
    let mut settings = settings(&url, &unique("bounded_statement"));
    settings.pool_size = 1;
    let store = PostgresStore::connect(&settings).expect("a store");
    let session = "SELECT pg_backend_pid()::text, current_setting('lock_timeout'), \
         current_setting('statement_timeout')";
    let before = rows(&store, session);
    assert_eq!(before[0][1].as_deref(), Some("5s"));
    assert_eq!(before[0][2].as_deref(), Some("30s"));

    let limits = "SELECT current_setting('lock_timeout'), current_setting('statement_timeout')";
    let budget = Duration::from_secs(2);
    assert_ne!(rows(&store.within(budget), limits)[0][0], before[0][1]);
    assert_eq!(
        rows(&store.within(Duration::from_secs(600)), limits),
        [[before[0][1].clone(), before[0][2].clone()]],
        "a budget never loosens the session"
    );

    for (effect, kind) in [
        (Effect::Read, StorageErrorKind::Unavailable),
        (Effect::Write, StorageErrorKind::OutcomeUnknown),
    ] {
        let started = Instant::now();
        let refused = sleep(&store.within(budget), effect, 30.0).expect_err("past its budget");
        assert_eq!(refused.kind(), kind, "{refused}");
        let took = started.elapsed();
        assert!(took < budget + SCHEDULING, "{took:?}");
    }
    store
        .within(budget)
        .ping()
        .expect("a bounded call that fits");

    // The server stopped it, so the connection is the same one.
    assert_eq!(rows(&store, session), before);
}

#[test]
fn a_connection_that_falls_short_is_dropped_before_it_is_used_again() {
    let Some(url) = database_url(DATABASE) else {
        return;
    };
    let mut settings = settings(&url, &unique("fell_short"));
    settings.pool_size = 1;
    let store = PostgresStore::connect(&settings).expect("a store");
    let backend = "SELECT pg_backend_pid()::text";
    let first = rows(&store, backend);
    assert_eq!(rows(&store, backend), first, "the pool keeps a good one");

    // A session weakened behind the store's back.
    rows(&store, "SET synchronous_commit = off");
    assert_ne!(rows(&store, backend), first, "a new connection");
    assert_eq!(setting(&store, "synchronous_commit"), "on");
}

#[test]
fn a_connection_raises_a_commit_that_would_not_wait_and_never_lowers_one() {
    let Some((scratch, store)) = fresh() else {
        return;
    };
    assert_eq!(setting(&store, "synchronous_commit"), "on");
    for (database_default, effective) in [
        ("off", "local"),
        ("local", "local"),
        ("remote_write", "remote_write"),
        ("remote_apply", "remote_apply"),
    ] {
        scratch.set_for_new_connections("synchronous_commit", database_default);
        assert_eq!(
            setting(&scratch.store(), "synchronous_commit"),
            effective,
            "the database's default is {database_default}"
        );
    }
}

/// Runs `sql` on `store` and waits until the server's `fsync` reads
/// `expected`.
fn set_fsync(store: &PostgresStore, sql: &str, expected: &str) {
    rows(store, sql);
    rows(store, "SELECT pg_reload_conf()");
    let deadline = Instant::now() + PATIENCE;
    while setting(store, "fsync") != expected {
        assert!(Instant::now() < deadline, "fsync never became {expected}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Everything on the one server with `fsync` off, in one test, since part
/// of it switches `fsync` on and back.
#[test]
fn a_weak_server_is_refused_when_enforced_and_reported_otherwise() {
    let Some(url) = database_url(WEAK_DATABASE) else {
        return;
    };
    let enforcing = settings(&url, &unique("weak_enforced"));
    assert_eq!(enforcing.limits.durability, Durability::Enforced);
    let refused = PostgresStore::connect(&enforcing).expect_err("fsync is off");
    assert_eq!(
        refused.kind(),
        StorageErrorKind::DurabilityUnmet,
        "{refused}"
    );
    let message = refused.to_string();
    assert!(message.contains("fsync is off, and needs on"), "{message}");
    assert!(message.contains("server's configuration"), "{message}");
    let parsed = url::Url::parse(&url).expect("a URL");
    assert!(!message.contains(&url), "{message}");
    assert!(
        !message.contains(parsed.password().expect("a password")),
        "{message}"
    );

    let mut reporting = settings(&url, &unique("weak_reported"));
    reporting.limits.durability = Durability::Reported;
    let store = PostgresStore::connect(&reporting).expect("a store that reports");
    let report = store.durability().expect("the report");
    assert!(!report.is_met(), "{report}");
    assert!(
        report
            .problem()
            .expect("a problem")
            .contains("fsync is off"),
        "{report}"
    );

    // A server weakened while a store runs is found at its next call.
    set_fsync(&store, "ALTER SYSTEM SET fsync = on", "on");
    let running = PostgresStore::connect(&enforcing).expect("a server that keeps its commits");
    running.ping().expect("a call");
    set_fsync(&store, "ALTER SYSTEM RESET fsync", "off");
    let refused = running.ping().expect_err("fsync went off");
    assert_eq!(
        refused.kind(),
        StorageErrorKind::DurabilityUnmet,
        "{refused}"
    );
}
