//! The bridge against a real database: every calling context, the pool's
//! bound, each call's deadline, a refused statement, a dropped store, a
//! wrong password, and TLS. They run against the database
//! `QUANTFORGE_TEST_POSTGRES_URL` names (and, for TLS,
//! `QUANTFORGE_TEST_POSTGRES_TLS_URL`: host `localhost`,
//! `sslmode=verify-full`, `sslrootcert`), and skip when it is unset.

use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio_postgres::types::Type;

/// The database these tests run against, or `None` to skip them.
pub(super) fn database_url(variable: &str) -> Option<String> {
    let url = std::env::var(variable).ok().filter(|url| !url.is_empty());
    if url.is_none() {
        eprintln!("skipping: {variable} is unset");
    }
    url
}

pub(super) const DATABASE: &str = "QUANTFORGE_TEST_POSTGRES_URL";
const TLS_DATABASE: &str = "QUANTFORGE_TEST_POSTGRES_TLS_URL";

/// An `application_name` no other test in this run uses.
pub(super) fn unique(prefix: &str) -> String {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    format!(
        "qf_{prefix}_{}_{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

/// Counts what the server shows in `pg_stat_activity`, one statement at a
/// time, so that each count is fresh rather than one transaction's
/// snapshot.
struct Observer {
    store: PostgresStore,
}

impl Observer {
    fn connect(url: &str) -> Self {
        let mut settings = settings(url, &unique("observer"));
        settings.pool_size = 1;
        Self {
            store: PostgresStore::connect(&settings).expect("an observer"),
        }
    }

    fn count(&self, sql: &'static str, application_name: &str) -> i64 {
        let name = application_name.to_string();
        self.store
            .bridge
            .execute(Effect::Read, move |client| {
                Box::pin(async move {
                    let row = client.query_typed_one(sql, &[(&name, Type::TEXT)]).await?;
                    row.try_get::<_, i64>(0)
                })
            })
            .expect("a count")
    }

    /// The connections open under `application_name`.
    fn connections(&self, application_name: &str) -> i64 {
        self.count(
            "SELECT count(*) FROM pg_stat_activity WHERE application_name = $1",
            application_name,
        )
    }

    /// Those of them waiting in `pg_sleep`.
    fn sleeping(&self, application_name: &str) -> i64 {
        self.count(
            "SELECT count(*) FROM pg_stat_activity
             WHERE application_name = $1 AND wait_event = 'PgSleep'",
            application_name,
        )
    }

    /// Waits up to five seconds for `count` to say `expected`.
    fn wait_for(&self, expected: i64, count: impl Fn(&Self) -> i64) -> i64 {
        let deadline = Instant::now() + PATIENCE;
        loop {
            let found = count(self);
            if found == expected || Instant::now() > deadline {
                return found;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

pub(super) fn sleep(
    store: &PostgresStore,
    effect: Effect,
    seconds: f64,
) -> Result<(), StorageError> {
    store.bridge.execute(effect, move |client| {
        Box::pin(async move {
            client
                .batch_execute(&format!("SELECT pg_sleep({seconds})"))
                .await
        })
    })
}

#[tokio::test]
async fn a_current_thread_runtime_reaches_the_database() {
    let Some(url) = database_url(DATABASE) else {
        return;
    };
    let store = PostgresStore::connect(&settings(&url, &unique("current"))).expect("a store");
    store.ping().expect("an answer");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_multi_thread_runtime_reaches_the_database() {
    let Some(url) = database_url(DATABASE) else {
        return;
    };
    let store = PostgresStore::connect(&settings(&url, &unique("multi"))).expect("a store");
    store.ping().expect("an answer");
}

#[tokio::test]
async fn a_blocking_thread_reaches_the_database() {
    let Some(url) = database_url(DATABASE) else {
        return;
    };
    let settings = settings(&url, &unique("blocking"));
    tokio::task::spawn_blocking(move || {
        PostgresStore::connect(&settings)
            .expect("a store")
            .ping()
            .expect("an answer");
    })
    .await
    .expect("the blocking task");
}

#[test]
fn a_full_pool_refuses_a_call_in_time_and_never_opens_another_connection() {
    let Some(url) = database_url(DATABASE) else {
        return;
    };
    let observer = Observer::connect(&url);
    let name = unique("exhausted");
    let mut settings = settings(&url, &name);
    settings.pool_size = 1;
    settings.wait_timeout = Duration::from_millis(300);
    let store = Arc::new(PostgresStore::connect(&settings).expect("a store"));

    let holder = Arc::clone(&store);
    let held = std::thread::spawn(move || sleep(&holder, Effect::Read, 1.5));
    assert_eq!(observer.wait_for(1, |o| o.sleeping(&name)), 1);

    let started = Instant::now();
    let refused = store.ping().expect_err("no free connection");
    assert!(
        matches!(&refused, StorageError::Unavailable(what) if what.contains("no free connection")),
        "{refused:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(observer.connections(&name), 1);
    held.join().expect("the holder").expect("its sleep");
}

#[test]
fn a_read_past_its_deadline_is_cancelled_and_its_connection_discarded() {
    let Some(url) = database_url(DATABASE) else {
        return;
    };
    let observer = Observer::connect(&url);
    let name = unique("deadline");
    let mut settings = settings(&url, &name);
    settings.pool_size = 1;
    settings.call_deadline = Duration::from_millis(300);
    let store = PostgresStore::connect(&settings).expect("a store");

    // Far longer than the observer waits below: without the cancel the
    // server would still be sleeping when it looks.
    let started = Instant::now();
    let lost = sleep(&store, Effect::Read, 30.0).expect_err("past its deadline");
    assert!(matches!(lost, StorageError::Unavailable(_)), "{lost:?}");
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );

    // The pool's only connection was discarded, not left busy.
    let started = Instant::now();
    store.ping().expect("a fresh connection");
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );

    // And the server stopped the abandoned query instead of running it out.
    assert_eq!(observer.wait_for(0, |o| o.sleeping(&name)), 0);
}

#[test]
fn a_write_past_its_deadline_has_an_unknown_outcome() {
    let Some(url) = database_url(DATABASE) else {
        return;
    };
    let mut settings = settings(&url, &unique("deadline_write"));
    settings.call_deadline = Duration::from_millis(300);
    let store = PostgresStore::connect(&settings).expect("a store");
    let lost = sleep(&store, Effect::Write, 5.0).expect_err("past its deadline");
    assert!(matches!(lost, StorageError::OutcomeUnknown(_)), "{lost:?}");
}

#[test]
fn a_statement_the_database_refuses_is_neither_unavailable_nor_unknown() {
    let Some(url) = database_url(DATABASE) else {
        return;
    };
    let store = PostgresStore::connect(&settings(&url, &unique("refused"))).expect("a store");
    let refused = store
        .bridge
        .execute(Effect::Write, |client| {
            Box::pin(async move { client.batch_execute("SELECT * FROM no_such_table").await })
        })
        .expect_err("refused");
    assert!(matches!(refused, StorageError::Other(_)), "{refused:?}");
}

#[tokio::test]
async fn dropping_the_store_inside_async_code_closes_its_connections() {
    let Some(url) = database_url(DATABASE) else {
        return;
    };
    let observer = Observer::connect(&url);
    let name = unique("dropped");
    let store = PostgresStore::connect(&settings(&url, &name)).expect("a store");
    store.ping().expect("an answer");
    assert_eq!(observer.connections(&name), 1);
    drop(store);
    assert_eq!(observer.wait_for(0, |o| o.connections(&name)), 0);
}

#[test]
fn a_wrong_password_is_refused_without_being_shown() {
    let Some(url) = database_url(DATABASE) else {
        return;
    };
    let mut wrong = url::Url::parse(&url).expect("a URL");
    if wrong.password().is_none() {
        eprintln!("skipping: {DATABASE} carries no password to get wrong");
        return;
    }
    wrong.set_password(Some(SENTINEL)).expect("a password");
    let refused = PostgresStore::connect(&settings(wrong.as_str(), &unique("wrong")))
        .expect_err("a wrong password");
    assert!(
        matches!(refused, StorageError::Unavailable(_)),
        "{refused:?}"
    );
    assert!(
        !format!("{refused} {refused:?}").contains(SENTINEL),
        "the password leaked"
    );
}

#[test]
fn verify_full_with_the_clusters_ca_connects_over_tls() {
    let Some(url) = database_url(TLS_DATABASE) else {
        return;
    };
    let store = PostgresStore::connect(&settings(&url, &unique("tls"))).expect("a TLS store");
    let ssl = store
        .bridge
        .execute(Effect::Read, |client| {
            Box::pin(async move {
                let row = client
                    .query_typed_one(
                        "SELECT ssl FROM pg_stat_ssl WHERE pid = pg_backend_pid()",
                        &[],
                    )
                    .await?;
                row.try_get::<_, bool>(0)
            })
        })
        .expect("the session's TLS state");
    assert!(ssl);
}

#[test]
fn without_the_ca_the_servers_certificate_is_refused() {
    let Some(url) = database_url(TLS_DATABASE) else {
        return;
    };
    let mut parsed = url::Url::parse(&url).expect("a URL");
    let kept: Vec<(String, String)> = parsed
        .query_pairs()
        .filter(|(key, _)| key != "sslrootcert")
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    parsed.query_pairs_mut().clear().extend_pairs(kept);
    let refused = PostgresStore::connect(&settings(parsed.as_str(), &unique("tls_no_ca")))
        .expect_err("an untrusted certificate");
    assert!(
        matches!(refused, StorageError::Unavailable(_)),
        "{refused:?}"
    );
}
