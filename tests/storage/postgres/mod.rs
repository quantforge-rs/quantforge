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
//! - `any_store`: every group again through the built-in factory, which
//!   hands a host the same store behind `AnyStore`.
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
use std::any::Any;
use std::process::Command;
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::Duration;
use tokio::sync::oneshot;

use crate::conformance::{self, Backend, Bounded, Fixture, Raw};
pub use server::{
    IsolatedDatabase, REQUIRED, VARIABLE, block_on, client, query, selection, server_url, settings,
    skip, wait_for_a_lock,
};

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

impl Bounded for PostgresStore {
    fn within(&self, budget: Duration) -> Self {
        PostgresStore::within(self, budget)
    }
}

/// The raw view of an isolated database, over its own connection.
pub struct PostgresRaw(String);

/// A table lock held from a thread of its own until it drops.
struct HeldLock {
    release: Option<oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for HeldLock {
    fn drop(&mut self) {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

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

    /// `EXCLUSIVE` stops every other writer of the table and no reader.
    fn hold_writes(&self, table: &str) -> Box<dyn Any> {
        let (url, table) = (self.0.clone(), table.to_string());
        let (locked, held) = mpsc::channel();
        let (release, released) = oneshot::channel();
        let thread = thread::spawn(move || {
            block_on(async move {
                let client = client(&url).await;
                client
                    .batch_execute(&format!("BEGIN; LOCK TABLE {table} IN EXCLUSIVE MODE;"))
                    .await
                    .expect("the lock");
                let _ = locked.send(());
                let _ = released.await;
            });
        });
        held.recv().expect("the lock is held");
        Box::new(HeldLock {
            release: Some(release),
            thread: Some(thread),
        })
    }

    fn end_connections(&self) {
        query(
            &self.0,
            "SELECT pg_terminate_backend(pid) FROM pg_stat_activity \
             WHERE datname = current_database() AND pid <> pg_backend_pid()",
        );
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

/// Where PostgreSQL must be tested, a test with no server to run on fails,
/// and anywhere else it skips: the test binary runs one of its tests again,
/// as a child, with the server's variable taken away.
#[test]
fn a_test_without_its_server_fails_only_where_postgresql_must_be_tested() {
    let run = |required: bool| {
        let mut child = Command::new(std::env::current_exe().expect("the test binary"));
        child
            .args(["postgres::candles", "--exact", "--nocapture"])
            .env_remove(VARIABLE)
            .env_remove(REQUIRED);
        if required {
            child.env(REQUIRED, "1");
        }
        let output = child.output().expect("the test binary runs");
        let text = String::from_utf8_lossy(&output.stdout).into_owned()
            + &String::from_utf8_lossy(&output.stderr);
        (output.status.success(), text)
    };

    let (passed, text) = run(false);
    assert!(passed && text.contains("1 passed"), "{text}");
    assert!(
        text.contains(&format!("skipping: {VARIABLE} is unset")),
        "{text}"
    );

    let (passed, text) = run(true);
    assert!(!passed && text.contains("1 failed"), "{text}");
    assert!(
        text.contains(&format!(
            "{VARIABLE} is unset, and {REQUIRED} says PostgreSQL must be tested"
        )),
        "{text}"
    );
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

#[test]
fn durability() {
    if server_url().is_none() {
        eprintln!("skipping: {VARIABLE} is unset");
        return;
    }
    conformance::durability::<Postgres>();
}
