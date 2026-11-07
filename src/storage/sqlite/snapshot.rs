//! A read-only snapshot of a SQLite store: the source of a transfer
//! (`docs/storage-migration.md`).
//!
//! The file is opened read-only, never as a store opens it, so nothing
//! migrates it, sets its journal mode or checkpoints it: the database file
//! keeps every byte it had. One connection holds a read transaction for the
//! snapshot's life, so every read sees the store as it was when it opened,
//! however long the copy takes, while a writer still running carries on
//! beside it in WAL mode. A second connection stays idle, and sees whether
//! anything committed since. SQLite may leave an empty `-wal` file and a
//! `-shm` file beside a WAL database a reader opened, as any reader does.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use rusqlite::types::{Value, ValueRef};
use rusqlite::{Connection, ErrorCode, OpenFlags, params_from_iter};

use super::errors::failed;
use super::wait::{self, Waits};
use super::{SCHEMA_VERSION, control, migrate};
use crate::model::{
    ColumnKind, StoreIdentity, TRANSFER_SCHEMA, TransferColumn, TransferMarket, TransferRow,
    TransferTable, TransferValue,
};
use crate::ports::{StorageError, TransferSnapshot, TransferSource};

/// How long opening waits for a writer's lock, which only a file that never
/// reached WAL makes a reader wait for.
const LOCK_WAIT: Duration = Duration::from_secs(5);

/// See the module docs.
pub struct SqliteSnapshot {
    path: PathBuf,
    /// In one read transaction from the moment the snapshot opened.
    pinned: Mutex<Connection>,
    /// Idle, so that it sees every commit.
    watcher: Mutex<Connection>,
    /// The watcher's `data_version` as the snapshot was taken.
    version: i64,
}

impl SqliteSnapshot {
    /// Opens the store at `path` read-only and takes the snapshot. A file
    /// that is no QuantForge store, or one at a schema other than the one
    /// this build copies, is refused, untouched.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, StorageError> {
        let path = path.into();
        let watcher = read_only(&path)?;
        let version = data_version(&watcher).map_err(|error| opening(&path, error))?;
        let pinned = read_only(&path)?;
        // The first read takes the snapshot the transaction keeps.
        pinned
            .execute_batch("BEGIN; SELECT count(*) FROM sqlite_master;")
            .map_err(|error| opening(&path, error))?;
        let found = migrate::current_version(&pinned)?;
        if found == 0 {
            return Err(StorageError::InvalidArgument(format!(
                "{} holds no QuantForge store",
                path.display()
            )));
        }
        if found > SCHEMA_VERSION {
            return Err(StorageError::SchemaUnsupported {
                found,
                supported: SCHEMA_VERSION,
            });
        }
        if found < TRANSFER_SCHEMA {
            return Err(StorageError::InvalidArgument(format!(
                "{path} is at schema {found}, and this build copies schema {TRANSFER_SCHEMA}; \
                 bring it forward first: any command of this build brings it forward in place, \
                 such as `quantforge --db {path} store status`, or do that to a copy of it and \
                 copy the copy",
                path = path.display()
            )));
        }
        Ok(Self {
            path,
            pinned: Mutex::new(pinned),
            watcher: Mutex::new(watcher),
            version,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn pinned(&self) -> MutexGuard<'_, Connection> {
        self.pinned.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl std::fmt::Debug for SqliteSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteSnapshot")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

/// A connection that cannot write: opened read-only and asked to stay so.
/// A file that does not exist is not created.
fn read_only(path: &Path) -> Result<Connection, StorageError> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| opening(path, error))?;
    wait::at_most(&connection, LOCK_WAIT, Waits::Apart).map_err(failed)?;
    connection
        .pragma_update(None, "query_only", "ON")
        .map_err(failed)?;
    Ok(connection)
}

fn data_version(connection: &Connection) -> Result<i64, rusqlite::Error> {
    connection.pragma_query_value(None, "data_version", |row| row.get(0))
}

/// Why a file could not be read as a store, with what to do about it.
fn opening(path: &Path, error: rusqlite::Error) -> StorageError {
    let path = path.display();
    match error.sqlite_error_code() {
        Some(ErrorCode::CannotOpen | ErrorCode::NotFound) => {
            StorageError::InvalidArgument(format!("cannot open {path}: {error}"))
        }
        Some(ErrorCode::NotADatabase) => {
            StorageError::InvalidArgument(format!("{path} is not a SQLite database"))
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

/// A value as a statement binds it.
fn bound(value: &TransferValue) -> Value {
    match value {
        TransferValue::Null => Value::Null,
        TransferValue::Integer(value) => Value::Integer(*value),
        TransferValue::Text(text) => Value::Text(text.clone()),
        TransferValue::Blob(bytes) => Value::Blob(bytes.clone()),
    }
}

/// A stored value as its column holds it, or the refusal of one no target
/// can hold as it is: another type than the column's, or text that is not
/// UTF-8 or holds a NUL byte, which PostgreSQL's text cannot.
fn value(
    table: &TransferTable,
    column: &TransferColumn,
    stored: ValueRef<'_>,
) -> Result<TransferValue, String> {
    let found = match (column.kind, stored) {
        (_, ValueRef::Null) => return Ok(TransferValue::Null),
        (ColumnKind::Integer, ValueRef::Integer(value)) => {
            return Ok(TransferValue::Integer(value));
        }
        (ColumnKind::Blob, ValueRef::Blob(bytes)) => {
            return Ok(TransferValue::Blob(bytes.to_vec()));
        }
        (ColumnKind::Text, ValueRef::Text(bytes)) => match std::str::from_utf8(bytes) {
            Ok(text) if !text.contains('\0') => return Ok(TransferValue::Text(text.to_string())),
            Ok(_) => "text with a NUL byte",
            Err(_) => "text that is not UTF-8",
        },
        (_, ValueRef::Integer(_)) => "an integer",
        (_, ValueRef::Real(_)) => "a real number",
        (_, ValueRef::Text(_)) => "text",
        (_, ValueRef::Blob(_)) => "a blob",
    };
    Err(format!(
        "{}.{} holds {found}, which a copy cannot keep as it is",
        table.name, column.name
    ))
}

/// A row as the table's columns hold it, or the refusal of a value that
/// names the row by its key.
fn row_values(table: &TransferTable, row: &rusqlite::Row<'_>) -> Result<TransferRow, StorageError> {
    let mut values = Vec::with_capacity(table.columns.len());
    for (index, column) in table.columns.iter().enumerate() {
        let stored = row.get_ref(index).map_err(failed)?;
        values.push(value(table, column, stored).map_err(|problem| {
            let key_index = table.key_index();
            let key = row
                .get_ref(key_index)
                .ok()
                .and_then(|stored| value(table, &table.columns[key_index], stored).ok())
                .map_or_else(|| "?".to_string(), |key| key.to_string());
            StorageError::InvalidArgument(format!("the row with {} {key}: {problem}", table.key))
        })?);
    }
    Ok(values)
}

impl TransferSource for SqliteSnapshot {
    fn transfer_identity(&self) -> Result<StoreIdentity, StorageError> {
        control::read_identity(&self.pinned())
    }

    fn markets(&self, table: &TransferTable) -> Result<Vec<TransferMarket>, StorageError> {
        // One indexed step per market, rather than reading every row.
        let first = format!(
            "SELECT exchange, symbol, interval FROM {} ORDER BY exchange, symbol, interval LIMIT 1",
            table.name
        );
        let next = format!(
            "SELECT exchange, symbol, interval FROM {} WHERE (exchange, symbol, interval) > \
             (?1, ?2, ?3) ORDER BY exchange, symbol, interval LIMIT 1",
            table.name
        );
        let connection = self.pinned();
        let mut markets: Vec<TransferMarket> = Vec::new();
        loop {
            let (sql, params) = match markets.last() {
                None => (&first, Vec::new()),
                Some(last) => (
                    &next,
                    vec![
                        Value::Text(last.exchange.clone()),
                        Value::Text(last.symbol.clone()),
                        Value::Text(last.interval.clone()),
                    ],
                ),
            };
            let mut statement = connection.prepare_cached(sql).map_err(failed)?;
            let mut rows = statement.query(params_from_iter(params)).map_err(failed)?;
            let Some(row) = rows.next().map_err(failed)? else {
                return Ok(markets);
            };
            let text = |index: usize| -> Result<String, StorageError> {
                match value(
                    table,
                    &table.columns[index],
                    row.get_ref(index).map_err(failed)?,
                ) {
                    Ok(TransferValue::Text(text)) => Ok(text),
                    Ok(other) => Err(StorageError::InvalidArgument(format!(
                        "{}.{} holds {}, where a market's text belongs",
                        table.name,
                        table.columns[index].name,
                        other.kind_name()
                    ))),
                    Err(problem) => Err(StorageError::InvalidArgument(problem)),
                }
            };
            markets.push(TransferMarket {
                exchange: text(0)?,
                symbol: text(1)?,
                interval: text(2)?,
            });
        }
    }

    fn read_rows(
        &self,
        table: &TransferTable,
        market: Option<&TransferMarket>,
        after: Option<&TransferValue>,
        limit: usize,
    ) -> Result<Vec<TransferRow>, StorageError> {
        let columns: Vec<&str> = table.columns.iter().map(|column| column.name).collect();
        let mut conditions = Vec::new();
        let mut params = Vec::new();
        if let Some(market) = market {
            for (column, text) in [
                ("exchange", &market.exchange),
                ("symbol", &market.symbol),
                ("interval", &market.interval),
            ] {
                params.push(Value::Text(text.clone()));
                conditions.push(format!("{column} = ?{}", params.len()));
            }
        }
        if let Some(after) = after {
            params.push(bound(after));
            conditions.push(format!("{} > ?{}", table.key, params.len()));
        }
        params.push(Value::Integer(i64::try_from(limit).unwrap_or(i64::MAX)));
        let filter = if conditions.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", conditions.join(" AND "))
        };
        let sql = format!(
            "SELECT {} FROM {}{filter} ORDER BY {} LIMIT ?{}",
            columns.join(", "),
            table.name,
            table.key,
            params.len()
        );
        let connection = self.pinned();
        let mut statement = connection.prepare_cached(&sql).map_err(failed)?;
        let mut rows = statement.query(params_from_iter(params)).map_err(failed)?;
        let mut read = Vec::new();
        while let Some(row) = rows.next().map_err(failed)? {
            read.push(row_values(table, row)?);
        }
        Ok(read)
    }
}

impl TransferSnapshot for SqliteSnapshot {
    fn moved(&self) -> Result<bool, StorageError> {
        let watcher = self.watcher.lock().unwrap_or_else(PoisonError::into_inner);
        Ok(data_version(&watcher).map_err(failed)? != self.version)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_transfer_copies_the_schema_this_build_writes() {
        // A schema step changes what a transfer copies: this fails until the
        // step says so in TRANSFER_TABLES and TRANSFER_SCHEMA.
        assert_eq!(TRANSFER_SCHEMA, SCHEMA_VERSION);
    }
}
