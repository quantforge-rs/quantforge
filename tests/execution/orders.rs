//! An intent's orders through the ledger, on each backend: the attempt
//! recorded with its reservation before anything is sent, the venue's
//! evidence that settles it, and a process that starts on the store and
//! asks the venue before it does anything else.

use quantforge::automation::{Evidence, ExecutionLedger, LedgerError, Resumed};
use quantforge::model::execution::{
    EventKind, IntentRecord, OperationKind, OrderPhase, ReleaseReason, ReservationState, Stale,
    Stored,
};
use quantforge::model::futures::{
    ClientOrderId, FuturesOrderStatus, IntentState, OrderLeg, PoolScope, ProtectionState,
};
use quantforge::model::{Asset, OperationId, VenueOrderId};
use quantforge::ports::{ExchangeError, ExchangeErrorKind};
use quantforge::{ExecutionStore, StoreControl};

use crate::fixtures::{Line, line, reported};
use crate::support::{Backend, Manual, T, block_on, count, on_each_backend};
use crate::venue::Venue;

/// An admitted signal with its operation numbered: what an executor
/// starts from.
pub(crate) struct Started {
    pub(crate) intent: Stored<IntentRecord>,
    pub(crate) operation: OperationId,
    pub(crate) id: ClientOrderId,
}

/// Admits a `kind` signal of `source` on `line` and numbers its
/// operation. The store must be active.
pub(crate) fn started(
    store: &(impl ExecutionStore + StoreControl),
    ledger: &ExecutionLedger<'_>,
    line: &Line,
    kind: &str,
    source: &str,
    minute: u32,
) -> Started {
    let delivered = line.delivery(kind, source, minute, "234.23");
    ledger
        .admit(
            &line.identity,
            &delivered.signal,
            &delivered.payload,
            &line.resolved(&delivered),
        )
        .expect("admitted");
    let intent = store
        .intent(&line.scope(), &delivered.signal.id)
        .expect("a read")
        .expect("the intent");
    let (operation_kind, leg) = match kind {
        "entry" => (OperationKind::Entry, OrderLeg::Entry),
        _ => (OperationKind::Close, OrderLeg::Close),
    };
    let operation = store
        .begin_operation(&line.operation(&delivered, operation_kind), T + 7_000)
        .expect("numbered")
        .record
        .id();
    let epoch = store.store_identity().expect("identity").activation_epoch;
    Started {
        intent,
        operation,
        id: line.client_order_id(epoch, operation, leg, 1),
    }
}

fn usdt_pool() -> PoolScope {
    PoolScope::Asset(Asset::new("USDT").expect("an asset"))
}

fn kinds(store: &impl ExecutionStore) -> Vec<EventKind> {
    store
        .pending_events(100)
        .expect("the events")
        .iter()
        .map(|event| event.record.kind())
        .collect()
}

fn an_order_is_recorded_with_its_reservation_before_it_is_sent_and_settled_by_evidence<
    B: Backend,
>(
    backend: &B,
) {
    let store = backend.open();
    store.activate(T).expect("activated");
    let clock = Manual::at(T + 6_000);
    let ledger = ExecutionLedger::new(&store, &clock);
    let line = line("bybit linear");
    let account = line.identity.account_ref();
    let sol = started(&store, &ledger, &line, "entry", "trend", 0);
    let lease = line.lease(&store, "executor-a", T + 7_000);

    // Sized against an untouched pool, and recorded with its reservation.
    let pool = store.pool(account, &usdt_pool()).expect("the pool");
    assert_eq!((pool.revision, pool.held.len()), (0, 0));
    clock.set(T + 8_000);
    let held = line.reservation(sol.operation, "205.18548", T + 8_000);
    let sent = ledger
        .record_attempt(
            &lease.fence(),
            &sol.intent,
            &sol.id,
            Some((held.clone(), pool.revision)),
        )
        .expect("recorded");
    assert_eq!(sent.record.intent().state(), IntentState::Submitting);
    assert_eq!(sent.record.operation(), Some(sol.operation));
    let pool = store.pool(account, &usdt_pool()).expect("the pool");
    assert_eq!((pool.revision, &pool.held), (1, &vec![held]));
    assert_eq!(
        pool.total(&usdt_pool())
            .expect("a total")
            .amount()
            .to_string(),
        "205.18548"
    );
    let order = store
        .order(&line.scope(), &sol.id.to_string())
        .expect("a read")
        .expect("the order");
    assert_eq!(order.record.phase(), OrderPhase::Recorded);
    assert_eq!(order.record.recorded_at_ms(), T + 8_000);

    // An entry on another instrument, sized before that reservation was
    // made, is not recorded: its pool has moved, and it is sized again.
    let btc_line = line.trading("BTCUSDT");
    let btc = started(&store, &ledger, &btc_line, "entry", "trend", 0);
    let btc_lease = btc_line.lease(&store, "executor-a", T + 8_000);
    let stale = ledger.record_attempt(
        &btc_lease.fence(),
        &btc.intent,
        &btc.id,
        Some((btc_line.reservation(btc.operation, "410.1", T + 8_000), 0)),
    );
    assert!(
        matches!(stale, Err(LedgerError::Stale(Stale::Pool { revision: 1 }))),
        "{stale:?}"
    );
    assert_eq!(
        count(backend, "execution_orders"),
        1,
        "nothing of it written"
    );
    assert_eq!(count(backend, "margin_reservations"), 1);

    // A first fill opens the cycle; the margin stays reserved until the
    // order has ended.
    clock.set(T + 9_500);
    let partial = reported(
        &line,
        &sol.id,
        FuturesOrderStatus::PartiallyFilled,
        "4.38",
        "2",
    );
    let first = [line
        .fill("t-1", &sol.id, "2", "234.25", sol.operation, None)
        .fill()
        .clone()];
    let evidence = Evidence::Order {
        order: &partial,
        fills: &first,
    };
    let exposed = ledger
        .settle(&lease.fence(), &sent, evidence)
        .expect("settled");
    assert_eq!(
        exposed.record.intent().state(),
        IntentState::PartiallyFilled
    );
    let cycle = exposed.record.cycle().expect("the cycle its fill opened");
    assert_eq!(
        cycle.get(),
        sol.operation.get(),
        "numbered as its operation"
    );
    let open = store
        .open_cycle(&line.key())
        .expect("a read")
        .expect("the open cycle");
    assert_eq!(
        open.record.cycle().protection(),
        ProtectionState::ProtectionPending
    );
    assert_eq!(
        open.record.cycle().opened_at_ms(),
        T + 9_000,
        "its first fill"
    );
    let carried = store
        .operation(sol.operation)
        .expect("a read")
        .expect("the operation");
    assert_eq!(carried.record.cycle(), Some(cycle));
    assert_eq!(
        store
            .pool(account, &usdt_pool())
            .expect("the pool")
            .held
            .len(),
        1
    );
    // The same evidence again is nothing new, and writes nothing.
    let again = ledger
        .settle(&lease.fence(), &exposed, evidence)
        .expect("nothing to do");
    assert_eq!(again, exposed);

    // Filled, the entry is settled and its margin is the position's.
    let whole = reported(&line, &sol.id, FuturesOrderStatus::Filled, "4.38", "4.38");
    let both = [
        first[0].clone(),
        line.fill("t-2", &sol.id, "2.38", "234.30", sol.operation, None)
            .fill()
            .clone(),
    ];
    let evidence = Evidence::Order {
        order: &whole,
        fills: &both,
    };
    let filled = ledger
        .settle(&lease.fence(), &exposed, evidence)
        .expect("settled");
    assert_eq!(filled.record.intent().state(), IntentState::Filled);
    assert_eq!(store.fills(cycle).expect("the fills").len(), 2);
    let released = store
        .reservation(sol.operation)
        .expect("a read")
        .expect("the reservation");
    assert_eq!(released.record.state(), ReservationState::Released);
    assert_eq!(
        released.record.released().map(|released| released.reason),
        Some(ReleaseReason::Opened)
    );
    let pool = store.pool(account, &usdt_pool()).expect("the pool");
    assert_eq!((pool.revision, pool.held.len()), (2, 0));
    assert_eq!(
        kinds(&store),
        [
            EventKind::SignalAdmitted,
            EventKind::OrderSubmitted,
            EventKind::SignalAdmitted,
            EventKind::OrderSettled,
            EventKind::OrderSettled,
        ]
    );
}

fn an_entry_the_venue_refuses_opens_nothing_and_gives_its_margin_back<B: Backend>(backend: &B) {
    let store = backend.open();
    store.activate(T).expect("activated");
    let clock = Manual::at(T + 6_000);
    let ledger = ExecutionLedger::new(&store, &clock);
    let line = line("bybit linear");
    let entry = started(&store, &ledger, &line, "entry", "trend", 0);
    let lease = line.lease(&store, "executor-a", T + 7_000);
    let held = line.reservation(entry.operation, "205.18548", T + 8_000);
    let sent = ledger
        .record_attempt(&lease.fence(), &entry.intent, &entry.id, Some((held, 0)))
        .expect("recorded");

    let refused = ExchangeError::api(
        ExchangeErrorKind::InsufficientBalance,
        Some(110007),
        "ab not enough for new order",
    );
    let rejection = refused.rejection(&entry.id).expect("a definitive refusal");
    let settled = ledger
        .settle(&lease.fence(), &sent, Evidence::Rejection(&rejection))
        .expect("settled");
    assert_eq!(settled.record.intent().state(), IntentState::Rejected);
    assert_eq!(settled.record.cycle(), None);
    assert_eq!(store.open_cycle(&line.key()).expect("a read"), None);
    assert_eq!(count(backend, "position_cycles"), 0);
    let order = store
        .order(&line.scope(), &entry.id.to_string())
        .expect("a read")
        .expect("the order");
    assert_eq!(order.record.phase(), OrderPhase::Refused);
    assert_eq!(
        order.record.refusal().map(|refusal| refusal.kind.as_str()),
        Some("insufficient_balance")
    );
    let released = store
        .reservation(entry.operation)
        .expect("a read")
        .expect("the reservation");
    assert_eq!(
        released.record.released().map(|released| released.reason),
        Some(ReleaseReason::NotOpened)
    );
    assert_eq!(store.unsettled_intents().expect("a read"), []);
}

/// The process dies after the attempt is recorded and before its request
/// leaves. Whoever starts on the store asks the venue, and concludes that
/// the entry was never placed only once ADR 0002 §9's proof is there.
fn an_attempt_found_after_a_restart_is_asked_for_and_never_sent_again<B: Backend>(backend: &B) {
    let line = line("bybit linear");
    let clock = Manual::at(T + 6_000);
    let (entry, fence) = {
        let store = backend.open();
        store.activate(T).expect("activated");
        let ledger = ExecutionLedger::new(&store, &clock);
        let entry = started(&store, &ledger, &line, "entry", "trend", 0);
        let lease = line.lease(&store, "executor-a", T + 7_000);
        clock.set(T + 8_000);
        let held = line.reservation(entry.operation, "205.18548", T + 8_000);
        ledger
            .record_attempt(&lease.fence(), &entry.intent, &entry.id, Some((held, 0)))
            .expect("recorded");
        (entry, lease.fence())
        // The process ends here: nothing was sent, and nothing says so.
    };

    let store = backend.open();
    let ledger = ExecutionLedger::new(&store, &clock);
    let venue = Venue::new(line.instrument().exchange, &clock);
    let found = store.unsettled_intents().expect("what to resume");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].record.intent().state(), IntentState::Submitting);
    assert_eq!(
        found[0].record.intent().attempts()[0].client_order_id,
        entry.id
    );

    // Ten seconds after it was recorded, "not found" proves nothing yet.
    clock.set(T + 18_000);
    let asked = block_on(ledger.resume(&fence, &found[0], &venue, &venue)).expect("asked");
    let Resumed::Unknown(unknown) = &asked else {
        panic!("still unknown: {asked:?}");
    };
    assert_eq!(unknown.record.intent().state(), IntentState::OutcomeUnknown);
    assert!(
        unknown.record.blocks_entries(),
        "its position stays blocked"
    );
    let order = store
        .order(&line.scope(), &entry.id.to_string())
        .expect("a read")
        .expect("the order");
    assert_eq!(order.record.phase(), OrderPhase::OutcomeUnknown);

    // Past the propagation delay, with the position still flat: never
    // placed, and its margin is given back.
    clock.set(T + 39_000);
    let asked = block_on(ledger.resume(&fence, unknown, &venue, &venue)).expect("asked");
    let Resumed::Settled(settled) = &asked else {
        panic!("settled: {asked:?}");
    };
    assert_eq!(settled.record.intent().state(), IntentState::NotPlaced);
    assert_eq!(venue.asked(), 2, "asked twice, and sent nothing");
    assert_eq!(
        count(backend, "execution_orders"),
        1,
        "one attempt, for good"
    );
    let released = store
        .reservation(entry.operation)
        .expect("a read")
        .expect("the reservation");
    assert_eq!(
        released.record.released().map(|released| released.reason),
        Some(ReleaseReason::NotOpened)
    );
    assert_eq!(
        kinds(&store),
        [
            EventKind::SignalAdmitted,
            EventKind::OrderSubmitted,
            EventKind::OutcomeUnknown,
            EventKind::OrderSettled,
        ]
    );
    // Settled, there is nothing left to ask.
    let asked = block_on(ledger.resume(&fence, settled, &venue, &venue)).expect("asked");
    assert_eq!(asked, Resumed::Settled(settled.clone()));
    assert_eq!(venue.asked(), 2);
}

/// "Not found" settles nothing where the venue's lookups are not verified
/// or where a position has appeared: the entry stays unknown, blocking.
fn an_entry_stays_unknown_without_the_whole_proof<B: Backend>(backend: &B) {
    let store = backend.open();
    store.activate(T).expect("activated");
    let line = line("bybit linear");
    let clock = Manual::at(T + 6_000);
    let ledger = ExecutionLedger::new(&store, &clock);
    let entry = started(&store, &ledger, &line, "entry", "trend", 0);
    let lease = line.lease(&store, "executor-a", T + 7_000);
    clock.set(T + 8_000);
    let sent = ledger
        .record_attempt(&lease.fence(), &entry.intent, &entry.id, None)
        .expect("recorded");

    let mut venue = Venue::new(line.instrument().exchange, &clock);
    venue.verified = false;
    clock.set(T + 60_000);
    let asked = block_on(ledger.resume(&lease.fence(), &sent, &venue, &venue)).expect("asked");
    let Resumed::Unknown(unknown) = asked else {
        panic!("an unverified lookup proves nothing");
    };
    // Verified now, but the account shows a position on the instrument.
    venue.verified = true;
    let mut held = block_on(quantforge::ports::futures::AccountStateConnector::position(
        &venue,
        &line.instrument(),
    ))
    .expect("a snapshot");
    held.exposure = Some(quantforge::model::futures::Exposure {
        side: quantforge::model::PositionSide::Long,
        size: reported(&line, &entry.id, FuturesOrderStatus::Filled, "1", "1").filled,
        entry_price: quantforge::model::Price::new(rust_decimal::Decimal::new(23423, 2))
            .expect("a price"),
    });
    held.observed_at_ms = T + 61_000;
    *venue.position.lock().expect("position") = Some(held);
    clock.set(T + 61_000);
    let asked = block_on(ledger.resume(&lease.fence(), &unknown, &venue, &venue)).expect("asked");
    assert!(matches!(asked, Resumed::Unknown(_)), "{asked:?}");
    assert_eq!(
        store.unsettled_intents().expect("a read").len(),
        1,
        "never placed is never guessed"
    );
}

/// The request left, and the process died before the answer. The venue
/// holds the order: it is settled from what the venue reports, once.
fn an_order_the_venue_holds_is_settled_from_its_report_and_not_sent_again<B: Backend>(backend: &B) {
    let store = backend.open();
    store.activate(T).expect("activated");
    let line = line("bybit linear");
    let clock = Manual::at(T + 6_000);
    let ledger = ExecutionLedger::new(&store, &clock);
    let entry = started(&store, &ledger, &line, "entry", "trend", 0);
    let lease = line.lease(&store, "executor-a", T + 7_000);
    clock.set(T + 8_000);
    let sent = ledger
        .record_attempt(&lease.fence(), &entry.intent, &entry.id, None)
        .expect("recorded");

    let venue = Venue::new(line.instrument().exchange, &clock);
    venue.holds(
        reported(&line, &entry.id, FuturesOrderStatus::Filled, "4.38", "4.38"),
        vec![
            line.fill("t-1", &entry.id, "4.38", "234.25", entry.operation, None)
                .fill()
                .clone(),
        ],
    );
    let restarted = backend.open();
    let later = ExecutionLedger::new(&restarted, &clock);
    clock.set(T + 12_000);
    let asked = block_on(later.resume(&lease.fence(), &sent, &venue, &venue)).expect("asked");
    let Resumed::Settled(filled) = &asked else {
        panic!("settled: {asked:?}");
    };
    assert_eq!(filled.record.intent().state(), IntentState::Filled);
    let cycle = filled.record.cycle().expect("the cycle it opened");
    assert_eq!(restarted.fills(cycle).expect("the fills").len(), 1);
    // Asked again, as a second restart would: nothing new.
    let asked = block_on(later.resume(&lease.fence(), filled, &venue, &venue)).expect("asked");
    assert_eq!(asked, Resumed::Settled(filled.clone()));
    assert_eq!(count(backend, "execution_fills"), 1);
    assert_eq!(count(backend, "execution_orders"), 1);
    assert_eq!(count(backend, "position_cycles"), 1);
}

/// A venue's trade records name their order by the venue's own ID, and on
/// some venues carry no client ID at all. The fills of a recovered order
/// are the ones under its venue ID: each is recorded, with or without a
/// client ID, and another order's fill on the same instrument in the same
/// minutes is not.
fn a_recovered_orders_fills_are_its_own_by_the_venues_id_for_it<B: Backend>(backend: &B) {
    let store = backend.open();
    store.activate(T).expect("activated");
    let line = line("bybit linear");
    let clock = Manual::at(T + 6_000);
    let ledger = ExecutionLedger::new(&store, &clock);
    let entry = started(&store, &ledger, &line, "entry", "trend", 0);
    let lease = line.lease(&store, "executor-a", T + 7_000);
    clock.set(T + 8_000);
    let sent = ledger
        .record_attempt(&lease.fence(), &entry.intent, &entry.id, None)
        .expect("recorded");

    let order = reported(&line, &entry.id, FuturesOrderStatus::Filled, "4.38", "4.38");
    let trade = |trade: &str, of_order: &str, quantity: &str, named: bool| {
        let recorded = line.fill(trade, &entry.id, quantity, "234.25", entry.operation, None);
        let mut fill = recorded.fill().clone();
        fill.venue_order_id = VenueOrderId::new(of_order).expect("an order id");
        if !named {
            fill.client_id = None;
        }
        fill
    };
    let its_own = order.venue_order_id.as_str().to_string();
    let venue = Venue::new(line.instrument().exchange, &clock);
    venue.holds(
        order,
        vec![
            trade("t-1", &its_own, "2", false),
            trade("t-2", &its_own, "2.38", true),
            // Someone else's order on the same instrument.
            trade("t-9", "777", "1", false),
        ],
    );
    clock.set(T + 12_000);
    let asked = block_on(ledger.resume(&lease.fence(), &sent, &venue, &venue)).expect("asked");
    let Resumed::Settled(filled) = &asked else {
        panic!("settled: {asked:?}");
    };
    assert_eq!(filled.record.intent().state(), IntentState::Filled);
    let cycle = filled.record.cycle().expect("the cycle it opened");
    let mut trades: Vec<String> = store
        .fills(cycle)
        .expect("the fills")
        .iter()
        .map(|fill| fill.fill().trade_id.as_str().to_string())
        .collect();
    trades.sort_unstable();
    assert_eq!(trades, ["t-1", "t-2"], "the order's own, named or not");
    assert_eq!(count(backend, "execution_fills"), 2);

    // Handed a fill of another order, a settlement refuses all of it.
    let other = started(
        &store,
        &ledger,
        &line.trading("BTCUSDT"),
        "entry",
        "trend",
        0,
    );
    let btc = line.trading("BTCUSDT");
    let btc_lease = btc.lease(&store, "executor-a", T + 12_000);
    let btc_sent = ledger
        .record_attempt(&btc_lease.fence(), &other.intent, &other.id, None)
        .expect("recorded");
    let report = reported(&btc, &other.id, FuturesOrderStatus::Filled, "4.38", "4.38");
    let mut stray = btc
        .fill("t-3", &other.id, "4.38", "234.25", other.operation, None)
        .fill()
        .clone();
    stray.venue_order_id = VenueOrderId::new("777").expect("an order id");
    let evidence = Evidence::Order {
        order: &report,
        fills: std::slice::from_ref(&stray),
    };
    let refused = ledger
        .settle(&btc_lease.fence(), &btc_sent, evidence)
        .expect_err("not this order's fill");
    assert!(
        refused
            .to_string()
            .contains("a fill of the order reported for the intent's attempt"),
        "{refused}"
    );
    assert_eq!(
        count(backend, "execution_fills"),
        2,
        "nothing of it written"
    );
}

/// An exit's attempt the venue does not know may be sent again, since a
/// close can only reduce; the new attempt is recorded first, and decides.
fn an_exit_may_be_sent_again_as_a_new_attempt_recorded_first<B: Backend>(backend: &B) {
    let store = backend.open();
    store.activate(T).expect("activated");
    let line = line("bybit linear");
    let clock = Manual::at(T + 6_000);
    let ledger = ExecutionLedger::new(&store, &clock);
    // An entry on its way gives the exit something to be for.
    let entry = started(&store, &ledger, &line, "entry", "trend", 0);
    let lease = line.lease(&store, "executor-a", T + 7_000);
    ledger
        .record_attempt(&lease.fence(), &entry.intent, &entry.id, None)
        .expect("recorded");
    let exit = started(&store, &ledger, &line, "exit", "trend", 1);
    clock.set(T + 70_000);
    let sent = ledger
        .record_attempt(&lease.fence(), &exit.intent, &exit.id, None)
        .expect("recorded");

    let venue = Venue::new(line.instrument().exchange, &clock);
    clock.set(T + 71_000);
    let asked = block_on(ledger.resume(&lease.fence(), &sent, &venue, &venue)).expect("asked");
    let Resumed::MayRetry(unknown) = asked else {
        panic!("an exit may be sent again");
    };
    assert_eq!(unknown.record.intent().state(), IntentState::OutcomeUnknown);
    let epoch = store.store_identity().expect("identity").activation_epoch;
    let second = line.client_order_id(epoch, exit.operation, OrderLeg::Close, 2);
    let retried = ledger
        .record_attempt(&lease.fence(), &unknown, &second, None)
        .expect("recorded");
    assert_eq!(retried.record.intent().attempts().len(), 2);
    venue.holds(
        reported(&line, &second, FuturesOrderStatus::Filled, "4.38", "4.38"),
        Vec::new(),
    );
    let asked = block_on(ledger.resume(&lease.fence(), &retried, &venue, &venue)).expect("asked");
    assert_eq!(
        asked.intent().record.intent().state(),
        IntentState::Filled,
        "{asked:?}"
    );
    assert_eq!(store.orders(exit.operation).expect("the orders").len(), 2);
}

/// An entry the venue reported fills of, whose order the venue no longer
/// lists: nothing new is known of it, and nothing is concluded. It is not
/// called never placed, since it was, and the position its fill opened
/// stays as it is.
fn a_partly_filled_entry_the_venue_no_longer_lists_is_left_as_it_stands<B: Backend>(backend: &B) {
    let store = backend.open();
    store.activate(T).expect("activated");
    let line = line("bybit linear");
    let clock = Manual::at(T + 6_000);
    let ledger = ExecutionLedger::new(&store, &clock);
    let entry = started(&store, &ledger, &line, "entry", "trend", 0);
    let lease = line.lease(&store, "executor-a", T + 7_000);
    clock.set(T + 8_000);
    let sent = ledger
        .record_attempt(&lease.fence(), &entry.intent, &entry.id, None)
        .expect("recorded");
    let partial = reported(
        &line,
        &entry.id,
        FuturesOrderStatus::PartiallyFilled,
        "4.38",
        "2",
    );
    let fills = [line
        .fill("t-1", &entry.id, "2", "234.25", entry.operation, None)
        .fill()
        .clone()];
    let evidence = Evidence::Order {
        order: &partial,
        fills: &fills,
    };
    let exposed = ledger
        .settle(&lease.fence(), &sent, evidence)
        .expect("settled");
    assert_eq!(
        exposed.record.intent().state(),
        IntentState::PartiallyFilled
    );

    // Long past every bound, on a venue that holds no such order and shows
    // a flat position: the proof that an entry was never placed.
    let venue = Venue::new(line.instrument().exchange, &clock);
    clock.set(T + 3_600_000);
    let asked = block_on(ledger.resume(&lease.fence(), &exposed, &venue, &venue)).expect("asked");
    assert_eq!(asked, Resumed::Unknown(exposed.clone()));
    assert_eq!(venue.asked(), 1);
    assert_eq!(
        store.unsettled_intents().expect("a read"),
        std::slice::from_ref(&exposed),
        "as it stood"
    );
    assert!(store.open_cycle(&line.key()).expect("a read").is_some());
    assert_eq!(count(backend, "execution_fills"), 1);
}

on_each_backend!(
    an_order_is_recorded_with_its_reservation_before_it_is_sent_and_settled_by_evidence,
    a_partly_filled_entry_the_venue_no_longer_lists_is_left_as_it_stands,
    an_entry_the_venue_refuses_opens_nothing_and_gives_its_margin_back,
    an_attempt_found_after_a_restart_is_asked_for_and_never_sent_again,
    an_entry_stays_unknown_without_the_whole_proof,
    an_order_the_venue_holds_is_settled_from_its_report_and_not_sent_again,
    a_recovered_orders_fills_are_its_own_by_the_venues_id_for_it,
    an_exit_may_be_sent_again_as_a_new_attempt_recorded_first,
);
