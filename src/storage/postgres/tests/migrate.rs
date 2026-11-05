//! The schema chain against a real database, over chains no build has: a
//! step 6 that upgrades a seeded store, one that fails half-way, and one
//! slow enough that stores starting together must wait on the lock. Each
//! test works in a scratch database of its own on the server
//! `QUANTFORGE_TEST_POSTGRES_URL` names, dropped when the test ends, and
//! skips without the variable. Two tests need no database: the chain's
//! shape, and its DDL held to the doc.

use super::database::{DATABASE, database_url};
use super::scratch::{Scratch, rows};
use super::*;
use crate::storage::postgres::migrate::{STEPS, Step, StepWork, ensure_current_with};
use crate::storage::postgres::schema::SCHEMA_5;
use crate::storage::sqlite::SCHEMA_VERSION;
use std::sync::Barrier;
use tokio_postgres::Transaction;
use tokio_postgres::error::SqlState;

#[test]
fn the_chain_starts_at_schema_5_and_ends_at_the_version_sqlite_writes() {
    assert_eq!(STEPS[0].to, 5, "PostgreSQL starts at schema 5");
    for pair in STEPS.windows(2) {
        assert_eq!(pair[1].to, pair[0].to + 1, "one step per version");
    }
    assert_eq!(
        STEPS.last().map(|step| step.to),
        Some(SCHEMA_VERSION),
        "a step of SQLite's chain has no PostgreSQL step"
    );
}

#[test]
fn the_schema_is_the_ddl_the_doc_shows() {
    let doc = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/docs/storage-schema.md"
    ))
    .expect("the doc");
    let lines: Vec<&str> = doc.lines().collect();
    let at = |marker: &str| {
        lines
            .iter()
            .position(|line| line.trim() == marker)
            .unwrap_or_else(|| panic!("docs/storage-schema.md has no {marker}"))
    };
    let shown: Vec<&str> = lines
        [at("<!-- begin: postgres schema -->") + 1..at("<!-- end: postgres schema -->")]
        .iter()
        .copied()
        .filter(|line| !line.starts_with("```"))
        .collect();
    let run: Vec<&str> = SCHEMA_5.trim_matches('\n').lines().collect();
    assert_eq!(run, shown);
}

/// Adds a table and a column, and fills the table from rows already there.
fn step_6_notes<'t>(transaction: &'t Transaction<'t>) -> StepWork<'t> {
    Box::pin(async move {
        transaction
            .batch_execute(
                "CREATE TABLE notes (run_id TEXT PRIMARY KEY, note TEXT NOT NULL);
                 ALTER TABLE candles ADD COLUMN note TEXT;
                 INSERT INTO notes(run_id, note) SELECT run_id, status FROM bot_runs;",
            )
            .await
    })
}

/// Creates a table and writes a row, then fails.
fn step_6_fails<'t>(transaction: &'t Transaction<'t>) -> StepWork<'t> {
    Box::pin(async move {
        transaction
            .batch_execute(
                "CREATE TABLE notes (note TEXT);
                 INSERT INTO notes(note) VALUES ('half-way');
                 SELECT 1 / 0;",
            )
            .await
    })
}

/// Takes long enough that every store starting together reaches the lock
/// while one holds it.
fn step_6_slow<'t>(transaction: &'t Transaction<'t>) -> StepWork<'t> {
    Box::pin(async move {
        transaction
            .batch_execute("SELECT pg_sleep(0.3); CREATE TABLE notes (note TEXT);")
            .await
    })
}

/// The real chain, then a step 6 that `apply` takes.
const fn with_step_6(apply: for<'t> fn(&'t Transaction<'t>) -> StepWork<'t>) -> [Step; 2] {
    [STEPS[0], Step { to: 6, apply }]
}

const UPGRADE: [Step; 2] = with_step_6(step_6_notes);
const FAILING: [Step; 2] = with_step_6(step_6_fails);
const SLOW: [Step; 2] = with_step_6(step_6_slow);

/// The scratch database, or `None` to skip.
fn scratch() -> Option<Scratch> {
    database_url(DATABASE).map(|url| Scratch::create(&url))
}

/// The first value of every row of `sql`.
fn column(store: &PostgresStore, sql: &str) -> Vec<String> {
    rows(store, sql)
        .into_iter()
        .map(|row| {
            row.into_iter()
                .next()
                .flatten()
                .unwrap_or_else(|| "NULL".to_string())
        })
        .collect()
}

fn version(store: &PostgresStore) -> Vec<String> {
    column(store, "SELECT value FROM meta WHERE key = 'schema_version'")
}

/// The migration log, as `to<-from` pairs.
fn log(store: &PostgresStore) -> Vec<String> {
    column(
        store,
        "SELECT version || '<-' || from_version FROM schema_migrations ORDER BY version",
    )
}

fn tables(store: &PostgresStore) -> Vec<String> {
    column(
        store,
        "SELECT CAST(tablename AS TEXT) FROM pg_tables WHERE schemaname = current_schema() \
         ORDER BY 1",
    )
}

/// One row in every table of schema 5, at the edges a step must keep:
/// decimal text with its scale, the `i64` extremes, a trade count stored
/// wrapped, a legacy run, and a blob holding every byte value.
const SEED: &str = r#"
INSERT INTO candles VALUES
  ('bybit_spot', 'BTCUSDT', '1m', -9223372036854775808, -9223372036854715809,
   '1.10', '1.1', '0.00000000', '79228162514264337593543950335', '-0.5', -1),
  ('bybit_inverse', 'BTCUSD', '1m', 9223372036854775807, 9223372036854775807,
   '1.10', '1.10', '1.10', '1.10', '0.0000000000000000000000000001', NULL);
INSERT INTO bot_runs VALUES
  ('run-recorded', 'bybit_spot', 'BTCUSDT', '1m', 'sma_cross', 'running', '{"run_id":"run-recorded"}',
   1792454400000, 1792454460000, NULL, NULL, 'testnet', 'key_fingerprint', '0123456789abcdef', -- gitleaks:allow
   'recorded', 1792454400000, NULL),
  ('run-legacy', 'bybit_spot', 'BTCUSDT', '1m', 'sma_cross', 'stopped', '{"run_id":"run-legacy"}',
   1700000000000, 1700000060000, 1700000060000, 'stopped by hand', NULL, NULL, NULL,
   'legacy', NULL, '{"run_id":"run-legacy","old":true}');
INSERT INTO order_events(run_id, symbol, side, order_type, status, order_id, client_order_id,
                         transact_time_ms, raw_json, legacy_raw_json) VALUES
  ('run-recorded', 'BTCUSDT', 'BUY', 'MARKET', 'FILLED', '12345678901234567890', 'qf-a',
   1792454400000, '{"orderId":"12345678901234567890"}', NULL),
  ('run-legacy', 'BTCUSDT', 'SELL', 'MARKET', 'FILLED', 'abc-XYZ_09', NULL, NULL, '{}', '{"orderId":7}');
INSERT INTO closed_trades(run_id, symbol, entry_time_ms, exit_time_ms, entry_price, exit_price,
                          qty, gross_quote_pnl, entry_order_id, exit_order_id) VALUES
  ('run-recorded', 'BTCUSDT', 1, 2, '100.10', '101.1', '0.00100000', '-0.00099', '7', NULL);
INSERT INTO segments
  SELECT 'sha256:' || repeat('ab', 32), 1, 256,
         decode(string_agg(lpad(to_hex(b), 2, '0'), '' ORDER BY b), 'hex')
  FROM generate_series(0, 255) AS b;
INSERT INTO segment_index VALUES
  ('bybit_spot', 'BTCUSDT', '1m', 1792454400000, 1792454400000, 1792540740000, 1440,
   'sha256:' || repeat('ab', 32));
INSERT INTO segment_dirty(exchange, symbol, interval, window_start_ms) VALUES
  ('bybit_spot', 'BTCUSDT', '1m', 1792540800000);
"#;

/// Every row of every table but the version's own, as the text of the
/// columns it has now, with the query that read it.
fn snapshot(store: &PostgresStore) -> Vec<(String, Vec<Vec<Option<String>>>)> {
    tables(store)
        .into_iter()
        .filter(|table| table != "meta" && table != "schema_migrations")
        .map(|table| {
            let columns = column(
                store,
                &format!(
                    "SELECT CAST(column_name AS TEXT) FROM information_schema.columns \
                     WHERE table_schema = current_schema() AND table_name = '{table}' \
                     ORDER BY ordinal_position"
                ),
            );
            let list: Vec<String> = columns
                .iter()
                .map(|name| format!("CAST(\"{name}\" AS TEXT)"))
                .collect();
            let order: Vec<String> = (1..=columns.len()).map(|n| n.to_string()).collect();
            let sql = format!(
                "SELECT {} FROM {table} ORDER BY {}",
                list.join(", "),
                order.join(", ")
            );
            let found = rows(store, &sql);
            (sql, found)
        })
        .collect()
}

/// The same queries again.
fn reread(
    store: &PostgresStore,
    before: &[(String, Vec<Vec<Option<String>>>)],
) -> Vec<(String, Vec<Vec<Option<String>>>)> {
    before
        .iter()
        .map(|(sql, _)| (sql.clone(), rows(store, sql)))
        .collect()
}

#[test]
fn an_upgrade_keeps_every_row_it_found() {
    let Some(scratch) = scratch() else {
        return;
    };
    let store = scratch.store();
    store.init().expect("schema 5");
    rows(&store, SEED);
    let before = snapshot(&store);
    assert!(before.iter().all(|(_, found)| !found.is_empty()), "seeded");

    ensure_current_with(&store.bridge, &UPGRADE, 6, "scratch").expect("the upgrade");
    assert_eq!(reread(&store, &before), before);
    let added = "SELECT CAST(column_name AS TEXT) FROM information_schema.columns WHERE \
                 table_schema = current_schema() AND table_name = 'candles' AND column_name = 'note'";
    assert_eq!(column(&store, added), ["note"]);
    assert_eq!(version(&store), ["6"]);
    assert_eq!(log(&store), ["5<-0", "6<-5"]);
    assert_eq!(
        column(&store, "SELECT note FROM notes ORDER BY run_id"),
        ["stopped", "running"]
    );

    // Run again, it finds the work done.
    ensure_current_with(&store.bridge, &UPGRADE, 6, "scratch").expect("a repeat");
    assert_eq!(log(&store), ["5<-0", "6<-5"]);
}

#[test]
fn a_failed_step_leaves_a_fresh_database_empty_and_the_lock_free() {
    let Some(scratch) = scratch() else {
        return;
    };
    let store = scratch.store();
    let failed =
        ensure_current_with(&store.bridge, &FAILING, 6, "scratch").expect_err("division by zero");
    assert!(matches!(failed, StorageError::Other(_)), "{failed:?}");
    assert_eq!(
        tables(&store),
        Vec::<String>::new(),
        "nothing of the chain is left"
    );

    // Nothing holds the lock: the real chain runs at once.
    store.init().expect("schema 5");
    assert_eq!(version(&store), ["5"]);
    assert_eq!(log(&store), ["5<-0"]);
}

#[test]
fn a_failed_upgrade_leaves_the_store_at_its_version_with_its_rows() {
    let Some(scratch) = scratch() else {
        return;
    };
    let store = scratch.store();
    store.init().expect("schema 5");
    rows(&store, SEED);
    let (before, tables_before) = (snapshot(&store), tables(&store));

    let failed =
        ensure_current_with(&store.bridge, &FAILING, 6, "scratch").expect_err("division by zero");
    let StorageError::Other(source) = &failed else {
        panic!("{failed:?}");
    };
    let code = source
        .downcast_ref::<tokio_postgres::Error>()
        .and_then(tokio_postgres::Error::code);
    assert_eq!(code, Some(&SqlState::DIVISION_BY_ZERO), "{failed:?}");
    assert_eq!(tables(&store), tables_before, "the step's table is gone");
    assert_eq!(version(&store), ["5"]);
    assert_eq!(log(&store), ["5<-0"]);
    assert_eq!(reread(&store, &before), before);

    ensure_current_with(&store.bridge, &UPGRADE, 6, "scratch").expect("a working step");
    assert_eq!(log(&store), ["5<-0", "6<-5"]);
}

#[test]
fn stores_starting_together_apply_each_step_once() {
    let Some(url) = database_url(DATABASE) else {
        return;
    };
    for from_schema_5 in [false, true] {
        let scratch = Scratch::create(&url);
        if from_schema_5 {
            scratch.store().init().expect("schema 5");
        }
        // Connected before the barrier, so they reach the chain together,
        // and each waits for the chain's lock past its own lock budget.
        let stores: Vec<PostgresStore> = (0..6).map(|_| scratch.impatient_store()).collect();
        let barrier = Barrier::new(stores.len());
        let outcomes: Vec<Result<(), StorageError>> = std::thread::scope(|scope| {
            let started: Vec<_> = stores
                .iter()
                .map(|store| {
                    let barrier = &barrier;
                    scope.spawn(move || {
                        barrier.wait();
                        ensure_current_with(&store.bridge, &SLOW, 6, "scratch")
                    })
                })
                .collect();
            started
                .into_iter()
                .map(|thread| thread.join().expect("a store's thread"))
                .collect()
        });
        for outcome in &outcomes {
            assert!(outcome.is_ok(), "{outcome:?}");
        }
        let expected = if from_schema_5 {
            vec!["5<-0", "6<-5"]
        } else {
            vec!["6<-0"]
        };
        assert_eq!(log(&stores[0]), expected, "from schema 5: {from_schema_5}");
        assert_eq!(
            column(
                &stores[0],
                "SELECT CAST(COUNT(*) AS TEXT) FROM store_control"
            ),
            ["1"]
        );
        assert!(tables(&stores[0]).contains(&"notes".to_string()));
    }
}
