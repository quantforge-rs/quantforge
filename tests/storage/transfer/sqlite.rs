//! The source of a transfer: a read-only snapshot of a SQLite store. What
//! it reads is what the independent oracle digested; the file keeps every
//! byte through it, WAL frames included; a writer committing beside it
//! changes nothing it reads, and is noticed; and a file it cannot copy as
//! it is, is refused before anything is read.

use std::path::{Path, PathBuf};

use quantforge::model::{TransferMarket, TransferRow, TransferTable, TransferValue};
use quantforge::storage::sqlite::SqliteSnapshot;
use quantforge::{
    Environment, StorageError, StoreIdentity, TransferConfig, TransferEngine, TransferRecord,
    TransferSnapshot, TransferSource, TransferTarget,
};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

use super::fixture::{self, Cases, Side, scopes_of};

fn seeded() -> (TempDir, PathBuf, Cases) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("source.sqlite");
    let cases = fixture::seed(&path);
    (dir, path, cases)
}

fn sha256(path: &Path) -> String {
    hex::encode(Sha256::digest(std::fs::read(path).expect("read the file")))
}

/// The snapshot as a target holding the finished copy of itself, so that a
/// verification reads the snapshot on both sides and writes nothing.
struct Itself<'a> {
    snapshot: &'a SqliteSnapshot,
    record: TransferRecord,
}

impl TransferSource for Itself<'_> {
    fn transfer_identity(&self) -> Result<StoreIdentity, StorageError> {
        self.snapshot.transfer_identity()
    }

    fn markets(&self, table: &TransferTable) -> Result<Vec<TransferMarket>, StorageError> {
        self.snapshot.markets(table)
    }

    fn read_rows(
        &self,
        table: &TransferTable,
        market: Option<&TransferMarket>,
        after: Option<&TransferValue>,
        limit: usize,
    ) -> Result<Vec<TransferRow>, StorageError> {
        self.snapshot.read_rows(table, market, after, limit)
    }
}

impl TransferTarget for Itself<'_> {
    fn transfer_record(&self) -> Result<Option<TransferRecord>, StorageError> {
        Ok(Some(self.record.clone()))
    }

    fn begin_transfer(
        &self,
        _: &TransferRecord,
        _: Option<Environment>,
    ) -> Result<(), StorageError> {
        Err(StorageError::InvalidArgument("read only".to_string()))
    }

    fn write_rows(&self, _: &TransferTable, _: &[TransferRow]) -> Result<u64, StorageError> {
        Err(StorageError::InvalidArgument("read only".to_string()))
    }

    fn finish_transfer(&self, _: &TransferRecord) -> Result<(), StorageError> {
        Err(StorageError::InvalidArgument("read only".to_string()))
    }
}

fn verify_itself(snapshot: &SqliteSnapshot, batch_rows: usize) -> quantforge::VerifyReport {
    let store_id = snapshot.transfer_identity().expect("identity").store_id;
    let itself = Itself {
        snapshot,
        record: TransferRecord::begin(store_id, 0).copied(1),
    };
    TransferEngine::new(
        snapshot,
        &itself,
        TransferConfig {
            batch_rows,
            assume_quiescent: false,
        },
    )
    .verify()
    .expect("verify")
}

fn refusal(path: &Path) -> String {
    match SqliteSnapshot::open(path) {
        Err(StorageError::InvalidArgument(reason)) => reason,
        Err(other) => panic!("refused as {other:?}"),
        Ok(_) => panic!("{} opened", path.display()),
    }
}

#[test]
fn a_snapshot_reads_every_row_of_the_fixture_as_the_independent_oracle_digests_it() {
    let (_dir, path, cases) = seeded();
    let snapshot = SqliteSnapshot::open(&path).expect("open");
    for batch_rows in [1, 2, 5000] {
        let report = verify_itself(&snapshot, batch_rows);
        assert!(report.is_ok(), "batch {batch_rows}");
        for side in [Side::Source, Side::Target] {
            assert_eq!(
                scopes_of(&report, side),
                cases.scopes,
                "batch {batch_rows}, {side:?}"
            );
        }
    }
    let identity = snapshot.transfer_identity().expect("identity");
    assert_eq!(identity.environment, Some(cases.environment));
}

#[test]
fn the_worked_example_is_the_canonical_text_of_its_row() {
    let cases = fixture::cases();
    let example = &cases.example;
    let table = quantforge::model::TRANSFER_TABLES
        .iter()
        .find(|table| table.name == example.table)
        .expect("a transferred table");
    let mut text = table.canonical_header();
    for row in &example.rows {
        quantforge::model::transfer::write_canonical_row(row, &mut text);
    }
    assert_eq!(
        String::from_utf8(text.clone()).expect("utf-8"),
        example.canonical
    );
    assert_eq!(
        format!("sha256:{}", hex::encode(Sha256::digest(&text))),
        example.checksum
    );
}

#[test]
fn every_byte_of_the_file_and_its_log_stays_through_a_snapshot() {
    let (_dir, path, _) = seeded();
    // A writer holding the file open, with a commit still in its log.
    let writer = rusqlite::Connection::open(&path).expect("writer");
    writer
        .execute_batch(
            "PRAGMA wal_autocheckpoint = 0; INSERT INTO candles VALUES ('binance_spot', \
             'WALUSDT', '1m', 0, 59999, '1', '1', '1', '1', '1', NULL);",
        )
        .expect("a commit in the log");
    let wal = PathBuf::from(format!("{}-wal", path.display()));
    let before = (sha256(&path), sha256(&wal));

    let snapshot = SqliteSnapshot::open(&path).expect("open");
    let markets = snapshot
        .markets(&quantforge::model::transfer::CANDLES)
        .expect("markets");
    assert!(markets.iter().any(|market| market.symbol == "WALUSDT"));
    assert!(verify_itself(&snapshot, 3).is_ok());
    drop(snapshot);

    assert_eq!((sha256(&path), sha256(&wal)), before);
}

#[test]
fn a_writer_committing_beside_a_snapshot_changes_nothing_it_reads_and_is_noticed() {
    let (_dir, path, cases) = seeded();
    let snapshot = SqliteSnapshot::open(&path).expect("open");
    assert!(!snapshot.moved().expect("moved"));
    let candles = &quantforge::model::transfer::CANDLES;
    let runs = &quantforge::model::transfer::BOT_RUNS;
    let read = |table| snapshot.read_rows(table, None, None, 100).expect("read");
    let markets_before = snapshot.markets(candles).expect("markets");
    let runs_before = read(runs);

    let writer = rusqlite::Connection::open(&path).expect("writer");
    writer
        .execute(
            "INSERT INTO candles VALUES ('binance_spot', 'NEWUSDT', '1m', 0, 59999, '1', '1', \
             '1', '1', '1', NULL)",
            [],
        )
        .expect("a commit");
    writer
        .execute(
            "UPDATE bot_runs SET status = 'running' WHERE run_id = 'run-a'",
            [],
        )
        .expect("another");

    assert_eq!(snapshot.markets(candles).expect("markets"), markets_before);
    assert_eq!(read(runs), runs_before);
    assert_eq!(runs_before.len(), cases.table("bot_runs").rows.len());
    assert!(snapshot.moved().expect("moved"));
    // What was read is no longer all of the store, so it is not verified.
    let store_id = snapshot.transfer_identity().expect("identity").store_id;
    let itself = Itself {
        snapshot: &snapshot,
        record: TransferRecord::begin(store_id, 0).copied(1),
    };
    let refused = TransferEngine::new(&snapshot, &itself, TransferConfig::default()).verify();
    assert!(
        matches!(&refused, Err(quantforge::EngineError::TransferRefused(reason))
            if reason.contains("changed while it was verified")),
        "{refused:?}"
    );
}

#[test]
fn a_store_at_an_earlier_schema_is_refused_and_left_as_it_was() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("old.sqlite");
    let sql = std::fs::read_to_string(super::super::conformance::cases::fixture(
        "tests/fixtures/sqlite/schema-4.sql",
    ))
    .expect("schema 4");
    rusqlite::Connection::open(&path)
        .expect("create")
        .execute_batch(&sql)
        .expect("load schema 4");
    let before = sha256(&path);

    let reason = refusal(&path);
    assert!(
        reason.contains("is at schema 4, and this build copies schema 5"),
        "{reason}"
    );
    assert!(reason.contains("store status"), "{reason}");
    assert_eq!(sha256(&path), before);
    let version: String = rusqlite::Connection::open(&path)
        .expect("open")
        .query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |row| row.get(0),
        )
        .expect("the version");
    assert_eq!(version, "4");
}

#[test]
fn a_store_from_a_newer_build_is_refused() {
    let (_dir, path, _) = seeded();
    rusqlite::Connection::open(&path)
        .expect("open")
        .execute(
            "UPDATE meta SET value = '6' WHERE key = 'schema_version'",
            [],
        )
        .expect("a newer version");
    assert!(matches!(
        SqliteSnapshot::open(&path),
        Err(StorageError::SchemaUnsupported {
            found: 6,
            supported: 5
        })
    ));
}

#[test]
fn anything_but_a_store_is_refused_as_what_it_is() {
    let dir = tempfile::tempdir().expect("tempdir");
    let missing = dir.path().join("missing.sqlite");
    assert!(
        refusal(&missing).starts_with("cannot open"),
        "{}",
        refusal(&missing)
    );
    assert!(!missing.exists(), "a missing source was created");

    let text = dir.path().join("notes.txt");
    std::fs::write(
        &text,
        "not a database, though long enough to look like one".repeat(200),
    )
    .expect("write");
    assert!(refusal(&text).ends_with("is not a SQLite database"));

    let other = dir.path().join("other.sqlite");
    rusqlite::Connection::open(&other)
        .expect("create")
        .execute_batch("CREATE TABLE notes (body TEXT)")
        .expect("a table");
    assert!(refusal(&other).ends_with("holds no QuantForge store"));
}

#[test]
fn a_value_no_target_can_hold_is_refused_naming_its_row() {
    let cases = [
        ("X'00'", "candles.open holds a blob"),
        (
            "CAST(X'610062' AS TEXT)",
            "candles.open holds text with a NUL byte",
        ),
        (
            "CAST(X'FF' AS TEXT)",
            "candles.open holds text that is not UTF-8",
        ),
        ("1.5", "candles.open holds text"),
    ];
    for (open, refused) in cases {
        let (_dir, path, _) = seeded();
        rusqlite::Connection::open(&path)
            .expect("open")
            .execute(
                &format!(
                    "INSERT INTO candles VALUES ('binance_spot', 'ODDUSDT', '1m', 60000, \
                     119999, {open}, '1', '1', '1', '1', NULL)"
                ),
                [],
            )
            .expect("an odd value");
        let snapshot = SqliteSnapshot::open(&path).expect("open");
        let odd = TransferMarket {
            exchange: "binance_spot".to_string(),
            symbol: "ODDUSDT".to_string(),
            interval: "1m".to_string(),
        };
        let read = snapshot.read_rows(&quantforge::model::transfer::CANDLES, Some(&odd), None, 10);
        if open == "1.5" {
            // SQLite stores a real given to a TEXT column as its text.
            let rows = read.expect("read");
            assert_eq!(rows[0][5], TransferValue::Text("1.5".to_string()));
            continue;
        }
        match read {
            Err(StorageError::InvalidArgument(reason)) => {
                assert!(reason.contains(refused), "{reason}");
                assert!(
                    reason.starts_with("the row with open_time_ms 60000: "),
                    "{reason}"
                );
            }
            other => panic!("{open}: {other:?}"),
        }
    }
}

#[cfg(unix)]
#[test]
fn a_database_in_a_directory_no_reader_can_write_to_is_refused_with_its_remedy() {
    use std::os::unix::fs::PermissionsExt;
    let (dir, path, _) = seeded();
    let mode = |mode| {
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(mode)).expect("chmod")
    };
    mode(0o555);
    // A user who writes anyway, such as root, cannot see the refusal.
    let writable = std::fs::write(dir.path().join("probe"), b"").is_ok();
    let opened = SqliteSnapshot::open(&path);
    mode(0o755);
    if writable {
        eprintln!("skipping: this user writes to read-only directories");
        return;
    }
    match opened {
        Err(StorageError::InvalidArgument(reason)) => {
            assert!(reason.contains("make its directory writable"), "{reason}")
        }
        other => panic!("{other:?}"),
    }
}
