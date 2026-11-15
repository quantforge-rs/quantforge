//! One signal from its delivery to its first fill, on each backend: the
//! delivery recorded and admitted in one write, sent again, and sent with
//! another payload; its operation numbered; its order recorded under the
//! position's lease before it is sent; and its fill opening a cycle.

use quantforge::model::execution::{
    Change, Commit, EventKind, Expect, Found, OperationKind, OrderEvidence, Record, RecordKind,
    SignalState, Stale,
};
use quantforge::model::futures::{
    CancelAttribution, Delivery, FuturesOrder, FuturesOrderStatus, IntentState, OrderLeg,
    OrderOrigin, PositionCycleId, RecordSource,
};
use quantforge::model::{Quantity, VenueOrderId};
use quantforge::{ExecutionStore, Side, StoreActivation, StoreControl};
use rust_decimal::Decimal;

use crate::fixtures::{fenced, line, submitting};
use crate::support::{Backend, T, count, on_each_backend};

fn one_signal_from_its_delivery_to_its_first_fill<B: Backend>(backend: &B) {
    let store = backend.open();
    let line = line("bybit linear");
    let scope = line.scope();
    let active = store.activate(T).expect("activated");
    assert_eq!(
        (active.activation, active.activation_epoch.0),
        (StoreActivation::Active, 1)
    );

    // The delivery, its judgment, its intent and its event: one write.
    let delivered = line.delivery("entry", "trend", 0, "234.23");
    let admission = line.admission(&delivered, T + 6_000);
    assert_eq!(store.commit(&admission).expect("a commit"), Commit::Applied);
    let signal = store
        .signal(&scope, &delivered.signal.id)
        .expect("a read")
        .expect("the signal");
    assert_eq!(
        (signal.record.state(), signal.revision),
        (SignalState::Admitted, 1)
    );
    let intent = store
        .intent(&scope, &delivered.signal.id)
        .expect("a read")
        .expect("the intent");
    assert_eq!(intent.record.intent().state(), IntentState::Admitted);
    assert_eq!(intent.record.intent().policy(), Some(&line.snapshot()));

    // Sent again, it is the same signal: noted, and nothing else changes.
    match store.commit(&admission).expect("a commit") {
        Commit::Duplicate(original) => assert_eq!(original, signal),
        other => panic!("a duplicate expected: {other:?}"),
    }
    // Another price under the same ID is a conflict, told once.
    let altered = line.delivery("entry", "trend", 0, "999.99");
    assert_eq!(altered.signal.id, delivered.signal.id);
    for _ in 0..2 {
        match store
            .commit(&line.admission(&altered, T + 7_000))
            .expect("a commit")
        {
            Commit::Conflict(original) => assert_eq!(original, signal),
            other => panic!("a conflict expected: {other:?}"),
        }
    }
    let noted: Vec<Delivery> = store
        .deliveries(&scope, &delivered.signal.id)
        .expect("the deliveries")
        .iter()
        .map(|delivery| delivery.disposition)
        .collect();
    assert_eq!(
        noted,
        [
            Delivery::New,
            Delivery::Duplicate,
            Delivery::Conflict,
            Delivery::Conflict
        ]
    );
    assert_eq!(count(backend, "signal_inbox"), 1);
    assert_eq!(count(backend, "execution_intents"), 1);
    let told: Vec<EventKind> = store
        .pending_events(10)
        .expect("the events")
        .iter()
        .map(|event| event.record.kind())
        .collect();
    assert_eq!(
        told,
        [EventKind::SignalAdmitted, EventKind::SignalConflict],
        "the conflict is told once however often it is delivered"
    );

    // The operation is numbered once: asked again, it is the same one.
    let asked = line.operation(&delivered, OperationKind::Entry);
    let operation = store.begin_operation(&asked, T + 8_000).expect("numbered");
    let again = store.begin_operation(&asked, T + 9_000).expect("found");
    assert_eq!(again, operation);
    let number = operation.record.id();
    assert_eq!(number.get(), 1);

    // The order's identity is recorded, under the lease, before it is sent.
    let lease = line.lease(&store, "executor-a", T + 8_000);
    let id = line.client_order_id(active.activation_epoch, number, OrderLeg::Entry, 1);
    let sent = submitting(&intent.record, &id, T + 8_500);
    let order = line.order(number, &id, T + 8_500);
    let unit = fenced(&lease.fence(), T + 8_500)
        .with(Change::replace(
            Record::Intent(sent.clone()),
            intent.revision,
        ))
        .with(Change::new(Record::Order(order.clone())));
    assert_eq!(store.commit(&unit).expect("a commit"), Commit::Applied);
    // The same write again finds the intent moved, and changes nothing.
    assert_eq!(
        store.commit(&unit).expect("a commit"),
        Commit::Stale(Stale::Record {
            kind: RecordKind::Intent,
            name: format!("main/{}", delivered.signal.id),
            found: Found::Present { revision: 2 },
        })
    );
    let held = store.position_intents(&line.key()).expect("the intents");
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].record, sent);
    assert_eq!(held[0].record.intent().state(), IntentState::Submitting);
    assert!(held[0].record.blocks_entries());

    // The venue's report, a fill and the cycle it opens: one write.
    let reported = FuturesOrder {
        instrument: line.instrument(),
        venue_order_id: VenueOrderId::new("8389765519").expect("an order id"),
        client_id: Some(id.to_string()),
        side: Side::Buy,
        reduce_only: false,
        origin: OrderOrigin::Client,
        status: FuturesOrderStatus::Filled,
        requested: Quantity::for_category(line.identity.category(), Decimal::new(438, 2))
            .expect("a quantity"),
        filled: Quantity::for_category(line.identity.category(), Decimal::new(438, 2))
            .expect("a quantity"),
        average_price: None,
        source: RecordSource::Query,
        updated_at_ms: T + 9_000,
    };
    let mut filled = sent.intent().clone();
    filled.on_order(&reported).expect("filled");
    let cycle = line.cycle(number, T + 9_000);
    let filled = sent
        .with_intent(filled)
        .and_then(|record| record.in_cycle(cycle.id()))
        .expect("a successor");
    let settled = order
        .reported(OrderEvidence::Futures(Box::new(reported)))
        .expect("reported");
    let fill = line.fill("t-1", &id, "4.38", "234.25", number, Some(cycle.id()));
    let unit = fenced(&lease.fence(), T + 9_500)
        .with(Change::replace(Record::Intent(filled), 2))
        .with(Change::replace(Record::Order(settled), 1))
        .with(Change::new(Record::Fill(fill.clone())))
        .with(Change::new(Record::Cycle(cycle.clone())));
    assert_eq!(store.commit(&unit).expect("a commit"), Commit::Applied);

    // A process that starts on the store finds all of it.
    let restarted = backend.open();
    let open = restarted
        .open_cycle(&line.key())
        .expect("a read")
        .expect("the open cycle");
    assert_eq!((open.record, open.revision), (cycle.clone(), 1));
    assert_eq!(restarted.fills(cycle.id()).expect("the fills"), [fill]);
    assert_eq!(
        restarted.unsettled_intents().expect("the intents"),
        [],
        "a filled intent is settled"
    );
    let orders = restarted.orders(number).expect("the orders");
    assert_eq!(orders.len(), 1);
    assert_eq!(orders[0].revision, 2);
    assert_eq!(
        restarted
            .cycle(PositionCycleId::new(number.get()).expect("a cycle id"))
            .expect("a read")
            .map(|stored| stored.record),
        Some(cycle)
    );
    assert!(store.release_lease(&lease.fence()).expect("released"));
    assert_eq!(restarted.lease(&line.key()).expect("a read"), None);
    // A change that expects no record finds the one recorded.
    let refused = fenced(&lease.fence(), T + 9_900).with(Change {
        record: Record::Cycle(line.cycle(number, T + 9_000)),
        expect: Expect::Absent,
    });
    assert!(matches!(
        store.commit(&refused).expect("a commit"),
        Commit::Fenced(_)
    ));
    let _ = CancelAttribution::ByUser;
}

on_each_backend!(one_signal_from_its_delivery_to_its_first_fill);
