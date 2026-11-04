//! PostgreSQL under the shared conformance suite: every fixture opens a
//! database of its own, created on the server `QUANTFORGE_TEST_POSTGRES_URL`
//! names and dropped when the fixture is, so scenarios never see one
//! another's rows and a run leaves nothing behind. Without the variable
//! the tests here skip.
//!
//! The URL reaches a user allowed to create databases, over plain TCP
//! (`sslmode=disable`, as CI's service container and the local recipe in
//! `docs/storage-conformance.md` serve it); the store itself connects to
//! the isolated database through `PostgresStore`, TLS rules and all.
//!
//! Every fixture's store has run `init`, so the schema of QUA-125 is in
//! place, and every port's group runs here, as `sqlite.rs` calls them: the
//! candle group since QUA-124, the journal and store-record groups since
//! QUA-123, and the segment group since QUA-112. `src/storage/postgres/tests/`
//! steps the races through the store's own read and write-back.
//!
//! - `any_store`: every group again behind `AnyStore`.
//! - `schema`: the schema `init` creates, held to the doc: a repeat, stores
//!   starting together, the refusals, exact round trips, and identities
//!   that never collide.
//! - `candles`: a correction racing a refresh's write-back, which never
//!   leaves its window published as clean.
//! - `journal`: the journal and store record under a linguistic collation,
//!   and writers appending to one run at once.
//! - `server`: the server and its isolated databases, shared with the live
//!   engine's tests.
//! - `plans`: representative data, and the index each query the ports run
//!   is planned on; the evidence `docs/postgres-query-plans.md` records.

mod any_store;
mod candles;
mod journal;
mod plans;
mod schema;
pub mod server;

use quantforge::storage::postgres::PostgresStore;

use crate::conformance::{self, Backend, Fixture, Raw};
pub use server::{IsolatedDatabase, VARIABLE, block_on, client, query, server_url, settings};

/// A `PostgresStore` on a database of its own.
pub struct Postgres;

impl Backend for Postgres {
    type Store = PostgresStore;
    const NAME: &'static str = "postgres";

    fn fresh() -> Fixture<PostgresStore> {
        let server = server_url().unwrap_or_else(|| panic!("{VARIABLE} is unset"));
        fixture_on(IsolatedDatabase::create(&server))
    }
}

/// A fixture whose store has run `init` on `database`.
pub fn fixture_on(database: IsolatedDatabase) -> Fixture<PostgresStore> {
    let reopen = {
        let settings = settings(&database.url);
        move || {
            let store = PostgresStore::connect(&settings).expect("the isolated database");
            store.init().expect("init");
            store
        }
    };
    Fixture {
        store: reopen(),
        raw: Box::new(PostgresRaw(database.url.clone())),
        reopen: Box::new(reopen),
        guard: Box::new(database),
    }
}

/// The raw view of an isolated database, over its own connection.
pub struct PostgresRaw(String);

impl Raw for PostgresRaw {
    fn select(&self, sql: &str) -> Vec<Vec<Option<String>>> {
        query(&self.0, sql)
    }

    fn execute(&self, sql: &str) {
        query(&self.0, sql);
    }

    fn value_types(&self, table: &str, column: &str) -> Vec<String> {
        let sql = format!(
            "SELECT DISTINCT CAST(pg_typeof({column}) AS TEXT) FROM {table} \
             WHERE {column} IS NOT NULL ORDER BY 1"
        );
        query(&self.0, &sql)
            .into_iter()
            .map(|row| row.into_iter().next().flatten().expect("a type"))
            .collect()
    }

    fn blob(&self, bytes: &[u8]) -> String {
        format!("'\\x{}'", hex::encode(bytes))
    }

    fn write_refusal(&self, table: &str) -> [String; 2] {
        [
            format!("ALTER TABLE {table} ADD CONSTRAINT refuse_writes CHECK (false) NOT VALID"),
            format!("ALTER TABLE {table} DROP CONSTRAINT refuse_writes"),
        ]
    }
}

#[test]
fn each_fixture_gets_a_database_of_its_own_and_drops_it() {
    let Some(server) = server_url() else {
        eprintln!("skipping: {VARIABLE} is unset");
        return;
    };
    let first = Postgres::fresh();
    let second = Postgres::fresh();
    first.store.ping().expect("the store reaches its database");
    let name = |fixture: &Fixture<PostgresStore>| {
        fixture
            .guard
            .downcast_ref::<IsolatedDatabase>()
            .expect("an isolated database")
            .name
            .clone()
    };
    let (first_name, second_name) = (name(&first), name(&second));
    assert_ne!(first_name, second_name);
    assert_eq!(
        first.raw.select("SELECT current_database()"),
        [[Some(first_name.clone())]]
    );

    // A table in one is not in the other.
    first.raw.execute("CREATE TABLE probe (value TEXT)");
    first
        .raw
        .execute("INSERT INTO probe(value) VALUES ('7'), (NULL)");
    assert_eq!(first.raw.value_types("probe", "value"), ["text"]);
    assert_eq!(
        first.raw.select("SELECT value FROM probe ORDER BY value"),
        [[Some("7".to_string())], [None]]
    );
    assert_eq!(
        second
            .raw
            .select("SELECT CAST(COUNT(*) AS TEXT) FROM pg_tables WHERE tablename = 'probe'"),
        [[Some("0".to_string())]]
    );

    // Opened again, as a restarted process would, it is the same database.
    let reopened = (first.reopen)();
    reopened.ping().expect("the reopened store");
    drop(reopened);
    drop(first);
    drop(second);

    let left = query(
        &server,
        &format!(
            "SELECT datname FROM pg_database WHERE datname IN ('{first_name}', '{second_name}')"
        ),
    );
    assert!(left.is_empty(), "left behind: {left:?}");
}

#[test]
fn candles() {
    if server_url().is_none() {
        eprintln!("skipping: {VARIABLE} is unset");
        return;
    }
    conformance::candles::<Postgres>();
}

#[test]
fn journal() {
    if server_url().is_none() {
        eprintln!("skipping: {VARIABLE} is unset");
        return;
    }
    conformance::journal::<Postgres>();
}

#[test]
fn control() {
    if server_url().is_none() {
        eprintln!("skipping: {VARIABLE} is unset");
        return;
    }
    conformance::control::<Postgres>();
}

#[test]
fn segments() {
    if server_url().is_none() {
        eprintln!("skipping: {VARIABLE} is unset");
        return;
    }
    conformance::segments::<Postgres>();
}
