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
//! place. Each port's group runs here once its store lands, as `sqlite.rs`
//! calls them: the candle group since QUA-124, the journal group with
//! QUA-123, and the segment group with QUA-112.
//!
//! - `schema`: the schema `init` creates, held to the doc: a repeat, stores
//!   starting together, the refusals, exact round trips, and identities
//!   that never collide.
//! - `candles`: a correction racing a refresh's write-back, which never
//!   leaves its window published as clean.
//! - `plans`: representative data, and the index each query the ports run
//!   is planned on; the evidence `docs/postgres-query-plans.md` records.

mod candles;
mod plans;
mod schema;

use quantforge::storage::postgres::PostgresStore;
use quantforge::storage::settings::PostgresSettings;
use quantforge::{
    ConnectionString, StorageBackend, StorageRequest, StorageSettings, StoreOwner, Supplied,
};
use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio_postgres::{Client, NoTls, SimpleQueryMessage};

use crate::conformance::{self, Backend, Fixture, Raw};

/// The server the isolated databases are created on, or `None` to skip.
fn server_url() -> Option<String> {
    std::env::var(VARIABLE).ok().filter(|url| !url.is_empty())
}

const VARIABLE: &str = "QUANTFORGE_TEST_POSTGRES_URL";

/// Runs one piece of async work on a runtime of its own: the raw view is
/// synchronous, like the ports.
fn block_on<F: Future>(work: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime")
        .block_on(work)
}

async fn client(url: &str) -> Client {
    let (client, connection) = tokio_postgres::connect(url, NoTls)
        .await
        .unwrap_or_else(|error| panic!("cannot reach {VARIABLE}'s server: {error}"));
    tokio::spawn(async move {
        let _ = connection.await;
    });
    client
}

/// Every row of `sql`, each value as the server's text, NULL as `None`.
fn query(url: &str, sql: &str) -> Vec<Vec<Option<String>>> {
    block_on(async {
        let messages = client(url)
            .await
            .simple_query(sql)
            .await
            .unwrap_or_else(|error| panic!("{sql}: {error:?}"));
        messages
            .into_iter()
            .filter_map(|message| match message {
                SimpleQueryMessage::Row(row) => Some(
                    (0..row.len())
                        .map(|index| row.get(index).map(str::to_string))
                        .collect(),
                ),
                _ => None,
            })
            .collect()
    })
}

/// A database of its own on the server, dropped with it.
pub struct IsolatedDatabase {
    server: String,
    pub name: String,
    pub url: String,
}

impl IsolatedDatabase {
    pub fn create(server: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let name = format!(
            "qf_conformance_{}_{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        query(server, &format!("CREATE DATABASE {name}"));
        let mut url = url::Url::parse(server).expect("the server URL");
        url.set_path(&format!("/{name}"));
        Self {
            server: server.to_string(),
            name,
            url: url.to_string(),
        }
    }
}

impl Drop for IsolatedDatabase {
    fn drop(&mut self) {
        // Never a second panic while a failed scenario unwinds.
        let drop = format!("DROP DATABASE IF EXISTS {} WITH (FORCE)", self.name);
        let _ = std::panic::catch_unwind(|| query(&self.server, &drop));
    }
}

/// The settings the store opens the isolated database with.
fn settings(url: &str) -> PostgresSettings {
    let mut request = StorageRequest::default();
    request.backend = Some(Supplied::new(StorageBackend::Postgres, "--storage"));
    request.postgres_url = Some(Supplied::new(ConnectionString::new(url), VARIABLE));
    match StorageSettings::resolve(request, StoreOwner::Command).expect("settings") {
        StorageSettings::Postgres(settings) => *settings,
        other => panic!("not postgres: {other}"),
    }
}

/// A `PostgresStore` on a database of its own.
pub struct Postgres;

impl Backend for Postgres {
    type Store = PostgresStore;
    const NAME: &'static str = "postgres";

    fn fresh() -> Fixture<PostgresStore> {
        let server = server_url().unwrap_or_else(|| panic!("{VARIABLE} is unset"));
        let database = IsolatedDatabase::create(&server);
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
