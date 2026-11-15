//! Schema 9 to schema 10: a store the build before the execution store
//! wrote (`tests/fixtures/sqlite/schema-9.sql`) gains the inbox, the
//! ledger, the outbox and its own leases, and keeps every row it held,
//! its profiles and every revision of their policies among them.

use quantforge::model::execution::{
    ExecutionScope, LeaseAnswer, LeaseHolder, LeaseRequest, NewOperation, OperationKind,
};
use quantforge::model::futures::{PolicyRevision, PositionKey, ProfileId};
use quantforge::model::{Asset, Category, InstrumentId};
use quantforge::storage::sqlite::SCHEMA_VERSION;
use quantforge::{
    CandleStore, Environment, ExchangeId, ExecutionStore, PolicyStore, StoreActivation,
    StoreControl, Symbol,
};

use crate::migration::{everything, load, shape, tables, version};

/// The tables schema 10 adds.
const ADDED: [&str; 13] = [
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
fn a_schema_9_store_gains_the_execution_tables_and_keeps_every_policy() {
    let fresh_dir = tempfile::tempdir().expect("tempdir");
    let fresh = quantforge::SqliteStore::new(fresh_dir.path().join("market.sqlite"));
    CandleStore::init(&fresh).expect("fresh");
    let (_tempdir, store) = load(9);
    assert_eq!(version(&store), 9);
    let mut kept = tables(&store);
    for table in ADDED {
        assert!(!kept.iter().any(|held| held == table), "{table}");
    }
    kept.push("sqlite_sequence".to_string());
    let before: Vec<_> = kept.iter().map(|table| everything(&store, table)).collect();
    let revisions = everything(&store, "policy_revisions");
    assert_eq!(everything(&store, "automation_profiles").len(), 2);
    assert_eq!(revisions.len(), 7, "five of one profile, two of the other");

    CandleStore::init(&store).expect("schema 10");

    assert_eq!(version(&store), SCHEMA_VERSION);
    assert_eq!(shape(&store), shape(&fresh), "not the fresh schema");
    for (table, rows_before) in kept.iter().zip(&before) {
        let after = everything(&store, table);
        match table.as_str() {
            "meta" => assert_eq!(
                after,
                [[
                    Some("schema_version".to_string()),
                    Some(SCHEMA_VERSION.to_string())
                ]]
            ),
            "schema_migrations" => {
                assert_eq!(after[..rows_before.len()], rows_before[..], "the log kept");
                assert_eq!(after.len(), rows_before.len() + 1, "one step logged");
            }
            _ => assert_eq!(&after, rows_before, "{table}"),
        }
    }
    let steps: Vec<(u32, u32)> = store
        .migrations()
        .expect("log")
        .iter()
        .map(|step| (step.from_version, step.to_version))
        .collect();
    assert_eq!(steps, [(4, 5), (5, 6), (6, 7), (7, 8), (8, 9), (9, 10)]);
    for table in ADDED {
        assert_eq!(
            everything(&store, table),
            Vec::<Vec<Option<String>>>::new(),
            "{table}"
        );
    }

    // Every profile the earlier build recorded, and every revision of its
    // policy, reads back through the port, word for word.
    let profiles = store.profiles().expect("profiles");
    let named: Vec<&str> = profiles
        .iter()
        .map(|profile| profile.id().as_str())
        .collect();
    assert_eq!(named, ["main", "second"]);
    for row in &revisions {
        let profile = ProfileId::new(row[1].as_deref().expect("a profile")).expect("an id");
        let revision: u64 = row[2]
            .as_deref()
            .expect("a revision")
            .parse()
            .expect("a number");
        let held = store
            .profile_policy_at(&profile, PolicyRevision::new(revision).expect("a revision"))
            .expect("a read")
            .expect("the revision");
        assert_eq!(
            Some(held.to_stored().expect("stored")),
            row[4],
            "{profile} at {revision}"
        );
    }
    let newest = store
        .profile_policy(&ProfileId::new("main").expect("an id"))
        .expect("a read")
        .expect("the policy");
    assert_eq!(newest.revision().get(), 5);

    // The store stays as the earlier build left its record: bound, and
    // never activated. It executes nothing until an operator activates
    // it, and then it does, under the profile the earlier build recorded.
    let identity = store.store_identity().expect("identity");
    assert_eq!(identity.environment, Some(Environment::Testnet));
    assert_eq!(
        (identity.activation, identity.activation_epoch.0),
        (StoreActivation::Inactive, 0)
    );
    assert_eq!(store.control_events().expect("the log"), []);
    let main = &profiles[0];
    assert_eq!(main.category(), Category::Linear);
    let instrument = InstrumentId::new(
        ExchangeId::from_parts(main.account_ref().venue(), main.category()),
        Symbol::new("SOLUSDT").expect("a symbol"),
        Asset::new("USDT").expect("an asset"),
    );
    let request = LeaseRequest {
        key: PositionKey::new(main.account_ref().clone(), instrument.clone()).expect("a key"),
        profile: main.id().clone(),
        holder: LeaseHolder::new("executor-a").expect("a holder"),
        ttl_ms: 30_000,
        now_ms: 1,
    };
    let refused = store.acquire_lease(&request).expect_err("never activated");
    assert!(refused.to_string().contains("is inactive"), "{refused}");
    let active = store.activate(1).expect("activated");
    assert_eq!(active.activation_epoch.0, 1);
    let granted = store.acquire_lease(&request).expect("answered");
    assert!(
        matches!(&granted, LeaseAnswer::Granted(lease) if lease.fence.get() == 1 && lease.epoch.0 == 1),
        "{granted:?}"
    );
    let operation = NewOperation {
        scope: ExecutionScope::of_profile(main),
        kind: OperationKind::Entry,
        key: None,
        instrument,
        signal: None,
        cycle: None,
        replacement: None,
    };
    let first = store.begin_operation(&operation, 2).expect("numbered");
    assert_eq!(
        first.record.id().get(),
        1,
        "the first operation of the store"
    );

    CandleStore::init(&store).expect("second init");
    assert_eq!(store.migrations().expect("log").len(), 6);
}
