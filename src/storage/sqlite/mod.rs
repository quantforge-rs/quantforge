//! SQLite-backed store: connection setup, the schema and its migration
//! chain, and the row-decoding helpers shared by its responsibilities. The
//! work is split across private sibling modules — `candles` implements
//! `CandleStore`, `journal` implements `RunJournalStore`, `segments`
//! implements `SegmentStore`, `control` implements `StoreControl`, and
//! `migrate` brings every earlier schema to [`SCHEMA_VERSION`]; `snapshot`
//! reads a store read-only as the source of a transfer. Only
//! [`SqliteStore`], [`SqliteSnapshot`] and the version leave this
//! namespace; the schema and the row decoders are not API.

mod candles;
mod control;
mod errors;
mod journal;
mod migrate;
mod segments;
mod snapshot;
mod wait;

pub use snapshot::SqliteSnapshot;

use crate::StorageError;
use crate::model::{DurabilityReport, DurabilitySetting};
use crate::storage::common::{InexactDecimal, read_decimal};
use crate::storage::settings::{Durability, StorageLimits, lock_wait};
use errors::failed;
use rusqlite::Connection;
use rust_decimal::Decimal;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use tracing::info;
use wait::Waits;

/// The schema this build writes, recorded in `meta.schema_version` and
/// checked on every `init`.
///
/// A change to the DDL or to the shape of `state_json`/`raw_json` bumps it
/// and adds a step to the chain in `migrate.rs`, which brings every
/// earlier schema forward in place (`docs/storage-schema.md`); a database
/// is never deleted for a schema change. Only a database from a newer
/// build is refused, as [`StorageError::SchemaUnsupported`].
pub const SCHEMA_VERSION: u32 = 5;

// Indentation inside this literal is load-bearing: SQLite records the exact
// statement text in `sqlite_master`, so reformatting it would change the
// schema of freshly created databases.
const META_DDL: &str = r#"
                CREATE TABLE IF NOT EXISTS meta (
                  key TEXT PRIMARY KEY,
                  value TEXT NOT NULL
                );
"#;

/// `synchronous=FULL`, as `PRAGMA synchronous` reads it back.
const SYNCHRONOUS_FULL: i64 = 2;

/// What an operator does about a connection that falls short. A setting
/// falls short only when the file cannot have it: `synchronous` and
/// `fullfsync` are the connection's own, and WAL needs shared memory.
const REMEDY: &str = "keep the database on a local disk, not a network share, and open it \
     through no VFS without shared memory";

#[derive(Clone, Debug)]
pub struct SqliteStore {
    path: PathBuf,
    durability: Durability,
    /// How long a connection waits on another writer's lock.
    lock_wait: Duration,
    /// Whether each of its waits has that long, or all of them.
    waits: Waits,
}

impl SqliteStore {
    /// A store with the shared limits, enforcing the durability contract.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self::with_limits(path, StorageLimits::default())
    }

    pub fn with_limits(path: impl Into<PathBuf>, limits: StorageLimits) -> Self {
        Self {
            path: path.into(),
            durability: limits.durability,
            lock_wait: limits.wait_budget,
            waits: Waits::Apart,
        }
    }

    /// The same file, each call through it bounded by `budget`: however many
    /// locks it meets, it gives up on them in time to answer inside it. The
    /// disk's own time cannot be bounded here, since a call runs on the
    /// caller's thread.
    pub fn within(&self, budget: Duration) -> Self {
        Self {
            lock_wait: self.lock_wait.min(lock_wait(budget)),
            waits: Waits::Together,
            ..self.clone()
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// A connection with the settings of the durability contract, checked
    /// when the store enforces them. The wait for a lock goes first, so that
    /// applying the rest already waits as it should.
    fn open(&self) -> Result<Connection, StorageError> {
        let connection = Connection::open(&self.path).map_err(failed)?;
        wait::at_most(&connection, self.lock_wait, self.waits).map_err(failed)?;
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .map_err(failed)?;
        connection
            .pragma_update(None, "synchronous", "FULL")
            .map_err(failed)?;
        // Without it macOS's fsync stops at the drive's cache.
        connection
            .pragma_update(None, "fullfsync", "ON")
            .map_err(failed)?;
        if self.durability == Durability::Enforced {
            if let Some(problem) = effective(&connection)?.problem() {
                return Err(StorageError::DurabilityUnmet(problem));
            }
        }
        Ok(connection)
    }

    /// Creates the schema in a fresh file, migrates an earlier one forward,
    /// and refuses a newer one, all in `migrate`.
    fn initialize_schema(&self) -> Result<(), StorageError> {
        migrate::ensure_current(self)?;
        info!(db_path = %self.path.display(), "sqlite store initialized");
        Ok(())
    }
}

/// The settings a connection actually has: asking for WAL where the file
/// cannot have it leaves the old journal in place, and no error says so.
fn effective(connection: &Connection) -> Result<DurabilityReport, StorageError> {
    let journal_mode: String = connection
        .pragma_query_value(None, "journal_mode", |row| row.get(0))
        .map_err(failed)?;
    let synchronous: i64 = connection
        .pragma_query_value(None, "synchronous", |row| row.get(0))
        .map_err(failed)?;
    let fullfsync: i64 = connection
        .pragma_query_value(None, "fullfsync", |row| row.get(0))
        .map_err(failed)?;
    let settings = vec![
        DurabilitySetting {
            name: "journal_mode",
            met: journal_mode.eq_ignore_ascii_case("wal"),
            effective: journal_mode.to_lowercase(),
            required: "wal",
        },
        DurabilitySetting {
            name: "synchronous",
            effective: match synchronous {
                0 => "off".to_string(),
                1 => "normal".to_string(),
                2 => "full".to_string(),
                3 => "extra".to_string(),
                other => other.to_string(),
            },
            required: "full",
            met: synchronous >= SYNCHRONOUS_FULL,
        },
        DurabilitySetting {
            name: "fullfsync",
            effective: if fullfsync == 0 { "off" } else { "on" }.to_string(),
            required: "on",
            met: fullfsync != 0,
        },
    ];
    Ok(DurabilityReport::new(settings, REMEDY))
}

/// Reads a stored decimal exactly, inside a row closure; [`row_error`]
/// turns a refusal back into the `InvalidArgument` it is.
fn parse_decimal_str(raw: &str) -> Result<Decimal, rusqlite::Error> {
    read_decimal(raw).map_err(to_from_sql_error)
}

/// A row's error as the store reports it: stored text no `Decimal` holds
/// exactly is refused as `InvalidArgument`, anything else is `Other`.
fn row_error(error: rusqlite::Error) -> StorageError {
    if let rusqlite::Error::FromSqlConversionFailure(_, _, source) = &error {
        if let Some(inexact) = source.downcast_ref::<InexactDecimal>() {
            return StorageError::InvalidArgument(inexact.to_string());
        }
    }
    failed(error)
}

fn to_from_sql_error<E>(err: E) -> rusqlite::Error
where
    E: std::error::Error + Send + Sync + 'static,
{
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CandleStore;
    use tempfile::tempdir;

    #[test]
    fn init_refuses_a_database_from_a_newer_build_and_leaves_it_untouched() {
        let tempdir = tempdir().expect("tempdir");
        let db_path = tempdir.path().join("market.sqlite");

        let store = SqliteStore::new(&db_path);
        CandleStore::init(&store).expect("first init");

        let connection = Connection::open(&db_path).expect("open");
        let newer = (SCHEMA_VERSION + 1).to_string();
        connection
            .execute(
                "UPDATE meta SET value = ?1 WHERE key = 'schema_version'",
                [&newer],
            )
            .expect("doctor version");
        drop(connection);

        let error = CandleStore::init(&store).expect_err("newer schema");
        assert!(matches!(
            error,
            StorageError::SchemaUnsupported { found, supported }
                if found == SCHEMA_VERSION + 1 && supported == SCHEMA_VERSION
        ));
        assert!(
            error.to_string().contains(&format!(
                "schema version {newer} is newer than this build supports ({SCHEMA_VERSION})"
            )),
            "got {error}"
        );
        let connection = Connection::open(&db_path).expect("open");
        let recorded: String = connection
            .query_row(
                "SELECT value FROM meta WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .expect("version");
        assert_eq!(recorded, newer, "the version stamp was not rewritten");
    }

    #[test]
    fn init_over_a_current_database_changes_nothing() {
        let tempdir = tempdir().expect("tempdir");
        let db_path = tempdir.path().join("market.sqlite");
        let store = SqliteStore::new(&db_path);
        CandleStore::init(&store).expect("first init");
        CandleStore::init(&store).expect("second init");
        let connection = Connection::open(&db_path).expect("open");
        let steps: i64 = connection
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .expect("count");
        assert_eq!(steps, 1, "a fresh store logs one step, from 0");
    }

    #[test]
    fn every_connection_has_the_settings_of_the_durability_contract() {
        let tempdir = tempdir().expect("tempdir");
        let store = SqliteStore::new(tempdir.path().join("market.sqlite"));
        let connection = store.open().expect("a connection");
        let text = |name: &str| -> String {
            connection
                .pragma_query_value(None, name, |row| row.get(0))
                .expect("a pragma")
        };
        let number = |name: &str| -> i64 {
            connection
                .pragma_query_value(None, name, |row| row.get(0))
                .expect("a pragma")
        };
        assert_eq!(text("journal_mode"), "wal");
        assert_eq!(number("synchronous"), SYNCHRONOUS_FULL);
        assert_eq!(number("fullfsync"), 1);
        let report = effective(&connection).expect("the report");
        assert!(report.is_met(), "{report}");
        assert_eq!(
            report.to_string(),
            "journal_mode=wal synchronous=full fullfsync=on"
        );
    }

    /// `unix-dotfile` has no shared memory, so WAL cannot work on it, and
    /// asking for it fails in silence: the journal stays as it was.
    #[cfg(unix)]
    #[test]
    fn a_file_that_cannot_have_wal_is_refused_when_enforced_and_reported_otherwise() {
        let tempdir = tempdir().expect("tempdir");
        let path = format!(
            "file:{}?vfs=unix-dotfile",
            tempdir.path().join("market.sqlite").display()
        );
        let refused = SqliteStore::new(&path)
            .open()
            .expect_err("no WAL on this VFS");
        assert_eq!(refused.kind(), crate::StorageErrorKind::DurabilityUnmet);
        let message = refused.to_string();
        assert!(
            message.contains("journal_mode is delete, and needs wal"),
            "{message}"
        );
        assert!(message.contains("local disk"), "{message}");

        let limits = StorageLimits {
            durability: Durability::Reported,
            ..StorageLimits::default()
        };
        let store = SqliteStore::with_limits(&path, limits);
        CandleStore::init(&store).expect("a store that reports");
        let report = crate::StoreControl::durability(&store).expect("the report");
        assert!(!report.is_met(), "{report}");
    }

    #[test]
    fn a_bounded_store_gives_up_on_a_lock_in_time_and_never_waits_longer_than_it_would() {
        let store = SqliteStore::new("market.sqlite");
        let full = store.lock_wait;
        assert_eq!(
            store.within(Duration::from_secs(2)).lock_wait,
            Duration::from_millis(1500)
        );
        assert_eq!(store.within(Duration::from_secs(60)).lock_wait, full);
    }

    /// Connections are opened with the settings of the durability contract
    /// and nothing else. A schema that relied on one for its integrity, as
    /// foreign keys rely on `foreign_keys`, would need `open` to set it.
    #[test]
    fn no_table_relies_on_a_connection_setting_for_its_integrity() {
        let tempdir = tempdir().expect("tempdir");
        let store = SqliteStore::new(tempdir.path().join("market.sqlite"));
        CandleStore::init(&store).expect("init");
        let connection = store.open().expect("a connection");
        let foreign_keys: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master m, pragma_foreign_key_list(m.name) \
                 WHERE m.type = 'table'",
                [],
                |row| row.get(0),
            )
            .expect("count");
        assert_eq!(foreign_keys, 0, "set foreign_keys in `open`, and check it");
    }
}
