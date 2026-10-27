//! `docs/storage-schema.md` held to the code: the schema version, the
//! SQLite DDL as a fresh store has it, the PostgreSQL DDL naming the same
//! tables and columns, and the ownership table covering every table. Each
//! part is found between `<!-- begin: name -->` and `<!-- end: name -->`.

use quantforge::storage::sqlite::SCHEMA_VERSION;
use quantforge::{CandleStore, SqliteStore};
use rusqlite::Connection;

use crate::cases::fixture;

fn section(name: &str) -> Vec<String> {
    let doc = std::fs::read_to_string(fixture("docs/storage-schema.md")).expect("read the doc");
    let lines: Vec<&str> = doc.lines().collect();
    let marker = |kind: &str| {
        let marker = format!("<!-- {kind}: {name} -->");
        lines
            .iter()
            .position(|line| line.trim() == marker)
            .unwrap_or_else(|| panic!("docs/storage-schema.md has no {marker}"))
    };
    let (begin, end) = (marker("begin"), marker("end"));
    lines[begin + 1..end]
        .iter()
        .map(|line| line.to_string())
        .collect()
}

/// One column as the DDL declares it.
#[derive(Debug, PartialEq, Eq)]
struct Column {
    name: String,
    kind: String,
    not_null: bool,
    primary_key: bool,
}

#[derive(Debug, PartialEq, Eq)]
struct Table {
    name: String,
    columns: Vec<Column>,
}

/// The tables and indexes of a fenced DDL block, in order.
fn parse(lines: &[String]) -> (Vec<Table>, Vec<String>) {
    let mut tables = Vec::new();
    let mut indexes = Vec::new();
    let mut current: Option<Table> = None;
    let mut table_keys: Vec<String> = Vec::new();
    for line in lines {
        let line = line.trim();
        if line.starts_with("```") || line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("CREATE TABLE ") {
            let name = rest.split_whitespace().next().expect("table name");
            current = Some(Table {
                name: name.to_string(),
                columns: Vec::new(),
            });
            table_keys.clear();
            continue;
        }
        if let Some(rest) = line.strip_prefix("CREATE INDEX ") {
            indexes.push(
                rest.split_whitespace()
                    .next()
                    .expect("index name")
                    .to_string(),
            );
            continue;
        }
        if line == ");" {
            let mut table = current.take().expect("a table to close");
            for column in &mut table.columns {
                if table_keys.contains(&column.name) {
                    column.primary_key = true;
                }
            }
            tables.push(table);
            continue;
        }
        let Some(table) = current.as_mut() else {
            continue;
        };
        let line = line.trim_end_matches(',');
        if let Some(keys) = line.strip_prefix("PRIMARY KEY (") {
            table_keys = keys
                .trim_end_matches(')')
                .split(',')
                .map(|key| key.trim().to_string())
                .collect();
            continue;
        }
        if line.starts_with("UNIQUE") || line.starts_with("CHECK") {
            continue;
        }
        let mut words = line.split_whitespace();
        let name = words.next().expect("column name").to_string();
        let kind = words.next().expect("column type").to_string();
        table.columns.push(Column {
            name,
            kind,
            not_null: line.contains("NOT NULL"),
            primary_key: line.contains("PRIMARY KEY"),
        });
    }
    (tables, indexes)
}

fn fresh_store() -> (tempfile::TempDir, SqliteStore) {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::new(tempdir.path().join("market.sqlite"));
    CandleStore::init(&store).expect("init");
    (tempdir, store)
}

#[test]
fn the_schema_version_in_the_doc_is_the_one_the_code_writes() {
    let lines: Vec<String> = section("schema version")
        .into_iter()
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
        .collect();
    assert_eq!(lines, [SCHEMA_VERSION.to_string()]);
}

#[test]
fn the_sqlite_ddl_in_the_doc_is_the_schema_a_fresh_store_has() {
    let (doc_tables, doc_indexes) = parse(&section("sqlite schema"));
    let (_tempdir, store) = fresh_store();
    let connection = Connection::open(store.path()).expect("open");
    let mut stmt = connection
        .prepare(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name != 'sqlite_sequence' ORDER BY name",
        )
        .expect("prepare");
    let mut actual_tables: Vec<String> = stmt
        .query_map([], |row| row.get(0))
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("rows");
    actual_tables.sort();
    let mut documented: Vec<String> = doc_tables.iter().map(|table| table.name.clone()).collect();
    documented.sort();
    assert_eq!(documented, actual_tables, "the tables");
    for table in &doc_tables {
        let mut stmt = connection
            .prepare(&format!(
                "SELECT name, type, \"notnull\", pk FROM pragma_table_info('{}')",
                table.name
            ))
            .expect("prepare");
        let actual: Vec<Column> = stmt
            .query_map([], |row| {
                Ok(Column {
                    name: row.get(0)?,
                    kind: row.get(1)?,
                    not_null: row.get::<_, i64>(2)? != 0,
                    primary_key: row.get::<_, i64>(3)? != 0,
                })
            })
            .expect("query")
            .collect::<Result<_, _>>()
            .expect("columns");
        assert_eq!(table.columns, actual, "the columns of {}", table.name);
    }
    let mut stmt = connection
        .prepare(
            "SELECT name FROM sqlite_master WHERE type = 'index' AND name NOT LIKE 'sqlite_autoindex%' ORDER BY name",
        )
        .expect("prepare");
    let actual_indexes: Vec<String> = stmt
        .query_map([], |row| row.get(0))
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("rows");
    let mut documented_indexes = doc_indexes;
    documented_indexes.sort();
    assert_eq!(documented_indexes, actual_indexes, "the indexes");
}

#[test]
fn the_postgres_ddl_names_the_same_tables_and_columns_in_the_same_order() {
    let (sqlite, sqlite_indexes) = parse(&section("sqlite schema"));
    let (postgres, postgres_indexes) = parse(&section("postgres schema"));
    let names = |tables: &[Table]| -> Vec<(String, Vec<String>)> {
        tables
            .iter()
            .map(|table| {
                (
                    table.name.clone(),
                    table
                        .columns
                        .iter()
                        .map(|column| column.name.clone())
                        .collect(),
                )
            })
            .collect()
    };
    assert_eq!(names(&postgres), names(&sqlite));
    assert_eq!(postgres_indexes, sqlite_indexes);
    let constraints = |tables: &[Table]| -> Vec<(String, String, bool, bool)> {
        tables
            .iter()
            .flat_map(|table| {
                table.columns.iter().map(move |column| {
                    (
                        table.name.clone(),
                        column.name.clone(),
                        column.not_null,
                        column.primary_key,
                    )
                })
            })
            .collect()
    };
    assert_eq!(
        constraints(&postgres),
        constraints(&sqlite),
        "NOT NULL and keys agree"
    );
    for table in &postgres {
        for column in &table.columns {
            assert!(
                !matches!(
                    column.kind.as_str(),
                    "NUMERIC" | "DECIMAL" | "REAL" | "DOUBLE"
                ),
                "{}.{} must stay exact text",
                table.name,
                column.name
            );
        }
    }
}

#[test]
fn every_table_has_an_owner() {
    let (tables, _) = parse(&section("sqlite schema"));
    let owners = section("table owners").join("\n");
    for table in tables {
        assert!(
            owners.contains(&format!("`{}`", table.name)),
            "{} has no owner in the table",
            table.name
        );
    }
}
