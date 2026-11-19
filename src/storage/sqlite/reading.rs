//! Connections that cannot write: what a read-only store and the snapshot
//! of a transfer read a file through.
//!
//! The file is opened read-only, never as a store opens it for a command,
//! so nothing creates it, migrates it, sets its journal mode or checkpoints
//! it: the database file keeps every byte it had, and a file that does not
//! exist is not created. SQLite may leave an empty `-wal` file and a `-shm`
//! file beside a WAL database a reader opened, as any reader does.

use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, ErrorCode, OpenFlags, ffi};

use super::errors::failed;
use super::wait::{self, Waits};
use super::{Access, SqliteStore, migrate};
use crate::ports::StorageError;
use crate::storage::settings::{Durability, StorageLimits};

impl SqliteStore {
    /// The store at `path`, read and never written: every connection it
    /// opens is read-only, so no call through it creates the file, changes
    /// its schema or writes a row, and a call that would is refused by
    /// SQLite itself. Its durability is not judged, since it commits
    /// nothing.
    pub fn read_only(path: impl Into<PathBuf>, limits: StorageLimits) -> Self {
        Self {
            path: path.into(),
            durability: Durability::Reported,
            lock_wait: limits.wait_budget,
            waits: Waits::Apart,
            access: Access::Read,
        }
    }

    /// The schema version the file records, 0 for a database that holds no
    /// store, read without changing anything. A file that cannot be read as
    /// a database is refused with what to do about it.
    pub(crate) fn recorded_version(&self) -> Result<u32, StorageError> {
        let connection = connection(&self.path, self.lock_wait, self.waits)?;
        // The first statement is where SQLite reads the file: one that is no
        // database, or cannot be read, fails here and not as it opens.
        connection
            .execute_batch("SELECT count(*) FROM sqlite_master")
            .map_err(|error| opening(&self.path, error))?;
        migrate::current_version(&connection)
    }
}

/// A connection that cannot write: opened read-only and asked to stay so. A
/// file that does not exist is not created.
pub(super) fn connection(
    path: &Path,
    lock_wait: Duration,
    waits: Waits,
) -> Result<Connection, StorageError> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| opening(path, error))?;
    wait::at_most(&connection, lock_wait, waits).map_err(failed)?;
    connection
        .pragma_update(None, "query_only", "ON")
        .map_err(failed)?;
    Ok(connection)
}

/// Why a file could not be read as a store, with what to do about it.
pub(super) fn opening(path: &Path, error: rusqlite::Error) -> StorageError {
    let path = path.display();
    match error.sqlite_error_code() {
        Some(ErrorCode::CannotOpen | ErrorCode::NotFound) => {
            StorageError::InvalidArgument(format!("cannot open {path}: {error}"))
        }
        Some(ErrorCode::NotADatabase) => {
            StorageError::InvalidArgument(format!("{path} is not a SQLite database"))
        }
        // A rollback journal a writer left when it stopped mid-write: the
        // database is read again only once a writer has rolled it back.
        Some(ErrorCode::ReadOnly)
            if error
                .sqlite_error()
                .is_some_and(|failure| failure.extended_code == ffi::SQLITE_READONLY_ROLLBACK) =>
        {
            StorageError::InvalidArgument(format!(
                "{path} holds the journal of a write that never finished, which only a writer \
                 rolls back; let a command of the quantforge command line open it first, such as \
                 `quantforge --db {path} store status`"
            ))
        }
        Some(ErrorCode::ReadOnly | ErrorCode::PermissionDenied) => {
            StorageError::InvalidArgument(format!(
                "cannot read {path} read-only: a reader of a WAL database writes the -shm file \
                 beside it; make its directory writable, or copy the database, with its -wal \
                 file, somewhere writable"
            ))
        }
        _ => failed(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Candle, ExchangeId, Interval, MarketId, Symbol};
    use crate::ports::{CandleQuery, CandleStore, StorageErrorKind};
    use crate::storage::sqlite::SCHEMA_VERSION;
    use rust_decimal::Decimal;
    use std::str::FromStr;
    use std::time::Instant;

    /// What a test allows a loaded machine past a limit: a sleeper can wake
    /// late.
    const SCHEDULING: Duration = Duration::from_secs(1);

    fn market() -> MarketId {
        MarketId::new(
            ExchangeId::BinanceSpot,
            Symbol::new("BTCUSDT").expect("a symbol"),
            Interval::M1,
        )
    }

    fn candle(index: i64) -> Candle {
        let price = Decimal::from_str("81940.01000000").expect("a price");
        Candle {
            open_time_ms: index * 60_000,
            close_time_ms: index * 60_000 + 59_999,
            open: price,
            high: price,
            low: price,
            close: price,
            volume: Decimal::ONE,
            trades: None,
        }
    }

    /// A store as a command leaves it: its schema, and two candles.
    fn written(path: &Path) -> Vec<Candle> {
        let store = SqliteStore::new(path);
        CandleStore::init(&store).expect("a schema");
        let candles = vec![candle(0), candle(1)];
        store
            .upsert_candles(&market(), &candles)
            .expect("candles written");
        candles
    }

    fn all(store: &SqliteStore) -> Result<Vec<Candle>, StorageError> {
        store.load_candles(&market(), CandleQuery::default())
    }

    #[test]
    fn a_store_opened_to_read_reads_what_a_command_wrote() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("market.sqlite");
        let candles = written(&path);
        let reader = SqliteStore::read_only(&path, StorageLimits::default());
        assert_eq!(all(&reader).expect("candles"), candles);
        assert_eq!(
            reader.max_open_time_ms(&market()).expect("a time"),
            Some(60_000)
        );
        assert_eq!(
            reader.recorded_version().expect("a version"),
            SCHEMA_VERSION
        );
        // Bounded, it still reads and still cannot write.
        let bounded = reader.within(Duration::from_secs(2));
        assert_eq!(all(&bounded).expect("candles"), candles);
        assert!(bounded.upsert_candles(&market(), &[candle(2)]).is_err());
    }

    #[test]
    fn a_store_opened_to_read_refuses_a_write_and_changes_no_byte() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("market.sqlite");
        let candles = written(&path);
        let before = std::fs::read(&path).expect("the file");
        let reader = SqliteStore::read_only(&path, StorageLimits::default());
        let refused = reader
            .upsert_candles(&market(), &[candle(2)])
            .expect_err("a write through a reader");
        // SQLite's own refusal, and no category a caller retries on.
        assert_eq!(refused.kind(), StorageErrorKind::Other, "{refused:?}");
        assert_eq!(all(&reader).expect("candles"), candles);
        assert_eq!(std::fs::read(&path).expect("the file"), before);
    }

    #[test]
    fn a_store_opened_to_read_waits_for_a_lock_as_long_as_its_limits_say_and_no_longer() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("market.sqlite");
        let candles = written(&path);
        // A store in rollback mode, where a writer's exclusive lock keeps
        // every reader out until it lets go.
        let holder = Connection::open(&path).expect("a connection");
        let mode: String = holder
            .pragma_update_and_check(None, "journal_mode", "DELETE", |row| row.get(0))
            .expect("rollback mode");
        assert_eq!(mode, "delete");
        holder.execute_batch("BEGIN EXCLUSIVE").expect("the lock");
        // Shorter than the five seconds rusqlite gives every connection it
        // opens, so that the store's own limit is what ends the wait.
        let wait = Duration::from_millis(300);
        let reader = SqliteStore::read_only(
            &path,
            StorageLimits {
                wait_budget: wait,
                ..StorageLimits::default()
            },
        );
        let asked = Instant::now();
        let refused = all(&reader).expect_err("a lock held past the wait");
        let took = asked.elapsed();
        assert_eq!(refused.kind(), StorageErrorKind::Contention, "{refused:?}");
        // The wait gives up once less than its last pause is left.
        assert!(took >= wait * 9 / 10, "gave up after {took:?}");
        assert!(took < wait + SCHEDULING, "waited {took:?}");
        holder.execute_batch("COMMIT").expect("the lock let go");
        assert_eq!(all(&reader).expect("candles"), candles);
    }

    #[test]
    fn a_file_that_does_not_exist_is_refused_and_not_created() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nowhere.sqlite");
        let reader = SqliteStore::read_only(&path, StorageLimits::default());
        for refused in [
            all(&reader).expect_err("no file"),
            reader.recorded_version().expect_err("no file"),
        ] {
            let StorageError::InvalidArgument(message) = refused else {
                panic!("not a refusal: {refused:?}");
            };
            assert!(message.starts_with("cannot open "), "{message}");
        }
        assert!(!path.exists());
        assert_eq!(
            std::fs::read_dir(dir.path())
                .expect("the directory")
                .count(),
            0
        );
    }

    #[test]
    fn the_recorded_version_is_read_and_nothing_is_created_for_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        // A database of something else holds no store, and gains no table.
        let other = dir.path().join("other.sqlite");
        Connection::open(&other)
            .and_then(|connection| connection.execute_batch("CREATE TABLE notes (text TEXT)"))
            .expect("another database");
        let before = std::fs::read(&other).expect("the file");
        let reader = SqliteStore::read_only(&other, StorageLimits::default());
        assert_eq!(reader.recorded_version().expect("a version"), 0);
        assert_eq!(std::fs::read(&other).expect("the file"), before);
        // And a file that is no database is said to be none.
        let text = dir.path().join("notes.txt");
        std::fs::write(&text, "not a database, and longer than a header\n").expect("a file");
        let refused = SqliteStore::read_only(&text, StorageLimits::default())
            .recorded_version()
            .expect_err("no database");
        assert_eq!(
            refused.to_string(),
            format!(
                "invalid argument: {} is not a SQLite database",
                text.display()
            )
        );
    }
}
