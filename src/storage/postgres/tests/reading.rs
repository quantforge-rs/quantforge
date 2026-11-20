//! A store opened to read (`PostgresStore::connect_read_only`): it reads
//! what a store wrote, the server refuses every write through it, and it
//! reads the version a database records without creating a schema.

use super::database::{DATABASE, WEAK_DATABASE, database_url, unique};
use super::scratch::{Scratch, rows};
use super::*;
use crate::model::{Candle, ExchangeId, Interval, MarketId, Symbol};
use crate::ports::{CandleQuery, CandleStore, StoreControl};
use crate::storage::settings::Durability;
use crate::storage::sqlite::SCHEMA_VERSION;
use rust_decimal::Decimal;
use std::str::FromStr;
use tokio_postgres::error::SqlState;

fn market() -> MarketId {
    MarketId::new(
        ExchangeId::BybitSpot,
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

/// The SQLSTATE the server refused a statement with.
fn sqlstate(refused: &StorageError) -> Option<SqlState> {
    let StorageError::Other(source) = refused else {
        return None;
    };
    source
        .downcast_ref::<tokio_postgres::Error>()
        .and_then(|error| error.code().cloned())
}

#[test]
fn a_store_opened_to_read_reads_and_the_server_refuses_its_writes() {
    let Some(url) = database_url(DATABASE) else {
        return;
    };
    let scratch = Scratch::create(&url);
    let writer = scratch.store();
    writer.init().expect("a schema");
    let candles = vec![candle(0), candle(1)];
    writer
        .upsert_candles(&market(), &candles)
        .expect("candles written");

    let reader = scratch.reader();
    assert_eq!(
        reader.recorded_version().expect("a version"),
        SCHEMA_VERSION
    );
    let all = || reader.load_candles(&market(), CandleQuery::default());
    assert_eq!(all().expect("candles"), candles);
    assert_eq!(
        reader.max_open_time_ms(&market()).expect("a time"),
        Some(60_000)
    );
    assert_eq!(reader.market_coverage().expect("the markets").len(), 1);
    reader.store_identity().expect("the store's record");

    // A write is refused by the server, in a transaction it made read-only,
    // and again by a call with a budget, which resets the session's limits
    // and nothing else of it.
    for store in [reader.within(Duration::from_secs(5)), scratch.reader()] {
        let refused = store
            .upsert_candles(&market(), &[candle(2)])
            .expect_err("a write through a reader");
        assert_eq!(
            sqlstate(&refused),
            Some(SqlState::READ_ONLY_SQL_TRANSACTION),
            "{refused:?}"
        );
    }
    assert_eq!(all().expect("candles"), candles);
    assert_eq!(
        rows(&writer, "SELECT count(*) FROM candles"),
        [[Some("2".to_string())]]
    );
}

#[test]
fn a_store_opened_to_read_finds_no_schema_and_creates_none() {
    let Some(url) = database_url(DATABASE) else {
        return;
    };
    let scratch = Scratch::create(&url);
    let reader = scratch.reader();
    assert_eq!(reader.recorded_version().expect("a version"), 0);
    // Its own `init` would create the schema: the server refuses that too.
    let refused = reader.init().expect_err("a schema created by a reader");
    assert_eq!(
        sqlstate(&refused),
        Some(SqlState::READ_ONLY_SQL_TRANSACTION),
        "{refused:?}"
    );
    let tables = "SELECT count(*) FROM pg_tables WHERE schemaname = current_schema()";
    assert_eq!(rows(&scratch.store(), tables), [[Some("0".to_string())]]);
}

/// A server that cannot keep its commits is refused to a store that writes
/// and enforces them (`durability.rs`). A store opened to read commits
/// nothing, so the same settings open it there.
#[test]
fn a_store_opened_to_read_is_not_held_to_how_a_server_keeps_its_commits() {
    let Some(url) = database_url(WEAK_DATABASE) else {
        return;
    };
    // The default limits enforce the durability contract.
    let enforcing = settings(&url, &unique("weak_reader"));
    assert_eq!(enforcing.limits.durability, Durability::Enforced);
    let reader = PostgresStore::connect_read_only(&enforcing).expect("a reader");
    reader.ping().expect("an answer");
}
