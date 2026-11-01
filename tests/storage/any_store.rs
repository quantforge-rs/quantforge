//! The storage contract through [`AnyStore`], opened by the built-in
//! factory: every port reaches the SQLite store behind it. The raw reads,
//! and the answers compared, come from a `SqliteStore` on the same file.

use quantforge::{
    AnyStore, Candle, CandleStore, Environment, ExchangeId, Interval, MarketId, SegmentRefresh,
    SegmentStore, SqliteStore, StorageBackend, StorageRequest, StorageSettings, StoreControl,
    StoreFactory, StoreOwner, Supplied, Symbol,
};
use rust_decimal::Decimal;
use std::str::FromStr;
use tempfile::TempDir;

use crate::contract;
use crate::sqlite::{raw_event_ids, raw_texts};

/// An `AnyStore` the factory opened, and a `SqliteStore` on its file.
fn opened() -> (TempDir, AnyStore, SqliteStore) {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let path = tempdir.path().join("market.sqlite");
    let mut request = StorageRequest::default();
    request.sqlite_path = Some(Supplied::new(path.clone(), "--db"));
    let settings = StorageSettings::resolve(request, StoreOwner::Command).expect("settings");
    let store = StoreFactory::builtin().open(&settings).expect("a store");
    assert_eq!(store.backend(), StorageBackend::Sqlite);
    (tempdir, store, SqliteStore::new(path))
}

#[test]
fn product_lines_are_stored_as_the_cases_say() {
    let (_tempdir, store, sqlite) = opened();
    contract::product_lines_are_stored_as_the_cases_say(&store, &raw_texts(&sqlite));
}

#[test]
fn decimal_texts_keep_their_scale() {
    let (_tempdir, store, sqlite) = opened();
    contract::decimal_texts_keep_their_scale(&store, &raw_texts(&sqlite));
}

#[test]
fn venue_ids_are_text() {
    let (_tempdir, store, sqlite) = opened();
    contract::venue_ids_are_text(&store, &raw_event_ids(&sqlite));
}

#[test]
fn environments_and_accounts_are_isolated() {
    let (_tempdir, store, _sqlite) = opened();
    contract::environments_and_accounts_are_isolated(&store);
}

const HOUR: i64 = 3_600_000;
/// An hourly window: 1440 bars.
const WINDOW: i64 = 1440 * HOUR;

fn hourly(open_time_ms: i64) -> Candle {
    let price = Decimal::from_str("101.50").expect("price");
    Candle {
        open_time_ms,
        close_time_ms: open_time_ms + HOUR - 1,
        open: price,
        high: price,
        low: price,
        close: price,
        volume: Decimal::from_str("2.000").expect("volume"),
        trades: Some(3),
    }
}

#[test]
fn segments_are_packed_resolved_and_loaded_through_it() {
    let (_tempdir, store, sqlite) = opened();
    let market = MarketId::new(
        ExchangeId::BinanceSpot,
        Symbol::new("ETHUSDT").expect("symbol"),
        Interval::H1,
    );
    let start = 300 * WINDOW;
    // Two bars in one window, and one in the next, which is the tip.
    let candles = [hourly(start), hourly(start + HOUR), hourly(start + WINDOW)];
    assert_eq!(store.upsert_candles(&market, &candles).expect("upsert"), 3);
    assert_eq!(
        store.refresh_segments(&market).expect("refresh"),
        SegmentRefresh {
            built: 1,
            ..SegmentRefresh::default()
        }
    );

    let plan = store
        .resolve_segments(&market, start, start + WINDOW)
        .expect("plan");
    assert_eq!(
        plan,
        sqlite
            .resolve_segments(&market, start, start + WINDOW)
            .expect("the same plan")
    );
    let [segment] = plan.segments.as_slice() else {
        panic!("one segment expected: {plan:?}");
    };
    assert_eq!(segment.bar_count, 2);
    let bytes = store.load_segment(&segment.hash).expect("load");
    assert!(bytes.is_some());
    assert_eq!(
        bytes,
        sqlite.load_segment(&segment.hash).expect("the same bytes")
    );
}

#[test]
fn the_stores_own_record_is_read_and_changed_through_it() {
    let (_tempdir, store, sqlite) = opened();
    assert_eq!(
        store.store_identity().expect("identity"),
        sqlite.store_identity().expect("the same identity")
    );
    assert_eq!(
        store.migrations().expect("migrations"),
        sqlite.migrations().expect("the same migrations")
    );
    assert!(store.legacy_runs().expect("legacy runs").is_empty());

    store.bind_environment(Environment::Testnet).expect("bound");
    assert_eq!(
        sqlite.store_identity().expect("identity").environment,
        Some(Environment::Testnet)
    );
    // Adoption reaches the store too: an unknown run is refused there.
    let identity = quantforge::RunIdentity::recorded(Some(Environment::Testnet), None);
    assert!(store.adopt_run("no-such-run", &identity, None).is_err());
}
