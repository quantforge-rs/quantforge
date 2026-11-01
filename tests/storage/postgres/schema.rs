//! The schema `PostgresStore::init` creates, each test on a database of its
//! own: the catalog the doc's DDL describes, a repeat that changes nothing,
//! stores starting together, the refusals, exact round trips, identities
//! that never collide, and each step of publishing a segment. Without
//! `QUANTFORGE_TEST_POSTGRES_URL` every test skips.

use quantforge::storage::postgres::PostgresStore;
use quantforge::storage::sqlite::SCHEMA_VERSION;
use quantforge::{Category, ExchangeId, KeyFingerprint, StorageError, Venue};
use rust_decimal::Decimal;
use std::sync::Barrier;
use tokio_postgres::error::SqlState;
use tokio_postgres::types::Type;

use super::{IsolatedDatabase, Postgres, VARIABLE, block_on, client, query, server_url, settings};
use crate::conformance::cases::identity_cases;
use crate::conformance::{Backend, Fixture};
use crate::docs::{parse, section_in};

/// A database of its own whose store has run `init`, or `None` to skip.
fn fresh() -> Option<Fixture<PostgresStore>> {
    if server_url().is_none() {
        eprintln!("skipping: {VARIABLE} is unset");
        return None;
    }
    Some(Postgres::fresh())
}

/// The server, for a test that opens its database itself, or `None` to
/// skip.
fn server() -> Option<String> {
    let server = server_url();
    if server.is_none() {
        eprintln!("skipping: {VARIABLE} is unset");
    }
    server
}

fn url(fixture: &Fixture<PostgresStore>) -> String {
    fixture
        .guard
        .downcast_ref::<IsolatedDatabase>()
        .expect("an isolated database")
        .url
        .clone()
}

/// The first value of every row of `sql`, NULL as `NULL`.
fn column(url: &str, sql: &str) -> Vec<String> {
    query(url, sql)
        .into_iter()
        .map(|row| {
            row.into_iter()
                .next()
                .flatten()
                .unwrap_or_else(|| "NULL".to_string())
        })
        .collect()
}

fn tables(url: &str) -> Vec<String> {
    column(
        url,
        "SELECT CAST(tablename AS TEXT) FROM pg_tables WHERE schemaname = current_schema() \
         ORDER BY 1",
    )
}

fn count(url: &str, table: &str) -> String {
    column(url, &format!("SELECT CAST(COUNT(*) AS TEXT) FROM {table}")).concat()
}

/// Runs `sql` on its own connection and returns the server's refusal.
fn refusal(url: &str, sql: &str) -> tokio_postgres::Error {
    block_on(async { client(url).await.batch_execute(sql).await })
        .expect_err("the database refuses it")
}

#[test]
fn a_fresh_database_has_exactly_the_documented_schema() {
    let Some(fixture) = fresh() else {
        return;
    };
    let url = url(&fixture);
    let doc = section_in("docs/storage-schema.md", "postgres schema");
    let (documented, documented_indexes) = parse(&doc);
    let mut names: Vec<String> = documented.iter().map(|table| table.name.clone()).collect();
    names.sort();
    assert_eq!(tables(&url), names, "the tables");

    for table in &documented {
        let found = query(
            &url,
            &format!(
                "SELECT CAST(c.column_name AS TEXT), CAST(c.data_type AS TEXT), \
                        CAST(c.is_nullable AS TEXT), CAST(c.is_identity AS TEXT), \
                        CAST(EXISTS (SELECT 1 FROM pg_index i JOIN pg_attribute a \
                                     ON a.attrelid = i.indrelid AND a.attnum = ANY(i.indkey) \
                                     WHERE i.indrelid = CAST('{0}' AS regclass) AND i.indisprimary \
                                       AND a.attname = c.column_name) AS TEXT) \
                 FROM information_schema.columns c \
                 WHERE c.table_schema = current_schema() AND c.table_name = '{0}' \
                 ORDER BY c.ordinal_position",
                table.name
            ),
        );
        let expected: Vec<Vec<Option<String>>> = table
            .columns
            .iter()
            .map(|column| {
                let yes_no = |yes: bool| Some(if yes { "YES" } else { "NO" }.to_string());
                vec![
                    Some(column.name.clone()),
                    Some(column.kind.to_lowercase()),
                    yes_no(!(column.not_null || column.primary_key)),
                    yes_no(column.identity),
                    Some(column.primary_key.to_string()),
                ]
            })
            .collect();
        assert_eq!(found, expected, "the columns of {}", table.name);
    }

    let mut indexes = documented_indexes.clone();
    indexes.sort();
    assert_eq!(
        column(
            &url,
            "SELECT CAST(indexname AS TEXT) FROM pg_indexes WHERE schemaname = current_schema() \
             AND indexname NOT IN (SELECT conname FROM pg_constraint WHERE contype IN ('p', 'u')) \
             ORDER BY 1"
        ),
        indexes,
        "the indexes"
    );
    let constraints = |kind: char| {
        column(
            &url,
            &format!(
                "SELECT CAST(COUNT(*) AS TEXT) FROM pg_constraint \
                 WHERE connamespace = CAST(current_schema() AS regnamespace) AND contype = '{kind}'"
            ),
        )
        .concat()
    };
    let lines = |prefix: &str| {
        doc.iter()
            .filter(|line| line.trim().starts_with(prefix) || line.contains(&format!(" {prefix}")))
            .count()
            .to_string()
    };
    assert_eq!(
        constraints('u'),
        lines("UNIQUE ("),
        "the unique constraints"
    );
    assert_eq!(constraints('c'), lines("CHECK ("), "the checks");

    // The version, the one step that wrote it, and the control record.
    assert_eq!(
        query(&url, "SELECT key, value FROM meta"),
        [[
            Some("schema_version".to_string()),
            Some(SCHEMA_VERSION.to_string())
        ]]
    );
    assert_eq!(
        query(
            &url,
            "SELECT CAST(version AS TEXT), CAST(from_version AS TEXT), quantforge_version \
             FROM schema_migrations"
        ),
        [[
            Some(SCHEMA_VERSION.to_string()),
            Some("0".to_string()),
            Some(env!("CARGO_PKG_VERSION").to_string())
        ]]
    );
    let control = query(
        &url,
        "SELECT CAST(id AS TEXT), store_id, environment, activation, \
                CAST(activation_epoch AS TEXT) FROM store_control",
    );
    assert_eq!(control.len(), 1, "{control:?}");
    let store_id = control[0][1].clone().expect("a store ID");
    assert!(
        store_id.len() == 32 && store_id.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')),
        "{store_id}"
    );
    assert_eq!(
        control[0],
        [
            Some("1".to_string()),
            Some(store_id),
            None,
            Some("inactive".to_string()),
            Some("0".to_string())
        ]
    );
}

#[test]
fn a_second_init_changes_nothing() {
    let Some(fixture) = fresh() else {
        return;
    };
    let url = url(&fixture);
    // `xmin` changes whenever a row is rewritten, even to the same values.
    let state = || {
        [
            "SELECT CAST(xmin AS TEXT), key, value FROM meta",
            "SELECT CAST(xmin AS TEXT), store_id, CAST(changed_at_ms AS TEXT) FROM store_control",
            "SELECT CAST(xmin AS TEXT), CAST(version AS TEXT), CAST(applied_at_ms AS TEXT) \
             FROM schema_migrations",
        ]
        .map(|sql| query(&url, sql))
    };
    let before = state();
    fixture.store.init().expect("a second init");
    // As a restarted process would: connect, then init.
    drop((fixture.reopen)());
    assert_eq!(state(), before);
}

#[test]
fn stores_starting_together_create_the_schema_once() {
    let Some(server) = server() else {
        return;
    };
    let database = IsolatedDatabase::create(&server);
    // Connected before the barrier, so they reach the chain together.
    let stores: Vec<PostgresStore> = (0..8)
        .map(|_| PostgresStore::connect(&settings(&database.url)).expect("a store"))
        .collect();
    let barrier = Barrier::new(stores.len());
    let outcomes: Vec<Result<(), StorageError>> = std::thread::scope(|scope| {
        let started: Vec<_> = stores
            .iter()
            .map(|store| {
                let barrier = &barrier;
                scope.spawn(move || {
                    barrier.wait();
                    store.init()
                })
            })
            .collect();
        started
            .into_iter()
            .map(|thread| thread.join().expect("a store's thread"))
            .collect()
    });
    for outcome in &outcomes {
        assert!(outcome.is_ok(), "{outcome:?}");
    }
    assert_eq!(count(&database.url, "schema_migrations"), "1");
    assert_eq!(count(&database.url, "store_control"), "1");
}

#[test]
fn a_schema_from_a_newer_build_is_refused_and_left_as_it_was() {
    let Some(fixture) = fresh() else {
        return;
    };
    let url = url(&fixture);
    let newer = SCHEMA_VERSION + 1;
    query(
        &url,
        &format!("UPDATE meta SET value = '{newer}' WHERE key = 'schema_version'"),
    );
    let refused = fixture.store.init().expect_err("a newer schema");
    assert!(
        matches!(
            refused,
            StorageError::SchemaUnsupported { found, supported }
                if found == newer && supported == SCHEMA_VERSION
        ),
        "{refused:?}"
    );
    assert!(
        refused.to_string().contains(&format!(
            "schema version {newer} is newer than this build supports ({SCHEMA_VERSION})"
        )),
        "{refused}"
    );
    assert_eq!(
        column(&url, "SELECT value FROM meta WHERE key = 'schema_version'"),
        [newer.to_string()]
    );
    assert_eq!(count(&url, "schema_migrations"), "1");
}

#[test]
fn tables_of_ours_without_a_recorded_version_are_refused_and_left_as_they_were() {
    let Some(server) = server() else {
        return;
    };
    let database = IsolatedDatabase::create(&server);
    let url = database.url.clone();
    query(&url, "CREATE TABLE candles (note TEXT)");
    let store = PostgresStore::connect(&settings(&url)).expect("a store");
    let refused = store.init().expect_err("someone else's candles");
    assert!(
        matches!(&refused, StorageError::InvalidArgument(message) if message
            == "schema public already holds candles but records no QuantForge schema version; \
                give QuantForge a database of its own"),
        "{refused:?}"
    );
    assert_eq!(tables(&url), ["candles"]);

    // A meta table that records no version is never taken for a new store.
    query(
        &url,
        "DROP TABLE candles; CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
    );
    let refused = store.init().expect_err("a meta without a version");
    assert!(
        matches!(&refused, StorageError::InvalidArgument(message) if message
            == "schema public has a meta table but records no QuantForge schema version; \
                give QuantForge a database of its own"),
        "{refused:?}"
    );
    assert_eq!(tables(&url), ["meta"]);

    // A table no build of ours names is no obstacle.
    query(&url, "DROP TABLE meta; CREATE TABLE probe (note TEXT)");
    store.init().expect("the schema beside it");
    assert!(tables(&url).contains(&"probe".to_string()));
    assert_eq!(count(&url, "schema_migrations"), "1");
}

#[test]
fn decimal_text_integers_and_bytes_come_back_exactly_as_written() {
    let Some(fixture) = fresh() else {
        return;
    };
    let url = url(&fixture);
    let mut texts = identity_cases().decimals;
    texts.extend(
        [
            "0.00000000",
            "-0.5",
            "100.10",
            "79228162514264337593543950335",
            "-79228162514264337593543950335",
            "0.0000000000000000000000000001",
        ]
        .map(String::from),
    );
    // Beyond what a `Decimal` holds: stored as written, refused by an exact
    // read, never rounded.
    let inexact = [
        "1.00000000000000000000000000001",
        "79228162514264337593543950336",
    ];
    let all: Vec<&str> = texts
        .iter()
        .map(String::as_str)
        .chain(inexact.iter().copied())
        .collect();
    block_on(async {
        let client = client(&url).await;
        for (index, text) in all.iter().enumerate() {
            let open = i64::try_from(index).expect("an index") * 60_000;
            client
                .execute_typed(
                    "INSERT INTO candles VALUES \
                     ('binance_spot', 'BTCUSDT', '1m', $1, $2, $3, $3, $3, $3, $3, NULL)",
                    &[
                        (&open, Type::INT8),
                        (&(open + 59_999), Type::INT8),
                        (text, Type::TEXT),
                    ],
                )
                .await
                .expect("a candle");
            client
                .execute_typed(
                    "INSERT INTO closed_trades(run_id, symbol, entry_time_ms, exit_time_ms, \
                     entry_price, exit_price, qty, gross_quote_pnl) \
                     VALUES ('run', 'BTCUSDT', $1, $1, $2, $2, $2, $2)",
                    &[(&open, Type::INT8), (text, Type::TEXT)],
                )
                .await
                .expect("a closed trade");
        }
        let candles = client
            .query_typed(
                "SELECT open, high, low, close, volume FROM candles ORDER BY open_time_ms",
                &[],
            )
            .await
            .expect("the candles");
        let trades = client
            .query_typed(
                "SELECT entry_price, exit_price, qty, gross_quote_pnl FROM closed_trades \
                 ORDER BY seq",
                &[],
            )
            .await
            .expect("the closed trades");
        assert_eq!((candles.len(), trades.len()), (all.len(), all.len()));
        for ((text, candle), trade) in all.iter().zip(&candles).zip(&trades) {
            for index in 0..5 {
                assert_eq!(candle.try_get::<_, &str>(index).expect("text"), *text);
            }
            for index in 0..4 {
                assert_eq!(trade.try_get::<_, &str>(index).expect("text"), *text);
            }
        }
        // What a store writes is `to_string`, which an exact read takes back
        // with its scale; what it cannot hold fails, never rounded.
        for text in &texts {
            let exact = Decimal::from_str_exact(text).expect("an exact decimal");
            assert_eq!(&exact.to_string(), text);
        }
        for text in inexact {
            assert!(Decimal::from_str_exact(text).is_err(), "{text}");
        }

        // `BIGINT` keeps the extremes, and a trade count beyond `i64::MAX`
        // round-trips through the signed column as SQLite stores it.
        let counts = [
            (i64::MIN, Some(u64::MAX)),
            (-1, Some(u64::MAX / 2 + 1)),
            (0, Some(0)),
            (1, None),
            (i64::MAX, Some(i64::MAX as u64)),
        ];
        for (open, trades) in counts {
            let stored = trades.map(|value| value as i64);
            client
                .execute_typed(
                    "INSERT INTO candles VALUES \
                     ('bybit_linear', 'BTCUSDT', '1m', $1, $1, '1', '1', '1', '1', '1', $2)",
                    &[(&open, Type::INT8), (&stored, Type::INT8)],
                )
                .await
                .expect("a candle");
        }
        let read: Vec<(i64, Option<u64>)> = client
            .query_typed(
                "SELECT open_time_ms, trades FROM candles WHERE exchange = 'bybit_linear' \
                 ORDER BY open_time_ms",
                &[],
            )
            .await
            .expect("the candles")
            .iter()
            .map(|row| {
                let open: i64 = row.try_get(0).expect("an open time");
                let trades: Option<i64> = row.try_get(1).expect("a trade count");
                (open, trades.map(|value| value as u64))
            })
            .collect();
        assert_eq!(read, counts);

        // `BYTEA` keeps every byte value.
        let bytes: Vec<u8> = (0..=255).collect();
        client
            .execute_typed(
                "INSERT INTO segments(hash, format, payload_len, data) VALUES ($1, 1, 256, $2)",
                &[(&"sha256:00", Type::TEXT), (&bytes, Type::BYTEA)],
            )
            .await
            .expect("a segment");
        let stored: Vec<u8> = client
            .query_typed_one("SELECT data FROM segments", &[])
            .await
            .expect("the segment")
            .try_get(0)
            .expect("bytes");
        assert_eq!(stored, bytes);
    });
    for (table, column, kind) in [
        ("candles", "close", "text"),
        ("candles", "open_time_ms", "bigint"),
        ("candles", "trades", "bigint"),
        ("closed_trades", "qty", "text"),
        ("segments", "data", "bytea"),
    ] {
        assert_eq!(
            fixture.raw.value_types(table, column),
            [kind],
            "{table}.{column}"
        );
    }
}

#[test]
fn the_same_symbol_and_open_time_on_every_product_line_never_collide() {
    let Some(fixture) = fresh() else {
        return;
    };
    let url = url(&fixture);
    let lines: Vec<&str> = Venue::ALL
        .into_iter()
        .flat_map(|venue| {
            Category::ALL
                .into_iter()
                .map(move |category| ExchangeId::from_parts(venue, category).as_str())
        })
        .collect();
    assert_eq!(lines.len(), 6, "Spot, linear and inverse on both venues");
    let candle = |exchange: &str, interval: &str| {
        format!(
            "INSERT INTO candles VALUES ('{exchange}', 'BTCUSDT', '{interval}', \
             1792454400000, 1792454459999, '1', '1', '1', '1', '1', 1)"
        )
    };
    let entry = |exchange: &str| {
        format!(
            "INSERT INTO segment_index VALUES ('{exchange}', 'BTCUSDT', '1m', \
             1792454400000, 1792454400000, 1792454400000, 1, 'sha256:{}')",
            "0".repeat(64)
        )
    };
    let mark = |exchange: &str| {
        format!(
            "INSERT INTO segment_dirty(exchange, symbol, interval, window_start_ms) \
             VALUES ('{exchange}', 'BTCUSDT', '1m', 1792454400000)"
        )
    };
    for line in &lines {
        query(&url, &candle(line, "1m"));
        query(&url, &entry(line));
        query(&url, &mark(line));
    }
    query(&url, &candle("binance_spot", "5m"));
    assert_eq!(count(&url, "candles"), "7");
    assert_eq!(count(&url, "segment_index"), "6");
    assert_eq!(count(&url, "segment_dirty"), "6");
    // A second row under one line's key is refused by the key.
    for sql in [
        candle("bybit_inverse", "1m"),
        entry("bybit_inverse"),
        mark("bybit_inverse"),
    ] {
        assert_eq!(
            refusal(&url, &sql).code(),
            Some(&SqlState::UNIQUE_VIOLATION),
            "{sql}"
        );
    }
}

#[test]
fn runs_of_one_market_under_distinct_identities_coexist() {
    let Some(fixture) = fresh() else {
        return;
    };
    let url = url(&fixture);
    let run = |run_id: &str, identity: &str| {
        format!(
            "INSERT INTO bot_runs(run_id, exchange, symbol, interval, strategy_name, status, \
             state_json, started_at_ms, updated_at_ms{}) VALUES ('{run_id}', 'binance_spot', \
             'BTCUSDT', '1m', 'sma_cross', 'running', '{{}}', 1, 1{})",
            if identity.is_empty() {
                ""
            } else {
                ", environment, account_kind, account_id, identity_source"
            },
            if identity.is_empty() {
                String::new()
            } else {
                format!(", {identity}")
            }
        )
    };
    let key = |name: &str| KeyFingerprint::of(name).as_str().to_string();
    let (a, b) = (key("a"), key("b"));
    query(
        &url,
        &run(
            "run-testnet-a",
            &format!("'testnet', 'key_fingerprint', '{a}', 'recorded'"),
        ),
    );
    query(
        &url,
        &run(
            "run-mainnet-a",
            &format!("'mainnet', 'key_fingerprint', '{a}', 'recorded'"),
        ),
    );
    query(
        &url,
        &run(
            "run-testnet-b",
            &format!("'testnet', 'key_fingerprint', '{b}', 'recorded'"),
        ),
    );
    query(&url, &run("run-unbound", "NULL, NULL, NULL, 'recorded'"));
    // As a transfer writes a run an earlier build left: no identity at all.
    query(&url, &run("run-legacy", ""));
    let null = || None;
    let some = |text: &str| Some(text.to_string());
    assert_eq!(
        query(
            &url,
            "SELECT run_id, environment, account_id, identity_source FROM bot_runs ORDER BY run_id"
        ),
        [
            vec![some("run-legacy"), null(), null(), some("legacy")],
            vec![
                some("run-mainnet-a"),
                some("mainnet"),
                some(&a),
                some("recorded")
            ],
            vec![
                some("run-testnet-a"),
                some("testnet"),
                some(&a),
                some("recorded")
            ],
            vec![
                some("run-testnet-b"),
                some("testnet"),
                some(&b),
                some("recorded")
            ],
            vec![some("run-unbound"), null(), null(), some("recorded")],
        ]
    );
    assert_eq!(
        refusal(&url, &run("run-legacy", "")).code(),
        Some(&SqlState::UNIQUE_VIOLATION)
    );
    // Two runs' journals may name the same venue order: each event is a row.
    for run_id in ["run-testnet-a", "run-mainnet-a", "run-testnet-a"] {
        query(
            &url,
            &format!(
                "INSERT INTO order_events(run_id, symbol, side, order_type, status, order_id, \
                 raw_json) VALUES ('{run_id}', 'BTCUSDT', 'BUY', 'MARKET', 'FILLED', '42', '{{}}')"
            ),
        );
    }
    assert_eq!(
        column(
            &url,
            "SELECT run_id || ':' || seq FROM order_events ORDER BY seq"
        ),
        ["run-testnet-a:1", "run-mainnet-a:2", "run-testnet-a:3"]
    );
}

#[test]
fn the_schema_carries_each_step_of_publishing_a_segment() {
    let Some(fixture) = fresh() else {
        return;
    };
    let url = url(&fixture);
    block_on(async {
        let client = client(&url).await;
        let (old, new) = (
            format!("sha256:{}", "ab".repeat(32)),
            format!("sha256:{}", "cd".repeat(32)),
        );
        let blob = "INSERT INTO segments(hash, format, payload_len, data) VALUES ($1, 1, 3, $2) \
                    ON CONFLICT (hash) DO NOTHING";
        let insert = |hash: &str, bytes: Vec<u8>| {
            let hash = hash.to_string();
            let client = &client;
            async move {
                client
                    .execute_typed(blob, &[(&hash, Type::TEXT), (&bytes, Type::BYTEA)])
                    .await
                    .expect("a blob")
            }
        };
        assert_eq!(insert(&old, vec![1, 2, 3]).await, 1);
        // The same name again changes nothing: a stored blob is never
        // rewritten.
        assert_eq!(insert(&old, vec![9, 9, 9]).await, 0);
        let stored = |hash: String| {
            let client = &client;
            async move {
                client
                    .query_typed_one(
                        "SELECT data FROM segments WHERE hash = $1",
                        &[(&hash, Type::TEXT)],
                    )
                    .await
                    .expect("the blob")
                    .try_get::<_, Vec<u8>>(0)
                    .expect("bytes")
            }
        };
        assert_eq!(stored(old.clone()).await, [1, 2, 3]);

        // Every mark of a window is a new, larger revision.
        let mark = "INSERT INTO segment_dirty(exchange, symbol, interval, window_start_ms) \
                    VALUES ('binance_spot', 'BTCUSDT', '1m', 1792454400000) \
                    ON CONFLICT (exchange, symbol, interval, window_start_ms) \
                    DO UPDATE SET id = DEFAULT RETURNING id";
        let revision = || {
            let client = &client;
            async move {
                client
                    .query_typed_one(mark, &[])
                    .await
                    .expect("a mark")
                    .try_get::<_, i64>(0)
                    .expect("a revision")
            }
        };
        let (read, marked_again) = (revision().await, revision().await);
        assert!(marked_again > read, "{read} then {marked_again}");

        // A write-back of the revision it read deletes no newer mark.
        let unmark = "DELETE FROM segment_dirty WHERE id = $1";
        let deleted = client
            .execute_typed(unmark, &[(&read, Type::INT8)])
            .await
            .expect("a stale write-back");
        assert_eq!(deleted, 0);
        let deleted = client
            .execute_typed(unmark, &[(&marked_again, Type::INT8)])
            .await
            .expect("a current write-back");
        assert_eq!(deleted, 1);

        // An index entry moved to a new segment leaves the old one loadable.
        let entry = "INSERT INTO segment_index VALUES ('binance_spot', 'BTCUSDT', '1m', \
                     1792454400000, 1792454400000, 1792454400000, 1, $1) \
                     ON CONFLICT (exchange, symbol, interval, window_start_ms) \
                     DO UPDATE SET hash = excluded.hash";
        client
            .execute_typed(entry, &[(&old, Type::TEXT)])
            .await
            .expect("an entry");
        assert_eq!(insert(&new, vec![4, 5, 6]).await, 1);
        client
            .execute_typed(entry, &[(&new, Type::TEXT)])
            .await
            .expect("the entry moved");
        assert_eq!(stored(old.clone()).await, [1, 2, 3]);
        assert_eq!(stored(new.clone()).await, [4, 5, 6]);
    });
    assert_eq!(count(&url, "segment_index"), "1");
    assert_eq!(count(&url, "segments"), "2");
}
