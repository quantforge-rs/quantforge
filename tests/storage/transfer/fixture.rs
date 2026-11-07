//! The transfer fixture, `tests/fixtures/storage_transfer_cases.json`, which
//! `spikes/storage-contract/transfer_cases.py` writes, never the Rust code
//! under test: a store's tables as rows, and what a verification reads from
//! them. The commands' tests take it through `#[path]`, so it uses nothing
//! but the crate's public API, rusqlite and the JSON.

use std::path::{Path, PathBuf};

use quantforge::model::TransferValue;
use quantforge::{CandleStore, Environment, SqliteStore, StoreControl, VerifyReport};
use serde_json::Value as Json;

/// A table of the fixture, its columns named as the DDL names them.
pub struct Table {
    pub name: String,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<TransferValue>>,
}

/// What a verification reads of a table, or of a market of it.
#[derive(Debug, PartialEq, Eq)]
pub struct Scope {
    pub table: String,
    /// `exchange symbol interval`, for a market table.
    pub market: Option<String>,
    pub rows: u64,
    pub first: Option<i64>,
    pub last: Option<i64>,
    pub checksum: String,
}

/// One row's canonical text, written out for the doc.
pub struct Example {
    pub table: String,
    pub rows: Vec<Vec<TransferValue>>,
    pub canonical: String,
    pub checksum: String,
}

pub struct Cases {
    pub environment: Environment,
    pub tables: Vec<Table>,
    pub scopes: Vec<Scope>,
    pub example: Example,
}

impl Cases {
    pub fn table(&self, name: &str) -> &Table {
        self.tables
            .iter()
            .find(|table| table.name == name)
            .unwrap_or_else(|| panic!("the fixture has no {name}"))
    }
}

pub fn path() -> PathBuf {
    ["tests", "fixtures", "storage_transfer_cases.json"]
        .iter()
        .fold(PathBuf::from(env!("CARGO_MANIFEST_DIR")), |path, part| {
            path.join(part)
        })
}

fn value(json: &Json) -> TransferValue {
    match json {
        Json::Null => TransferValue::Null,
        Json::Number(number) => TransferValue::Integer(number.as_i64().expect("an i64")),
        Json::String(text) => TransferValue::Text(text.clone()),
        Json::Object(object) => TransferValue::Blob(
            hex::decode(object["hex"].as_str().expect("hex")).expect("hex digits"),
        ),
        other => panic!("no such value in the fixture: {other}"),
    }
}

fn rows(json: &Json) -> Vec<Vec<TransferValue>> {
    json.as_array()
        .expect("rows")
        .iter()
        .map(|row| row.as_array().expect("a row").iter().map(value).collect())
        .collect()
}

pub fn cases() -> Cases {
    let text = std::fs::read_to_string(path()).expect("read the transfer fixture");
    let json: Json = serde_json::from_str(&text).expect("the fixture's JSON");
    assert_eq!(json["format"], 1, "the fixture's format");
    let environment = json["environment"].as_str().expect("an environment");
    let environment = Environment::ALL
        .into_iter()
        .find(|known| known.as_str() == environment)
        .expect("a known environment");
    let tables = json["tables"]
        .as_object()
        .expect("tables")
        .iter()
        .map(|(name, table)| Table {
            name: name.clone(),
            columns: table["columns"]
                .as_array()
                .expect("columns")
                .iter()
                .map(|column| column.as_str().expect("a column").to_string())
                .collect(),
            rows: rows(&table["rows"]),
        })
        .collect();
    let scopes = json["scopes"]
        .as_array()
        .expect("scopes")
        .iter()
        .map(|scope| Scope {
            table: scope["table"].as_str().expect("a table").to_string(),
            market: scope["market"].as_array().map(|parts| {
                parts
                    .iter()
                    .map(|part| part.as_str().expect("a market part"))
                    .collect::<Vec<_>>()
                    .join(" ")
            }),
            rows: scope["rows"].as_u64().expect("rows"),
            first: scope["first"].as_i64(),
            last: scope["last"].as_i64(),
            checksum: scope["checksum"].as_str().expect("a checksum").to_string(),
        })
        .collect();
    let example = &json["worked_example"];
    Cases {
        environment,
        tables,
        scopes,
        example: Example {
            table: example["table"].as_str().expect("a table").to_string(),
            rows: rows(&example["rows"]),
            canonical: example["canonical"].as_str().expect("text").to_string(),
            checksum: example["checksum"]
                .as_str()
                .expect("a checksum")
                .to_string(),
        },
    }
}

/// A SQLite store at `path` holding every row of the fixture, bound to its
/// environment, as a store this build created and wrote would.
pub fn seed(path: &Path) -> Cases {
    let cases = cases();
    let store = SqliteStore::new(path);
    CandleStore::init(&store).expect("create the source");
    store
        .bind_environment(cases.environment)
        .expect("bind the source");
    let connection = rusqlite::Connection::open(path).expect("open the source");
    for table in &cases.tables {
        let marks: Vec<String> = (1..=table.columns.len())
            .map(|index| format!("?{index}"))
            .collect();
        let sql = format!(
            "INSERT INTO {} ({}) VALUES ({})",
            table.name,
            table.columns.join(", "),
            marks.join(", ")
        );
        for row in &table.rows {
            let values: Vec<rusqlite::types::Value> = row
                .iter()
                .map(|value| match value {
                    TransferValue::Null => rusqlite::types::Value::Null,
                    TransferValue::Integer(value) => rusqlite::types::Value::Integer(*value),
                    TransferValue::Text(text) => rusqlite::types::Value::Text(text.clone()),
                    TransferValue::Blob(bytes) => rusqlite::types::Value::Blob(bytes.clone()),
                })
                .collect();
            connection
                .execute(&sql, rusqlite::params_from_iter(values))
                .expect("seed a row");
        }
    }
    cases
}

/// Which side of a report.
#[derive(Clone, Copy, Debug)]
pub enum Side {
    Source,
    Target,
}

/// A report's scopes as the fixture lists them.
pub fn scopes_of(report: &VerifyReport, side: Side) -> Vec<Scope> {
    report
        .scopes
        .iter()
        .map(|scope| {
            let digest = match side {
                Side::Source => &scope.source,
                Side::Target => &scope.target,
            };
            let integer = |value: &Option<TransferValue>| match value {
                Some(TransferValue::Integer(value)) => Some(*value),
                _ => None,
            };
            Scope {
                table: scope.table.to_string(),
                market: scope.market.as_ref().map(ToString::to_string),
                rows: digest.rows,
                first: integer(&digest.first),
                last: integer(&digest.last),
                checksum: digest.checksum.clone(),
            }
        })
        .collect()
}
