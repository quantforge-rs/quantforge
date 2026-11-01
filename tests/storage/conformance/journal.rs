//! The run journal's steps, and the checks that read the identity cases:
//! venue IDs of every shape are text, and runs of two environments or two
//! accounts are never one another's.

use quantforge::{
    BotRunState, ClosedTrade, Environment, ExchangeId, ExchangeOrder, ExecutionMode, Fill,
    IdentitySource, Interval, KeyFingerprint, MarketId, ModelError, OrderStatus, PositionState,
    RunIdentity, RunJournalStore, RunStatus, Side, Symbol, VenueAccountId, VenueOrderId,
    VenueTradeId,
};
use rust_decimal::Decimal;

use super::cases::{Cases, identity_cases};
use super::{Coded, Fixture, Step, expect_refused};

/// The checks coded here: id and statement.
pub const CHECKS: &[(&str, &str)] = &[
    (
        "journal.identity_venue_ids",
        "venue IDs of every shape in the identity cases round-trip as text through both journals",
    ),
    (
        "journal.environments_and_accounts",
        "runs recorded under two environments or two accounts are never one another's, and binding a store later adopts no run",
    ),
];

pub fn check<S: RunJournalStore>(id: &str) -> Coded<S> {
    match id {
        "journal.identity_venue_ids" => venue_ids_are_text,
        "journal.environments_and_accounts" => environments_and_accounts_are_isolated,
        other => panic!("no journal check {other}"),
    }
}

/// Compares two values exactly: as their JSON, which holds every decimal
/// as its text, and as values.
fn same<T: serde::Serialize + PartialEq + std::fmt::Debug>(found: &T, expected: &T, what: &str) {
    assert_eq!(
        serde_json::to_value(found).expect("json"),
        serde_json::to_value(expected).expect("json"),
        "{what}"
    );
    assert_eq!(found, expected, "{what}");
}

/// Runs a journal step; `false` for a step of another port.
pub fn step<S: RunJournalStore + ?Sized>(cases: &Cases, store: &S, step: &Step) -> bool {
    match step {
        Step::CreateRun {
            run,
            identity,
            refused,
        } => {
            let outcome = store.create_run(&cases.run(run), &identity.identity());
            match refused {
                None => outcome.expect("create run"),
                Some(refused) => expect_refused(outcome, refused, "create run"),
            }
        }
        Step::SaveRun { run, refused } => {
            let outcome = store.save_run_state(&cases.run(run));
            match refused {
                None => outcome.expect("save run"),
                Some(refused) => expect_refused(outcome, refused, "save run"),
            }
        }
        Step::LoadRun {
            run_id,
            expect,
            refused,
        } => {
            let outcome = store.load_run_state(run_id);
            match refused {
                Some(refused) => expect_refused(outcome, refused, "load run"),
                None => {
                    let loaded = outcome.expect("load run");
                    let expected = expect.as_deref().map(|name| cases.run(name));
                    same(&loaded, &expected, &format!("run {run_id}"));
                }
            }
        }
        Step::RunIdentity { run_id, expect } => {
            let found = store.run_identity(run_id).expect("run identity");
            let expected = expect.as_ref().map(|text| text.identity());
            assert_eq!(found, expected, "identity of {run_id}");
        }
        Step::LatestRun {
            market,
            strategy,
            expect,
            refused,
        } => {
            let outcome = store.latest_run_for_market(&cases.market(market), strategy);
            match refused {
                Some(refused) => expect_refused(outcome, refused, "latest run"),
                None => {
                    let found = outcome.expect("latest run");
                    let expected = expect.as_deref().map(|name| cases.run(name));
                    same(
                        &found,
                        &expected,
                        &format!("latest {strategy} run of {market}"),
                    );
                }
            }
        }
        Step::IdentityMatches {
            run_id,
            invocation,
            expect,
        } => {
            let recorded = store
                .run_identity(run_id)
                .expect("run identity")
                .expect("a recorded identity");
            let answer = match recorded.matches(&invocation.identity()) {
                Ok(()) => "ok",
                Err(ModelError::UnverifiedIdentity) => "unverified",
                Err(ModelError::IdentityMismatch { subject, .. }) => subject,
                Err(other) => panic!("{other}"),
            };
            assert_eq!(answer, expect, "{run_id} driven by {invocation:?}");
        }
        Step::AppendOrderEvent { run_id, order } => store
            .append_order_event(run_id, &cases.order(order))
            .expect("append order event"),
        Step::ListOrderEvents {
            run_id,
            limit,
            expect,
        } => {
            let found = store
                .list_order_events(run_id, *limit)
                .expect("list order events");
            let expected: Vec<ExchangeOrder> =
                expect.iter().map(|name| cases.order(name)).collect();
            same(&found, &expected, &format!("order events of {run_id}"));
        }
        Step::AppendClosedTrade { run_id, trade } => store
            .append_closed_trade(run_id, &cases.trade(trade))
            .expect("append closed trade"),
        Step::ListClosedTrades {
            run_id,
            limit,
            expect,
            refused,
        } => {
            let outcome = store.list_closed_trades(run_id, *limit);
            let what = format!("closed trades of {run_id}");
            match (expect, refused) {
                (Some(expect), None) => {
                    let found = outcome.expect("list closed trades");
                    let expected: Vec<ClosedTrade> =
                        expect.iter().map(|name| cases.trade(name)).collect();
                    same(&found, &expected, &what);
                }
                (None, Some(refused)) => expect_refused(outcome, refused, &what),
                _ => panic!("a closed-trade listing expects trades or a refusal"),
            }
        }
        _ => return false,
    }
    true
}

fn spot() -> MarketId {
    MarketId::new(
        ExchangeId::BinanceSpot,
        Symbol::new("BTCUSDT").expect("symbol"),
        Interval::M1,
    )
}

fn run(run_id: &str, market: MarketId, mode: ExecutionMode) -> BotRunState {
    BotRunState {
        run_id: run_id.to_string(),
        market,
        strategy_name: "sma_cross".to_string(),
        strategy_config: serde_json::json!({"kind":"sma_cross","fast":20,"slow":50}),
        execution_mode: mode,
        status: RunStatus::Running,
        last_processed_open_time_ms: None,
        started_at_ms: 1_792_454_400_000,
        updated_at_ms: 1_792_454_400_000,
        stopped_at_ms: None,
        last_error: None,
        position: PositionState::flat(),
    }
}

/// Venue IDs of every shape the identity cases list round-trip as text
/// through the journals, with no number ever read or written.
fn venue_ids_are_text<S: RunJournalStore>(fixture: &Fixture<S>) {
    let store = &fixture.store;
    let cases = identity_cases();
    store
        .create_run(
            &run("run-ids", spot(), ExecutionMode::Live),
            &RunIdentity::recorded(None, None),
        )
        .expect("create");
    for id in &cases.venue_ids {
        let order = ExchangeOrder {
            symbol: Symbol::new("BTCUSDT").expect("symbol"),
            side: Side::Buy,
            order_type: "MARKET".to_string(),
            status: OrderStatus::Filled,
            order_id: Some(VenueOrderId::new(id).expect("order id")),
            client_order_id: None,
            requested_qty: None,
            requested_quote_qty: None,
            executed_qty: Some(Decimal::ONE),
            cumulative_quote_qty: Some(Decimal::ONE),
            avg_price: None,
            transact_time_ms: Some(1),
            fills: Some(vec![Fill {
                price: Decimal::ONE,
                qty: Decimal::ONE,
                commission: Decimal::ZERO,
                commission_asset: None,
                trade_id: Some(VenueTradeId::new(id).expect("trade id")),
            }]),
            raw: serde_json::json!({}),
        };
        store.append_order_event("run-ids", &order).expect("append");
        store
            .append_closed_trade(
                "run-ids",
                &ClosedTrade {
                    symbol: Symbol::new("BTCUSDT").expect("symbol"),
                    entry_time_ms: 1,
                    exit_time_ms: 2,
                    entry_price: Decimal::ONE,
                    exit_price: Decimal::ONE,
                    qty: Decimal::ONE,
                    gross_quote_pnl: Decimal::ZERO,
                    entry_order_id: Some(VenueOrderId::new(id).expect("order id")),
                    exit_order_id: None,
                },
            )
            .expect("append trade");
    }
    let events = store.list_order_events("run-ids", 100).expect("events");
    let mut ids: Vec<String> = events
        .iter()
        .map(|event| {
            event
                .order_id
                .as_ref()
                .expect("order id")
                .as_str()
                .to_string()
        })
        .collect();
    ids.sort();
    let mut expected = cases.venue_ids.clone();
    expected.sort();
    assert_eq!(ids, expected);
    for event in &events {
        let fills = event.fills.as_ref().expect("fills");
        assert_eq!(
            fills[0].trade_id.as_ref().map(VenueTradeId::as_str),
            event.order_id.as_ref().map(VenueOrderId::as_str)
        );
    }
    let trades = store.list_closed_trades("run-ids", 100).expect("trades");
    let mut entries: Vec<String> = trades
        .iter()
        .map(|trade| {
            trade
                .entry_order_id
                .as_ref()
                .expect("entry order id")
                .as_str()
                .to_string()
        })
        .collect();
    entries.sort();
    assert_eq!(entries, expected);
    assert!(trades.iter().all(|trade| trade.exit_order_id.is_none()));
    // The backend's own columns hold the text, typed as text.
    assert_eq!(
        fixture.raw.value_types("order_events", "order_id"),
        ["text"]
    );
    assert_eq!(
        fixture.raw.value_types("closed_trades", "entry_order_id"),
        ["text"]
    );
}

/// Runs recorded under two environments or two accounts never read as one
/// another's, and a run recorded under none is not driven once the store
/// is bound: nothing inherits a newly selected environment or key.
fn environments_and_accounts_are_isolated<S: RunJournalStore>(fixture: &Fixture<S>) {
    let store = &fixture.store;
    let market = spot();
    let key = |name: &str| VenueAccountId::KeyFingerprint(KeyFingerprint::of(name));
    let testnet = RunIdentity::recorded(Some(Environment::Testnet), Some(key("a")));
    let mainnet = RunIdentity::recorded(Some(Environment::Mainnet), Some(key("a")));
    let other_key = RunIdentity::recorded(Some(Environment::Testnet), Some(key("b")));
    let unbound = RunIdentity::recorded(None, None);
    for (run_id, identity) in [
        ("run-testnet", &testnet),
        ("run-mainnet", &mainnet),
        ("run-other-key", &other_key),
        ("run-unbound", &unbound),
    ] {
        store
            .create_run(&run(run_id, market.clone(), ExecutionMode::Live), identity)
            .expect("create");
        assert_eq!(
            store.run_identity(run_id).expect("identity").as_ref(),
            Some(identity)
        );
    }
    let recorded = |run_id: &str| store.run_identity(run_id).expect("identity").expect("some");
    assert!(recorded("run-testnet").matches(&testnet).is_ok());
    assert!(recorded("run-testnet").matches(&mainnet).is_err());
    assert!(recorded("run-mainnet").matches(&testnet).is_err());
    assert!(recorded("run-testnet").matches(&other_key).is_err());
    assert!(recorded("run-other-key").matches(&testnet).is_err());
    assert!(recorded("run-unbound").matches(&unbound).is_ok());
    // Binding the store later never adopts the run by itself.
    let bound_later = RunIdentity::recorded(Some(Environment::Testnet), None);
    assert!(recorded("run-unbound").matches(&bound_later).is_err());
    // A legacy identity is the migration's alone, and drives nothing.
    let error = store
        .create_run(
            &run("run-legacy", market.clone(), ExecutionMode::Live),
            &RunIdentity::legacy(),
        )
        .expect_err("a new run is never legacy");
    assert!(
        error
            .to_string()
            .contains("only a migration marks runs legacy"),
        "{error}"
    );
    assert!(RunIdentity::legacy().matches(&unbound).is_err());
    assert!(RunIdentity::legacy().matches(&bound_later).is_err());
    // The latest run for the market is still found: the refusal is explicit,
    // after the pick, never a silent switch to another run.
    let latest = store
        .latest_run_for_market(&market, "sma_cross")
        .expect("latest")
        .expect("a run");
    assert_eq!(recorded(&latest.run_id).source, IdentitySource::Recorded);
}
