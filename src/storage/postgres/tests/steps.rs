//! Each real step of the schema chain, from the schema before it, against
//! a real database: every row the earlier schema held is kept, the step's
//! own tables and collations are there, the log gains one step, and a
//! repeat changes nothing. Each test works in a scratch database of its
//! own, as `migrate.rs` describes, and skips without the server.

use super::migrate::{
    SEED, SEED_6, SEED_7, SEED_8, SEED_9, column, log, reread, scratch, snapshot, tables, version,
};
use super::scratch::rows;
use super::*;
use crate::storage::postgres::migrate::{STEPS, Step, ensure_current_with};
use tokio_postgres::error::SqlState;

/// The chain as each schema before this one knew it.
const SCHEMA_5_CHAIN: [Step; 1] = [STEPS[0]];
const SCHEMA_6_CHAIN: [Step; 2] = [STEPS[0], STEPS[1]];
const SCHEMA_7_CHAIN: [Step; 3] = [STEPS[0], STEPS[1], STEPS[2]];
const SCHEMA_8_CHAIN: [Step; 4] = [STEPS[0], STEPS[1], STEPS[2], STEPS[3]];

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

    store.init().expect("schema 9");
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
    store.init().expect("a repeat");
    assert_eq!(log(&store), ["8<-0", "9<-8"]);
}
