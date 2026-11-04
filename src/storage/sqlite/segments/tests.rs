//! The races of the shared cases on SQLite, each on a fresh store, a read
//! on a connection of its own.

use crate::storage::races;
use crate::{CandleStore, SqliteStore};

#[test]
fn every_race_of_the_shared_cases_ends_as_the_cases_say() {
    races::every_race(|cases, race| {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let store = SqliteStore::new(tempdir.path().join("market.sqlite"));
        CandleStore::init(&store).expect("init");
        races::step_through(cases, race, &store, || store.open().expect("connection"));
    });
}
