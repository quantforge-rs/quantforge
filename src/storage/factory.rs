//! [`StoreFactory`]: opens the store [`StorageSettings`] describe, through
//! the provider registered for its backend. A host builds it, opens one
//! store with it, and owns that store, its pool and its credentials for the
//! life of the process (ADR 0001): the CLI's context, the server's parent
//! process, the Python package's reader. A job worker never receives
//! settings, and never opens a store.
//!
//! A backend with no provider is refused, and no other provider is asked
//! instead: selecting PostgreSQL never ends in a SQLite file. The built-in
//! factory registers SQLite and, with the `postgres` feature, PostgreSQL.
//! The execution store (ALG-738) is the next thing a host opens from the
//! same settings, in the same process, through this factory. The source of
//! a transfer is opened here too, read-only, beside the selected store, and
//! so is a store a host only reads ([`StoreFactory::open_reader`]).

use crate::ports::{CandleStore, StorageError, TransferSnapshot};
use crate::storage::SqliteStore;
use crate::storage::any::AnyStore;
use crate::storage::common::{may_be_a_secret, path_named};
#[cfg(feature = "postgres")]
use crate::storage::postgres::PostgresStore;
use crate::storage::reader::CandleReader;
use crate::storage::settings::{StorageBackend, StorageConfigError, StorageSettings};
use crate::storage::sqlite::SqliteSnapshot;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use thiserror::Error;

/// Opens a store from settings of its backend, and brings its schema up to
/// date before handing it over.
pub type StoreProvider =
    Arc<dyn Fn(&StorageSettings) -> Result<AnyStore, StorageError> + Send + Sync>;

/// The providers a host can open stores with, one per backend.
#[derive(Clone)]
pub struct StoreFactory {
    providers: Vec<(StorageBackend, StoreProvider)>,
}

impl StoreFactory {
    /// A factory with no provider, which refuses every backend.
    pub fn empty() -> Self {
        Self {
            providers: Vec::new(),
        }
    }

    /// The providers this build ships: SQLite and, with the `postgres`
    /// feature, PostgreSQL.
    pub fn builtin() -> Self {
        let factory = Self::empty().register(StorageBackend::Sqlite, Arc::new(open_sqlite));
        #[cfg(feature = "postgres")]
        let factory = factory.register(StorageBackend::Postgres, Arc::new(open_postgres));
        factory
    }

    /// Registers `provider` for `backend`, replacing any earlier one.
    pub fn register(mut self, backend: StorageBackend, provider: StoreProvider) -> Self {
        self.providers
            .retain(|(registered, _)| *registered != backend);
        self.providers.push((backend, provider));
        self
    }

    /// The backends it has a provider for, in registration order.
    pub fn registered(&self) -> impl Iterator<Item = StorageBackend> + '_ {
        self.providers.iter().map(|(backend, _)| *backend)
    }

    /// Opens the store `settings` describe with its backend's provider, or
    /// refuses a backend that has none.
    pub fn open(&self, settings: &StorageSettings) -> Result<AnyStore, StoreOpenError> {
        let backend = settings.backend();
        let (_, provider) = self
            .providers
            .iter()
            .find(|(registered, _)| *registered == backend)
            .ok_or(StorageConfigError::NoAdapter { backend })?;
        Ok(provider(settings)?)
    }

    /// Opens the store `settings` describe to read its candles and nothing
    /// else: no file or schema is created, no schema migrated, and every
    /// connection is read-only ([`CandleReader`]). A SQLite file that does
    /// not exist is [`StoreOpenError::NoSuchFile`]; a database that holds no
    /// store, a store at another schema than this build reads, and one that
    /// holds an unfinished copy are refused as storage errors. A backend
    /// with no provider is refused as it is for [`open`](Self::open). A
    /// SQLite path is settled as the reader opens: one that does not start
    /// at the root names the file it finds where the process runs then.
    pub fn open_reader(&self, settings: &StorageSettings) -> Result<CandleReader, StoreOpenError> {
        let backend = settings.backend();
        let no_adapter = || StorageConfigError::NoAdapter { backend };
        if !self.registered().any(|registered| registered == backend) {
            return Err(no_adapter().into());
        }
        let store = match settings {
            StorageSettings::Sqlite(sqlite) => {
                // A path that does not start at the root names the file where
                // the process runs now: the directory is read once, and the
                // look, the store and a refusal all go by it, wherever another
                // thread moves the process meanwhile or later. Each call of
                // the store opens the file anew.
                let directory = if sqlite.path.is_absolute() {
                    None
                } else {
                    Some(std::env::current_dir().map_err(StorageError::other)?)
                };
                let path = match &directory {
                    Some(directory) => directory.join(&sqlite.path),
                    None => sqlite.path.clone(),
                };
                // What is there, through any link: a file is read; anything
                // else, such as a directory or a pipe, which opening would
                // wait on for good, is refused.
                match std::fs::metadata(&path) {
                    Ok(found) if found.is_file() => {}
                    Ok(found) => {
                        let kind = if found.is_dir() {
                            "a directory"
                        } else {
                            "no file"
                        };
                        return Err(StorageError::InvalidArgument(format!(
                            "{} is {kind}, and a SQLite database is a file",
                            path_named(&sqlite.path)
                        ))
                        .into());
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        return Err(StoreOpenError::NoSuchFile {
                            path: sqlite.path.clone(),
                            directory,
                        });
                    }
                    // A directory on the way that may not be entered, say,
                    // where the file may well be; or a name the system
                    // refuses, as Windows refuses a URL's `:`.
                    Err(error) => {
                        return Err(StorageError::InvalidArgument(format!(
                            "cannot reach {}: {error}",
                            path_named(&sqlite.path)
                        ))
                        .into());
                    }
                }
                AnyStore::Sqlite(SqliteStore::read_only(path, sqlite.limits))
            }
            #[cfg(feature = "postgres")]
            StorageSettings::Postgres(postgres) => {
                AnyStore::Postgres(PostgresStore::connect_read_only(postgres)?)
            }
            // No build without the feature resolves such settings.
            #[cfg(not(feature = "postgres"))]
            StorageSettings::Postgres(_) => return Err(no_adapter().into()),
        };
        Ok(CandleReader::over(store, settings)?)
    }

    /// Opens the SQLite store at `path` as the source of a transfer: one
    /// snapshot, read-only, so the file keeps every byte it had
    /// (`docs/storage-migration.md`). Nothing migrates it: a store at an
    /// earlier schema is refused.
    pub fn open_snapshot(&self, path: &Path) -> Result<Box<dyn TransferSnapshot>, StorageError> {
        Ok(Box::new(SqliteSnapshot::open(path)?))
    }
}

impl Default for StoreFactory {
    fn default() -> Self {
        Self::builtin()
    }
}

impl fmt::Debug for StoreFactory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StoreFactory")
            .field("registered", &self.registered().collect::<Vec<_>>())
            .finish()
    }
}

/// Why a store could not be opened: its settings were refused, its backend
/// failed, or, for a reader, its file is not there.
#[derive(Error)]
#[non_exhaustive]
pub enum StoreOpenError {
    #[error(transparent)]
    Config(#[from] StorageConfigError),
    #[error(transparent)]
    Storage(#[from] StorageError),
    /// A reader was asked for a SQLite store whose file does not exist. A
    /// command would create it empty; a reader creates nothing. `path` is
    /// the path as it was given, and `directory` the one a path that does
    /// not start at the root was looked for in. The path is shown, by
    /// `Display` and `Debug` alike, unless it holds an `@` or an `=`, which
    /// a URL or a connection string with a password given in its place
    /// holds, and a path to a file rarely does.
    #[error(
        "there is no SQLite database at {}; a reader creates none, and `quantforge data sync` \
         does",
        shown(path)
    )]
    NoSuchFile {
        path: PathBuf,
        directory: Option<PathBuf>,
    },
}

impl fmt::Debug for StoreOpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(refused) => f.debug_tuple("Config").field(refused).finish(),
            Self::Storage(failed) => f.debug_tuple("Storage").field(failed).finish(),
            Self::NoSuchFile { path, directory } => f
                .debug_struct("NoSuchFile")
                .field("path", &path_named(path))
                .field("directory", directory)
                .finish(),
        }
    }
}

/// A path as the refusal of a missing file shows it: whole, or not at all
/// where it may be a secret, with why.
fn shown(path: &Path) -> String {
    let text = path.to_string_lossy();
    if may_be_a_secret(&text) {
        "the path given, which is not shown: it holds an @ or an =, as a URL or a connection \
         string with a password does"
            .to_string()
    } else {
        text.into_owned()
    }
}

/// SQLite: the file's parent directory is created if missing, then the file
/// itself, and `init` brings its schema up to date.
fn open_sqlite(settings: &StorageSettings) -> Result<AnyStore, StorageError> {
    let StorageSettings::Sqlite(sqlite) = settings else {
        return Err(StorageError::InvalidArgument(format!(
            "the sqlite provider was handed {} settings",
            settings.backend()
        )));
    };
    if let Some(parent) = sqlite.path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            StorageError::other(std::io::Error::new(
                error.kind(),
                format!(
                    "failed to create parent directory {}: {error}",
                    parent.display()
                ),
            ))
        })?;
    }
    let store = SqliteStore::with_limits(&sqlite.path, sqlite.limits);
    CandleStore::init(&store)?;
    Ok(AnyStore::Sqlite(store))
}

/// PostgreSQL: connecting proves the URL, the password and the CA. The
/// database must exist; `init` creates the schema in it, or brings an
/// earlier one up to date.
#[cfg(feature = "postgres")]
fn open_postgres(settings: &StorageSettings) -> Result<AnyStore, StorageError> {
    let StorageSettings::Postgres(postgres) = settings else {
        return Err(StorageError::InvalidArgument(format!(
            "the postgres provider was handed {} settings",
            settings.backend()
        )));
    };
    let store = PostgresStore::connect(postgres)?;
    store.init()?;
    Ok(AnyStore::Postgres(store))
}

#[cfg(test)]
mod tests;
