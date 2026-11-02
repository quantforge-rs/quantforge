//! The PostgreSQL server the tests run against, and a database of its own
//! on it for each fixture: shared by the storage tests and, through
//! `#[path]`, by the live engine's, so it uses nothing but the crate's
//! public API and the drivers.

use quantforge::storage::settings::PostgresSettings;
use quantforge::{
    ConnectionString, StorageBackend, StorageRequest, StorageSettings, StoreOwner, Supplied,
};
use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio_postgres::{Client, NoTls, SimpleQueryMessage};

/// The server the isolated databases are created on, or `None` to skip.
pub fn server_url() -> Option<String> {
    std::env::var(VARIABLE).ok().filter(|url| !url.is_empty())
}

pub const VARIABLE: &str = "QUANTFORGE_TEST_POSTGRES_URL";

/// Runs one piece of async work on a runtime of its own: the raw view is
/// synchronous, like the ports.
pub fn block_on<F: Future>(work: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime")
        .block_on(work)
}

pub async fn client(url: &str) -> Client {
    let (client, connection) = tokio_postgres::connect(url, NoTls)
        .await
        .unwrap_or_else(|error| panic!("cannot reach {VARIABLE}'s server: {error}"));
    tokio::spawn(async move {
        let _ = connection.await;
    });
    client
}

/// Every row of `sql`, each value as the server's text, NULL as `None`.
/// It runs on a thread of its own, so async tests may call it, and drop
/// an isolated database, without nesting runtimes.
pub fn query(url: &str, sql: &str) -> Vec<Vec<Option<String>>> {
    let (url, sql) = (url.to_string(), sql.to_string());
    let answer = std::thread::spawn(move || {
        block_on(async {
            let messages = client(&url)
                .await
                .simple_query(&sql)
                .await
                .map_err(|error| format!("{sql}: {error:?}"))?;
            Ok::<_, String>(
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
                    .collect(),
            )
        })
    })
    .join()
    .expect("the query's thread");
    answer.unwrap_or_else(|error| panic!("{error}"))
}

/// A database of its own on the server, dropped with it.
pub struct IsolatedDatabase {
    server: String,
    pub name: String,
    pub url: String,
}

impl IsolatedDatabase {
    pub fn create(server: &str) -> Self {
        Self::create_with(server, "")
    }

    /// A database created with `options` after its name, such as a
    /// template and a collation.
    pub fn create_with(server: &str, options: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let name = format!(
            "qf_conformance_{}_{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        query(server, &format!("CREATE DATABASE {name} {options}"));
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
pub fn settings(url: &str) -> PostgresSettings {
    let mut request = StorageRequest::default();
    request.backend = Some(Supplied::new(StorageBackend::Postgres, "--storage"));
    request.postgres_url = Some(Supplied::new(ConnectionString::new(url), VARIABLE));
    match StorageSettings::resolve(request, StoreOwner::Command).expect("settings") {
        StorageSettings::Postgres(settings) => *settings,
        other => panic!("not postgres: {other}"),
    }
}
