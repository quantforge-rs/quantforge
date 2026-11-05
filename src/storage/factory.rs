//! [`StoreFactory`]: opens the store [`StorageSettings`] describe, through
//! the provider registered for its backend. A host builds it, opens one
//! store with it, and owns that store, its pool and its credentials for the
//! life of the process (ADR 0001): the CLI's context, the server's parent
//! process, later the Python reader. A job worker never receives settings,
//! and never opens a store.
//!
//! A backend with no provider is refused, and no other provider is asked
//! instead: selecting PostgreSQL never ends in a SQLite file. The built-in
//! factory registers SQLite and, with the `postgres` feature, PostgreSQL.
//! The execution store (ALG-738) is the next thing a host opens from the
//! same settings, in the same process, through this factory.

use crate::ports::{CandleStore, StorageError};
use crate::storage::SqliteStore;
use crate::storage::any::AnyStore;
#[cfg(feature = "postgres")]
use crate::storage::postgres::PostgresStore;
use crate::storage::settings::{StorageBackend, StorageConfigError, StorageSettings};
use std::fmt;
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

/// Why a store could not be opened: its settings were refused, or its
/// backend failed.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum StoreOpenError {
    #[error(transparent)]
    Config(#[from] StorageConfigError),
    #[error(transparent)]
    Storage(#[from] StorageError),
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
