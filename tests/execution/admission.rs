//! Admission through the ledger, on each backend: a delivery recorded
//! once with what became of it; the same delivery again, and another
//! payload under its ID; a signal recorded first and judged later; and
//! what the store's records refuse.

use quantforge::automation::{Admission, Evidence, ExecutionLedger};
use quantforge::model::Refusal;
use quantforge::model::execution::{
    Change, Commit, EventKind, Halt, Incident, IncidentKey, IncidentKind, OperationKind, Record,
    SignalState,
};
use quantforge::model::futures::{
    Delivery, FuturesOrderStatus, IntentState, OrderLeg, OwnerTag, ProfileId, ProfileIdentity,
    SignalRefusal,
};
use quantforge::{ExecutionStore, StoreControl};

use crate::fixtures::{Line, line, reported};
use crate::support::{Backend, Manual, T, count, on_each_backend};

fn kinds(store: &impl ExecutionStore) -> Vec<EventKind> {
    store
        .pending_events(100)
        .expect("the events")
        .iter()
        .map(|event| event.record.kind())
        .collect()
}

fn a_delivery_is_recorded_once_whatever_is_sent_after_it<B: Backend>(backend: &B) {
    let store = backend.open();
    let clock = Manual::at(T + 6_000);
    let ledger = ExecutionLedger::new(&store, &clock);
    let line = line("bybit linear");
    let delivered = line.delivery("entry", "trend", 0, "234.23");
    let resolved = line.resolved(&delivered);

    let first = ledger
        .admit(
            &line.identity,
            &delivered.signal,
            &delivered.payload,
            &resolved,
        )
        .expect("admitted");
    let Admission::Recorded(record) = &first else {
        panic!("the first delivery is recorded: {first:?}");
    };
    assert_eq!(record.state(), SignalState::Admitted);
    assert_eq!(record.state().answered(first.delivery()), "admitted");

    // TradingView sends it again five seconds later: the same signal.
    clock.advance(5_000);
    let again = ledger
        .admit(
            &line.identity,
            &delivered.signal,
            &delivered.payload,
            &resolved,
        )
        .expect("answered");
    assert_eq!(again, Admission::Duplicate(record.clone()));
    assert_eq!(record.state().answered(again.delivery()), "duplicate");

    // The same alert on the same bar with another price: a conflict.
    let altered = line.delivery("entry", "trend", 0, "235.00");
    let conflict = ledger
        .admit(
            &line.identity,
            &altered.signal,
            &altered.payload,
            &line.resolved(&altered),
        )
        .expect("answered");
    assert_eq!(conflict, Admission::Conflict(record.clone()));
    assert_eq!(conflict.delivery().refusal(), Some(Refusal::Conflict));
    assert_eq!(record.state().answered(conflict.delivery()), "rejected");

    // One signal, one intent, three deliveries noted, and two events.
    assert_eq!(count(backend, "signal_inbox"), 1);
    assert_eq!(count(backend, "execution_intents"), 1);
    assert_eq!(count(backend, "signal_deliveries"), 3);
    assert_eq!(
        kinds(&store),
        [EventKind::SignalAdmitted, EventKind::SignalConflict]
    );
    let held = store
        .intent(&line.scope(), &delivered.signal.id)
        .expect("a read")
        .expect("the intent");
    assert_eq!(held.record.intent().state(), IntentState::Admitted);
    assert_eq!(held.record.intent().policy(), Some(&line.snapshot()));
}

fn a_refused_signal_is_recorded_with_its_refusal_and_refused_the_same_way_again<B: Backend>(
    backend: &B,
) {
    let store = backend.open();
    let clock = Manual::at(T + 90_000);
    let ledger = ExecutionLedger::new(&store, &clock);
    let line = line("bybit linear");
    let delivered = line.delivery("entry", "trend", 0, "234.23");
    let stale = Err(SignalRefusal {
        refusal: Refusal::Stale,
        message: "the alert fired 85 s ago, and profile main takes entries up to 60 s old"
            .to_string(),
    });
    let first = ledger
        .admit(
            &line.identity,
            &delivered.signal,
            &delivered.payload,
            &stale,
        )
        .expect("recorded");
    let record = first.record().clone();
    assert_eq!(first.delivery(), Delivery::New);
    assert_eq!(record.state(), SignalState::Rejected);
    assert_eq!(
        record.refusal().map(|refused| refused.refusal),
        Some(Refusal::Stale)
    );
    // Delivered again, even resolved, it is the signal already refused.
    let again = ledger
        .admit(
            &line.identity,
            &delivered.signal,
            &delivered.payload,
            &line.resolved(&delivered),
        )
        .expect("answered");
    assert_eq!(again, Admission::Duplicate(record));
    assert_eq!(count(backend, "execution_intents"), 0, "nothing to execute");
    assert_eq!(kinds(&store), [EventKind::SignalRejected]);
    let event = &store.pending_events(1).expect("the events")[0].record;
    assert_eq!(
        event.facts().get("refusal").map(String::as_str),
        Some("stale")
    );
}

fn a_signal_recorded_first_is_judged_once_in_place<B: Backend>(backend: &B) {
    let store = backend.open();
    let clock = Manual::at(T + 6_000);
    let ledger = ExecutionLedger::new(&store, &clock);
    let line = line("bybit linear");
    let delivered = line.delivery("entry", "trend", 0, "234.23");

    // The route answers within its budget: recorded, not yet judged.
    let received = ledger
        .receive(&line.identity, &delivered.signal, &delivered.payload)
        .expect("recorded");
    assert_eq!(received.record().state(), SignalState::Received);
    assert_eq!(received.delivery(), Delivery::New);
    assert_eq!(
        store.received_signals(10).expect("a read").len(),
        1,
        "what a process that starts judges first"
    );
    assert_eq!(count(backend, "outbox_events"), 0);

    // A process that starts on the store judges it.
    let restarted = backend.open();
    let later = ExecutionLedger::new(&restarted, &clock);
    clock.advance(1_000);
    let judged = later
        .admit(
            &line.identity,
            &delivered.signal,
            &delivered.payload,
            &line.resolved(&delivered),
        )
        .expect("judged");
    assert_eq!(judged.delivery(), Delivery::Duplicate);
    assert_eq!(judged.record().state(), SignalState::Admitted);
    assert_eq!(judged.record().judged_at_ms(), Some(T + 7_000));
    assert_eq!(
        judged.record().received_at_ms(),
        T + 6_000,
        "when it was first recorded"
    );
    // Judged, it stays as it was judged.
    let refusal = Err(SignalRefusal {
        refusal: Refusal::Stale,
        message: "too old now".to_string(),
    });
    let again = later
        .admit(
            &line.identity,
            &delivered.signal,
            &delivered.payload,
            &refusal,
        )
        .expect("answered");
    assert_eq!(again.record(), judged.record());
    assert_eq!(restarted.received_signals(10).expect("a read"), []);
    assert_eq!(count(backend, "execution_intents"), 1);
    assert_eq!(kinds(&restarted), [EventKind::SignalAdmitted]);
}

/// A second writer cannot judge a signal twice by hand either: the
/// judgment is a compare and swap on the signal's record.
fn a_judgment_is_written_only_over_the_record_it_read<B: Backend>(backend: &B) {
    let store = backend.open();
    let line = line("bybit linear");
    let delivered = line.delivery("exit", "trend", 0, "0");
    let received = line.received(&delivered, T);
    let mut unit = quantforge::model::execution::ExecutionUnit::at(T);
    unit.delivery = Some(quantforge::model::execution::DeliveryWrite {
        conflict_event: line.conflict_event(&received),
        record: received.clone(),
    });
    assert_eq!(store.commit(&unit).expect("a commit"), Commit::Applied);

    let noop = received.noop("nothing to close", T + 1).expect("judged");
    let judge = |revision| {
        quantforge::model::execution::ExecutionUnit::at(T + 1)
            .with(Change::replace(Record::Signal(noop.clone()), revision))
    };
    assert_eq!(store.commit(&judge(1)).expect("a commit"), Commit::Applied);
    assert!(matches!(
        store.commit(&judge(1)).expect("a commit"),
        Commit::Stale(_)
    ));
    // Named at its new revision, a second judgment is no successor.
    let refused = store.commit(&judge(2)).expect_err("judged once");
    assert!(
        refused
            .to_string()
            .contains("signal cannot move from noop to noop"),
        "{refused}"
    );
}

fn open_incident(line: &Line, halts: Halt) -> Incident {
    Incident::open(
        line.scope(),
        IncidentKey::new("op/1/unsettled_entry").expect("a key"),
        IncidentKind::UnsettledEntry,
        halts,
        None,
        None,
        "the entry of operation 1 was still unsettled 60 s after it was sent",
        T,
    )
    .expect("an incident")
}

fn what_the_store_holds_refuses_an_entry_and_decides_what_an_exit_is_for<B: Backend>(backend: &B) {
    let store = backend.open();
    store.activate(T).expect("activated");
    let clock = Manual::at(T + 6_000);
    let ledger = ExecutionLedger::new(&store, &clock);
    let line = line("bybit linear");
    let admit = |kind: &str, source: &str, minute: u32| {
        let delivered = line.delivery(kind, source, minute, "234.23");
        let resolved = line.resolved(&delivered);
        let admission = ledger
            .admit(
                &line.identity,
                &delivered.signal,
                &delivered.payload,
                &resolved,
            )
            .expect("answered");
        (delivered, admission.record().clone())
    };
    let refusal = |record: &quantforge::model::execution::SignalRecord| {
        record.refusal().map(|refused| refused.refusal)
    };

    // An exit with no position of its own to close has nothing to do.
    let (_, early_exit) = admit("exit", "trend", 0);
    assert_eq!(early_exit.state(), SignalState::Noop);
    assert_eq!(
        early_exit.note(),
        Some("profile main holds no long position on SOLUSDT that this exit is for")
    );

    // An incident on the instrument halts entries there until cleared.
    let incident = open_incident(&line, Halt::Position(line.instrument()));
    let raised = quantforge::model::execution::ExecutionUnit::at(T)
        .with(Change::new(Record::Incident(incident.clone())));
    assert_eq!(store.commit(&raised).expect("a commit"), Commit::Applied);
    let (_, halted) = admit("entry", "halted", 1);
    assert_eq!(refusal(&halted), Some(Refusal::IncidentOpen));
    let cleared = incident
        .clear("flat, checked by hand", T + 1)
        .expect("cleared");
    let clearing = quantforge::model::execution::ExecutionUnit::at(T + 1)
        .with(Change::replace(Record::Incident(cleared), 1));
    assert_eq!(store.commit(&clearing).expect("a commit"), Commit::Applied);
    assert_eq!(store.open_incidents().expect("a read"), []);

    // The first entry is admitted; while it is unresolved, no other is.
    let (entry, admitted) = admit("entry", "trend", 2);
    assert_eq!(admitted.state(), SignalState::Admitted);
    let intent = store
        .intent(&line.scope(), &entry.signal.id)
        .expect("a read")
        .expect("the intent");
    let operation = store
        .begin_operation(
            &line.operation(&entry, quantforge::model::execution::OperationKind::Entry),
            T + 7_000,
        )
        .expect("numbered")
        .record
        .id();
    let lease = line.lease(&store, "executor-a", T + 7_000);
    let epoch = store.store_identity().expect("identity").activation_epoch;
    let id = line.client_order_id(
        epoch,
        operation,
        quantforge::model::futures::OrderLeg::Entry,
        1,
    );
    let sent = ledger
        .record_attempt(&lease.fence(), &intent, &id, None)
        .expect("recorded");
    let (_, second) = admit("entry", "second", 3);
    assert_eq!(refusal(&second), Some(Refusal::UnresolvedEntry));

    // An exit that fired after the entry waits for it; one from before
    // the entry was never meant for it.
    let (late_exit, waits) = admit("exit", "trend", 4);
    assert_eq!(waits.state(), SignalState::Admitted);
    let waiting = store
        .intent(&line.scope(), &late_exit.signal.id)
        .expect("a read")
        .expect("the exit's intent");
    assert_eq!(waiting.record.cycle(), None, "no cycle to name yet");
    let (_, earlier) = admit("exit", "other", 1);
    assert_eq!(earlier.state(), SignalState::Noop);

    // Filled, the entry holds the position: more of it is pyramiding.
    let order = crate::fixtures::reported(
        &line,
        &id,
        quantforge::model::futures::FuturesOrderStatus::Filled,
        "4.38",
        "4.38",
    );
    let fills = [line
        .fill("t-1", &id, "4.38", "234.25", operation, None)
        .fill()
        .clone()];
    let evidence = quantforge::automation::Evidence::Order {
        order: &order,
        fills: &fills,
    };
    let filled = ledger
        .settle(&lease.fence(), &sent, evidence)
        .expect("settled");
    let cycle = filled.record.cycle().expect("the cycle it opened");
    let (_, more) = admit("entry", "more", 5);
    assert_eq!(refusal(&more), Some(Refusal::Pyramiding));
    // An exit now names the cycle it closes.
    let (bound_exit, bound) = admit("exit", "trend", 6);
    assert_eq!(bound.state(), SignalState::Admitted);
    let closing = store
        .intent(&line.scope(), &bound_exit.signal.id)
        .expect("a read")
        .expect("the exit's intent");
    assert_eq!(closing.record.cycle(), Some(cycle));
}

/// Two profiles of one account take the same alerts. The first to fill
/// holds the position; the other's exit, though its own entry carries the
/// very signal ID that opened the position, has nothing to close.
fn an_exit_never_closes_another_profiles_position_on_its_account<B: Backend>(backend: &B) {
    let store = backend.open();
    store.activate(T).expect("activated");
    let clock = Manual::at(T + 6_000);
    let ledger = ExecutionLedger::new(&store, &clock);
    let main = line("bybit linear");
    let second = Line {
        identity: ProfileIdentity::new(
            ProfileId::new("second").expect("a profile id"),
            main.identity.account_ref().clone(),
            main.identity.category(),
            OwnerTag::new("a1b2c3").expect("an owner tag"),
        )
        .expect("an identity"),
        ..main.clone()
    };
    let admit = |line: &Line, kind: &str, minute: u32| {
        let delivered = line.delivery(kind, "trend", minute, "234.23");
        let resolved = line.resolved(&delivered);
        let admission = ledger
            .admit(
                &line.identity,
                &delivered.signal,
                &delivered.payload,
                &resolved,
            )
            .expect("answered");
        (delivered, admission.record().clone())
    };

    // One alert, both profiles: one signal ID in two inboxes.
    let (entry, mine) = admit(&main, "entry", 0);
    let (same_alert, theirs) = admit(&second, "entry", 0);
    assert_eq!(entry.signal.id, same_alert.signal.id);
    assert_eq!(mine.state(), SignalState::Admitted);
    assert_eq!(theirs.state(), SignalState::Admitted);

    // The first profile's entry fills, and opens the position's cycle.
    let intent = store
        .intent(&main.scope(), &entry.signal.id)
        .expect("a read")
        .expect("the intent");
    let operation = store
        .begin_operation(&main.operation(&entry, OperationKind::Entry), T + 7_000)
        .expect("numbered")
        .record
        .id();
    let lease = main.lease(&store, "executor-a", T + 7_000);
    let epoch = store.store_identity().expect("identity").activation_epoch;
    let id = main.client_order_id(epoch, operation, OrderLeg::Entry, 1);
    let sent = ledger
        .record_attempt(&lease.fence(), &intent, &id, None)
        .expect("recorded");
    let filled = reported(&main, &id, FuturesOrderStatus::Filled, "4.38", "4.38");
    let fills = [main
        .fill("t-1", &id, "4.38", "234.25", operation, None)
        .fill()
        .clone()];
    let evidence = Evidence::Order {
        order: &filled,
        fills: &fills,
    };
    let settled = ledger
        .settle(&lease.fence(), &sent, evidence)
        .expect("settled");
    let cycle = settled.record.cycle().expect("the cycle it opened");

    // Its own exit closes it; the other profile's has nothing to close.
    clock.set(T + 70_000);
    let (_, not_theirs) = admit(&second, "exit", 1);
    assert_eq!(not_theirs.state(), SignalState::Noop);
    assert_eq!(
        not_theirs.note(),
        Some("profile second holds no long position on SOLUSDT that this exit is for")
    );
    let (leaving, own) = admit(&main, "exit", 1);
    assert_eq!(own.state(), SignalState::Admitted);
    let closing = store
        .intent(&main.scope(), &leaving.signal.id)
        .expect("a read")
        .expect("the exit's intent");
    assert_eq!(closing.record.cycle(), Some(cycle));
    assert_eq!(
        store
            .intent(&second.scope(), &leaving.signal.id)
            .expect("a read"),
        None
    );
}

on_each_backend!(
    a_delivery_is_recorded_once_whatever_is_sent_after_it,
    a_refused_signal_is_recorded_with_its_refusal_and_refused_the_same_way_again,
    a_signal_recorded_first_is_judged_once_in_place,
    a_judgment_is_written_only_over_the_record_it_read,
    what_the_store_holds_refuses_an_entry_and_decides_what_an_exit_is_for,
    an_exit_never_closes_another_profiles_position_on_its_account,
);
