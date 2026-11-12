//! What the store-backed policy tests share: a store of each backend that
//! can be opened again as a process starting on it would, a venue that
//! lists a fixture line's instruments, and a clock that ticks.

use std::sync::atomic::{AtomicI64, Ordering};

use async_trait::async_trait;
use quantforge::model::futures::{FuturesInstrument, PriceSnapshot, ProfilePolicy};
use quantforge::model::{Environment, ExchangeId, InstrumentId, TimestampMs};
use quantforge::ports::futures::{FuturesCapabilities, FuturesConnector, InstrumentConnector};
use quantforge::ports::{Clock, ExchangeError};
use quantforge::{CandleStore, PolicyStore, SqliteStore, StoreControl};
use tempfile::TempDir;

use crate::cases::Line;

/// 2026-10-20T00:00:00Z.
pub(crate) const T: TimestampMs = 1_792_454_400_000;

/// A store of one backend, bound to demo, the environment of the cases'
/// profiles.
pub(crate) trait Backend {
    type Store: PolicyStore + StoreControl + Send + 'static;

    /// The store, opened anew: what a process that starts on it gets.
    fn open(&self) -> Self::Store;

    /// Another store of the same backend, empty and bound to demo too, for
    /// a test that needs more than one. `None` where the backend has no
    /// server to make one on.
    fn another(&self) -> Option<Self>
    where
        Self: Sized;
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
}

impl Backend for Sqlite {
    type Store = SqliteStore;

    fn open(&self) -> SqliteStore {
        SqliteStore::new(&self.path)
    }

    fn another(&self) -> Option<Self> {
        Some(Self::new())
    }
}

/// The store on a database of its own on the server
/// `QUANTFORGE_TEST_POSTGRES_URL` names.
#[cfg(feature = "postgres")]
pub(crate) mod postgres {
    use super::Backend;
    use crate::server_helpers::{IsolatedDatabase, VARIABLE, server_url, settings};
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
    }

    impl Backend for Postgres {
        type Store = PostgresStore;

        fn open(&self) -> PostgresStore {
            PostgresStore::connect(&settings(&self.database.url)).expect("a store")
        }

        fn another(&self) -> Option<Self> {
            Self::new()
        }
    }
}

/// Each scenario as a test per backend: `<scenario>::sqlite`, and
/// `<scenario>::postgres` with the feature.
macro_rules! on_each_backend {
    ($($scenario:ident),* $(,)?) => {$(
        mod $scenario {
            #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
            async fn sqlite() {
                super::$scenario(&crate::support::Sqlite::new()).await;
            }

            #[cfg(feature = "postgres")]
            #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
            async fn postgres() {
                if let Some(backend) = crate::support::postgres::Postgres::new() {
                    super::$scenario(&backend).await;
                }
            }
        }
    )*};
}
pub(crate) use on_each_backend;

/// A clock that reads one millisecond later each time, from [`T`].
#[derive(Debug, Default)]
pub(crate) struct Ticking(AtomicI64);

impl Clock for Ticking {
    fn now_ms(&self) -> TimestampMs {
        T + self.0.fetch_add(1, Ordering::SeqCst)
    }
}

/// A venue's instrument connector that lists what a fixture line lists.
#[derive(Debug)]
pub(crate) struct Venue {
    pub(crate) exchange: ExchangeId,
    pub(crate) environment: Environment,
    pub(crate) instruments: Vec<FuturesInstrument>,
}

impl Venue {
    /// The connector of `line`'s product line on demo.
    pub(crate) fn of(line: &Line) -> Self {
        Self {
            exchange: line.exchange(),
            environment: Environment::Demo,
            instruments: line.instruments.clone(),
        }
    }
}

impl FuturesConnector for Venue {
    fn exchange_id(&self) -> ExchangeId {
        self.exchange
    }

    fn environment(&self) -> Environment {
        self.environment
    }

    fn capabilities(&self) -> FuturesCapabilities {
        FuturesCapabilities::default()
    }
}

#[async_trait]
impl InstrumentConnector for Venue {
    async fn instruments(&self) -> Result<Vec<FuturesInstrument>, ExchangeError> {
        Ok(self.instruments.clone())
    }

    async fn prices(&self, instrument: &InstrumentId) -> Result<PriceSnapshot, ExchangeError> {
        Err(ExchangeError::InvalidRequest {
            message: format!("no test asks the price of {instrument}"),
        })
    }
}

/// The stored text of a policy.
pub(crate) fn stored(policy: &ProfilePolicy) -> String {
    policy.to_stored().expect("a policy as stored")
}
