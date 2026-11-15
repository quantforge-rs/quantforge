//! What the execution tests share: a store of each backend that can be
//! opened again as a process starting on it would, bound to demo, and a
//! raw view of its tables.

use quantforge::model::Environment;
use quantforge::{CandleStore, ExecutionStore, SqliteStore, StoreControl};
use tempfile::TempDir;

/// 2026-10-20T00:00:00Z.
pub(crate) const T: i64 = 1_792_454_400_000;

/// A store of one backend, bound to demo.
pub(crate) trait Backend {
    type Store: ExecutionStore + StoreControl + Send + Sync + 'static;

    /// The store, opened anew: what a process that starts on it gets.
    fn open(&self) -> Self::Store;

    /// Every row a query returns from the database itself, each value as
    /// text, NULL as `None`. The SQL is portable: every selected column is
    /// text or `CAST(… AS TEXT)`.
    fn select(&self, sql: &str) -> Vec<Vec<Option<String>>>;

    /// What a child process is told to open the same store.
    fn address(&self) -> String;
}

/// The one value of the one row `sql` returns.
pub(crate) fn scalar(backend: &impl Backend, sql: &str) -> String {
    let rows = backend.select(sql);
    match rows.as_slice() {
        [row] if row.len() == 1 => row[0].clone().unwrap_or_default(),
        other => panic!("one value expected from {sql}: {other:?}"),
    }
}

/// How many rows `table` holds.
pub(crate) fn count(backend: &impl Backend, table: &str) -> usize {
    scalar(
        backend,
        &format!("SELECT CAST(COUNT(*) AS TEXT) FROM {table}"),
    )
    .parse()
    .expect("a count")
}

pub(crate) struct Sqlite {
    _dir: TempDir,
    path: std::path::PathBuf,
}

impl Sqlite {
    pub(crate) fn new() -> Self {
        let dir = tempfile::tempdir().expect("a directory");
        let path = dir.path().join("market.sqlite");
        let store = SqliteStore::new(&path);
        CandleStore::init(&store).expect("init");
        store.bind_environment(Environment::Demo).expect("bound");
        Self { _dir: dir, path }
    }

    /// The store an address names.
    pub(crate) fn at(address: &str) -> Option<SqliteStore> {
        address.strip_prefix("sqlite:").map(SqliteStore::new)
    }
}

impl Backend for Sqlite {
    type Store = SqliteStore;

    fn open(&self) -> SqliteStore {
        SqliteStore::new(&self.path)
    }

    fn select(&self, sql: &str) -> Vec<Vec<Option<String>>> {
        let connection = rusqlite::Connection::open(&self.path).expect("open");
        let mut statement = connection.prepare(sql).expect("prepare");
        let columns = statement.column_count();
        let rows = statement
            .query_map([], |row| {
                (0..columns)
                    .map(|index| row.get::<_, Option<String>>(index))
                    .collect::<Result<Vec<_>, _>>()
            })
            .expect("query");
        rows.collect::<Result<_, _>>()
            .unwrap_or_else(|error| panic!("{sql}: {error}"))
    }

    fn address(&self) -> String {
        format!("sqlite:{}", self.path.display())
    }
}

/// The store on a database of its own on the server
/// `QUANTFORGE_TEST_POSTGRES_URL` names.
#[cfg(feature = "postgres")]
pub(crate) mod postgres {
    use super::Backend;
    use crate::server_helpers::{IsolatedDatabase, VARIABLE, query, server_url, settings};
    use quantforge::model::Environment;
    use quantforge::storage::postgres::PostgresStore;
    use quantforge::{CandleStore, StoreControl};

    pub(crate) struct Postgres {
        database: IsolatedDatabase,
    }

    impl Postgres {
        pub(crate) fn new() -> Option<Self> {
            let Some(server) = server_url() else {
                eprintln!("skipping: {VARIABLE} is unset");
                return None;
            };
            let backend = Self {
                database: IsolatedDatabase::create(&server),
            };
            let store = backend.open();
            CandleStore::init(&store).expect("init");
            store.bind_environment(Environment::Demo).expect("bound");
            Some(backend)
        }

        /// The store an address names.
        pub(crate) fn at(address: &str) -> Option<PostgresStore> {
            let url = address.strip_prefix("postgres:")?;
            Some(PostgresStore::connect(&settings(url)).expect("a store"))
        }
    }

    impl Backend for Postgres {
        type Store = PostgresStore;

        fn open(&self) -> PostgresStore {
            PostgresStore::connect(&settings(&self.database.url)).expect("a store")
        }

        fn select(&self, sql: &str) -> Vec<Vec<Option<String>>> {
            query(&self.database.url, sql)
        }

        fn address(&self) -> String {
            format!("postgres:{}", self.database.url)
        }
    }
}

/// Each scenario as a test per backend: `<scenario>::sqlite`, and
/// `<scenario>::postgres` with the feature.
macro_rules! on_each_backend {
    ($($scenario:ident),* $(,)?) => {$(
        mod $scenario {
            #[test]
            fn sqlite() {
                super::$scenario(&crate::support::Sqlite::new());
            }

            #[cfg(feature = "postgres")]
            #[test]
            fn postgres() {
                if let Some(backend) = crate::support::postgres::Postgres::new() {
                    super::$scenario(&backend);
                }
            }
        }
    )*};
}
pub(crate) use on_each_backend;

/// A clock a test sets.
#[derive(Debug)]
pub(crate) struct Manual(std::sync::atomic::AtomicI64);

impl Manual {
    pub(crate) fn at(now_ms: i64) -> Self {
        Self(std::sync::atomic::AtomicI64::new(now_ms))
    }

    pub(crate) fn set(&self, now_ms: i64) {
        self.0.store(now_ms, std::sync::atomic::Ordering::SeqCst);
    }

    pub(crate) fn advance(&self, by_ms: i64) {
        self.0.fetch_add(by_ms, std::sync::atomic::Ordering::SeqCst);
    }
}

impl quantforge::ports::Clock for Manual {
    fn now_ms(&self) -> i64 {
        self.0.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// Runs async work to its end on a runtime of its own.
pub(crate) fn block_on<F: std::future::Future>(work: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime")
        .block_on(work)
}
