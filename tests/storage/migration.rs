//! The schema chain from every schema an earlier build wrote. Each fixture
//! in `tests/fixtures/sqlite/` is that build's DDL, verbatim, with rows the
//! chain must carry forward: candles with their decimal texts, runs from
//! 0.2.0 without an execution mode, order events and closed trades with
//! integer venue IDs, and a segment with its hash.

use quantforge::storage::sqlite::SCHEMA_VERSION;
use quantforge::{
    CandleStore, Environment, ExchangeId, ExecutionMode, IdentitySource, Interval, KeyFingerprint,
    MarketId, RunIdentity, RunJournalStore, SegmentStore, SqliteStore, StorageError,
    StoreActivation, StoreControl, Symbol, VenueAccountId,
};
use rusqlite::Connection;
use rusqlite::types::Value;
use tempfile::TempDir;

use crate::cases::fixture;

/// The hash of the one-bar segment the schema-4 fixture holds, and the
/// candle it packs: segment case `one bar`.
const ONE_BAR: &str = "sha256:9b80ffd85cbcbb3c03e5e7ba06b6ad4a7778e10b4845d7f263f518383106778a";
const DAY_ONE: i64 = 1_792_454_400_000;
const DAY_TWO: i64 = 1_792_540_800_000;

fn load(version: u32) -> (TempDir, SqliteStore) {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let path = tempdir.path().join("market.sqlite");
    let sql = std::fs::read_to_string(fixture(&format!(
        "tests/fixtures/sqlite/schema-{version}.sql"
    )))
    .expect("fixture");
    let connection = Connection::open(&path).expect("open");
    connection.execute_batch(&sql).expect("load fixture");
    drop(connection);
    (tempdir, SqliteStore::new(path))
}

fn open(store: &SqliteStore) -> Connection {
    Connection::open(store.path()).expect("open")
}

fn version(store: &SqliteStore) -> u32 {
    open(store)
        .query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |row| row.get::<_, String>(0),
        )
        .expect("version")
        .parse()
        .expect("number")
}

/// Every row of `sql`, each column as text (`NULL` as `None`).
fn rows(store: &SqliteStore, sql: &str) -> Vec<Vec<Option<String>>> {
    let connection = open(store);
    let mut stmt = connection.prepare(sql).expect("prepare");
    let count = stmt.column_count();
    let rows = stmt
        .query_map([], |row| {
            (0..count)
                .map(|index| {
                    Ok(match row.get::<_, Value>(index)? {
                        Value::Null => None,
                        Value::Integer(number) => Some(number.to_string()),
                        Value::Real(number) => Some(number.to_string()),
                        Value::Text(text) => Some(text),
                        Value::Blob(bytes) => Some(hex::encode(bytes)),
                    })
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .expect("query");
    rows.collect::<Result<_, _>>().expect("rows")
}

fn tables(store: &SqliteStore) -> Vec<String> {
    rows(
        store,
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name != 'sqlite_sequence' ORDER BY name",
    )
    .into_iter()
    .map(|row| row[0].clone().expect("name"))
    .collect()
}

/// The shape of every table and index: what a migrated store must share
/// with a fresh one.
fn shape(store: &SqliteStore) -> Vec<(String, Vec<Vec<Option<String>>>)> {
    let mut shape = Vec::new();
    for table in tables(store) {
        let columns = rows(
            store,
            &format!(
                "SELECT name, type, \"notnull\", dflt_value, pk FROM pragma_table_info('{table}')"
            ),
        );
        shape.push((table.clone(), columns));
        let indexes = rows(
            store,
            &format!(
                "SELECT name, \"unique\", origin FROM pragma_index_list('{table}') ORDER BY name"
            ),
        );
        shape.push((format!("{table} indexes"), indexes));
    }
    shape
}

const CANDLES: &str = "SELECT exchange, symbol, interval, open_time_ms, close_time_ms, open, high, low, close, volume, trades FROM candles ORDER BY exchange, symbol, interval, open_time_ms";
const RUNS: &str = "SELECT run_id, exchange, symbol, interval, strategy_name, status, state_json, started_at_ms, updated_at_ms, stopped_at_ms, last_error FROM bot_runs ORDER BY run_id";
const EVENTS: &str = "SELECT seq, run_id, symbol, side, order_type, status, CAST(order_id AS TEXT), client_order_id, transact_time_ms, raw_json FROM order_events ORDER BY seq";
const TRADES: &str = "SELECT seq, run_id, symbol, entry_time_ms, exit_time_ms, entry_price, exit_price, qty, gross_quote_pnl, CAST(entry_order_id AS TEXT), CAST(exit_order_id AS TEXT) FROM closed_trades ORDER BY seq";
const SEGMENTS: &str = "SELECT hash, format, payload_len, hex(data) FROM segments ORDER BY hash";
const INDEX: &str = "SELECT exchange, symbol, interval, window_start_ms, first_open_ms, last_open_ms, bar_count, hash FROM segment_index ORDER BY exchange, symbol, interval, window_start_ms";

fn btc_1m() -> MarketId {
    MarketId::new(
        ExchangeId::BinanceSpot,
        Symbol::new("BTCUSDT").expect("symbol"),
        Interval::M1,
    )
}

#[test]
fn every_earlier_schema_migrates_to_the_current_one_with_its_rows_unchanged() {
    let (_fresh_dir, fresh) = {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let store = SqliteStore::new(tempdir.path().join("market.sqlite"));
        CandleStore::init(&store).expect("fresh");
        (tempdir, store)
    };
    let fresh_shape = shape(&fresh);
    for from in 1..=4u32 {
        let (_tempdir, store) = load(from);
        assert_eq!(version(&store), from);
        let candles = rows(&store, CANDLES);
        let runs = if from >= 2 {
            rows(&store, RUNS)
        } else {
            Vec::new()
        };
        let events = if from >= 2 {
            rows(&store, EVENTS)
        } else {
            Vec::new()
        };
        let trades = if from >= 2 {
            rows(&store, TRADES)
        } else {
            Vec::new()
        };
        let segments = if from >= 4 {
            rows(&store, SEGMENTS)
        } else {
            Vec::new()
        };
        let index = if from >= 4 {
            rows(&store, INDEX)
        } else {
            Vec::new()
        };

        CandleStore::init(&store).unwrap_or_else(|error| panic!("schema {from}: {error}"));

        assert_eq!(version(&store), SCHEMA_VERSION, "schema {from}");
        assert_eq!(
            shape(&store),
            fresh_shape,
            "schema {from}: not the fresh schema"
        );
        let migrations = store.migrations().expect("log");
        let steps: Vec<(u32, u32)> = migrations
            .iter()
            .map(|step| (step.from_version, step.to_version))
            .collect();
        let expected: Vec<(u32, u32)> = (from..SCHEMA_VERSION).map(|v| (v, v + 1)).collect();
        assert_eq!(steps, expected, "schema {from}");
        assert!(
            migrations
                .iter()
                .all(|step| step.quantforge_version == env!("CARGO_PKG_VERSION"))
        );
        let identity = store.store_identity().expect("identity");
        assert_eq!(identity.schema_version, SCHEMA_VERSION);
        assert_eq!(identity.environment, None, "never guessed");
        assert_eq!(identity.activation, StoreActivation::Inactive);
        assert_eq!(identity.activation_epoch.0, 0);

        assert_eq!(rows(&store, CANDLES), candles, "schema {from}: candles");
        // The journal rows: every column as it was, the IDs now text of the
        // same digits, and the original JSON kept beside the rewritten one.
        let migrated_runs = rows(
            &store,
            "SELECT run_id, exchange, symbol, interval, strategy_name, status, legacy_state_json, started_at_ms, updated_at_ms, stopped_at_ms, last_error FROM bot_runs ORDER BY run_id",
        );
        assert_eq!(migrated_runs, runs, "schema {from}: runs");
        for row in rows(
            &store,
            "SELECT state_json, legacy_state_json, identity_source, environment, account_id FROM bot_runs ORDER BY run_id",
        ) {
            let state: serde_json::Value =
                serde_json::from_str(row[0].as_deref().expect("state")).expect("json");
            let mut original: serde_json::Value =
                serde_json::from_str(row[1].as_deref().expect("legacy")).expect("json");
            if let Some(id) = original["position"].get_mut("entry_order_id") {
                if let Some(number) = id.as_i64() {
                    *id = serde_json::Value::String(number.to_string());
                }
            }
            assert_eq!(state, original, "schema {from}: only the ID changed");
            assert_eq!(row[2].as_deref(), Some("legacy"));
            assert_eq!(row[3], None);
            assert_eq!(row[4], None);
        }
        let migrated_events = rows(
            &store,
            "SELECT seq, run_id, symbol, side, order_type, status, order_id, client_order_id, transact_time_ms, legacy_raw_json FROM order_events ORDER BY seq",
        );
        assert_eq!(migrated_events, events, "schema {from}: order events");
        for row in rows(
            &store,
            "SELECT order_id, typeof(order_id), raw_json FROM order_events ORDER BY seq",
        ) {
            assert_eq!(
                row[1].as_deref(),
                Some(if row[0].is_some() { "text" } else { "null" })
            );
            let raw: serde_json::Value =
                serde_json::from_str(row[2].as_deref().expect("raw")).expect("json");
            assert_eq!(
                raw["order_id"].as_str(),
                row[0].as_deref(),
                "the JSON ID is text"
            );
            if let Some(fill) = raw["fills"].get(0) {
                assert!(fill["trade_id"].is_string(), "schema {from}: {fill}");
            }
        }
        assert_eq!(rows(&store, TRADES), trades, "schema {from}: closed trades");
        for row in rows(
            &store,
            "SELECT typeof(entry_order_id) FROM closed_trades WHERE entry_order_id IS NOT NULL",
        ) {
            assert_eq!(row[0].as_deref(), Some("text"));
        }
        if from >= 4 {
            assert_eq!(rows(&store, SEGMENTS), segments, "schema {from}: segments");
            assert_eq!(rows(&store, INDEX), index, "schema {from}: index");
            let hash = ONE_BAR.parse().expect("hash");
            let bytes = store
                .load_segment(&hash)
                .expect("load")
                .expect("the fixture's segment");
            let candles =
                quantforge::storage::segments::open(&bytes, &hash).expect("the bytes still verify");
            assert_eq!(candles.len(), 1);
        }
        if from >= 2 {
            let legacy = store.legacy_runs().expect("legacy runs");
            let ids: Vec<&str> = legacy.iter().map(|run| run.run_id.as_str()).collect();
            assert_eq!(ids, ["run-a", "run-b"], "schema {from}");
            let modes: Vec<Option<ExecutionMode>> =
                legacy.iter().map(|run| run.execution_mode).collect();
            let expected = if from == 2 {
                vec![None, None]
            } else {
                vec![Some(ExecutionMode::DryRun), Some(ExecutionMode::Live)]
            };
            assert_eq!(modes, expected, "schema {from}: 0.2.0 recorded no mode");
            assert_eq!(
                store.run_identity("run-b").expect("identity"),
                Some(RunIdentity::legacy())
            );
        }
        // Opening the store again changes nothing.
        CandleStore::init(&store).expect("second init");
        assert_eq!(store.migrations().expect("log").len(), migrations.len());
    }
}

#[test]
fn a_fresh_store_logs_one_step_from_zero_and_is_the_documented_schema() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::new(tempdir.path().join("market.sqlite"));
    CandleStore::init(&store).expect("init");
    let migrations = store.migrations().expect("log");
    assert_eq!(migrations.len(), 1);
    assert_eq!(
        (migrations[0].from_version, migrations[0].to_version),
        (0, SCHEMA_VERSION)
    );
    assert_eq!(
        tables(&store),
        [
            "bot_runs",
            "candles",
            "closed_trades",
            "meta",
            "order_events",
            "schema_migrations",
            "segment_dirty",
            "segment_index",
            "segments",
            "store_control",
        ]
    );
}

#[test]
fn a_zero_two_zero_store_gets_its_windows_marked_and_its_runs_adopted_one_by_one() {
    let (_tempdir, store) = load(2);
    CandleStore::init(&store).expect("migrate");

    // Step 4 marked every window with candles: two for BTCUSDT 1m, one for
    // ETHUSDT 8h (a 480-day window, so it starts well before the candle);
    // the first refresh packs the closed one into the segment the shared
    // case names.
    let eight_hour_window = quantforge::storage::segments::window(Interval::H8, DAY_ONE)
        .expect("window")
        .start;
    let marks = rows(
        &store,
        "SELECT exchange, symbol, interval, window_start_ms FROM segment_dirty ORDER BY symbol, window_start_ms",
    );
    assert_eq!(
        marks,
        [
            vec![
                Some("binance_spot".to_string()),
                Some("BTCUSDT".to_string()),
                Some("1m".to_string()),
                Some(DAY_ONE.to_string())
            ],
            vec![
                Some("binance_spot".to_string()),
                Some("BTCUSDT".to_string()),
                Some("1m".to_string()),
                Some(DAY_TWO.to_string())
            ],
            vec![
                Some("binance_spot".to_string()),
                Some("ETHUSDT".to_string()),
                Some("8h".to_string()),
                Some(eight_hour_window.to_string())
            ],
        ]
    );
    store.refresh_segments(&btc_1m()).expect("refresh");
    let plan = store
        .resolve_segments(&btc_1m(), DAY_ONE, DAY_TWO + 60_000)
        .expect("plan");
    assert_eq!(plan.segments.len(), 1, "{plan:?}");
    assert_eq!(plan.segments[0].hash.to_string(), ONE_BAR);

    // A 0.2.0 run loads only once the operator states its mode.
    let error = store
        .load_run_state("run-a")
        .expect_err("no execution mode");
    assert!(
        error
            .to_string()
            .contains("adopt it with `quantforge store adopt-run --run-id run-a"),
        "{error}"
    );
    let error = store
        .latest_run_for_market(&btc_1m(), "sma_cross")
        .expect_err("the latest run is a 0.2.0 run");
    assert!(matches!(error, StorageError::InvalidArgument(_)), "{error}");

    let dry = RunIdentity::adopted(Some(Environment::Testnet), None);
    let error = store
        .adopt_run("run-a", &dry, None)
        .expect_err("the mode must be stated");
    assert!(
        error.to_string().contains("recorded no execution mode"),
        "{error}"
    );
    let error = store
        .adopt_run(
            "run-a",
            &RunIdentity::recorded(Some(Environment::Testnet), None),
            Some(ExecutionMode::DryRun),
        )
        .expect_err("an adoption is adopted");
    assert!(error.to_string().contains("adopted"), "{error}");
    store
        .adopt_run("run-a", &dry, Some(ExecutionMode::DryRun))
        .expect("adopt");
    let state = store.load_run_state("run-a").expect("load").expect("run");
    assert_eq!(state.execution_mode, ExecutionMode::DryRun);
    assert_eq!(state.position.entry_order_id, None);
    assert_eq!(
        store.run_identity("run-a").expect("identity"),
        Some(dry.clone())
    );
    assert_eq!(
        store.legacy_runs().expect("legacy").len(),
        1,
        "run-a is adopted, run-b still waits"
    );
    let error = store
        .adopt_run("run-a", &dry, Some(ExecutionMode::DryRun))
        .expect_err("adopted once");
    assert!(
        error.to_string().contains("already has a adopted identity")
            || error.to_string().contains("only a legacy run"),
        "{error}"
    );
    let original = rows(
        &store,
        "SELECT legacy_state_json FROM bot_runs WHERE run_id = 'run-a'",
    );
    assert!(
        !original[0][0]
            .as_deref()
            .expect("legacy json")
            .contains("execution_mode"),
        "the original record is kept as 0.2.0 wrote it"
    );

    // The live run, with the account that will close it.
    let live = RunIdentity::adopted(
        Some(Environment::Testnet),
        Some(VenueAccountId::KeyFingerprint(KeyFingerprint::of("key"))),
    );
    store
        .adopt_run("run-b", &live, Some(ExecutionMode::Live))
        .expect("adopt live");
    let state = store.load_run_state("run-b").expect("load").expect("run");
    assert_eq!(state.execution_mode, ExecutionMode::Live);
    assert_eq!(
        state.position.entry_order_id.as_ref().map(|id| id.as_str()),
        Some("7"),
        "the integer ID became text"
    );
    let events = store.list_order_events("run-b", 10).expect("events");
    assert_eq!(events[0].order_id.as_ref().map(|id| id.as_str()), Some("7"));
    let fills = events[0].fills.as_ref().expect("fills");
    assert_eq!(fills[0].trade_id.as_ref().map(|id| id.as_str()), Some("42"));
    let trades = store.list_closed_trades("run-a", 10).expect("trades");
    assert_eq!(trades.len(), 2);
    assert_eq!(
        trades[1].entry_order_id.as_ref().map(|id| id.as_str()),
        Some("7")
    );
    assert_eq!(
        trades[1].exit_order_id.as_ref().map(|id| id.as_str()),
        Some("8")
    );
    assert_eq!(trades[0].entry_order_id, None);
    assert_eq!(trades[1].qty.to_string(), "0.010", "scale kept");
    assert!(store.legacy_runs().expect("legacy").is_empty());
    assert_eq!(
        store
            .run_identity("run-b")
            .expect("identity")
            .map(|identity| identity.source),
        Some(IdentitySource::Adopted)
    );
}

#[test]
fn a_step_that_fails_leaves_the_store_at_the_schema_it_had() {
    // A schema-4 file someone gave a column by hand: step 5 cannot add it,
    // rolls back whole, and the file stays at 4 with nothing of 5 in it.
    let (_tempdir, store) = load(4);
    open(&store)
        .execute_batch("ALTER TABLE bot_runs ADD COLUMN environment TEXT;")
        .expect("hand-added column");
    let before = (
        rows(&store, RUNS),
        rows(&store, EVENTS),
        rows(&store, TRADES),
    );

    let error = CandleStore::init(&store).expect_err("the step fails");
    assert!(error.to_string().contains("storage error"), "{error}");

    assert_eq!(version(&store), 4);
    assert_eq!(
        (
            rows(&store, RUNS),
            rows(&store, EVENTS),
            rows(&store, TRADES)
        ),
        before
    );
    assert!(!tables(&store).iter().any(|table| table == "store_control"));
    assert!(
        !tables(&store)
            .iter()
            .any(|table| table == "schema_migrations")
    );
    // No v5 copy of a journal survives the rollback.
    assert!(!tables(&store).iter().any(|table| table.ends_with("_v5")));
}

#[test]
fn a_store_from_a_newer_build_is_refused_untouched() {
    let (_tempdir, store) = load(4);
    CandleStore::init(&store).expect("migrate");
    open(&store)
        .execute(
            "UPDATE meta SET value = ?1 WHERE key = 'schema_version'",
            [(SCHEMA_VERSION + 1).to_string()],
        )
        .expect("newer");
    let error = CandleStore::init(&store).expect_err("newer");
    assert!(
        matches!(error, StorageError::SchemaUnsupported { .. }),
        "{error}"
    );
    assert_eq!(version(&store), SCHEMA_VERSION + 1);
}
