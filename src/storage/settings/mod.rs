//! Which backend keeps the data, and how to reach it: the settings every
//! host (the CLI and its server today, the Python reader later) resolves
//! the same way. The library reads no environment variable. A host passes
//! the values its flags and variables held, each labelled with where it
//! came from, and every error names that label, never the value.
//!
//! - [`StorageSettings::resolve`] picks the backend and checks its settings.
//!   A backend is selected explicitly or is SQLite, never guessed: a
//!   PostgreSQL setting beside an omitted backend is refused rather than
//!   left unused, and a SQLite path beside PostgreSQL is refused rather than
//!   ignored.
//! - [`PostgresConnection`] is a checked `postgres://` URL, read into what
//!   the driver takes without the driver: the same checks run in a build
//!   without the `postgres` feature. Its password never leaves it in a
//!   `Display`, a `Debug` or an error.
//!
//! [`crate::storage::factory`] opens the store the settings describe.
//! `docs/configuration.md#storage-backends` is the operator's account.

mod connection;
#[cfg(test)]
mod tests;

pub use connection::{ConnectionString, PostgresConnection, PostgresTls, TlsMode, UrlProblem};

use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;
use thiserror::Error;

/// Where the SQLite database lives when nothing names it.
pub const DEFAULT_SQLITE_PATH: &str = "data/market.sqlite";

/// How long opening a PostgreSQL connection, TLS included, may take when
/// nothing sets it (ADR 0001).
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

// The rest of ADR 0001's connection table, fixed until a story needs them
// settable.
const POSTGRES_WAIT_TIMEOUT: Duration = Duration::from_secs(5);
const POSTGRES_STATEMENT_TIMEOUT: Duration = Duration::from_secs(30);
const POSTGRES_CALL_DEADLINE: Duration = Duration::from_secs(35);
const POSTGRES_KEEPALIVE_IDLE: Duration = Duration::from_secs(30);
const APPLICATION_NAME: &str = "quantforge";

/// A storage backend. TimescaleDB would join as another variant, with its
/// own settings and provider.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum StorageBackend {
    Sqlite,
    Postgres,
}

impl StorageBackend {
    pub const ALL: [Self; 2] = [Self::Sqlite, Self::Postgres];

    /// `sqlite` or `postgres`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sqlite => "sqlite",
            Self::Postgres => "postgres",
        }
    }

    /// Whether this build can open it: PostgreSQL needs the `postgres`
    /// cargo feature, which is on by default.
    pub const fn is_built(self) -> bool {
        match self {
            Self::Sqlite => true,
            Self::Postgres => cfg!(feature = "postgres"),
        }
    }
}

impl fmt::Display for StorageBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for StorageBackend {
    type Err = StorageConfigError;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|backend| backend.as_str() == name)
            .ok_or_else(|| StorageConfigError::UnknownBackend {
                name: name.to_string(),
            })
    }
}

/// The process that opens a store and owns it, its pool and its
/// credentials (ADR 0001: one store per process). It decides the pool's
/// size when nothing sets it. A job worker is never an owner: it receives
/// candles, never settings. The Python reader and the execution service
/// would join as variants.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum StoreOwner {
    /// A CLI command: the only task in its process.
    Command,
    /// `serve run`, which serves many requests at once.
    Server,
}

impl StoreOwner {
    /// ADR 0001's pool sizes: 2 for a command, 8 for the server.
    pub const fn default_pool_size(self) -> u32 {
        match self {
            Self::Command => 2,
            Self::Server => 8,
        }
    }
}

/// A value a host supplied, with the host's name for where it came from,
/// such as `--postgres-url` or `QF_POSTGRES_URL`. Errors name the label and
/// never the value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Supplied<T> {
    pub value: T,
    pub label: String,
}

impl<T> Supplied<T> {
    pub fn new(value: T, label: impl Into<String>) -> Self {
        Self {
            value,
            label: label.into(),
        }
    }
}

/// What a host supplied, before it is checked: a field is `None` when its
/// setting was left at its default.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct StorageRequest {
    pub backend: Option<Supplied<StorageBackend>>,
    pub sqlite_path: Option<Supplied<PathBuf>>,
    pub postgres_url: Option<Supplied<ConnectionString>>,
    pub postgres_pool_size: Option<Supplied<u32>>,
    pub postgres_connect_timeout_secs: Option<Supplied<u64>>,
}

impl StorageRequest {
    /// The labels of the PostgreSQL settings supplied, in a fixed order.
    fn postgres_labels(&self) -> impl Iterator<Item = &str> {
        let url = self.postgres_url.as_ref().map(|s| s.label.as_str());
        let pool = self.postgres_pool_size.as_ref().map(|s| s.label.as_str());
        let timeout = self
            .postgres_connect_timeout_secs
            .as_ref()
            .map(|s| s.label.as_str());
        [url, pool, timeout].into_iter().flatten()
    }
}

/// What a committed write survives. Every store keeps today's behavior,
/// which [`Durability::Normal`] names; ALG-758 adds the stronger mode and
/// enforces the choice on both backends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Durability {
    /// SQLite in WAL mode with `synchronous=NORMAL`, as every SQLite
    /// connection is opened today, and the PostgreSQL server's own commit
    /// settings: a commit survives the application crashing, though not
    /// necessarily the machine losing power.
    Normal,
}

/// Limits every backend shares. They hold today's values; ALG-758 enforces
/// them on both backends and decides how an operator sets them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct StorageLimits {
    pub durability: Durability,
    /// The longest a storage call waits on others, for a lock or a free
    /// connection, before it fails as unavailable. SQLite's busy timeout
    /// and PostgreSQL's pool wait are both this long today.
    pub wait_budget: Duration,
}

impl Default for StorageLimits {
    fn default() -> Self {
        Self {
            durability: Durability::Normal,
            wait_budget: Duration::from_secs(5),
        }
    }
}

/// The resolved settings of one backend.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum StorageSettings {
    Sqlite(SqliteSettings),
    Postgres(Box<PostgresSettings>),
}

/// A SQLite store: one file, created with its parent directory if missing.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct SqliteSettings {
    pub path: PathBuf,
    pub limits: StorageLimits,
}

/// A PostgreSQL store and its pool, as ADR 0001 sets them.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct PostgresSettings {
    pub connection: PostgresConnection,
    /// The most connections the pool opens.
    pub pool_size: u32,
    /// How long opening a connection, TLS included, may take.
    pub connect_timeout: Duration,
    /// How long a call waits for a free connection before it fails as
    /// unavailable.
    pub wait_timeout: Duration,
    /// The server's limit on every statement, set on each new connection.
    pub statement_timeout: Duration,
    /// How long one call may take, end to end; past it the query is
    /// cancelled on the server and its connection discarded.
    pub call_deadline: Duration,
    /// Idle time before TCP keepalives probe a connection.
    pub keepalive_idle: Duration,
    /// Shown in `pg_stat_activity`.
    pub application_name: String,
    pub limits: StorageLimits,
}

impl StorageSettings {
    /// Picks the backend and checks its settings; see the module docs for
    /// the rules and [`StorageConfigError`] for each refusal. Nothing is
    /// opened or created; only a CA file named by `sslrootcert` is looked
    /// at.
    pub fn resolve(request: StorageRequest, owner: StoreOwner) -> Result<Self, StorageConfigError> {
        match request.backend.as_ref().map(|backend| backend.value) {
            Some(StorageBackend::Postgres) => Self::postgres(request, owner),
            Some(StorageBackend::Sqlite) => Self::sqlite(request),
            None => {
                let unused = request.postgres_labels().next().map(str::to_string);
                match unused {
                    Some(label) => Err(StorageConfigError::PostgresSettingWithoutBackend { label }),
                    None => Self::sqlite(request),
                }
            }
        }
    }

    /// The backend these settings are for.
    pub fn backend(&self) -> StorageBackend {
        match self {
            Self::Sqlite(_) => StorageBackend::Sqlite,
            Self::Postgres(_) => StorageBackend::Postgres,
        }
    }

    fn sqlite(request: StorageRequest) -> Result<Self, StorageConfigError> {
        let path = match request.sqlite_path {
            // SQLite would open a temporary database that vanishes on close.
            Some(path) if path.value.as_os_str().is_empty() => {
                return Err(StorageConfigError::EmptySqlitePath { label: path.label });
            }
            Some(path) => path.value,
            None => PathBuf::from(DEFAULT_SQLITE_PATH),
        };
        Ok(Self::Sqlite(SqliteSettings {
            path,
            limits: StorageLimits::default(),
        }))
    }

    fn postgres(request: StorageRequest, owner: StoreOwner) -> Result<Self, StorageConfigError> {
        if !StorageBackend::Postgres.is_built() {
            return Err(StorageConfigError::NotBuilt {
                backend: StorageBackend::Postgres,
            });
        }
        if let Some(path) = request.sqlite_path {
            return Err(StorageConfigError::SqlitePathWithPostgres { label: path.label });
        }
        let url = request
            .postgres_url
            .ok_or(StorageConfigError::MissingPostgresUrl)?;
        let connection = PostgresConnection::parse(&url.value).map_err(|problem| {
            StorageConfigError::InvalidPostgresUrl {
                label: url.label,
                problem,
            }
        })?;
        let pool_size = match request.postgres_pool_size {
            Some(size) => at_least_one(size)?,
            None => owner.default_pool_size(),
        };
        let connect_timeout = match request.postgres_connect_timeout_secs {
            Some(secs) => Duration::from_secs(at_least_one(secs)?),
            None => DEFAULT_CONNECT_TIMEOUT,
        };
        Ok(Self::Postgres(Box::new(PostgresSettings {
            connection,
            pool_size,
            connect_timeout,
            wait_timeout: POSTGRES_WAIT_TIMEOUT,
            statement_timeout: POSTGRES_STATEMENT_TIMEOUT,
            call_deadline: POSTGRES_CALL_DEADLINE,
            keepalive_idle: POSTGRES_KEEPALIVE_IDLE,
            application_name: APPLICATION_NAME.to_string(),
            limits: StorageLimits::default(),
        })))
    }
}

/// `sqlite <path>` or `postgres postgres://host:port/database`: what an
/// operator may see, never a user or password.
impl fmt::Display for StorageSettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(sqlite) => write!(f, "sqlite {}", sqlite.path.display()),
            Self::Postgres(postgres) => write!(f, "postgres {}", postgres.connection),
        }
    }
}

fn at_least_one<T: Copy + PartialOrd + From<u8>>(
    supplied: Supplied<T>,
) -> Result<T, StorageConfigError> {
    if supplied.value < T::from(1) {
        return Err(StorageConfigError::BelowOne {
            label: supplied.label,
        });
    }
    Ok(supplied.value)
}

/// Why storage settings were refused. A message names the label a host
/// gave a setting, never its value, so none can carry a password.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum StorageConfigError {
    #[error("unknown storage backend {name:?}; use sqlite or postgres")]
    UnknownBackend { name: String },

    #[error(
        "{backend} storage is not in this build, which was compiled without the `{backend}` \
         feature; use a build with default features, or select the sqlite backend"
    )]
    NotBuilt { backend: StorageBackend },

    #[error(
        "{label} is a PostgreSQL setting, but no storage backend is selected, so this would use \
         SQLite; select the postgres backend to use it, or the sqlite backend explicitly to keep \
         SQLite"
    )]
    PostgresSettingWithoutBackend { label: String },

    #[error(
        "{label} names a SQLite file, but the postgres backend is selected; unset it, or select \
         the sqlite backend"
    )]
    SqlitePathWithPostgres { label: String },

    #[error("{label} is empty; name the SQLite database file, or leave it unset")]
    EmptySqlitePath { label: String },

    #[error("postgres storage needs a connection URL: postgres://USER:PASSWORD@HOST:PORT/DATABASE")]
    MissingPostgresUrl,

    #[error("{label} is not a usable PostgreSQL URL: {problem}")]
    InvalidPostgresUrl { label: String, problem: UrlProblem },

    #[error("{label} must be at least 1")]
    BelowOne { label: String },

    #[error("{backend} storage has no adapter yet")]
    NoAdapter { backend: StorageBackend },
}
