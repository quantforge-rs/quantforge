//! The SQLite store under the shared conformance suite, and the raw view
//! of its database the suite reads through `rusqlite`.

use quantforge::{CandleStore, SqliteStore};
use rusqlite::Connection;
use rusqlite::types::ValueRef;
use std::path::{Path, PathBuf};

use crate::conformance::{self, Backend, Fixture, Raw};

/// A `SqliteStore` on a file of its own in a temporary directory.
pub struct Sqlite;

impl Backend for Sqlite {
    type Store = SqliteStore;
    const NAME: &'static str = "sqlite";

    fn fresh() -> Fixture<SqliteStore> {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let path = tempdir.path().join("market.sqlite");
        let reopen = {
            let path = path.clone();
            // As a restarted process opens it: a new store, initialized.
            move || {
                let store = SqliteStore::new(&path);
                CandleStore::init(&store).expect("init");
                store
            }
        };
        Fixture {
            store: reopen(),
            raw: Box::new(SqliteRaw(path)),
            reopen: Box::new(reopen),
            guard: Box::new(tempdir),
        }
    }
}

/// The raw view of a SQLite database file.
pub struct SqliteRaw(pub PathBuf);

impl SqliteRaw {
    fn connection(&self) -> Connection {
        open(&self.0)
    }
}

fn open(path: &Path) -> Connection {
    Connection::open(path).expect("open the database")
}

impl Raw for SqliteRaw {
    fn select(&self, sql: &str) -> Vec<Vec<Option<String>>> {
        let connection = self.connection();
        let mut stmt = connection.prepare(sql).expect("prepare");
        let columns = stmt.column_count();
        let rows = stmt
            .query_map([], |row| {
                (0..columns)
                    .map(|index| {
                        Ok(match row.get_ref(index)? {
                            ValueRef::Null => None,
                            ValueRef::Integer(value) => Some(value.to_string()),
                            ValueRef::Text(text) => {
                                Some(String::from_utf8(text.to_vec()).expect("utf-8"))
                            }
                            other => panic!("select text, not {other:?}: {sql}"),
                        })
                    })
                    .collect::<Result<Vec<_>, rusqlite::Error>>()
            })
            .expect("query");
        rows.collect::<Result<_, _>>().expect("rows")
    }

    fn execute(&self, sql: &str) {
        self.connection().execute_batch(sql).expect("execute");
    }

    fn value_types(&self, table: &str, column: &str) -> Vec<String> {
        let sql = format!(
            "SELECT DISTINCT typeof({column}) FROM {table} WHERE {column} IS NOT NULL ORDER BY 1"
        );
        self.select(&sql)
            .into_iter()
            .map(|row| row.into_iter().next().flatten().expect("a type"))
            .collect()
    }
}

#[test]
fn candles() {
    conformance::candles::<Sqlite>();
}

#[test]
fn journal() {
    conformance::journal::<Sqlite>();
}

#[test]
fn control() {
    conformance::control::<Sqlite>();
}

#[test]
fn segments() {
    conformance::segments::<Sqlite>();
}
