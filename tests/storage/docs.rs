//! The storage documents held to the code and the fixtures:
//! `docs/storage-schema.md`'s schema version, its SQLite DDL as a fresh
//! store has it, its PostgreSQL DDL naming the same tables and columns, its
//! indexes differing only as the doc lists, and its ownership table
//! covering every table; and
//! `docs/storage-conformance.md`'s lists of every scenario and check; and
//! `docs/storage-migration.md`'s tables a transfer copies and keeps, which
//! cover the schema, its format, and its worked example. Each part is found
//! between `<!-- begin: name -->` and `<!-- end: name -->`.

use quantforge::model::{ColumnKind, KEPT_BY_EACH_STORE, TRANSFER_FORMAT, TRANSFER_TABLES};
use quantforge::storage::sqlite::SCHEMA_VERSION;
use quantforge::{CandleStore, SqliteStore};
use rusqlite::Connection;

use crate::conformance::{self, cases::fixture};

fn section(name: &str) -> Vec<String> {
    section_in("docs/storage-schema.md", name)
}

pub(crate) fn section_in(path: &str, name: &str) -> Vec<String> {
    let doc = std::fs::read_to_string(fixture(path)).expect("read the doc");
    let lines: Vec<&str> = doc.lines().collect();
    let marker = |kind: &str| {
        let marker = format!("<!-- {kind}: {name} -->");
        lines
            .iter()
            .position(|line| line.trim() == marker)
            .unwrap_or_else(|| panic!("{path} has no {marker}"))
    };
    let (begin, end) = (marker("begin"), marker("end"));
    lines[begin + 1..end]
        .iter()
        .map(|line| line.to_string())
        .collect()
}

/// One column as the DDL declares it.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Column {
    pub(crate) name: String,
    pub(crate) kind: String,
    pub(crate) not_null: bool,
    pub(crate) primary_key: bool,
    /// Numbered by the database, as PostgreSQL's identity columns are.
    pub(crate) identity: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Table {
    pub(crate) name: String,
    pub(crate) columns: Vec<Column>,
}

/// The tables and indexes of a fenced DDL block, in order.
pub(crate) fn parse(lines: &[String]) -> (Vec<Table>, Vec<String>) {
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
            identity: line.contains("GENERATED ALWAYS AS IDENTITY"),
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
                    identity: false,
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
    // The indexes differ only as the doc lists, each on one side alone.
    let differences = items(&section("postgres index differences"));
    let listed: Vec<&str> = differences.iter().map(|(name, _)| name.as_str()).collect();
    for name in &listed {
        let (on_sqlite, on_postgres) = (
            sqlite_indexes.iter().any(|index| index == name),
            postgres_indexes.iter().any(|index| index == name),
        );
        assert!(
            on_sqlite != on_postgres,
            "{name} is listed as a difference but is on both backends or neither"
        );
    }
    let shared = |indexes: &[String]| -> Vec<String> {
        indexes
            .iter()
            .filter(|index| !listed.contains(&index.as_str()))
            .cloned()
            .collect()
    };
    assert_eq!(
        shared(&postgres_indexes),
        shared(&sqlite_indexes),
        "an index on one backend only must be listed with its reason"
    );
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

/// The items of a list, `- `id`: statement`, each joined onto one line.
pub(crate) fn items(lines: &[String]) -> Vec<(String, String)> {
    let mut items: Vec<(String, String)> = Vec::new();
    for line in lines.iter().filter(|line| !line.trim().is_empty()) {
        if let Some(item) = line.strip_prefix("- `") {
            let (id, statement) = item
                .split_once("`: ")
                .unwrap_or_else(|| panic!("an item is `id`: statement: {line}"));
            items.push((id.to_string(), statement.to_string()));
        } else {
            let (_, statement) = items
                .last_mut()
                .unwrap_or_else(|| panic!("a line before the first item: {line}"));
            statement.push(' ');
            statement.push_str(line.trim());
        }
    }
    items
}

#[test]
fn the_conformance_doc_lists_every_scenario_and_check_with_its_statement() {
    let cases = conformance::cases::load();
    let pairs = |scenarios: &[conformance::Scenario], checks: &[(&str, &str)]| {
        scenarios
            .iter()
            .map(|scenario| (scenario.id.clone(), scenario.doc.clone()))
            .chain(
                checks
                    .iter()
                    .map(|(id, doc)| (id.to_string(), doc.to_string())),
            )
            .collect::<Vec<_>>()
    };
    let races = cases
        .races
        .iter()
        .map(|race| {
            let text = |key: &str| race[key].as_str().expect("text").to_string();
            (text("id"), text("doc"))
        })
        .collect();
    for (group, expected) in [
        (
            "candles",
            pairs(&cases.candles, conformance::candles::CHECKS),
        ),
        (
            "journal",
            pairs(&cases.journal, conformance::journal::CHECKS),
        ),
        (
            "segments",
            pairs(&cases.segments, conformance::segments::CHECKS),
        ),
        ("races", races),
        ("durability", pairs(&[], conformance::durability::CHECKS)),
        (
            "control",
            pairs(&cases.control, conformance::control::CHECKS),
        ),
    ] {
        for (id, _) in &expected {
            assert!(
                id.starts_with(&format!("{group}.")),
                "{id} is not in {group}"
            );
        }
        let listed = items(&section_in("docs/storage-conformance.md", group));
        assert_eq!(
            listed, expected,
            "the {group} list of docs/storage-conformance.md"
        );
    }
    // The rule the Rust generator follows names every setting a series has.
    for setting in ["count", "range", "skip", "spacing_ms", "close"] {
        assert!(cases.series_rule.contains(setting), "{setting}");
    }
}

/// The backticked words of each item of a list in
/// `docs/storage-migration.md`, with the item's text.
fn migration_list(name: &str) -> Vec<(Vec<String>, String)> {
    section_in("docs/storage-migration.md", name)
        .into_iter()
        .filter(|line| line.starts_with("- "))
        .map(|line| {
            let words = line
                .split('`')
                .skip(1)
                .step_by(2)
                .map(str::to_string)
                .collect();
            (words, line)
        })
        .collect()
}

#[test]
fn the_migration_doc_lists_every_transferred_table_in_copy_order_with_its_key() {
    let listed: Vec<(String, String, bool, bool)> = migration_list("transfer tables")
        .into_iter()
        .map(|(words, line)| {
            (
                words.first().cloned().unwrap_or_default(),
                words.last().cloned().unwrap_or_default(),
                line.contains("market by market"),
                line.contains("a number the store gave"),
            )
        })
        .collect();
    let code: Vec<(String, String, bool, bool)> = TRANSFER_TABLES
        .iter()
        .map(|table| {
            (
                table.name.to_string(),
                table.key.to_string(),
                table.by_market,
                table.numbered,
            )
        })
        .collect();
    assert_eq!(listed, code);
}

#[test]
fn every_table_of_the_schema_is_transferred_with_its_columns_or_kept_by_each_store() {
    let kept: Vec<String> = migration_list("kept by each store")
        .into_iter()
        .map(|(words, _)| words.concat())
        .collect();
    assert_eq!(kept, KEPT_BY_EACH_STORE);

    let (tables, _) = parse(&section("sqlite schema"));
    let kind = |declared: &str| match declared {
        "INTEGER" => ColumnKind::Integer,
        "TEXT" => ColumnKind::Text,
        "BLOB" => ColumnKind::Blob,
        other => panic!("no transfer kind for {other}"),
    };
    for table in &tables {
        match TRANSFER_TABLES.iter().find(|t| t.name == table.name) {
            Some(transferred) => {
                let doc: Vec<(&str, ColumnKind)> = table
                    .columns
                    .iter()
                    .map(|column| (column.name.as_str(), kind(&column.kind)))
                    .collect();
                let code: Vec<(&str, ColumnKind)> = transferred
                    .columns
                    .iter()
                    .map(|column| (column.name, column.kind))
                    .collect();
                assert_eq!(code, doc, "{}", table.name);
            }
            None => assert!(
                KEPT_BY_EACH_STORE.contains(&table.name.as_str()),
                "{} is neither transferred nor kept by each store",
                table.name
            ),
        }
    }
    let names: Vec<&str> = tables.iter().map(|table| table.name.as_str()).collect();
    for name in TRANSFER_TABLES
        .iter()
        .map(|table| table.name)
        .chain(KEPT_BY_EACH_STORE)
    {
        assert!(names.contains(&name), "{name} is not a table of the schema");
    }
}

#[test]
fn the_migration_doc_states_the_format_and_the_worked_example_the_oracle_wrote() {
    let format: Vec<String> = section_in("docs/storage-migration.md", "transfer format")
        .into_iter()
        .filter(|line| !line.trim().is_empty())
        .collect();
    assert_eq!(format, [TRANSFER_FORMAT.to_string()]);

    let example = crate::transfer::fixture::cases().example;
    let lines = section_in("docs/storage-migration.md", "transfer worked example");
    let fence = |from: usize| {
        lines[from..]
            .iter()
            .position(|line| line.starts_with("```"))
            .map(|index| index + from)
            .expect("a fence")
    };
    let (open, close) = (fence(0), fence(fence(0) + 1));
    let text: String = lines[open + 1..close]
        .iter()
        .map(|line| format!("{line}\n"))
        .collect();
    assert_eq!(text, example.canonical);
    let checksum: Vec<&String> = lines[close + 1..]
        .iter()
        .filter(|line| !line.trim().is_empty())
        .collect();
    assert_eq!(checksum, [&example.checksum]);
}
