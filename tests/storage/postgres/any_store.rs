//! The shared conformance suite through [`AnyStore`] on PostgreSQL: every
//! port reaches the `PostgresStore` behind it, as QUA-116's provider will
//! hand it to a host. Without `QUANTFORGE_TEST_POSTGRES_URL` every test
//! skips.

use quantforge::{AnyStore, StorageBackend};

use super::{Postgres, VARIABLE, server_url};
use crate::conformance::{self, Backend, Fixture};

/// A `PostgresStore` on a database of its own, behind `AnyStore`.
struct ThroughAnyStore;

impl Backend for ThroughAnyStore {
    type Store = AnyStore;
    const NAME: &'static str = "any_store (postgres)";

    fn fresh() -> Fixture<AnyStore> {
        let Fixture {
            store,
            raw,
            reopen,
            guard,
        } = Postgres::fresh();
        let store = AnyStore::Postgres(store);
        assert_eq!(store.backend(), StorageBackend::Postgres);
        Fixture {
            store,
            raw,
            reopen: Box::new(move || AnyStore::Postgres(reopen())),
            guard,
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
