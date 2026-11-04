//! The run journal and the store's record on PostgreSQL beyond what the
//! shared groups show: a database whose collation orders text as people
//! read it, where only `COLLATE "C"` keeps run IDs in SQLite's byte order;
//! and two processes appending to one run at once, whose events all land
//! in the order each wrote them. Without `QUANTFORGE_TEST_POSTGRES_URL`
//! every test skips.

use quantforge::storage::postgres::PostgresStore;
use quantforge::{
    ExchangeId, ExchangeOrder, ExecutionMode, Interval, MarketId, OrderStatus, PositionState,
    RunIdentity, RunJournalStore, RunStatus, Side, Symbol, VenueOrderId,
};
use rust_decimal::Decimal;
use std::sync::Barrier;

use super::{
    IsolatedDatabase, Postgres, VARIABLE, block_on, client, fixture_on, server_url, settings,
};
use crate::conformance::{self, Backend, Fixture};

/// A collation that orders `run-B` after `run-a`, as byte order never does.
const LINGUISTIC: &str = "TEMPLATE template0 LC_COLLATE 'en_US.UTF-8' LC_CTYPE 'en_US.UTF-8'";

/// A `PostgresStore` on a database of its own whose collation is
/// `en_US.UTF-8`.
struct Linguistic;

impl Backend for Linguistic {
    type Store = PostgresStore;
    const NAME: &'static str = "postgres (en_US.UTF-8)";

    fn fresh() -> Fixture<PostgresStore> {
        let server = server_url().unwrap_or_else(|| panic!("{VARIABLE} is unset"));
        fixture_on(IsolatedDatabase::create_with(&server, LINGUISTIC))
    }
}

/// Whether the server can create a database with the linguistic collation.
fn linguistic_collation(server: &str) -> bool {
    block_on(async {
        let client = client(server).await;
        let probe = "qf_collation_probe";
        let created = client
            .simple_query(&format!(
                "CREATE DATABASE {probe}_{} {LINGUISTIC}",
                std::process::id()
            ))
            .await
            .is_ok();
        if created {
            let _ = client
                .simple_query(&format!("DROP DATABASE {probe}_{}", std::process::id()))
                .await;
        }
        created
    })
}

#[test]
fn the_journal_and_store_record_keep_byte_order_under_a_linguistic_collation() {
    let Some(server) = server_url() else {
        eprintln!("skipping: {VARIABLE} is unset");
        return;
    };
    if !linguistic_collation(&server) {
        eprintln!("skipping: the server has no en_US.UTF-8 locale");
        return;
    }
    conformance::journal::<Linguistic>();
    conformance::control::<Linguistic>();
}

fn order(order_id: String) -> ExchangeOrder {
    ExchangeOrder {
        symbol: Symbol::new("BTCUSDT").expect("symbol"),
        side: Side::Buy,
        order_type: "MARKET".to_string(),
        status: OrderStatus::Filled,
        order_id: Some(VenueOrderId::new(order_id).expect("order id")),
        client_order_id: None,
        requested_qty: None,
        requested_quote_qty: None,
        executed_qty: Some(Decimal::ONE),
        cumulative_quote_qty: Some(Decimal::ONE),
        avg_price: None,
        transact_time_ms: Some(1),
        fills: None,
        raw: serde_json::json!({}),
    }
}

#[test]
fn writers_appending_to_one_run_at_once_lose_nothing_and_keep_their_own_order() {
    if server_url().is_none() {
        eprintln!("skipping: {VARIABLE} is unset");
        return;
    }
    let fixture = Postgres::fresh();
    let url = fixture
        .guard
        .downcast_ref::<IsolatedDatabase>()
        .expect("an isolated database")
        .url
        .clone();
    fixture
        .store
        .create_run(
            &quantforge::BotRunState {
                run_id: "run-shared".to_string(),
                market: MarketId::new(
                    ExchangeId::BybitSpot,
                    Symbol::new("BTCUSDT").expect("symbol"),
                    Interval::M1,
                ),
                strategy_name: "sma_cross".to_string(),
                strategy_config: serde_json::json!({}),
                execution_mode: ExecutionMode::DryRun,
                status: RunStatus::Running,
                last_processed_open_time_ms: None,
                started_at_ms: 1,
                updated_at_ms: 1,
                stopped_at_ms: None,
                last_error: None,
                position: PositionState::flat(),
            },
            &RunIdentity::recorded(None, None),
        )
        .expect("the run");
    // Two processes: a store, and so a pool, each.
    let writers: Vec<PostgresStore> = ["a", "b"]
        .iter()
        .map(|_| PostgresStore::connect(&settings(&url)).expect("a writer"))
        .collect();
    let barrier = Barrier::new(writers.len());
    std::thread::scope(|scope| {
        for (writer, name) in writers.iter().zip(["a", "b"]) {
            let barrier = &barrier;
            scope.spawn(move || {
                barrier.wait();
                for index in 0..50 {
                    writer
                        .append_order_event("run-shared", &order(format!("{name}-{index}")))
                        .expect("an append");
                }
            });
        }
    });
    let listed: Vec<String> = fixture
        .store
        .list_order_events("run-shared", 1_000)
        .expect("the events")
        .iter()
        .map(|event| event.order_id.as_ref().expect("an id").as_str().to_string())
        .collect();
    assert_eq!(listed.len(), 100, "every append is a row");
    for name in ["a", "b"] {
        // Newest first: each writer's events in the reverse of its order.
        let own: Vec<&String> = listed
            .iter()
            .filter(|id| id.starts_with(&format!("{name}-")))
            .collect();
        let expected: Vec<String> = (0..50)
            .rev()
            .map(|index| format!("{name}-{index}"))
            .collect();
        assert_eq!(own, expected.iter().collect::<Vec<_>>(), "writer {name}");
    }
}
