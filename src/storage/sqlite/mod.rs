//! SQLite-backed store: connection setup, the schema and its migration
//! chain, and the row-decoding helpers shared by its responsibilities. The
//! work is split across private sibling modules — `candles` implements
//! `CandleStore`, `journal` implements `RunJournalStore`, `segments`
//! implements `SegmentStore`, `control` implements `StoreControl`, and
//! `migrate` brings every earlier schema to [`SCHEMA_VERSION`]. Only
//! [`SqliteStore`] and the version leave this namespace; the schema and the
//! row decoders are not API.

mod candles;
mod control;
mod journal;
mod migrate;
mod segments;

use crate::StorageError;
use rusqlite::Connection;
use rust_decimal::Decimal;
use std::{
    path::{Path, PathBuf},
    str::FromStr,
    time::Duration,
};
use tracing::info;

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

#[derive(Clone, Debug)]
pub struct SqliteStore {
    path: PathBuf,
}

impl SqliteStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn open(&self) -> Result<Connection, StorageError> {
        let connection = Connection::open(&self.path).map_err(StorageError::other)?;
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .map_err(StorageError::other)?;
        connection
            .pragma_update(None, "synchronous", "NORMAL")
            .map_err(StorageError::other)?;
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(StorageError::other)?;
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

fn parse_decimal_str(raw: &str) -> Result<Decimal, rusqlite::Error> {
    Decimal::from_str(raw).map_err(to_from_sql_error)
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
}
