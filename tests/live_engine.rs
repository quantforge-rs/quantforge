//! Black-box tests for the live trading engine, driven through its public
//! API with hand-written port fakes. Moved out of `src/engine/live/` so no
//! module there carries a test block larger than the code it covers; every
//! symbol used here is part of the crate's public surface.

use quantforge::{
    AccountTrade, AssetBalance, BotRunState, BuiltInStrategyConfig, CancelOrderRequest, Candle,
    CandleStore, ClosedTrade, EngineError, ExchangeError, ExchangeId, ExchangeOrder, ExecutionMode,
    Interval, KlineRequest, LiveTradeConfig, LiveTradeEngine, LiveTradeSummary, MarketDataSource,
    MarketId, MarketOrderRequest, OrderQueryRequest, OrderStatus, PositionState, RunJournalStore,
    RunStatus, Side, SqliteStore, StorageError, Symbol, SymbolRules, TimestampMs, TradingVenue,
    now_utc_ms,
};
use rust_decimal::Decimal;
use std::collections::VecDeque;
use std::str::FromStr;
use std::sync::Mutex;
use std::time::Duration;
use tempfile::tempdir;

// Fixtures duplicated from the unit tests that stayed with the private
// helpers in `src/engine/live/execution.rs`.
fn market() -> MarketId {
    MarketId::new(
        ExchangeId::BinanceSpot,
        Symbol::new("BTCUSDT").expect("symbol"),
        Interval::M1,
    )
}

fn rules() -> SymbolRules {
    SymbolRules {
        symbol: Symbol::new("BTCUSDT").expect("symbol"),
        base_asset: "BTC".to_string(),
        quote_asset: "USDT".to_string(),
        min_qty: Some(Decimal::from_str("0.001").expect("decimal")),
        max_qty: None,
        step_size: Some(Decimal::from_str("0.001").expect("decimal")),
        market_min_qty: Some(Decimal::from_str("0.001").expect("decimal")),
        market_max_qty: None,
        market_step_size: Some(Decimal::from_str("0.001").expect("decimal")),
        min_notional: Some(Decimal::from(10)),
        tick_size: Some(Decimal::from_str("0.01").expect("decimal")),
    }
}

// ── Execution-mode safety: engine-level behavior ─────────────────────
//
// The structural dry-run has two layers. The CLI wires trading_venue:
// None for dry-run (handle_trade_run), so a dry-run process cannot even
// hold an order endpoint. The tests below prove the second, engine-side
// layer: even when a venue IS present, the ExecutionMode::DryRun arm of
// execute_target never touches it (RefusingVenue panics on any call).
// Dry-run fills are synthesized from the reference bar close and
// journaled like real orders ("execution_mode": "dry_run" in raw,
// qf-dry- client ids), and dry-run exits sell the journaled position
// quantity without reading balances. Live mode requires a venue at the
// moment an order is due (InvalidConfig, run journaled as Failed),
// sizes entries by quote_order_qty without a balance read, and records
// the position from the venue-reported fill, not the reference bar.
// The min-notional entry check runs before the mode branch, in both
// modes. CLI-side gates (dry-run default, --yes confirmation,
// credential requirements, PRODUCTION marking) are covered black-box in
// tests/cli.rs.

// Market data for engine tests: one candle batch is consumed per
// fetch_klines call (a drained queue yields an empty batch, which ends
// a sync loop); symbol rules are served for real because
// LiveTradeEngine::run fetches them in both execution modes.
struct ScriptedMarketData {
    batches: Mutex<VecDeque<Vec<Candle>>>,
}

impl ScriptedMarketData {
    fn new(batches: Vec<Vec<Candle>>) -> Self {
        Self {
            batches: Mutex::new(batches.into_iter().collect()),
        }
    }
}

#[async_trait::async_trait]
impl MarketDataSource for ScriptedMarketData {
    fn exchange_id(&self) -> ExchangeId {
        ExchangeId::BinanceSpot
    }

    async fn fetch_klines(&self, _request: &KlineRequest) -> Result<Vec<Candle>, ExchangeError> {
        Ok(self
            .batches
            .lock()
            .expect("lock")
            .pop_front()
            .unwrap_or_default())
    }

    async fn fetch_symbol_rules(&self, _symbol: &Symbol) -> Result<SymbolRules, ExchangeError> {
        Ok(rules())
    }
}

// A venue whose every method panics: passed as Some(&RefusingVenue) to
// prove the DryRun arm of execute_target structurally never reaches the
// venue, and that live mode leaves it untouched while no order is due.
struct RefusingVenue;

#[async_trait::async_trait]
impl TradingVenue for RefusingVenue {
    fn exchange_id(&self) -> ExchangeId {
        ExchangeId::BinanceSpot
    }

    async fn account_balances(&self) -> Result<Vec<AssetBalance>, ExchangeError> {
        unreachable!("dry-run must never call the trading venue")
    }

    async fn open_orders(
        &self,
        _symbol: Option<&Symbol>,
    ) -> Result<Vec<ExchangeOrder>, ExchangeError> {
        unreachable!("dry-run must never call the trading venue")
    }

    async fn recent_trades(
        &self,
        _symbol: &Symbol,
        _limit: usize,
    ) -> Result<Vec<AccountTrade>, ExchangeError> {
        unreachable!("dry-run must never call the trading venue")
    }

    async fn submit_market_order(
        &self,
        _request: &MarketOrderRequest,
    ) -> Result<ExchangeOrder, ExchangeError> {
        unreachable!("dry-run must never call the trading venue")
    }

    async fn cancel_order(
        &self,
        _request: &CancelOrderRequest,
    ) -> Result<ExchangeOrder, ExchangeError> {
        unreachable!("dry-run must never call the trading venue")
    }

    async fn query_order(
        &self,
        _request: &OrderQueryRequest,
    ) -> Result<ExchangeOrder, ExchangeError> {
        unreachable!("dry-run must never call the trading venue")
    }
}

// Records every submitted market order and answers with one canned
// fill. account_balances is unreachable on purpose: live entries size
// by quote_order_qty and must not read balances.
struct ScriptedVenue {
    submitted: Mutex<Vec<MarketOrderRequest>>,
    fill: ExchangeOrder,
}

#[async_trait::async_trait]
impl TradingVenue for ScriptedVenue {
    fn exchange_id(&self) -> ExchangeId {
        ExchangeId::BinanceSpot
    }

    async fn account_balances(&self) -> Result<Vec<AssetBalance>, ExchangeError> {
        unreachable!("live entries size by quote_order_qty and never read balances")
    }

    async fn open_orders(
        &self,
        _symbol: Option<&Symbol>,
    ) -> Result<Vec<ExchangeOrder>, ExchangeError> {
        unreachable!("the trade engine submits market orders only")
    }

    async fn recent_trades(
        &self,
        _symbol: &Symbol,
        _limit: usize,
    ) -> Result<Vec<AccountTrade>, ExchangeError> {
        unreachable!("the trade engine submits market orders only")
    }

    async fn submit_market_order(
        &self,
        request: &MarketOrderRequest,
    ) -> Result<ExchangeOrder, ExchangeError> {
        self.submitted.lock().expect("lock").push(request.clone());
        Ok(self.fill.clone())
    }

    async fn cancel_order(
        &self,
        _request: &CancelOrderRequest,
    ) -> Result<ExchangeOrder, ExchangeError> {
        unreachable!("the trade engine submits market orders only")
    }

    async fn query_order(
        &self,
        _request: &OrderQueryRequest,
    ) -> Result<ExchangeOrder, ExchangeError> {
        unreachable!("the trade engine submits market orders only")
    }
}

// Wraps the real store so order-event appends fail while everything
// else delegates; every saved (status, position qty) snapshot is
// recorded so tests can assert the position-carrying save happened
// BEFORE the deferred journaling error propagated — the ordering that
// keeps a restart from doubling exposure.
struct FailingOrderJournal<'a> {
    inner: &'a SqliteStore,
    saves: Mutex<Vec<(RunStatus, Decimal)>>,
}

impl RunJournalStore for FailingOrderJournal<'_> {
    fn init(&self) -> Result<(), StorageError> {
        RunJournalStore::init(self.inner)
    }

    fn save_run_state(&self, state: &BotRunState) -> Result<(), StorageError> {
        self.saves
            .lock()
            .expect("lock")
            .push((state.status, state.position.qty));
        self.inner.save_run_state(state)
    }

    fn load_run_state(&self, run_id: &str) -> Result<Option<BotRunState>, StorageError> {
        self.inner.load_run_state(run_id)
    }

    fn latest_run_for_market(
        &self,
        market: &MarketId,
        strategy_name: &str,
    ) -> Result<Option<BotRunState>, StorageError> {
        self.inner.latest_run_for_market(market, strategy_name)
    }

    fn append_order_event(
        &self,
        _run_id: &str,
        _order: &ExchangeOrder,
    ) -> Result<(), StorageError> {
        Err(StorageError::InvalidArgument(
            "scripted order-journal failure".to_string(),
        ))
    }

    fn append_closed_trade(&self, run_id: &str, trade: &ClosedTrade) -> Result<(), StorageError> {
        self.inner.append_closed_trade(run_id, trade)
    }

    fn list_order_events(
        &self,
        run_id: &str,
        limit: usize,
    ) -> Result<Vec<ExchangeOrder>, StorageError> {
        self.inner.list_order_events(run_id, limit)
    }

    fn list_closed_trades(
        &self,
        run_id: &str,
        limit: usize,
    ) -> Result<Vec<ClosedTrade>, StorageError> {
        self.inner.list_closed_trades(run_id, limit)
    }
}

// The live engine reads the wall clock internally (bootstrap window,
// poll range, closed-bar filter), so a fixed epoch anchor would leave
// every fixture bar either unclosed or centuries stale. Fixtures anchor
// to the current minute, offset far enough into the past that every bar
// stays closed at every clock read for any test runtime; all offsets
// are exact multiples of the interval step, so everything except the
// anchor itself is deterministic.
fn anchor_ms() -> TimestampMs {
    now_utc_ms() / 60_000 * 60_000 - 10 * 60_000
}

fn engine_bar(open_time_ms: TimestampMs, close: Decimal) -> Candle {
    Candle {
        open_time_ms,
        close_time_ms: open_time_ms + 59_999,
        open: close,
        high: close,
        low: close,
        close,
        volume: Decimal::ONE,
        trades: Some(1),
    }
}

// Shared engine-test choreography. Warm-up bars are pre-seeded straight
// into the store (the bootstrap replay reads the store, not the
// source), and the scripted source serves [empty, vec![poll_bar]]: the
// bootstrap sync pops the empty batch and stops without touching the
// store, the poll sync delivers the signal bar, and the then-drained
// queue ends the poll sync. The strategy therefore sees every bar
// exactly once. Warm-up closes are equal — signal-silent under
// fast=1/slow=2 — so the poll bar's close relative to 10_000 is the
// entire signal script.
fn timeline(poll_close: Decimal) -> (TimestampMs, Vec<Candle>, Candle) {
    let anchor = anchor_ms();
    let warmup = vec![
        engine_bar(anchor, Decimal::from(10_000)),
        engine_bar(anchor + 60_000, Decimal::from(10_000)),
    ];
    let poll_bar = engine_bar(anchor + 120_000, poll_close);

    // Guard against pathological clock skew at fixture-build time
    // instead of debugging a silent zero-bar run.
    let now = now_utc_ms();
    assert!(
        poll_bar.close_time_ms + 60_000 <= now,
        "fixture bars must be closed with >= 60s margin; clock skew detected"
    );

    (anchor, warmup, poll_bar)
}

fn seeded_store(dir: &tempfile::TempDir, warmup: &[Candle]) -> SqliteStore {
    let store = SqliteStore::new(dir.path().join("live.sqlite"));
    CandleStore::init(&store).expect("init");
    store
        .upsert_candles(&market(), warmup)
        .expect("seed warmup");
    store
}

fn engine_config(execution_mode: ExecutionMode, run_id: &str) -> LiveTradeConfig {
    LiveTradeConfig {
        market: market(),
        // fast=1/slow=2 keeps the script tiny: a bar's signal is its
        // close vs the previous close, and equal closes emit nothing.
        strategy: BuiltInStrategyConfig::SmaCross { fast: 1, slow: 2 },
        execution_mode,
        quote_order_qty: Decimal::from(100),
        poll_interval: Duration::from_millis(1),
        bootstrap_bars: 10,
        bootstrap_enter: false,
        batch_limit: 1000,
        run_id: Some(run_id.to_string()),
        max_loops: Some(1),
    }
}

// The open position and processed-bar cursor a resumed dry-run exits
// from. Built from the config itself so apply_resume_checks always sees
// an identical strategy config shape.
fn seeded_open_run_state(cfg: &LiveTradeConfig, anchor: TimestampMs) -> BotRunState {
    BotRunState {
        run_id: cfg.run_id.clone().expect("run id"),
        market: cfg.market.clone(),
        strategy_name: cfg.strategy.strategy_name().to_string(),
        strategy_config: serde_json::to_value(&cfg.strategy).expect("strategy config"),
        execution_mode: cfg.execution_mode,
        status: RunStatus::Running,
        last_processed_open_time_ms: Some(anchor + 60_000),
        started_at_ms: anchor,
        updated_at_ms: anchor + 60_000,
        stopped_at_ms: None,
        last_error: None,
        position: PositionState {
            qty: Decimal::from_str("0.010").expect("decimal"),
            entry_price: Some(Decimal::from(10_000)),
            entry_time_ms: Some(anchor + 59_999),
            entry_order_id: None,
        },
    }
}

#[tokio::test]
async fn dry_run_entry_journals_synthetic_order_and_never_calls_the_trading_venue() {
    let (_anchor, warmup, poll_bar) = timeline(Decimal::from(12_500));
    let tempdir = tempdir().expect("tempdir");
    let store = seeded_store(&tempdir, &warmup);
    let source = ScriptedMarketData::new(vec![Vec::new(), vec![poll_bar.clone()]]);
    let venue = RefusingVenue;

    let engine = LiveTradeEngine::new(&source, &store, &store, Some(&venue));
    let summary = engine
        .run(&engine_config(ExecutionMode::DryRun, "run-dry-entry"))
        .await
        .expect("dry run");

    assert_eq!(
        summary,
        LiveTradeSummary {
            run_id: "run-dry-entry".to_string(),
            processed_bars: 1,
            submitted_orders: 1,
            closed_trades: 0,
            last_processed_open_time_ms: Some(poll_bar.open_time_ms),
        }
    );

    let state = store
        .load_run_state("run-dry-entry")
        .expect("load state")
        .expect("state");
    assert_eq!(state.status, RunStatus::Stopped);
    assert_eq!(
        state.position.qty,
        Decimal::from_str("0.008").expect("decimal")
    );
    assert_eq!(state.position.entry_price, Some(Decimal::from(12_500)));
    assert_eq!(state.position.entry_time_ms, Some(poll_bar.close_time_ms));
    assert_eq!(state.position.entry_order_id, None);

    let events = store
        .list_order_events("run-dry-entry", 10)
        .expect("events");
    assert_eq!(events.len(), 1);
    let order = &events[0];
    assert_eq!(order.side, Side::Buy);
    assert_eq!(order.status, OrderStatus::Filled);
    assert_eq!(order.order_id, None);
    assert_eq!(order.requested_quote_qty, Some(Decimal::from(100)));
    assert_eq!(
        order.executed_qty,
        Some(Decimal::from_str("0.008").expect("decimal"))
    );
    assert_eq!(order.fills, Some(Vec::new()));
    // Pin the reported fill price itself: average_price() would derive
    // the same value from cumulative/executed even if avg_price were
    // dropped, so downstream math alone cannot detect that regression.
    assert_eq!(order.avg_price, Some(Decimal::from(12_500)));
    assert_eq!(order.raw["execution_mode"], "dry_run");
    assert_eq!(order.raw["reference_open_time_ms"], poll_bar.open_time_ms);
    let client_id = order.client_order_id.as_deref().expect("client id");
    assert!(client_id.starts_with("qf-dry-"), "got {client_id}");
    assert!(client_id.len() <= 36, "got {client_id}");
}

#[tokio::test]
async fn dry_run_exit_on_resume_sells_journaled_qty_and_writes_closed_trade_without_venue_calls() {
    let (anchor, warmup, poll_bar) = timeline(Decimal::from(9_000));
    let tempdir = tempdir().expect("tempdir");
    let store = seeded_store(&tempdir, &warmup);
    let cfg = engine_config(ExecutionMode::DryRun, "run-dry-exit");
    store
        .save_run_state(&seeded_open_run_state(&cfg, anchor))
        .expect("seed state");
    let source = ScriptedMarketData::new(vec![Vec::new(), vec![poll_bar.clone()]]);
    let venue = RefusingVenue;

    let engine = LiveTradeEngine::new(&source, &store, &store, Some(&venue));
    let summary = engine.run(&cfg).await.expect("dry run");

    assert_eq!(
        summary,
        LiveTradeSummary {
            run_id: "run-dry-exit".to_string(),
            processed_bars: 1,
            submitted_orders: 1,
            closed_trades: 1,
            last_processed_open_time_ms: Some(poll_bar.open_time_ms),
        }
    );

    let state = store
        .load_run_state("run-dry-exit")
        .expect("load state")
        .expect("state");
    assert_eq!(state.status, RunStatus::Stopped);
    assert_eq!(state.position, PositionState::flat());

    let events = store.list_order_events("run-dry-exit", 10).expect("events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].side, Side::Sell);
    assert_eq!(
        events[0].requested_qty,
        Some(Decimal::from_str("0.010").expect("decimal"))
    );
    assert_eq!(
        events[0].executed_qty,
        Some(Decimal::from_str("0.010").expect("decimal"))
    );
    assert_eq!(events[0].avg_price, Some(Decimal::from(9_000)));
    assert_eq!(events[0].raw["execution_mode"], "dry_run");
    let client_id = events[0].client_order_id.as_deref().expect("client id");
    assert!(client_id.starts_with("qf-dry-"), "got {client_id}");

    let trades = store
        .list_closed_trades("run-dry-exit", 10)
        .expect("trades");
    assert_eq!(
        trades,
        vec![ClosedTrade {
            symbol: Symbol::new("BTCUSDT").expect("symbol"),
            entry_time_ms: anchor + 59_999,
            exit_time_ms: poll_bar.close_time_ms,
            entry_price: Decimal::from(10_000),
            exit_price: Decimal::from(9_000),
            qty: Decimal::from_str("0.010").expect("decimal"),
            gross_quote_pnl: Decimal::from(-10),
            entry_order_id: None,
            exit_order_id: None,
        }]
    );
}

#[tokio::test]
async fn live_mode_without_a_venue_fails_the_run_when_an_order_is_due() {
    let (anchor, warmup, poll_bar) = timeline(Decimal::from(12_500));
    let tempdir = tempdir().expect("tempdir");
    let store = seeded_store(&tempdir, &warmup);
    let source = ScriptedMarketData::new(vec![Vec::new(), vec![poll_bar]]);

    let engine = LiveTradeEngine::new(&source, &store, &store, None);
    let error = engine
        .run(&engine_config(ExecutionMode::Live, "run-live-none"))
        .await
        .expect_err("missing venue");

    assert!(matches!(error, EngineError::InvalidConfig(_)));
    assert!(
        error
            .to_string()
            .contains("live mode requires a trading venue"),
        "got {error}"
    );

    let state = store
        .load_run_state("run-live-none")
        .expect("load state")
        .expect("state");
    assert_eq!(state.status, RunStatus::Failed);
    assert!(
        state
            .last_error
            .as_deref()
            .expect("last error")
            .contains("live mode requires a trading venue")
    );
    assert_eq!(state.position, PositionState::flat());
    // The failing bar was never watermarked: a restart reprocesses it.
    assert_eq!(state.last_processed_open_time_ms, Some(anchor + 60_000));
    assert!(
        store
            .list_order_events("run-live-none", 10)
            .expect("events")
            .is_empty()
    );
}

#[tokio::test]
async fn live_entry_submits_one_market_order_and_records_venue_reported_quantity() {
    let (_anchor, warmup, poll_bar) = timeline(Decimal::from(12_500));
    let tempdir = tempdir().expect("tempdir");
    let store = seeded_store(&tempdir, &warmup);
    let source = ScriptedMarketData::new(vec![Vec::new(), vec![poll_bar.clone()]]);
    // Every canned-fill field differs from what a synthetic fill would
    // fabricate (qty 0.007 vs 0.008, price 88.2/0.007 = 12_600 vs the
    // 12_500 bar close, timestamp close_time + 1), so the position
    // asserts prove the venue-reported data won.
    let venue = ScriptedVenue {
        submitted: Mutex::new(Vec::new()),
        fill: ExchangeOrder {
            symbol: Symbol::new("BTCUSDT").expect("symbol"),
            side: Side::Buy,
            order_type: "MARKET".to_string(),
            status: OrderStatus::Filled,
            order_id: Some(42),
            client_order_id: Some("venue-echo".to_string()),
            requested_qty: None,
            requested_quote_qty: Some(Decimal::from(100)),
            executed_qty: Some(Decimal::from_str("0.007").expect("decimal")),
            cumulative_quote_qty: Some(Decimal::from_str("88.2").expect("decimal")),
            avg_price: None,
            transact_time_ms: Some(poll_bar.close_time_ms + 1),
            fills: Some(Vec::new()),
            raw: serde_json::json!({}),
        },
    };

    let engine = LiveTradeEngine::new(&source, &store, &store, Some(&venue));
    let summary = engine
        .run(&engine_config(ExecutionMode::Live, "run-live-1"))
        .await
        .expect("live run");
    assert_eq!(summary.submitted_orders, 1);
    assert_eq!(summary.closed_trades, 0);

    let submitted = venue.submitted.lock().expect("lock");
    assert_eq!(submitted.len(), 1);
    let request = &submitted[0];
    assert_eq!(request.side, Side::Buy);
    assert_eq!(request.quantity, None);
    assert_eq!(request.quote_order_qty, Some(Decimal::from(100)));
    assert_eq!(request.symbol.as_str(), "BTCUSDT");
    let client_id = request.new_client_order_id.as_deref().expect("client id");
    assert!(client_id.starts_with("qf-entry-"), "got {client_id}");
    assert!(client_id.len() <= 36, "got {client_id}");
    drop(submitted);

    let state = store
        .load_run_state("run-live-1")
        .expect("load state")
        .expect("state");
    assert_eq!(
        state.position.qty,
        Decimal::from_str("0.007").expect("decimal")
    );
    assert_eq!(state.position.entry_price, Some(Decimal::from(12_600)));
    assert_eq!(state.position.entry_order_id, Some(42));
    assert_eq!(
        state.position.entry_time_ms,
        Some(poll_bar.close_time_ms + 1)
    );

    let events = store.list_order_events("run-live-1", 10).expect("events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].order_id, Some(42));
}

#[tokio::test]
async fn live_mode_with_no_target_change_never_touches_the_venue() {
    let (_anchor, warmup, poll_bar) = timeline(Decimal::from(10_000));
    let tempdir = tempdir().expect("tempdir");
    let store = seeded_store(&tempdir, &warmup);
    let source = ScriptedMarketData::new(vec![Vec::new(), vec![poll_bar.clone()]]);
    let venue = RefusingVenue;

    let engine = LiveTradeEngine::new(&source, &store, &store, Some(&venue));
    let summary = engine
        .run(&engine_config(ExecutionMode::Live, "run-live-idle"))
        .await
        .expect("live run");

    assert_eq!(
        summary,
        LiveTradeSummary {
            run_id: "run-live-idle".to_string(),
            processed_bars: 1,
            submitted_orders: 0,
            closed_trades: 0,
            last_processed_open_time_ms: Some(poll_bar.open_time_ms),
        }
    );

    let state = store
        .load_run_state("run-live-idle")
        .expect("load state")
        .expect("state");
    assert_eq!(state.status, RunStatus::Stopped);
    assert_eq!(state.position, PositionState::flat());
    assert!(
        store
            .list_order_events("run-live-idle", 10)
            .expect("events")
            .is_empty()
    );
}

#[tokio::test]
async fn dry_run_entry_persists_the_position_before_surfacing_a_journal_failure() {
    let (_anchor, warmup, poll_bar) = timeline(Decimal::from(12_500));
    let tempdir = tempdir().expect("tempdir");
    let store = seeded_store(&tempdir, &warmup);
    let journal = FailingOrderJournal {
        inner: &store,
        saves: Mutex::new(Vec::new()),
    };
    let source = ScriptedMarketData::new(vec![Vec::new(), vec![poll_bar]]);
    let venue = RefusingVenue;

    let engine = LiveTradeEngine::new(&source, &store, &journal, Some(&venue));
    let error = engine
        .run(&engine_config(ExecutionMode::DryRun, "run-journal-fail"))
        .await
        .expect_err("journal failure");

    assert!(matches!(error, EngineError::InvalidState(_)));
    assert!(
        error
            .to_string()
            .contains("journaling the order event failed"),
        "got {error}"
    );

    // The executed entry reached the journal with its position and
    // Running status BEFORE the deferred journaling error propagated:
    // a restart sees the exposure instead of doubling it.
    let entry_qty = Decimal::from_str("0.008").expect("decimal");
    let saves = journal.saves.lock().expect("lock");
    assert!(
        saves.contains(&(RunStatus::Running, entry_qty)),
        "got {saves:?}"
    );
    drop(saves);

    let state = store
        .load_run_state("run-journal-fail")
        .expect("load state")
        .expect("state");
    assert_eq!(state.status, RunStatus::Failed);
    assert_eq!(state.position.qty, entry_qty);
    assert!(
        state
            .last_error
            .as_deref()
            .expect("last error")
            .contains("journaling the order event failed")
    );
}
