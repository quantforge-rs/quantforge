//! The storage contract on the SQLite store.

use quantforge::{CandleStore, MarketId, SqliteStore};
use rusqlite::{Connection, params};
use tempfile::TempDir;

use crate::contract;

fn store() -> (TempDir, SqliteStore) {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::new(tempdir.path().join("market.sqlite"));
    CandleStore::init(&store).expect("init");
    (tempdir, store)
}

/// The text of `column` for every candle of `market`, in open-time order.
fn raw_texts(store: &SqliteStore) -> impl Fn(&MarketId, &str) -> Vec<String> + '_ {
    move |market, column| {
        let connection = Connection::open(store.path()).expect("open");
        let mut stmt = connection
            .prepare(&format!(
                "SELECT {column} FROM candles WHERE exchange = ?1 AND symbol = ?2 AND interval = ?3 \
                 ORDER BY open_time_ms"
            ))
            .expect("prepare");
        let rows = stmt
            .query_map(
                params![
                    market.exchange.as_str(),
                    market.symbol.as_str(),
                    market.interval.as_str()
                ],
                |row| row.get::<_, String>(0),
            )
            .expect("query");
        rows.collect::<Result<_, _>>().expect("rows")
    }
}

#[test]
fn product_lines_are_stored_as_the_cases_say() {
    let (_tempdir, store) = store();
    contract::product_lines_are_stored_as_the_cases_say(&store, &raw_texts(&store));
}

#[test]
fn decimal_texts_keep_their_scale() {
    let (_tempdir, store) = store();
    contract::decimal_texts_keep_their_scale(&store, &raw_texts(&store));
}

#[test]
fn venue_ids_are_text() {
    let (_tempdir, store) = store();
    let raw_event_ids = || {
        let connection = Connection::open(store.path()).expect("open");
        let mut stmt = connection
            .prepare("SELECT order_id, typeof(order_id) FROM order_events ORDER BY seq")
            .expect("prepare");
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .expect("query");
        rows.collect::<Result<Vec<_>, _>>().expect("rows")
    };
    contract::venue_ids_are_text(&store, &raw_event_ids);
}

#[test]
fn environments_and_accounts_are_isolated() {
    let (_tempdir, store) = store();
    contract::environments_and_accounts_are_isolated(&store);
}
