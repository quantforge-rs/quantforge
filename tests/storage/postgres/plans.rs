//! The index every query the stores run is planned on, over representative
//! data: the evidence `docs/postgres-query-plans.md` records, and the BRIN
//! evaluation behind its decision. Each query is the PostgreSQL form of
//! the statement the SQLite store runs, with literal values where the
//! store binds parameters; tokio-postgres' unnamed statements are planned
//! for their values the same way. Run with `--nocapture` to see every plan
//! with its timings and buffers. Without `QUANTFORGE_TEST_POSTGRES_URL`
//! the database tests skip; the doc's list is checked either way.

use quantforge::storage::postgres::PostgresStore;
use serde_json::Value;

use super::{IsolatedDatabase, VARIABLE, query, server_url, settings};
use crate::docs::{items, section_in};

/// One query the stores run, the indexes its plan must use, and the tables
/// it must never scan whole.
struct Query {
    id: &'static str,
    indexes: &'static [&'static str],
    never_scanned: &'static [&'static str],
    /// A write is explained without being run.
    write: bool,
    sql: &'static str,
}

/// Every statement shape the SQLite stores run, as the PostgreSQL stores
/// run it (`src/storage/postgres/`). The values pick one market, run and
/// window of the data.
const QUERIES: &[Query] = &[
    Query {
        id: "candles.load",
        indexes: &["candles_pkey"],
        never_scanned: &["candles"],
        write: false,
        sql: "SELECT open_time_ms, close_time_ms, open, high, low, close, volume, trades \
              FROM candles \
              WHERE exchange = 'binance_spot' AND symbol = 'BTCUSDT' AND interval = '1m' \
                AND (CAST(1790812800000 AS bigint) IS NULL OR open_time_ms >= 1790812800000) \
                AND (CAST(1791417600000 AS bigint) IS NULL OR open_time_ms <= 1791417600000) \
              ORDER BY open_time_ms ASC LIMIT 1000",
    },
    Query {
        id: "candles.recent",
        indexes: &["candles_pkey"],
        never_scanned: &["candles"],
        write: false,
        sql: "SELECT open_time_ms, close_time_ms, open, high, low, close, volume, trades \
              FROM candles \
              WHERE exchange = 'binance_spot' AND symbol = 'BTCUSDT' AND interval = '1m' \
              ORDER BY open_time_ms DESC LIMIT 300",
    },
    Query {
        id: "candles.max_open_time",
        indexes: &["candles_pkey"],
        never_scanned: &["candles"],
        write: false,
        sql: "SELECT MAX(open_time_ms) FROM candles \
              WHERE exchange = 'binance_spot' AND symbol = 'BTCUSDT' AND interval = '1m'",
    },
    Query {
        id: "candles.upsert",
        indexes: &["candles_pkey"],
        never_scanned: &["candles"],
        write: true,
        sql: "INSERT INTO candles(exchange, symbol, interval, open_time_ms, close_time_ms, \
                                  open, high, low, close, volume, trades) \
              SELECT 'binance_spot', 'BTCUSDT', '1m', t.open_time_ms, t.close_time_ms, t.open, \
                     t.high, t.low, t.close, t.volume, t.trades \
              FROM unnest(CAST(ARRAY[1790812800000, 1790812860000] AS bigint[]), \
                          CAST(ARRAY[1790812859999, 1790812919999] AS bigint[]), \
                          CAST(ARRAY['1', '1'] AS text[]), CAST(ARRAY['1', '1'] AS text[]), \
                          CAST(ARRAY['1', '1'] AS text[]), CAST(ARRAY['1', '1'] AS text[]), \
                          CAST(ARRAY['1', '1'] AS text[]), CAST(ARRAY[1, 1] AS bigint[])) \
                AS t(open_time_ms, close_time_ms, open, high, low, close, volume, trades) \
              ON CONFLICT (exchange, symbol, interval, open_time_ms) DO UPDATE SET \
                close_time_ms = excluded.close_time_ms, open = excluded.open, \
                high = excluded.high, low = excluded.low, close = excluded.close, \
                volume = excluded.volume, trades = excluded.trades \
              WHERE candles.close_time_ms IS DISTINCT FROM excluded.close_time_ms \
                 OR candles.open IS DISTINCT FROM excluded.open \
                 OR candles.high IS DISTINCT FROM excluded.high \
                 OR candles.low IS DISTINCT FROM excluded.low \
                 OR candles.close IS DISTINCT FROM excluded.close \
                 OR candles.volume IS DISTINCT FROM excluded.volume \
                 OR candles.trades IS DISTINCT FROM excluded.trades \
              RETURNING open_time_ms",
    },
    Query {
        id: "candles.window",
        indexes: &["candles_pkey"],
        never_scanned: &["candles"],
        write: false,
        sql: "SELECT open_time_ms, close_time_ms, open, high, low, close, volume, trades \
              FROM candles \
              WHERE exchange = 'binance_spot' AND symbol = 'BTCUSDT' AND interval = '1m' \
                AND open_time_ms >= 1790812800000 AND open_time_ms < 1790899200000 \
              ORDER BY open_time_ms ASC",
    },
    Query {
        id: "runs.latest",
        indexes: &["idx_bot_runs_market"],
        never_scanned: &["bot_runs"],
        write: false,
        sql: "SELECT run_id, state_json FROM bot_runs \
              WHERE exchange = 'binance_spot' AND symbol = 'BTCUSDT' AND interval = '1m' \
                AND strategy_name = 'sma_cross' \
              ORDER BY updated_at_ms DESC, started_at_ms DESC, run_id COLLATE \"C\" DESC \
              LIMIT 1",
    },
    Query {
        id: "runs.by_id",
        indexes: &["bot_runs_pkey"],
        never_scanned: &["bot_runs"],
        write: false,
        sql: "SELECT state_json FROM bot_runs WHERE run_id = 'run-1234'",
    },
    Query {
        id: "runs.legacy",
        indexes: &[],
        never_scanned: &[],
        write: false,
        sql: "SELECT run_id, exchange, symbol, interval, state_json, started_at_ms \
              FROM bot_runs WHERE identity_source = 'legacy' \
              ORDER BY started_at_ms ASC, run_id COLLATE \"C\" ASC",
    },
    Query {
        id: "order_events.latest",
        indexes: &["idx_order_events_run"],
        never_scanned: &["order_events"],
        write: false,
        sql: "SELECT raw_json FROM order_events WHERE run_id = 'run-7' \
              ORDER BY seq DESC LIMIT 50",
    },
    Query {
        id: "closed_trades.latest",
        indexes: &["idx_closed_trades_run"],
        never_scanned: &["closed_trades"],
        write: false,
        sql: "SELECT symbol, entry_time_ms, exit_time_ms, entry_price, exit_price, qty, \
                     gross_quote_pnl, entry_order_id, exit_order_id \
              FROM closed_trades WHERE run_id = 'run-7' ORDER BY seq DESC LIMIT 50",
    },
    Query {
        id: "segments.lowest_dirty",
        indexes: &["segment_dirty_exchange_symbol_interval_window_start_ms_key"],
        never_scanned: &["segment_dirty"],
        write: false,
        sql: "SELECT MIN(window_start_ms) FROM segment_dirty \
              WHERE exchange = 'binance_spot' AND symbol = 'BTCUSDT' AND interval = '1m' \
                AND (CAST(1750000000000 AS bigint) IS NULL OR window_start_ms > 1750000000000) \
                AND (CAST(1790812800000 AS bigint) IS NULL OR window_start_ms < 1790812800000)",
    },
    Query {
        id: "segments.mark",
        indexes: &["segment_dirty_exchange_symbol_interval_window_start_ms_key"],
        never_scanned: &["segment_dirty"],
        write: true,
        sql: "INSERT INTO segment_dirty(exchange, symbol, interval, window_start_ms) \
              SELECT 'binance_spot', 'BTCUSDT', '1m', \
                     unnest(CAST(ARRAY[1790812800000, 1790899200000] AS bigint[])) \
              ON CONFLICT (exchange, symbol, interval, window_start_ms) DO UPDATE SET id = DEFAULT",
    },
    Query {
        id: "segments.mark_of_window",
        indexes: &["segment_dirty_exchange_symbol_interval_window_start_ms_key"],
        never_scanned: &["segment_dirty"],
        write: false,
        sql: "SELECT id FROM segment_dirty \
              WHERE exchange = 'binance_spot' AND symbol = 'BTCUSDT' AND interval = '1m' \
                AND window_start_ms = 1780704000000",
    },
    Query {
        id: "segments.lock_mark",
        indexes: &["segment_dirty_exchange_symbol_interval_window_start_ms_key"],
        never_scanned: &["segment_dirty"],
        write: false,
        sql: "SELECT id FROM segment_dirty \
              WHERE exchange = 'binance_spot' AND symbol = 'BTCUSDT' AND interval = '1m' \
                AND window_start_ms = 1780704000000 \
              FOR UPDATE",
    },
    Query {
        id: "segments.entry_of_window",
        indexes: &["segment_index_pkey"],
        never_scanned: &["segment_index"],
        write: false,
        sql: "SELECT hash FROM segment_index \
              WHERE exchange = 'binance_spot' AND symbol = 'BTCUSDT' AND interval = '1m' \
                AND window_start_ms = 1780704000000",
    },
    Query {
        id: "segments.plan",
        indexes: &["segment_index_pkey", "segments_pkey"],
        never_scanned: &["segment_index", "segments"],
        write: false,
        sql: "SELECT i.window_start_ms, i.first_open_ms, i.last_open_ms, i.bar_count, i.hash, \
                     s.payload_len, length(s.data) \
              FROM segment_index i JOIN segments s ON s.hash = i.hash \
              WHERE i.exchange = 'binance_spot' AND i.symbol = 'BTCUSDT' AND i.interval = '1m' \
                AND i.window_start_ms >= 1787616000000 AND i.window_start_ms < 1790208000000 \
                AND i.last_open_ms >= 1787616000000 AND i.first_open_ms <= 1790208000000 \
              ORDER BY i.window_start_ms ASC",
    },
    Query {
        id: "segments.blob",
        indexes: &["segments_pkey"],
        never_scanned: &["segments"],
        write: false,
        sql: "SELECT data FROM segments \
              WHERE hash = 'sha256:0000000000000000000000000000000000000000000000000000000000000400'",
    },
    Query {
        id: "segments.store",
        indexes: &["segments_pkey"],
        never_scanned: &["segments"],
        write: true,
        sql: "INSERT INTO segments(hash, format, payload_len, data) \
              VALUES ('sha256:0000000000000000000000000000000000000000000000000000000000000400', \
                      1, 2048, '\\x00') \
              ON CONFLICT (hash) DO NOTHING",
    },
    Query {
        id: "segments.index",
        indexes: &["segment_index_pkey"],
        never_scanned: &["segment_index"],
        write: true,
        sql: "INSERT INTO segment_index(exchange, symbol, interval, window_start_ms, \
                                        first_open_ms, last_open_ms, bar_count, hash) \
              VALUES ('binance_spot', 'BTCUSDT', '1m', 1780704000000, 1780704000000, \
                      1780790340000, 1440, \
                      'sha256:0000000000000000000000000000000000000000000000000000000000000400') \
              ON CONFLICT (exchange, symbol, interval, window_start_ms) DO UPDATE SET \
                first_open_ms = excluded.first_open_ms, last_open_ms = excluded.last_open_ms, \
                bar_count = excluded.bar_count, hash = excluded.hash",
    },
    Query {
        id: "segments.unindex",
        indexes: &["segment_index_pkey"],
        never_scanned: &["segment_index"],
        write: true,
        sql: "DELETE FROM segment_index \
              WHERE exchange = 'binance_spot' AND symbol = 'BTCUSDT' AND interval = '1m' \
                AND window_start_ms = 1780704000000",
    },
    Query {
        id: "segments.unmark",
        indexes: &["segment_dirty_pkey"],
        never_scanned: &["segment_dirty"],
        write: true,
        sql: "DELETE FROM segment_dirty WHERE id = 4000",
    },
];

/// The order rows reach the heap: market by market, as a backfill writes
/// them, or minute by minute across markets, as following markets live
/// does.
#[derive(Clone, Copy, Debug)]
enum Order {
    MarketByMarket,
    TimeAcrossMarkets,
}

/// The markets of the data, each with the number its segments are named
/// from.
const MARKETS: &str = "(VALUES ('binance_spot', 'BTCUSDT', 0), ('binance_linear', 'BTCUSDT', 1), \
     ('binance_inverse', 'BTCUSD', 2), ('bybit_spot', 'BTCUSDT', 3), \
     ('bybit_linear', 'BTCUSDT', 4), ('bybit_inverse', 'BTCUSD', 5), \
     ('binance_spot', 'ETHUSDT', 6), ('bybit_linear', 'ETHUSDT', 7)) AS m(exchange, symbol, n)";

/// A store some years in, written in `order`:
/// - 8 markets of `1m` candles, 30 days each from 1790208000000, and 2
///   years of `1h` candles for 4 of them: 415,680 candles;
/// - 2,000 runs over the 8 markets and 3 strategies, with ties in
///   `updated_at_ms`; 100,000 order events and 30,000 closed trades over
///   them;
/// - 3 years of daily segments for each `1m` market before the candles:
///   8,760 segments and index entries, and a dirty mark on each window, as a
///   deep backfill leaves them before its refresh.
fn seed(url: &str, order: Order) {
    let (minutes, hours) = match order {
        Order::MarketByMarket => ("m.n, t", "m.n, t"),
        Order::TimeAcrossMarkets => ("t, m.n", "t, m.n"),
    };
    query(
        url,
        &format!(
            "INSERT INTO candles \
             SELECT m.exchange, m.symbol, '1m', t, t + 59999, '100.10', '100.20', '100.00', \
                    '100.15', '1.50000000', (t / 60000) % 97 \
             FROM {MARKETS}, \
                  generate_series(1790208000000, 1792799940000, 60000) AS t \
             ORDER BY {minutes}; \
             INSERT INTO candles \
             SELECT m.exchange, m.symbol, '1h', t, t + 3599999, '100.10', '100.20', '100.00', \
                    '100.15', '61.25000000', (t / 3600000) % 89 \
             FROM {MARKETS}, \
                  generate_series(1727136000000, 1790204400000, 3600000) AS t \
             WHERE m.n IN (0, 2, 4, 6) \
             ORDER BY {hours}; \
             INSERT INTO bot_runs(run_id, exchange, symbol, interval, strategy_name, status, \
                                  state_json, started_at_ms, updated_at_ms, environment, \
                                  account_kind, account_id, identity_source, identity_at_ms) \
             SELECT 'run-' || i, m.exchange, m.symbol, '1m', \
                    (ARRAY['sma_cross', 'sma_fast', 'breakout'])[1 + i % 3], 'stopped', \
                    '{{\"run_id\":\"run-' || i || '\"}}', 1790208000000 + i * 1000, \
                    1790208000000 + (i / 48) * 1000, 'testnet', 'key_fingerprint', \
                    '0123456789abcdef', 'recorded', 1790208000000 \
             FROM generate_series(1, 2000) AS i JOIN {MARKETS} ON m.n = i % 8; \
             INSERT INTO order_events(run_id, symbol, side, order_type, status, order_id, \
                                      client_order_id, transact_time_ms, raw_json) \
             SELECT 'run-' || (1 + i % 2000), 'BTCUSDT', \
                    CASE WHEN i % 2 = 0 THEN 'BUY' ELSE 'SELL' END, 'MARKET', 'FILLED', \
                    CAST(i AS TEXT), 'qf-' || i, 1790208000000 + i, \
                    '{{\"orderId\":\"' || i || '\"}}' \
             FROM generate_series(1, 100000) AS i; \
             INSERT INTO closed_trades(run_id, symbol, entry_time_ms, exit_time_ms, \
                                       entry_price, exit_price, qty, gross_quote_pnl, \
                                       entry_order_id, exit_order_id) \
             SELECT 'run-' || (1 + i % 2000), 'BTCUSDT', i, i + 60000, '100.10', '101.10', \
                    '0.00100000', '0.00100000', CAST(i AS TEXT), CAST(i + 1 AS TEXT) \
             FROM generate_series(1, 30000) AS i; \
             INSERT INTO segments(hash, format, payload_len, data) \
             SELECT 'sha256:' || lpad(to_hex(i), 64, '0'), 1, 2048, \
                    decode(repeat(lpad(to_hex(i % 256), 2, '0'), 2048), 'hex') \
             FROM generate_series(1, 8760) AS i; \
             INSERT INTO segment_index \
             SELECT m.exchange, m.symbol, '1m', w, w, w + 86340000, 1440, \
                    'sha256:' || lpad(to_hex(m.n * 1095 + d + 1), 64, '0') \
             FROM {MARKETS}, generate_series(0, 1094) AS d, \
                  LATERAL (SELECT 1790208000000 - (1095 - d) * CAST(86400000 AS BIGINT) AS w) AS x \
             ORDER BY m.n, d; \
             INSERT INTO segment_dirty(exchange, symbol, interval, window_start_ms) \
             SELECT exchange, symbol, interval, window_start_ms FROM segment_index \
             ORDER BY exchange, symbol, window_start_ms; \
             ANALYZE;"
        ),
    );
}

/// A database of its own, its schema created and its data seeded.
fn seeded(server: &str, order: Order) -> IsolatedDatabase {
    let database = IsolatedDatabase::create(server);
    PostgresStore::connect(&settings(&database.url))
        .expect("a store")
        .init()
        .expect("init");
    seed(&database.url, order);
    database
}

/// The plan of `sql`, as PostgreSQL's JSON.
fn plan(url: &str, statement: &Query) -> Value {
    let options = if statement.write {
        "FORMAT JSON"
    } else {
        "ANALYZE, BUFFERS, FORMAT JSON"
    };
    let rows = query(url, &format!("EXPLAIN ({options}) {}", statement.sql));
    let text = rows
        .into_iter()
        .next()
        .and_then(|row| row.into_iter().next().flatten())
        .expect("a plan");
    serde_json::from_str(&text).expect("the plan's JSON")
}

/// The plan as PostgreSQL prints it, for the evidence.
fn printed(url: &str, statement: &Query) -> String {
    let options = if statement.write {
        "COSTS OFF"
    } else {
        "ANALYZE, BUFFERS, COSTS OFF"
    };
    query(url, &format!("EXPLAIN ({options}) {}", statement.sql))
        .into_iter()
        .filter_map(|row| row.into_iter().next().flatten())
        .collect::<Vec<_>>()
        .join("\n")
}

/// One node of a plan: its type, the table it reads, and the indexes it
/// reads through, or checks an upsert's conflicts on.
#[derive(Debug)]
struct Node {
    kind: String,
    relation: Option<String>,
    indexes: Vec<String>,
}

/// Every node of a plan.
fn nodes(plan: &Value) -> Vec<Node> {
    fn walk(node: &Value, found: &mut Vec<Node>) {
        let text = |key: &str| node.get(key).and_then(Value::as_str).map(str::to_string);
        let mut indexes: Vec<String> = text("Index Name").into_iter().collect();
        if let Some(arbiters) = node
            .get("Conflict Arbiter Indexes")
            .and_then(Value::as_array)
        {
            indexes.extend(
                arbiters
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string),
            );
        }
        found.push(Node {
            kind: text("Node Type").unwrap_or_default(),
            relation: text("Relation Name"),
            indexes,
        });
        for child in node
            .get("Plans")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            walk(child, found);
        }
    }
    let mut found = Vec::new();
    walk(&plan[0]["Plan"], &mut found);
    found
}

/// The indexes `statement`'s plan reads through, after checking it never
/// scans whole a table it must not.
fn indexes_of(url: &str, statement: &Query) -> Vec<String> {
    let found = nodes(&plan(url, statement));
    for node in &found {
        if let Some(relation) = &node.relation {
            assert!(
                !(node.kind == "Seq Scan" && statement.never_scanned.contains(&relation.as_str())),
                "{} scans {relation} whole: {found:?}",
                statement.id
            );
        }
    }
    found.into_iter().flat_map(|node| node.indexes).collect()
}

#[test]
fn the_doc_lists_every_query_with_its_indexes() {
    let expected: Vec<(String, String)> = QUERIES
        .iter()
        .map(|statement| {
            let indexes = if statement.indexes.is_empty() {
                "none".to_string()
            } else {
                statement
                    .indexes
                    .iter()
                    .map(|index| format!("`{index}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            (statement.id.to_string(), indexes)
        })
        .collect();
    assert_eq!(
        items(&section_in("docs/postgres-query-plans.md", "queries")),
        expected
    );
}

#[test]
fn every_query_the_stores_run_is_planned_on_its_index() {
    let Some(server) = server_url() else {
        eprintln!("skipping: {VARIABLE} is unset");
        return;
    };
    for order in [Order::MarketByMarket, Order::TimeAcrossMarkets] {
        let database = seeded(&server, order);
        for statement in QUERIES {
            let used = indexes_of(&database.url, statement);
            for index in statement.indexes {
                assert!(
                    used.iter().any(|name| name == index),
                    "{} with rows {order:?} is not planned on {index}: {used:?}",
                    statement.id
                );
            }
            eprintln!(
                "== {} ({order:?})\n{}\n",
                statement.id,
                printed(&database.url, statement)
            );
        }
    }
}

#[test]
fn a_brin_index_on_open_time_serves_no_query_the_stores_run() {
    let Some(server) = server_url() else {
        eprintln!("skipping: {VARIABLE} is unset");
        return;
    };
    for order in [Order::MarketByMarket, Order::TimeAcrossMarkets] {
        let database = seeded(&server, order);
        query(
            &database.url,
            "CREATE INDEX candles_open_time_brin ON candles USING brin (open_time_ms); ANALYZE;",
        );
        for statement in QUERIES {
            let used = indexes_of(&database.url, statement);
            assert!(
                !used.iter().any(|name| name == "candles_open_time_brin"),
                "{} with rows {order:?} is planned on the BRIN: {used:?}",
                statement.id
            );
        }
        let sizes = query(
            &database.url,
            "SELECT pg_size_pretty(pg_relation_size('candles')), \
                    pg_size_pretty(pg_relation_size('candles_pkey')), \
                    pg_size_pretty(pg_relation_size('candles_open_time_brin'))",
        );
        // The one shape a BRIN serves: every market's candles in a span of
        // time, which no store reads.
        let across = Query {
            id: "candles.across_markets",
            indexes: &[],
            never_scanned: &[],
            write: false,
            sql: "SELECT count(*) FROM candles \
                  WHERE open_time_ms >= 1790812800000 AND open_time_ms < 1790816400000",
        };
        eprintln!(
            "== BRIN ({order:?}): candles, candles_pkey, candles_open_time_brin: {sizes:?}\n\
             == {} ({order:?})\n{}\n",
            across.id,
            printed(&database.url, &across)
        );
    }
}
