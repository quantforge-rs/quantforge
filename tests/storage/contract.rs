//! The checks every backend passes, written against the ports and a reader
//! of raw column text, so that the PostgreSQL store runs them unchanged.
//! `sqlite.rs` runs them today.

use quantforge::{
    BotRunState, Candle, CandleQuery, CandleStore, Category, ClosedTrade, Environment, ExchangeId,
    ExchangeOrder, ExecutionMode, Fill, IdentitySource, Interval, KeyFingerprint, MarketId,
    OrderStatus, PositionState, RunIdentity, RunJournalStore, RunStatus, Side, Symbol, Venue,
    VenueAccountId, VenueOrderId, VenueTradeId,
};
use rust_decimal::Decimal;
use std::str::FromStr;

use crate::cases;

/// Reads one text column of a table for a market, ordered by open time:
/// the backend's own view of what it stored.
pub type RawTexts<'a> = &'a dyn Fn(&MarketId, &str) -> Vec<String>;

fn venue(name: &str) -> Venue {
    Venue::ALL
        .into_iter()
        .find(|venue| venue.as_str() == name)
        .unwrap_or_else(|| panic!("unknown venue {name}"))
}

fn category(name: &str) -> Category {
    Category::ALL
        .into_iter()
        .find(|category| category.as_str() == name)
        .unwrap_or_else(|| panic!("unknown category {name}"))
}

fn candle(open_time_ms: i64, close: &str) -> Candle {
    let close = Decimal::from_str(close).expect("decimal");
    Candle {
        open_time_ms,
        close_time_ms: open_time_ms + 59_999,
        open: close,
        high: close,
        low: close,
        close,
        volume: Decimal::from_str("1.000").expect("decimal"),
        trades: Some(1),
    }
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

/// Every product line of the cases stores and returns its market under the
/// case's storage spelling, keeps the same symbol on another line apart,
/// and spells itself on the wire and in `state_json` as the case says.
pub fn product_lines_are_stored_as_the_cases_say<S>(store: &S, raw: RawTexts<'_>)
where
    S: CandleStore + ?Sized,
{
    let cases = cases::load();
    assert_eq!(cases.markets.len(), 6, "one case per product line");
    for (index, case) in cases.markets.iter().enumerate() {
        let line = ExchangeId::from_parts(venue(&case.venue), category(&case.category));
        assert_eq!(line.as_str(), case.storage.exchange, "{case:?}");
        assert_eq!(line.venue().as_str(), case.wire.exchange);
        assert_eq!(line.category().as_str(), case.wire.category);
        assert_eq!(
            case.wire.symbol, case.storage.symbol,
            "one spelling of a symbol"
        );
        assert_eq!(
            case.wire.interval, case.storage.interval,
            "one spelling of an interval"
        );
        assert_eq!(
            serde_json::to_value(line).expect("token"),
            serde_json::Value::String(case.state_json_exchange.clone())
        );
        let market = MarketId::new(
            line,
            Symbol::new(&case.symbol).expect("symbol"),
            Interval::from_str(&case.interval).expect("interval"),
        );
        assert_eq!(market.symbol.as_str(), case.storage.symbol);
        assert_eq!(market.interval.as_str(), case.storage.interval);
        // The same open time on every line, with a close only this line has.
        let close = format!("{}.{index}0", 100 + index);
        store
            .upsert_candles(&market, &[candle(1_792_454_400_000, &close)])
            .expect("upsert");
        assert_eq!(raw(&market, "close"), vec![close]);
    }
    // Each line reads back only its own candle.
    for (index, case) in cases.markets.iter().enumerate() {
        let market = MarketId::new(
            ExchangeId::from_storage_name(&case.storage.exchange).expect("storage name"),
            Symbol::new(&case.storage.symbol).expect("symbol"),
            Interval::from_str(&case.storage.interval).expect("interval"),
        );
        let loaded = store
            .load_candles(&market, CandleQuery::default())
            .expect("load");
        assert_eq!(loaded.len(), 1, "{case:?}");
        assert_eq!(
            loaded[0].close.to_string(),
            format!("{}.{index}0", 100 + index)
        );
    }
}

/// Every decimal text of the cases is stored and returned with its scale.
pub fn decimal_texts_keep_their_scale<S>(store: &S, raw: RawTexts<'_>)
where
    S: CandleStore + ?Sized,
{
    let cases = cases::load();
    let market = MarketId::new(
        ExchangeId::BybitInverse,
        Symbol::new("BTCUSD").expect("symbol"),
        Interval::M1,
    );
    let candles: Vec<Candle> = cases
        .decimals
        .iter()
        .enumerate()
        .map(|(index, text)| candle(1_792_454_400_000 + index as i64 * 60_000, text))
        .collect();
    store.upsert_candles(&market, &candles).expect("upsert");
    assert_eq!(raw(&market, "close"), cases.decimals, "the stored text");
    let loaded = store
        .load_candles(&market, CandleQuery::default())
        .expect("load");
    let texts: Vec<String> = loaded
        .iter()
        .map(|candle| candle.close.to_string())
        .collect();
    assert_eq!(texts, cases.decimals, "the text read back");
}

/// Venue IDs of every shape the cases list round-trip as text through the
/// journals, with no number ever read or written.
pub fn venue_ids_are_text<S>(store: &S, raw_event_ids: &dyn Fn() -> Vec<(String, String)>)
where
    S: RunJournalStore + ?Sized,
{
    let cases = cases::load();
    let market = MarketId::new(
        ExchangeId::BinanceSpot,
        Symbol::new("BTCUSDT").expect("symbol"),
        Interval::M1,
    );
    store
        .create_run(
            &run("run-ids", market, ExecutionMode::Live),
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
    assert!(trades.iter().all(|trade| trade.exit_order_id.is_none()));
    // The backend's own columns hold the text, typed as text.
    for (id, kind) in raw_event_ids() {
        assert!(cases.venue_ids.contains(&id), "{id}");
        assert_eq!(kind, "text", "{id}");
    }
}

/// Runs recorded under two environments or two accounts never read as one
/// another's, and a run recorded under none is not driven once the store
/// is bound: nothing inherits a newly selected environment or key.
pub fn environments_and_accounts_are_isolated<S>(store: &S)
where
    S: RunJournalStore + ?Sized,
{
    let market = MarketId::new(
        ExchangeId::BinanceSpot,
        Symbol::new("BTCUSDT").expect("symbol"),
        Interval::M1,
    );
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
