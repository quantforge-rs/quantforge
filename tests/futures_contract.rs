//! The futures contract end to end, against a hand-written fake venue: every
//! port can be implemented outside the crate and handed out as a trait
//! object, and the records move only on the venue's evidence. An
//! acknowledgement never fills an intent, a "not found" proves nothing
//! before ADR 0002 §9's propagation delay, and only a read-back of a
//! covering stop protects a position. Nothing here talks to a venue.

use std::str::FromStr;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use quantforge::model::futures::{
    AccountKind, AccountSnapshot, ActivationEpoch, CancelAttribution, ClientOrderId, CloseOrder,
    CloseReason, ContractKind, CoveredStop, EntryIntent, EntryOrder, ExitIntent, Exposure,
    ExternalIntent, FeeRates, FuturesFill, FuturesInstrument, FuturesOrder, FuturesOrderStatus,
    InstrumentSettings, InstrumentSpec, IntentKind, IntentState, LedgerRecord, LeverageBracket,
    Liquidation, LiquidationGap, MarginMode, NotPlacedEvidence, OperatorRemoval, OrderAck,
    OrderAttempt, OrderLeg, OrderLookup, OrderLookupGuarantee, OrderOrigin, OwnerTag,
    PositionCycle, PositionCycleId, PositionKey, PositionMode, PositionSnapshot, PriceProtection,
    PriceSnapshot, PrivateEvent, ProfileId, ProtectionState, RecordSource, RecordedStop, SignalId,
    StopLookup, StopOrder, StopPlacement, StopRef, StopRequest, StopScope, StopStatus, TimeWindow,
    TradingStatus, TriggerBasis,
};
use quantforge::model::{
    AccountRef, Asset, Category, Environment, ExchangeId, InstrumentId, Leverage, Money,
    OperationId, PositionSide, Price, Quantity, Side, Symbol, Venue, VenueAccountId, VenueOrderId,
    VenueTradeId,
};
use quantforge::ports::futures::{
    AccountStateConnector, EntryConnector, FuturesCapabilities, FuturesConnector,
    FuturesConnectors, InstrumentConnector, LeverageConnector, OrderQueryConnector,
    PrivateEventConnector, ProtectionConnector, StopReplaceError, StopReplacement,
};
use quantforge::{ExchangeError, ExchangeErrorKind};
use rust_decimal::Decimal;

fn dec(text: &str) -> Decimal {
    Decimal::from_str(text).expect("decimal")
}

fn price(text: &str) -> Price {
    Price::new(dec(text)).expect("price")
}

fn base(text: &str) -> Quantity {
    Quantity::for_category(Category::Linear, dec(text)).expect("quantity")
}

fn usdt(text: &str) -> Money {
    Money::new(dec(text), Asset::new("USDT").expect("asset"))
}

fn instrument() -> InstrumentId {
    InstrumentId::new(
        ExchangeId::BybitLinear,
        Symbol::new("SOLUSDT").expect("symbol"),
        Asset::new("USDT").expect("asset"),
    )
}

fn key() -> PositionKey {
    let id = VenueAccountId::issued("71002233").expect("account id");
    let account = AccountRef::new(Venue::Bybit, Environment::Demo, id).expect("account");
    PositionKey::new(account, instrument()).expect("position key")
}

fn client_id(operation: u64, leg: OrderLeg, attempt: u16) -> ClientOrderId {
    ClientOrderId::new(
        OwnerTag::new("k3x9p2").expect("owner tag"),
        ActivationEpoch(1),
        OperationId::new(operation).expect("operation"),
        leg,
        attempt,
    )
    .expect("client order id")
}

/// SOLUSDT on a linear venue: 0.01 tick, 0.1 step, a 5 USDT minimum.
fn sol(contract: ContractKind) -> FuturesInstrument {
    FuturesInstrument::try_from(InstrumentSpec {
        id: instrument(),
        base: Asset::new("SOL").expect("asset"),
        quote: Asset::new("USDT").expect("asset"),
        contract,
        status: TradingStatus::Trading,
        delist_at_ms: None,
        inverse: None,
        tick: price("0.01"),
        step: base("0.1"),
        market_step: base("0.1"),
        min_qty: base("0.1"),
        market_min_qty: base("0.1"),
        market_max_qty: base("12000"),
        min_notional: Some(usdt("5")),
        leverage: None,
        observed_at_ms: 0,
    })
    .expect("instrument")
}

fn snapshot(exposure: Option<(PositionSide, &str)>, observed_at_ms: i64) -> PositionSnapshot {
    PositionSnapshot {
        instrument: instrument(),
        exposure: exposure.map(|(side, size)| Exposure {
            side,
            size: base(size),
            entry_price: price("234.23"),
        }),
        notional: None,
        leverage: Leverage::new(dec("5")).expect("leverage"),
        margin_mode: MarginMode::Isolated,
        mark_price: None,
        liquidation: Liquidation::Unavailable(LiquidationGap::ReportedEmpty),
        unrealized_pnl: None,
        margin: None,
        observed_at_ms,
    }
}

fn order(id: &ClientOrderId, status: FuturesOrderStatus, filled: &str) -> FuturesOrder {
    FuturesOrder {
        instrument: instrument(),
        venue_order_id: VenueOrderId::new("5501").expect("order id"),
        client_id: Some(id.to_string()),
        side: Side::Buy,
        reduce_only: false,
        origin: OrderOrigin::Client,
        status,
        requested: base("4.3"),
        filled: base(filled),
        average_price: None,
        source: RecordSource::Query,
        updated_at_ms: 1,
    }
}

fn fill(id: &ClientOrderId) -> FuturesFill {
    FuturesFill {
        trade_id: VenueTradeId::new("e-1").expect("trade id"),
        venue_order_id: VenueOrderId::new("5501").expect("order id"),
        client_id: Some(id.to_string()),
        instrument: instrument(),
        origin: OrderOrigin::Client,
        side: Side::Buy,
        price: price("234.25"),
        quantity: base("1.0"),
        fee: usdt("0.117125"),
        realized_pnl: None,
        time_ms: 2,
        source: RecordSource::Stream,
    }
}

/// An entry for 4.3 SOL long, admitted and sent once as operation 1 at
/// time 0.
fn sent_entry() -> (ExternalIntent, ClientOrderId) {
    let mut intent = ExternalIntent::new(IntentKind::Entry(EntryIntent {
        profile: ProfileId::new("main").expect("profile"),
        signal: SignalId::new("entry SOLUSDT.P long 1729584000000").expect("signal"),
        signal_time_ms: 0,
        key: key(),
        side: PositionSide::Long,
        expected_price: price("234.23"),
        stop: Some(price("200.00")),
    }));
    intent.move_to(IntentState::Admitted).expect("admitted");
    let id = client_id(1, OrderLeg::Entry, 1);
    let attempt = OrderAttempt {
        client_order_id: id.clone(),
        sent_at_ms: 0,
    };
    intent.record_attempt(attempt).expect("sent");
    (intent, id)
}

/// The stop QuantForge placed for the long position.
fn our_stop(status: StopStatus) -> StopOrder {
    StopOrder {
        stop: StopRef::Position {
            instrument: instrument(),
        },
        instrument: instrument(),
        position_side: PositionSide::Long,
        trigger_price: price("200.00"),
        basis: Some(TriggerBasis::Mark),
        price_protection: PriceProtection::NotApplicable,
        scope: StopScope::WholePosition,
        status,
        triggered_order: None,
        updated_at_ms: 3,
    }
}

/// A venue that answers from a script. Each port is the same object, as an
/// adapter's usually are.
struct FakeVenue {
    lookup: Mutex<Option<FuturesOrder>>,
    stops: Mutex<Vec<StopOrder>>,
    crossed: bool,
}

impl FakeVenue {
    fn new() -> Self {
        Self {
            lookup: Mutex::new(None),
            stops: Mutex::new(Vec::new()),
            crossed: false,
        }
    }

    fn unscripted() -> ExchangeError {
        ExchangeError::InvalidRequest {
            message: "not scripted".to_string(),
        }
    }
}

impl FuturesConnector for FakeVenue {
    fn exchange_id(&self) -> ExchangeId {
        ExchangeId::BybitLinear
    }

    fn environment(&self) -> Environment {
        Environment::Demo
    }

    fn capabilities(&self) -> FuturesCapabilities {
        let mut capabilities = FuturesCapabilities::default();
        capabilities.stop_replacement = StopReplacement::AmendInPlace;
        capabilities.order_lookup = OrderLookupGuarantee {
            visibility_verified: true,
            unfilled_retention_ms: 24 * 60 * 60 * 1000,
        };
        capabilities
    }
}

#[async_trait]
impl InstrumentConnector for FakeVenue {
    async fn instruments(&self) -> Result<Vec<FuturesInstrument>, ExchangeError> {
        Ok(vec![sol(ContractKind::Perpetual)])
    }

    async fn prices(&self, _instrument: &InstrumentId) -> Result<PriceSnapshot, ExchangeError> {
        Err(Self::unscripted())
    }
}

#[async_trait]
impl AccountStateConnector for FakeVenue {
    async fn account(&self) -> Result<AccountSnapshot, ExchangeError> {
        Ok(AccountSnapshot {
            account: key().account().clone(),
            kind: AccountKind::Standard,
            multi_asset_margin: None,
            wallets: Vec::new(),
            pools: Vec::new(),
            observed_at_ms: 0,
        })
    }

    async fn positions(&self) -> Result<Vec<PositionSnapshot>, ExchangeError> {
        Ok(Vec::new())
    }

    async fn position(
        &self,
        _instrument: &InstrumentId,
    ) -> Result<PositionSnapshot, ExchangeError> {
        Ok(snapshot(None, 0))
    }

    async fn instrument_settings(
        &self,
        instrument: &InstrumentId,
    ) -> Result<InstrumentSettings, ExchangeError> {
        Ok(InstrumentSettings {
            instrument: instrument.clone(),
            position_mode: PositionMode::OneWay,
            margin_mode: MarginMode::Isolated,
            leverage: Leverage::new(dec("5")).expect("leverage"),
        })
    }

    async fn leverage_brackets(
        &self,
        _instrument: &InstrumentId,
    ) -> Result<Vec<LeverageBracket>, ExchangeError> {
        Err(Self::unscripted())
    }

    async fn fee_rates(&self, _instrument: &InstrumentId) -> Result<FeeRates, ExchangeError> {
        Ok(FeeRates {
            maker: dec("0.0002"),
            taker: dec("0.00055"),
        })
    }
}

#[async_trait]
impl LeverageConnector for FakeVenue {
    async fn set_leverage(
        &self,
        _instrument: &InstrumentId,
        leverage: Leverage,
    ) -> Result<Leverage, ExchangeError> {
        Ok(leverage)
    }
}

#[async_trait]
impl EntryConnector for FakeVenue {
    async fn submit_entry(&self, order: &EntryOrder) -> Result<OrderAck, ExchangeError> {
        Ok(OrderAck {
            client_order_id: order.client_order_id().clone(),
            venue_order_id: Some(VenueOrderId::new("5501").expect("order id")),
            acknowledged_at_ms: 1,
        })
    }
}

#[async_trait]
impl ProtectionConnector for FakeVenue {
    async fn submit_close(&self, order: &CloseOrder) -> Result<OrderAck, ExchangeError> {
        Ok(OrderAck {
            client_order_id: order.client_order_id().clone(),
            venue_order_id: None,
            acknowledged_at_ms: 1,
        })
    }

    async fn place_stop(&self, stop: &StopRequest) -> Result<StopPlacement, ExchangeError> {
        if self.crossed {
            return Ok(StopPlacement::PriceCrossed);
        }
        let mut placed = our_stop(StopStatus::Active);
        placed.trigger_price = stop.trigger_price();
        self.stops.lock().expect("stops").push(placed.clone());
        Ok(StopPlacement::Placed(placed.stop))
    }

    async fn stops(&self, _instrument: &InstrumentId) -> Result<Vec<StopOrder>, ExchangeError> {
        Ok(self.stops.lock().expect("stops").clone())
    }

    async fn stop(
        &self,
        _instrument: &InstrumentId,
        _stop: &StopRef,
    ) -> Result<StopLookup, ExchangeError> {
        Ok(StopLookup::NotFound { queried_at_ms: 9 })
    }

    async fn replace_stop(
        &self,
        current: &StopRef,
        _replacement: &StopRequest,
        bridge: &StopRequest,
    ) -> Result<StopRef, StopReplaceError> {
        // The bridge is in place, and the old stop's cancel fails.
        let bridge = StopRef::Order {
            order_id: VenueOrderId::new("bridge-1").expect("order id"),
            client_id: Some(bridge.client_order_id().to_string()),
        };
        Err(StopReplaceError {
            remaining: vec![current.clone(), bridge],
            source: ExchangeError::api(ExchangeErrorKind::Unavailable, Some(-1), "busy"),
        })
    }

    async fn cancel_stop(&self, _stop: &StopRef) -> Result<(), ExchangeError> {
        Ok(())
    }
}

#[async_trait]
impl OrderQueryConnector for FakeVenue {
    async fn order(
        &self,
        _instrument: &InstrumentId,
        _client_order_id: &ClientOrderId,
    ) -> Result<OrderLookup, ExchangeError> {
        Ok(match self.lookup.lock().expect("lookup").clone() {
            Some(order) => OrderLookup::Found(Box::new(order)),
            None => OrderLookup::NotFound {
                queried_at_ms: 31_000,
            },
        })
    }

    async fn open_orders(
        &self,
        _instrument: &InstrumentId,
    ) -> Result<Vec<FuturesOrder>, ExchangeError> {
        Ok(Vec::new())
    }

    async fn fills(
        &self,
        _instrument: &InstrumentId,
        _window: TimeWindow,
    ) -> Result<Vec<FuturesFill>, ExchangeError> {
        Ok(Vec::new())
    }

    async fn ledger(&self, _window: TimeWindow) -> Result<Vec<LedgerRecord>, ExchangeError> {
        Ok(Vec::new())
    }
}

#[async_trait]
impl PrivateEventConnector for FakeVenue {
    async fn next_event(&self) -> Result<PrivateEvent, ExchangeError> {
        Err(Self::unscripted())
    }
}

fn connectors(venue: FakeVenue) -> FuturesConnectors {
    let venue = Arc::new(venue);
    FuturesConnectors {
        instruments: venue.clone(),
        account: venue.clone(),
        leverage: venue.clone(),
        entry: venue.clone(),
        protection: venue.clone(),
        orders: venue.clone(),
        events: venue,
    }
}

#[tokio::test]
async fn every_port_is_implemented_outside_the_crate_and_handed_out_together() {
    let ports = connectors(FakeVenue::new());
    assert_eq!(
        format!("{ports:?}"),
        "FuturesConnectors { exchange: BybitLinear, environment: Demo, .. }"
    );
    assert_eq!(
        ports
            .instruments
            .instruments()
            .await
            .expect("instruments")
            .len(),
        1
    );
    let account = ports.account.account().await.expect("account");
    assert_eq!(account.account, *key().account());
    let leverage = Leverage::new(dec("3")).expect("leverage");
    assert_eq!(
        ports
            .leverage
            .set_leverage(&instrument(), leverage)
            .await
            .expect("set"),
        leverage
    );
    let window = TimeWindow::new(0, 10).expect("window");
    assert!(
        ports
            .orders
            .ledger(window)
            .await
            .expect("ledger")
            .is_empty()
    );
    assert!(ports.events.next_event().await.is_err());
    let capabilities = ports.protection.capabilities();
    assert_eq!(capabilities.stop_replacement, StopReplacement::AmendInPlace);
    assert_eq!(
        FuturesCapabilities::default().stop_replacement,
        StopReplacement::Bridge
    );
}

#[tokio::test]
async fn an_acknowledgement_leaves_an_entry_submitting_until_the_venue_reports_fills() {
    let venue = FakeVenue::new();
    let (mut intent, id) = sent_entry();
    let entry = EntryOrder::new(
        &sol(ContractKind::Perpetual),
        PositionSide::Long,
        base("4.3"),
        id.clone(),
        None,
    )
    .expect("entry order");
    let ack = venue.submit_entry(&entry).await.expect("ack");
    assert_eq!(ack.client_order_id, id);
    assert_eq!(
        intent.state(),
        IntentState::Submitting,
        "an ack proves nothing"
    );
    intent.on_fill(&fill(&id)).expect("fill");
    assert_eq!(intent.state(), IntentState::PartiallyFilled);
    *venue.lookup.lock().expect("lookup") = Some(order(&id, FuturesOrderStatus::Filled, "4.3"));
    let OrderLookup::Found(found) = venue.order(&instrument(), &id).await.expect("lookup") else {
        panic!("the order is found");
    };
    intent.on_order(&found).expect("final order");
    assert_eq!(intent.state(), IntentState::Filled);
}

#[test]
fn only_a_definitive_refusal_rejects_and_an_unknown_outcome_waits_for_evidence() {
    let transport = ExchangeError::transport(std::io::Error::other("reset"));
    assert!(transport.rejection().is_none());
    let unknown = ExchangeError::api(ExchangeErrorKind::OutcomeUnknown, Some(503), "unknown");
    assert!(unknown.rejection().is_none());
    let refused = ExchangeError::api(ExchangeErrorKind::InsufficientBalance, Some(110007), "no");
    let rejection = refused.rejection().expect("a definitive refusal");
    assert_eq!(rejection.kind(), "insufficient_balance");

    let (mut rejected, _) = sent_entry();
    rejected.on_rejection(&rejection).expect("rejected");
    assert_eq!(rejected.state(), IntentState::Rejected);

    let (mut intent, id) = sent_entry();
    intent
        .move_to(IntentState::OutcomeUnknown)
        .expect("unknown");
    assert!(intent.on_rejection(&rejection).is_err(), "never relabelled");
    let late = order(
        &id,
        FuturesOrderStatus::Canceled(CancelAttribution::BySystem),
        "0",
    );
    intent.on_order(&late).expect("unfilled");
    assert_eq!(intent.state(), IntentState::Unfilled);
}

#[test]
fn evidence_for_another_order_is_never_applied() {
    let (mut intent, _) = sent_entry();
    let someone_else = order(
        &client_id(9, OrderLeg::Entry, 1),
        FuturesOrderStatus::Filled,
        "4.3",
    );
    assert!(intent.on_order(&someone_else).is_err());
    let mut foreign = order(
        &client_id(1, OrderLeg::Entry, 1),
        FuturesOrderStatus::Filled,
        "4.3",
    );
    foreign.client_id = Some("web_1729584000.1".to_string());
    assert!(intent.on_order(&foreign).is_err());
    assert_eq!(intent.state(), IntentState::Submitting);
}

#[test]
fn not_found_settles_an_entry_only_past_the_propagation_delay_with_the_position_flat() {
    let (mut intent, id) = sent_entry();
    intent
        .move_to(IntentState::OutcomeUnknown)
        .expect("unknown");
    let guarantee = OrderLookupGuarantee {
        visibility_verified: true,
        unfilled_retention_ms: 3 * 24 * 60 * 60 * 1000,
    };
    let evidence = |queried_at_ms: i64, guarantee, position| NotPlacedEvidence {
        client_order_id: id.clone(),
        queried_at_ms,
        guarantee,
        position,
    };
    let unproven = [
        evidence(29_999, guarantee, snapshot(None, 30_000)),
        evidence(
            30_000,
            OrderLookupGuarantee::default(),
            snapshot(None, 30_000),
        ),
        evidence(30_000, guarantee, snapshot(None, 29_999)),
        evidence(
            30_000,
            guarantee,
            snapshot(Some((PositionSide::Long, "4.3")), 30_000),
        ),
    ];
    for not_yet in unproven {
        assert!(intent.entry_not_placed(&not_yet).is_err(), "{not_yet:?}");
        assert_eq!(intent.state(), IntentState::OutcomeUnknown);
    }
    assert!(
        intent.unsettled_past_window(60_000),
        "the settle window has passed"
    );
    intent
        .entry_not_placed(&evidence(30_000, guarantee, snapshot(None, 30_000)))
        .expect("never placed");
    assert_eq!(intent.state(), IntentState::NotPlaced);
}

#[test]
fn an_exit_retries_as_a_new_attempt_and_never_expires() {
    let mut exit = ExternalIntent::new(IntentKind::Exit(ExitIntent {
        profile: ProfileId::new("main").expect("profile"),
        signal: SignalId::new("exit SOLUSDT.P long 1729584060000").expect("signal"),
        signal_time_ms: 60_000,
        key: key(),
        side: PositionSide::Long,
    }));
    exit.move_to(IntentState::Admitted).expect("admitted");
    assert!(exit.move_to(IntentState::Expired).is_err());
    let attempt = |n| OrderAttempt {
        client_order_id: client_id(2, OrderLeg::Close, n),
        sent_at_ms: i64::from(n),
    };
    exit.record_attempt(attempt(1)).expect("first");
    exit.move_to(IntentState::OutcomeUnknown).expect("unknown");
    exit.record_attempt(attempt(2)).expect("a retry");
    assert_eq!(exit.attempts().len(), 2);
    let entry_leg = OrderAttempt {
        client_order_id: client_id(2, OrderLeg::Entry, 3),
        sent_at_ms: 3,
    };
    assert!(exit.record_attempt(entry_leg).is_err());
    let (mut entry, _) = sent_entry();
    entry.move_to(IntentState::OutcomeUnknown).expect("unknown");
    let again = OrderAttempt {
        client_order_id: client_id(1, OrderLeg::Entry, 2),
        sent_at_ms: 5,
    };
    assert!(
        entry.record_attempt(again).is_err(),
        "an entry is never resubmitted"
    );
}

#[tokio::test]
async fn a_read_back_of_the_recorded_stop_protects_the_cycle_and_a_reversal_closes_it() {
    let venue = FakeVenue::new();
    let mut cycle = PositionCycle::open(
        PositionCycleId::new(1).expect("cycle"),
        key(),
        ProfileId::new("main").expect("profile"),
        PositionSide::Long,
        OperationId::new(1).expect("operation"),
        2,
    );
    assert_eq!(cycle.protection(), ProtectionState::ProtectionPending);
    assert!(cycle.move_protection(ProtectionState::Protected).is_err());
    let request = StopRequest::new(
        instrument(),
        PositionSide::Long,
        price("200.00"),
        TriggerBasis::Mark,
        PriceProtection::NotApplicable,
        StopScope::WholePosition,
        client_id(1, OrderLeg::Stop, 1),
    )
    .expect("stop request");
    let placed = venue.place_stop(&request).await.expect("placed");
    assert!(matches!(placed, StopPlacement::Placed(_)));
    let recorded = RecordedStop {
        trigger_price: request.trigger_price(),
        basis: request.basis(),
        price_protection: request.price_protection(),
    };
    cycle.record_stop(recorded);
    let position = snapshot(Some((PositionSide::Long, "4.3")), 3);
    let read_back = venue.stops(&instrument()).await.expect("stops");
    let covered = CoveredStop::ours(&read_back[0], &position, &recorded).expect("covered");
    cycle.verified(&covered).expect("protected");
    assert_eq!(cycle.protection(), ProtectionState::Protected);

    // The operator cancels the stop by hand, then reverses the position.
    let cancelled = our_stop(StopStatus::Canceled(CancelAttribution::ByUser));
    let removal = OperatorRemoval::user_cancel(&cancelled).expect("user cancel");
    cycle
        .removed_by_operator(&removal)
        .expect("unprotected by operator");
    assert!(OperatorRemoval::user_cancel(&our_stop(StopStatus::Expired)).is_err());
    let still_long = snapshot(Some((PositionSide::Long, "1.0")), 4);
    assert!(cycle.close(&still_long, CloseReason::Manual).is_err());
    let reversed = snapshot(Some((PositionSide::Short, "2.0")), 5);
    cycle.close(&reversed, CloseReason::Manual).expect("closed");
    assert_eq!(cycle.protection(), ProtectionState::Closed);
    assert_eq!(cycle.close_reason(), Some(CloseReason::Manual));
    assert_eq!(cycle.closed_at_ms(), Some(5));
}

#[tokio::test]
async fn a_crossed_stop_is_final_and_a_replacement_that_stops_part_way_lists_what_remains() {
    let mut venue = FakeVenue::new();
    venue.crossed = true;
    let stop = |leg, scope| {
        StopRequest::new(
            instrument(),
            PositionSide::Long,
            price("240.00"),
            TriggerBasis::Mark,
            PriceProtection::NotApplicable,
            scope,
            client_id(1, leg, 1),
        )
        .expect("stop request")
    };
    let replacement = stop(OrderLeg::Stop, StopScope::WholePosition);
    let placed = venue.place_stop(&replacement).await.expect("answered");
    assert_eq!(placed, StopPlacement::PriceCrossed);
    let bridge = stop(OrderLeg::Bridge, StopScope::Quantity(base("4.3")));
    let current = StopRef::Position {
        instrument: instrument(),
    };
    let error = venue
        .replace_stop(&current, &replacement, &bridge)
        .await
        .expect_err("stopped part-way");
    assert_eq!(error.remaining.len(), 2);
    assert_eq!(
        error.to_string(),
        "stop replacement stopped part-way, leaving 2 stops in place"
    );
    assert_eq!(error.source.kind(), ExchangeErrorKind::Unavailable);
    let wrong_leg = StopRequest::new(
        instrument(),
        PositionSide::Long,
        price("240.00"),
        TriggerBasis::Mark,
        PriceProtection::NotApplicable,
        StopScope::WholePosition,
        client_id(1, OrderLeg::Bridge, 1),
    );
    assert!(
        wrong_leg.is_err(),
        "a whole-position stop is never a bridge"
    );
}

#[test]
fn an_entry_order_is_refused_outside_the_universe_in_the_wrong_unit_or_on_another_leg() {
    let id = client_id(1, OrderLeg::Entry, 1);
    let long = PositionSide::Long;
    let tradfi = sol(ContractKind::TradFiPerpetual);
    let error = EntryOrder::new(&tradfi, long, base("1"), id.clone(), None).expect_err("tradfi");
    assert_eq!(
        error.to_string(),
        "unsupported instrument bybit_linear SOLUSDT: TradFi perpetuals are outside the v0.3 universe"
    );
    let contracts = Quantity::for_category(Category::Inverse, dec("10")).expect("contracts");
    let perpetual = sol(ContractKind::Perpetual);
    assert!(EntryOrder::new(&perpetual, long, contracts, id, None).is_err());
    let close_id = client_id(1, OrderLeg::Close, 1);
    assert!(EntryOrder::new(&perpetual, long, base("1"), close_id, None).is_err());
}
