//! The shared conformance suite through [`AnyStore`] on PostgreSQL, opened
//! by the built-in factory as a host opens it: every port reaches the
//! `PostgresStore` behind it. Without `QUANTFORGE_TEST_POSTGRES_URL` every
//! test skips.

use quantforge::{AnyStore, StorageBackend, StoreFactory};

use super::{IsolatedDatabase, PostgresRaw, VARIABLE, selection, server_url};
use crate::conformance::{self, Backend, Fixture};

/// What the factory opens for an isolated database's URL.
fn open(url: &str) -> AnyStore {
    let store = StoreFactory::builtin()
        .open(&selection(url))
        .expect("a store");
    assert_eq!(store.backend(), StorageBackend::Postgres);
    store
}

/// An `AnyStore` the factory opened on a database of its own.
struct ThroughAnyStore;

impl Backend for ThroughAnyStore {
    type Store = AnyStore;
    const NAME: &'static str = "any_store (postgres)";

    fn fresh() -> Fixture<AnyStore> {
        let server = server_url().unwrap_or_else(|| panic!("{VARIABLE} is unset"));
        let database = IsolatedDatabase::create(&server);
        let reopen = {
            let url = database.url.clone();
            move || open(&url)
        };
        Fixture {
            store: reopen(),
            raw: Box::new(PostgresRaw(database.url.clone())),
            reopen: Box::new(reopen),
            guard: Box::new(database),
        }
    }
}

#[test]
fn candles() {
    if server_url().is_none() {
        eprintln!("skipping: {VARIABLE} is unset");
        return;
    }
    conformance::candles::<ThroughAnyStore>();
}

#[test]
fn journal() {
    if server_url().is_none() {
        eprintln!("skipping: {VARIABLE} is unset");
        return;
    }
    conformance::journal::<ThroughAnyStore>();
}

#[test]
fn control() {
    if server_url().is_none() {
        eprintln!("skipping: {VARIABLE} is unset");
        return;
    }
    conformance::control::<ThroughAnyStore>();
}

#[test]
fn segments() {
    if server_url().is_none() {
        eprintln!("skipping: {VARIABLE} is unset");
        return;
    }
    conformance::segments::<ThroughAnyStore>();
}

#[test]
fn jobs() {
    if server_url().is_none() {
        eprintln!("skipping: {VARIABLE} is unset");
        return;
    }
    conformance::jobs::<ThroughAnyStore>();
}

#[test]
fn durability() {
    if server_url().is_none() {
        eprintln!("skipping: {VARIABLE} is unset");
        return;
    }
    conformance::durability::<ThroughAnyStore>();
}
