//! SQLite's failures as the categories every backend shares
//! (`docs/storage-durability.md`). SQLite has no connection to lose, so a
//! write's outcome is unknown only when the disk fails it at the commit.

use rusqlite::{ErrorCode, ffi};

use crate::StorageError;

/// A failed statement. Whatever the transaction had written is rolled back
/// as it drops, so nothing was changed.
pub(super) fn failed(error: rusqlite::Error) -> StorageError {
    let Some(failure) = error.sqlite_error() else {
        return StorageError::other(error);
    };
    match failure.code {
        ErrorCode::DatabaseBusy if failure.extended_code == ffi::SQLITE_BUSY_SNAPSHOT => {
            StorageError::Conflict(
                "another writer committed after this transaction's snapshot".to_string(),
            )
        }
        ErrorCode::DatabaseBusy
        | ErrorCode::DatabaseLocked
        | ErrorCode::FileLockingProtocolFailed => {
            StorageError::Contention("another writer held the database past the wait".to_string())
        }
        ErrorCode::CannotOpen
        | ErrorCode::PermissionDenied
        | ErrorCode::SystemIoFailure
        | ErrorCode::DiskFull
        | ErrorCode::OutOfMemory => StorageError::Unavailable(error.to_string()),
        _ => StorageError::other(error),
    }
}

/// A failed commit, or a failed write that is its own transaction. Disk
/// trouble there may leave the commit in the log, so its outcome is
/// unknown; anything else failed before it committed.
pub(super) fn commit_failed(error: rusqlite::Error) -> StorageError {
    match error.sqlite_error_code() {
        Some(ErrorCode::SystemIoFailure | ErrorCode::DiskFull) => {
            StorageError::OutcomeUnknown(error.to_string())
        }
        _ => failed(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StorageErrorKind;
    use rusqlite::{Connection, TransactionBehavior};
    use std::os::raw::c_int;

    fn failure(code: c_int) -> rusqlite::Error {
        rusqlite::Error::SqliteFailure(ffi::Error::new(code), None)
    }

    #[test]
    fn each_failure_of_a_statement_falls_in_one_category() {
        let cases = [
            (ffi::SQLITE_BUSY, StorageErrorKind::Contention),
            (ffi::SQLITE_BUSY_RECOVERY, StorageErrorKind::Contention),
            (ffi::SQLITE_BUSY_TIMEOUT, StorageErrorKind::Contention),
            (ffi::SQLITE_LOCKED, StorageErrorKind::Contention),
            (ffi::SQLITE_PROTOCOL, StorageErrorKind::Contention),
            (ffi::SQLITE_BUSY_SNAPSHOT, StorageErrorKind::Conflict),
            (ffi::SQLITE_CANTOPEN, StorageErrorKind::Unavailable),
            (ffi::SQLITE_PERM, StorageErrorKind::Unavailable),
            (ffi::SQLITE_IOERR, StorageErrorKind::Unavailable),
            (ffi::SQLITE_IOERR_FSYNC, StorageErrorKind::Unavailable),
            (ffi::SQLITE_FULL, StorageErrorKind::Unavailable),
            (ffi::SQLITE_NOMEM, StorageErrorKind::Unavailable),
            (ffi::SQLITE_CORRUPT, StorageErrorKind::Other),
            (ffi::SQLITE_NOTADB, StorageErrorKind::Other),
            (ffi::SQLITE_READONLY, StorageErrorKind::Other),
            (ffi::SQLITE_CONSTRAINT, StorageErrorKind::Other),
        ];
        for (code, kind) in cases {
            assert_eq!(failed(failure(code)).kind(), kind, "code {code}");
        }
        assert_eq!(
            failed(rusqlite::Error::QueryReturnedNoRows).kind(),
            StorageErrorKind::Other
        );
    }

    #[test]
    fn only_disk_trouble_leaves_a_commit_unknown() {
        for code in [ffi::SQLITE_IOERR, ffi::SQLITE_IOERR_FSYNC, ffi::SQLITE_FULL] {
            assert_eq!(
                commit_failed(failure(code)).kind(),
                StorageErrorKind::OutcomeUnknown,
                "code {code}"
            );
        }
        // A commit that never got the write lock wrote nothing.
        for (code, kind) in [
            (ffi::SQLITE_BUSY, StorageErrorKind::Contention),
            (ffi::SQLITE_BUSY_SNAPSHOT, StorageErrorKind::Conflict),
            (ffi::SQLITE_CONSTRAINT, StorageErrorKind::Other),
        ] {
            assert_eq!(commit_failed(failure(code)).kind(), kind, "code {code}");
        }
    }

    /// Two connections, each given no time to wait.
    fn two_connections(path: &std::path::Path) -> (Connection, Connection) {
        let open = || {
            let connection = Connection::open(path).expect("open");
            connection
                .busy_timeout(std::time::Duration::ZERO)
                .expect("no wait");
            connection
                .pragma_update(None, "journal_mode", "WAL")
                .expect("wal");
            connection
        };
        let first = open();
        first
            .execute_batch("CREATE TABLE t (value INTEGER); INSERT INTO t VALUES (1);")
            .expect("a table");
        (first, open())
    }

    #[test]
    fn a_held_writer_lock_on_a_real_file_is_contention() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let (mut holder, waiter) = two_connections(&tempdir.path().join("market.sqlite"));
        let _held = holder
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .expect("the write lock");
        let refused = waiter
            .execute("UPDATE t SET value = 2", [])
            .expect_err("the lock is held");
        assert_eq!(failed(refused).kind(), StorageErrorKind::Contention);
    }

    #[test]
    fn a_snapshot_overtaken_by_a_commit_on_a_real_file_is_a_conflict() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let (first, mut second) = two_connections(&tempdir.path().join("market.sqlite"));
        let reader = second.transaction().expect("a deferred transaction");
        let found: i64 = reader
            .query_row("SELECT value FROM t", [], |row| row.get(0))
            .expect("its snapshot");
        assert_eq!(found, 1);
        first
            .execute("UPDATE t SET value = 2", [])
            .expect("a commit after the snapshot");
        let refused = reader
            .execute("UPDATE t SET value = 3", [])
            .expect_err("its snapshot is old");
        assert_eq!(failed(refused).kind(), StorageErrorKind::Conflict);
    }
}
