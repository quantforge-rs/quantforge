//! Each real step of the schema chain, from the schema before it, against
//! a real database: every row the earlier schema held is kept, the step's
//! own tables and collations are there, the log gains one step, and a
//! repeat changes nothing. Each test works in a scratch database of its
//! own, as `migrate.rs` describes, and skips without the server.

use super::migrate::{column, log, reread, scratch, snapshot, tables, version};
use super::scratch::rows;
use super::seeds::{SEED, SEED_6, SEED_7, SEED_8, SEED_9, SEED_10, SEED_11};
use super::*;
use crate::storage::postgres::migrate::{STEPS, Step, ensure_current_with};
use tokio_postgres::error::SqlState;

/// The chain as each schema before this one knew it.
const SCHEMA_5_CHAIN: [Step; 1] = [STEPS[0]];
const SCHEMA_6_CHAIN: [Step; 2] = [STEPS[0], STEPS[1]];
const SCHEMA_7_CHAIN: [Step; 3] = [STEPS[0], STEPS[1], STEPS[2]];
const SCHEMA_8_CHAIN: [Step; 4] = [STEPS[0], STEPS[1], STEPS[2], STEPS[3]];
const SCHEMA_9_CHAIN: [Step; 5] = [STEPS[0], STEPS[1], STEPS[2], STEPS[3], STEPS[4]];
const SCHEMA_10_CHAIN: [Step; 6] = [STEPS[0], STEPS[1], STEPS[2], STEPS[3], STEPS[4], STEPS[5]];

/// The collation of `table.column`.
fn collation(store: &PostgresStore, table: &str, column: &str) -> Vec<String> {
    self::column(
        store,
        &format!(
            "SELECT CAST(collation_name AS TEXT) FROM information_schema.columns \
             WHERE table_schema = current_schema() AND table_name = '{table}' \
             AND column_name = '{column}'"
        ),
    )
}

#[test]
fn schema_6_keeps_every_row_of_schema_5_and_collates_the_keys_a_transfer_reads() {
    let Some(scratch) = scratch() else {
        return;
    };
    let store = scratch.store();
    ensure_current_with(&store.bridge, &SCHEMA_5_CHAIN, 5, "scratch").expect("schema 5");
    rows(&store, SEED);
    let before = snapshot(&store);
    assert!(before.iter().all(|(_, found)| !found.is_empty()), "seeded");
    assert_eq!(collation(&store, "segments", "hash"), ["NULL"]);

    ensure_current_with(&store.bridge, &SCHEMA_6_CHAIN, 6, "scratch").expect("schema 6");
    assert_eq!(reread(&store, &before), before);
    assert_eq!(version(&store), ["6"]);
    assert_eq!(log(&store), ["5<-0", "6<-5"]);
    for (table, key) in [
        ("segments", "hash"),
        ("bot_runs", "run_id"),
        ("bundles", "code_hash"),
    ] {
        assert_eq!(collation(&store, table, key), ["C"], "{table}.{key}");
    }
    assert!(tables(&store).contains(&"jobs".to_string()));
    rows(&store, SEED_6);

    // Run again, it finds the work done.
    ensure_current_with(&store.bridge, &SCHEMA_6_CHAIN, 6, "scratch").expect("a repeat");
    assert_eq!(log(&store), ["5<-0", "6<-5"]);
}

#[test]
fn schema_7_keeps_every_row_of_schema_6_and_adds_the_results_table() {
    let Some(scratch) = scratch() else {
        return;
    };
    let store = scratch.store();
    ensure_current_with(&store.bridge, &SCHEMA_6_CHAIN, 6, "scratch").expect("schema 6");
    rows(&store, SEED);
    rows(&store, SEED_6);
    let before = snapshot(&store);
    assert!(before.iter().all(|(_, found)| !found.is_empty()), "seeded");
    assert!(!tables(&store).contains(&"job_results".to_string()));

    ensure_current_with(&store.bridge, &SCHEMA_7_CHAIN, 7, "scratch").expect("schema 7");
    assert_eq!(reread(&store, &before), before);
    assert_eq!(version(&store), ["7"]);
    assert_eq!(log(&store), ["6<-0", "7<-6"]);
    assert_eq!(collation(&store, "job_results", "hash"), ["C"]);
    rows(&store, SEED_7);
    assert_eq!(
        column(&store, "SELECT encode(data, 'hex') FROM job_results"),
        ["000102"]
    );

    // Run again, it finds the work done.
    ensure_current_with(&store.bridge, &SCHEMA_7_CHAIN, 7, "scratch").expect("a repeat");
    assert_eq!(log(&store), ["6<-0", "7<-6"]);
}

#[test]
fn schema_8_keeps_every_row_of_schema_7_and_adds_the_logs_table() {
    let Some(scratch) = scratch() else {
        return;
    };
    let store = scratch.store();
    ensure_current_with(&store.bridge, &SCHEMA_7_CHAIN, 7, "scratch").expect("schema 7");
    rows(&store, SEED);
    rows(&store, SEED_6);
    rows(&store, SEED_7);
    let before = snapshot(&store);
    assert!(before.iter().all(|(_, found)| !found.is_empty()), "seeded");
    assert!(!tables(&store).contains(&"job_logs".to_string()));

    ensure_current_with(&store.bridge, &SCHEMA_8_CHAIN, 8, "scratch").expect("schema 8");
    assert_eq!(reread(&store, &before), before);
    assert_eq!(version(&store), ["8"]);
    assert_eq!(log(&store), ["7<-0", "8<-7"]);
    assert_eq!(collation(&store, "job_logs", "job_id"), ["C"]);
    rows(&store, SEED_8);
    assert_eq!(
        column(&store, "SELECT log_json FROM job_logs"),
        [r#"{"dropped_lines":0,"lines":[]}"#]
    );

    // Run again, it finds the work done.
    ensure_current_with(&store.bridge, &SCHEMA_8_CHAIN, 8, "scratch").expect("a repeat");
    assert_eq!(log(&store), ["7<-0", "8<-7"]);
}

#[test]
fn schema_9_keeps_every_row_of_schema_8_and_adds_the_policy_tables() {
    let Some(scratch) = scratch() else {
        return;
    };
    let store = scratch.store();
    ensure_current_with(&store.bridge, &SCHEMA_8_CHAIN, 8, "scratch").expect("schema 8");
    rows(&store, SEED);
    rows(&store, SEED_6);
    rows(&store, SEED_7);
    rows(&store, SEED_8);
    let before = snapshot(&store);
    assert!(before.iter().all(|(_, found)| !found.is_empty()), "seeded");
    for table in ["automation_profiles", "policy_revisions"] {
        assert!(!tables(&store).contains(&table.to_string()), "{table}");
    }

    ensure_current_with(&store.bridge, &SCHEMA_9_CHAIN, 9, "scratch").expect("schema 9");
    assert_eq!(reread(&store, &before), before);
    assert_eq!(version(&store), ["9"]);
    assert_eq!(log(&store), ["8<-0", "9<-8"]);
    // The keys a transfer reads in byte order are collated for it.
    for table in ["automation_profiles", "policy_revisions"] {
        assert_eq!(collation(&store, table, "profile_id"), ["C"], "{table}");
    }
    rows(&store, SEED_9);
    assert_eq!(
        column(
            &store,
            "SELECT CAST(seq AS TEXT) || ' ' || profile_id || ' ' || CAST(revision AS TEXT) \
             FROM policy_revisions"
        ),
        ["1 main 1"]
    );
    // A profile's revision is written once, and its owner tag is its own.
    for refused in [
        "INSERT INTO policy_revisions(profile_id, revision, changed_at_ms, policy_json) \
         VALUES ('main', 1, 1, '{}')",
        "INSERT INTO automation_profiles VALUES \
         ('other', 'bybit', 'demo', 'venue', '12345', 'linear', 'k3x9p2', 1)",
    ] {
        let failed = store
            .bridge
            .execute(Effect::Write, move |client| {
                Box::pin(async move { client.batch_execute(refused).await })
            })
            .expect_err(refused);
        let StorageError::Other(source) = &failed else {
            panic!("{failed:?}");
        };
        let code = source
            .downcast_ref::<tokio_postgres::Error>()
            .and_then(tokio_postgres::Error::code);
        assert_eq!(code, Some(&SqlState::UNIQUE_VIOLATION), "{failed:?}");
    }

    // Run again, it finds the work done.
    ensure_current_with(&store.bridge, &SCHEMA_9_CHAIN, 9, "scratch").expect("a repeat");
    assert_eq!(log(&store), ["8<-0", "9<-8"]);
}

/// The tables schema 10 adds: ten a transfer copies, and three that are
/// the store's own.
const EXECUTION_TABLES: [&str; 13] = [
    "signal_inbox",
    "signal_deliveries",
    "execution_operations",
    "execution_intents",
    "execution_orders",
    "execution_fills",
    "position_cycles",
    "margin_reservations",
    "execution_incidents",
    "outbox_events",
    "position_leases",
    "reservation_pools",
    "store_control_events",
];

#[test]
fn schema_10_keeps_every_row_of_schema_9_and_adds_the_execution_tables() {
    let Some(scratch) = scratch() else {
        return;
    };
    let store = scratch.store();
    ensure_current_with(&store.bridge, &SCHEMA_9_CHAIN, 9, "scratch").expect("schema 9");
    for seed in [SEED, SEED_6, SEED_7, SEED_8, SEED_9] {
        rows(&store, seed);
    }
    let before = snapshot(&store);
    assert!(before.iter().all(|(_, found)| !found.is_empty()), "seeded");
    for table in EXECUTION_TABLES {
        assert!(!tables(&store).contains(&table.to_string()), "{table}");
    }

    ensure_current_with(&store.bridge, &SCHEMA_10_CHAIN, 10, "scratch").expect("schema 10");
    assert_eq!(reread(&store, &before), before);
    assert_eq!(version(&store), ["10"]);
    assert_eq!(log(&store), ["9<-0", "10<-9"]);
    for table in EXECUTION_TABLES {
        assert!(tables(&store).contains(&table.to_string()), "{table}");
    }
    // The store's own record is as it was: nothing activates a store.
    assert_eq!(
        column(
            &store,
            "SELECT activation || ' ' || CAST(activation_epoch AS TEXT) FROM store_control"
        ),
        ["inactive 0"]
    );
    rows(&store, SEED_10);
    // A signal is recorded once for its profile, a position has one
    // unresolved entry at a time, and an event is told once under its key.
    for refused in [
        "INSERT INTO signal_inbox(venue, category, environment, account_kind, account_id, \
         profile_id, signal_id, payload_hash, state, revision, signal_json, received_at_ms) \
         VALUES ('bybit', 'linear', 'demo', 'venue', '12345', 'main', \
         'trend:entry:long:SOLUSDT.P:t', 'sha256:other', 'received', 1, '{}', 1)",
        "INSERT INTO execution_intents(venue, category, environment, account_kind, account_id, \
         profile_id, signal_id, kind, symbol, side, state, entry_slot, revision, intent_json, \
         created_at_ms, updated_at_ms) VALUES ('bybit', 'linear', 'demo', 'venue', '12345', \
         'other', 'another signal', 'entry', 'SOLUSDT', 'long', 'submitting', 1, 1, '{}', 1, 1)",
        "INSERT INTO outbox_events(venue, category, environment, account_kind, account_id, \
         profile_id, event_key, kind, delivery, revision, event_json, created_at_ms) VALUES \
         ('bybit', 'linear', 'demo', 'venue', '12345', 'other', 'sig/main/signal_admitted/t', \
         'signal_admitted', 'pending', 1, '{}', 1)",
    ] {
        let failed = store
            .bridge
            .execute(Effect::Write, move |client| {
                Box::pin(async move { client.batch_execute(refused).await })
            })
            .expect_err(refused);
        let StorageError::Other(source) = &failed else {
            panic!("{failed:?}");
        };
        let code = source
            .downcast_ref::<tokio_postgres::Error>()
            .and_then(tokio_postgres::Error::code);
        assert_eq!(code, Some(&SqlState::UNIQUE_VIOLATION), "{failed:?}");
    }
    // The same signal ID and the same symbol on another venue, and a
    // settled entry on the same position, are rows of their own.
    rows(
        &store,
        "INSERT INTO execution_intents(venue, category, environment, account_kind, account_id, \
         profile_id, signal_id, kind, symbol, side, state, entry_slot, revision, intent_json, \
         created_at_ms, updated_at_ms) VALUES \
         ('elsewhere', 'linear', 'demo', 'venue', '12345', 'main', \
          'trend:entry:long:SOLUSDT.P:t', 'entry', 'SOLUSDT', 'long', 'submitting', 1, 1, '{}', 1, 1), \
         ('bybit', 'linear', 'demo', 'venue', '12345', 'main', 'an earlier signal', 'entry', \
          'SOLUSDT', 'long', 'filled', NULL, 1, '{}', 1, 1), \
         ('bybit', 'linear', 'demo', 'venue', '12345', 'main', 'one before that', 'entry', \
          'SOLUSDT', 'long', 'rejected', NULL, 1, '{}', 1, 1)",
    );
    assert_eq!(
        column(
            &store,
            "SELECT CAST(COUNT(*) AS TEXT) FROM execution_intents"
        ),
        ["4"]
    );

    // Run again, it finds the work done.
    ensure_current_with(&store.bridge, &SCHEMA_10_CHAIN, 10, "scratch").expect("a repeat");
    assert_eq!(log(&store), ["9<-0", "10<-9"]);
}

#[test]
fn schema_11_keeps_every_row_of_schema_10_and_adds_the_provenance_table() {
    let Some(scratch) = scratch() else {
        return;
    };
    let store = scratch.store();
    ensure_current_with(&store.bridge, &SCHEMA_10_CHAIN, 10, "scratch").expect("schema 10");
    for seed in [SEED, SEED_6, SEED_7, SEED_8, SEED_9, SEED_10] {
        rows(&store, seed);
    }
    let before = snapshot(&store);
    assert!(before.iter().all(|(_, found)| !found.is_empty()), "seeded");
    assert!(!tables(&store).contains(&"job_provenance".to_string()));

    store.init().expect("schema 11");
    assert_eq!(reread(&store, &before), before);
    assert_eq!(version(&store), ["11"]);
    assert_eq!(log(&store), ["10<-0", "11<-10"]);
    assert!(tables(&store).contains(&"job_provenance".to_string()));
    assert_eq!(collation(&store, "job_provenance", "job_id"), ["C"]);
    rows(&store, SEED_11);
    // A job's provenance is recorded once.
    let again = "INSERT INTO job_provenance VALUES (repeat('a', 32), 'sha256:' || \
                 repeat('34', 32), '{}', 'postgres', 11, 1)";
    let failed = store
        .bridge
        .execute(Effect::Write, move |client| {
            Box::pin(async move { client.batch_execute(again).await })
        })
        .expect_err(again);
    let StorageError::Other(source) = &failed else {
        panic!("{failed:?}");
    };
    let code = source
        .downcast_ref::<tokio_postgres::Error>()
        .and_then(tokio_postgres::Error::code);
    assert_eq!(code, Some(&SqlState::UNIQUE_VIOLATION), "{failed:?}");

    // Run again, it finds the work done.
    store.init().expect("a repeat");
    assert_eq!(log(&store), ["10<-0", "11<-10"]);
}
