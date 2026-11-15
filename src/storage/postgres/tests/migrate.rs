//! The schema chain against a real database: chains no build has, with a
//! step past the last that upgrades a seeded store, one that fails
//! half-way, and one slow enough that stores starting together must wait on
//! the lock. Each test works in a scratch database of its own on the server
//! `QUANTFORGE_TEST_POSTGRES_URL` names, dropped when the test ends, and
//! skips without the variable. Two tests need no database: the chain's
//! shape, and its DDL held to the doc. `steps.rs` takes each real step
//! from the schema before it, with the helpers kept here and the rows of
//! `seeds.rs`.

use super::database::{DATABASE, database_url};
use super::scratch::{Scratch, rows};
use super::seeds::{SEED, SEED_6, SEED_7, SEED_8, SEED_9, SEED_10};
use super::*;
use crate::storage::postgres::execution::schema::SCHEMA_10;
use crate::storage::postgres::migrate::{STEPS, Step, StepWork, ensure_current_with};
use crate::storage::postgres::schema::{SCHEMA_5, SCHEMA_6, SCHEMA_7, SCHEMA_8, SCHEMA_9};
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
    let shown = |name: &str| -> Vec<&str> {
        lines[at(&format!("<!-- begin: {name} -->")) + 1..at(&format!("<!-- end: {name} -->"))]
            .iter()
            .copied()
            .filter(|line| !line.starts_with("```"))
            .collect()
    };
    for (name, run) in [
        ("postgres schema", SCHEMA_5),
        ("postgres step 6", SCHEMA_6),
        ("postgres step 7", SCHEMA_7),
        ("postgres step 8", SCHEMA_8),
        ("postgres step 9", SCHEMA_9),
        ("postgres step 10", SCHEMA_10),
    ] {
        let run: Vec<&str> = run.trim_matches('\n').lines().collect();
        assert_eq!(run, shown(name), "{name}");
    }
}

/// Adds a table and a column, and fills the table from rows already there.
fn step_11_notes<'t>(transaction: &'t Transaction<'t>) -> StepWork<'t> {
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
fn step_11_fails<'t>(transaction: &'t Transaction<'t>) -> StepWork<'t> {
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
fn step_11_slow<'t>(transaction: &'t Transaction<'t>) -> StepWork<'t> {
    Box::pin(async move {
        transaction
            .batch_execute("SELECT pg_sleep(0.3); CREATE TABLE notes (note TEXT);")
            .await
    })
}

/// The real chain, then a step 11 that `apply` takes.
const fn with_step_11(apply: for<'t> fn(&'t Transaction<'t>) -> StepWork<'t>) -> [Step; 7] {
    [
        STEPS[0],
        STEPS[1],
        STEPS[2],
        STEPS[3],
        STEPS[4],
        STEPS[5],
        Step { to: 11, apply },
    ]
}

const UPGRADE: [Step; 7] = with_step_11(step_11_notes);
const FAILING: [Step; 7] = with_step_11(step_11_fails);
const SLOW: [Step; 7] = with_step_11(step_11_slow);

/// The scratch database, or `None` to skip.
pub(super) fn scratch() -> Option<Scratch> {
    database_url(DATABASE).map(|url| Scratch::create(&url))
}

/// The first value of every row of `sql`.
pub(super) fn column(store: &PostgresStore, sql: &str) -> Vec<String> {
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

pub(super) fn version(store: &PostgresStore) -> Vec<String> {
    column(store, "SELECT value FROM meta WHERE key = 'schema_version'")
}

/// The migration log, as `to<-from` pairs.
pub(super) fn log(store: &PostgresStore) -> Vec<String> {
    column(
        store,
        "SELECT version || '<-' || from_version FROM schema_migrations ORDER BY version",
    )
}

pub(super) fn tables(store: &PostgresStore) -> Vec<String> {
    column(
        store,
        "SELECT CAST(tablename AS TEXT) FROM pg_tables WHERE schemaname = current_schema() \
         ORDER BY 1",
    )
}

/// Every row of every table but the version's own, as the text of the
/// columns it has now, with the query that read it.
pub(super) fn snapshot(store: &PostgresStore) -> Vec<(String, Vec<Vec<Option<String>>>)> {
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
pub(super) fn reread(
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
    store.init().expect("schema 10");
    rows(&store, SEED);
    rows(&store, SEED_6);
    rows(&store, SEED_7);
    rows(&store, SEED_8);
    rows(&store, SEED_9);
    rows(&store, SEED_10);
    let before = snapshot(&store);
    assert!(before.iter().all(|(_, found)| !found.is_empty()), "seeded");

    ensure_current_with(&store.bridge, &UPGRADE, 11, "scratch").expect("the upgrade");
    assert_eq!(reread(&store, &before), before);
    let added = "SELECT CAST(column_name AS TEXT) FROM information_schema.columns WHERE \
                 table_schema = current_schema() AND table_name = 'candles' AND column_name = 'note'";
    assert_eq!(column(&store, added), ["note"]);
    assert_eq!(version(&store), ["11"]);
    assert_eq!(log(&store), ["10<-0", "11<-10"]);
    assert_eq!(
        column(&store, "SELECT note FROM notes ORDER BY run_id"),
        ["stopped", "running"]
    );

    // Run again, it finds the work done.
    ensure_current_with(&store.bridge, &UPGRADE, 11, "scratch").expect("a repeat");
    assert_eq!(log(&store), ["10<-0", "11<-10"]);
}

#[test]
fn a_failed_step_leaves_a_fresh_database_empty_and_the_lock_free() {
    let Some(scratch) = scratch() else {
        return;
    };
    let store = scratch.store();
    let failed =
        ensure_current_with(&store.bridge, &FAILING, 11, "scratch").expect_err("division by zero");
    assert!(matches!(failed, StorageError::Other(_)), "{failed:?}");
    assert_eq!(
        tables(&store),
        Vec::<String>::new(),
        "nothing of the chain is left"
    );

    // Nothing holds the lock: the real chain runs at once.
    store.init().expect("schema 10");
    assert_eq!(version(&store), ["10"]);
    assert_eq!(log(&store), ["10<-0"]);
}

#[test]
fn a_failed_upgrade_leaves_the_store_at_its_version_with_its_rows() {
    let Some(scratch) = scratch() else {
        return;
    };
    let store = scratch.store();
    store.init().expect("schema 10");
    rows(&store, SEED);
    rows(&store, SEED_6);
    rows(&store, SEED_7);
    rows(&store, SEED_8);
    rows(&store, SEED_9);
    rows(&store, SEED_10);
    let (before, tables_before) = (snapshot(&store), tables(&store));

    let failed =
        ensure_current_with(&store.bridge, &FAILING, 11, "scratch").expect_err("division by zero");
    let StorageError::Other(source) = &failed else {
        panic!("{failed:?}");
    };
    let code = source
        .downcast_ref::<tokio_postgres::Error>()
        .and_then(tokio_postgres::Error::code);
    assert_eq!(code, Some(&SqlState::DIVISION_BY_ZERO), "{failed:?}");
    assert_eq!(tables(&store), tables_before, "the step's table is gone");
    assert_eq!(version(&store), ["10"]);
    assert_eq!(log(&store), ["10<-0"]);
    assert_eq!(reread(&store, &before), before);

    ensure_current_with(&store.bridge, &UPGRADE, 11, "scratch").expect("a working step");
    assert_eq!(log(&store), ["10<-0", "11<-10"]);
}

#[test]
fn stores_starting_together_apply_each_step_once() {
    let Some(url) = database_url(DATABASE) else {
        return;
    };
    for from_schema_10 in [false, true] {
        let scratch = Scratch::create(&url);
        if from_schema_10 {
            scratch.store().init().expect("schema 10");
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
                        ensure_current_with(&store.bridge, &SLOW, 11, "scratch")
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
        let expected = if from_schema_10 {
            vec!["10<-0", "11<-10"]
        } else {
            vec!["11<-0"]
        };
        assert_eq!(
            log(&stores[0]),
            expected,
            "from schema 10: {from_schema_10}"
        );
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
