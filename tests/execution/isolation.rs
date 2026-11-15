//! Two profiles that share everything a signal can name: one profile
//! name, one symbol, one alert, byte for byte. Only the venue and the
//! account tell them apart, and nothing of one ever reaches the other:
//! no delivery, no intent, no order, no fill, no reservation, no lease
//! and no event.

use quantforge::automation::{Admission, Evidence, ExecutionLedger};
use quantforge::model::Asset;
use quantforge::model::execution::{Change, Commit, EventKind, OperationKind, Record, SignalState};
use quantforge::model::futures::{Delivery, FuturesOrderStatus, IntentState, OrderLeg, PoolScope};
use quantforge::{ExecutionStore, StoreControl};

use crate::fixtures::{fenced, linear_pair, reported};
use crate::support::{Backend, Manual, T, on_each_backend};

fn two_venues_profiles_with_one_name_one_symbol_and_one_alert_never_meet<B: Backend>(backend: &B) {
    let store = backend.open();
    let active = store.activate(T).expect("activated");
    let clock = Manual::at(T + 6_000);
    let ledger = ExecutionLedger::new(&store, &clock);
    let pair = linear_pair();
    let [bybit, other] = &pair;

    // What a signal names is the same on both, to the byte.
    let alert = pair
        .each_ref()
        .map(|line| line.delivery("entry", "trend", 0, "234.23"));
    assert_eq!(bybit.identity.id(), other.identity.id());
    assert_eq!(bybit.instrument().symbol, other.instrument().symbol);
    assert_eq!(alert[0].signal, alert[1].signal);
    assert_eq!(alert[0].payload, alert[1].payload);
    assert_ne!(bybit.scope(), other.scope());
    assert_ne!(bybit.key(), other.key());

    // Each profile's inbox takes it as its own first delivery.
    for (line, delivered) in pair.iter().zip(&alert) {
        let resolved = line.resolved(delivered);
        let admission = ledger
            .admit(
                &line.identity,
                &delivered.signal,
                &delivered.payload,
                &resolved,
            )
            .expect("admitted");
        assert_eq!(admission.delivery(), Delivery::New, "{}", line.name);
        assert_eq!(admission.record().state(), SignalState::Admitted);
        assert_eq!(admission.record().scope(), &line.scope());
    }
    // Another price on one venue conflicts there, and only there.
    let altered = bybit.delivery("entry", "trend", 0, "999");
    let conflict = ledger
        .admit(
            &bybit.identity,
            &altered.signal,
            &altered.payload,
            &bybit.resolved(&altered),
        )
        .expect("answered");
    assert!(matches!(conflict, Admission::Conflict(_)));
    let again = ledger
        .admit(
            &other.identity,
            &alert[1].signal,
            &alert[1].payload,
            &other.resolved(&alert[1]),
        )
        .expect("answered");
    assert_eq!(
        again.delivery(),
        Delivery::Duplicate,
        "its own, and no conflict"
    );
    let noted = |line: &crate::fixtures::Line| -> Vec<Delivery> {
        store
            .deliveries(&line.scope(), &alert[0].signal.id)
            .expect("the deliveries")
            .iter()
            .map(|delivery| delivery.disposition)
            .collect()
    };
    assert_eq!(noted(bybit), [Delivery::New, Delivery::Conflict]);
    assert_eq!(noted(other), [Delivery::New, Delivery::Duplicate]);

    // One key text, two operations: each account's own.
    let operations = [0, 1].map(|index| {
        let asked = pair[index].operation(&alert[index], OperationKind::Entry);
        store
            .begin_operation(&asked, T + 7_000)
            .expect("numbered")
            .record
    });
    assert_eq!(operations[0].key(), operations[1].key());
    assert_ne!(operations[0].id(), operations[1].id());
    assert_eq!(operations[0].scope(), &bybit.scope());
    assert_eq!(operations[1].scope(), &other.scope());

    // Both positions are leased at once, to different workers, and both
    // entries reserve from "the USDT pool": each from its own account's.
    let usdt = PoolScope::Asset(Asset::new("USDT").expect("an asset"));
    let leases =
        [0, 1].map(|index| pair[index].lease(&store, &format!("executor-{index}"), T + 7_000));
    clock.set(T + 8_000);
    let mut sent = Vec::new();
    for index in [0, 1] {
        let line = &pair[index];
        let intent = store
            .intent(&line.scope(), &alert[index].signal.id)
            .expect("a read")
            .expect("the intent");
        let id = line.client_order_id(
            active.activation_epoch,
            operations[index].id(),
            OrderLeg::Entry,
            1,
        );
        let held = line.reservation(operations[index].id(), "205.18548", T + 8_000);
        let stored = ledger
            .record_attempt(&leases[index].fence(), &intent, &id, Some((held, 0)))
            .expect("recorded: its own pool is untouched");
        assert!(stored.record.blocks_entries(), "{}", line.name);
        sent.push((stored, id));
    }
    for line in &pair {
        let pool = store
            .pool(line.identity.account_ref(), &usdt)
            .expect("the pool");
        assert_eq!((pool.revision, pool.held.len()), (1, 1), "{}", line.name);
        assert_eq!(pool.held[0].scope(), &line.scope());
    }

    // The same client order ID and the same trade ID under the other
    // account are other records, and neither is found from the wrong one.
    let (bybit_sent, bybit_id) = &sent[0];
    let twin = other.order(operations[1].id(), bybit_id, T + 8_500);
    let unit = fenced(&leases[1].fence(), T + 8_500).with(Change::new(Record::Order(twin)));
    assert_eq!(store.commit(&unit).expect("a commit"), Commit::Applied);
    for (line, operation) in pair.iter().zip(&operations) {
        let found = store
            .order(&line.scope(), &bybit_id.to_string())
            .expect("a read")
            .expect("its own order");
        assert_eq!(found.record.operation(), operation.id(), "{}", line.name);
    }

    // One venue's order fills. Its cycle, its fill and its events are its
    // own; the other venue's entry is where it was.
    clock.set(T + 9_500);
    let order = reported(bybit, bybit_id, FuturesOrderStatus::Filled, "4.38", "4.38");
    let fills = [bybit
        .fill("t-1", bybit_id, "4.38", "234.25", operations[0].id(), None)
        .fill()
        .clone()];
    let evidence = Evidence::Order {
        order: &order,
        fills: &fills,
    };
    let filled = ledger
        .settle(&leases[0].fence(), bybit_sent, evidence)
        .expect("settled");
    assert_eq!(filled.record.intent().state(), IntentState::Filled);
    assert!(store.open_cycle(&bybit.key()).expect("a read").is_some());
    assert_eq!(store.open_cycle(&other.key()).expect("a read"), None);
    let trade = &fills[0].trade_id;
    assert!(
        store
            .fill(&bybit.scope(), &bybit.instrument(), trade)
            .expect("a read")
            .is_some()
    );
    assert_eq!(
        store
            .fill(&other.scope(), &other.instrument(), trade)
            .expect("a read"),
        None
    );
    let waiting = store
        .intent(&other.scope(), &alert[1].signal.id)
        .expect("a read")
        .expect("the intent");
    assert_eq!(waiting.record.intent().state(), IntentState::Submitting);
    assert_eq!(waiting.revision, sent[1].0.revision, "untouched");
    let pool = |line: &crate::fixtures::Line| {
        let pool = store
            .pool(line.identity.account_ref(), &usdt)
            .expect("the pool");
        (pool.revision, pool.held.len())
    };
    assert_eq!((pool(bybit), pool(other)), ((2, 0), (1, 1)));

    // One lease is released; the other still fences its own position.
    assert!(store.release_lease(&leases[0].fence()).expect("released"));
    assert!(store.lease(&other.key()).expect("a read").is_some());

    // Every event is told under its own scope, under keys that match word
    // for word across the two.
    let events = store.pending_events(100).expect("the events");
    let told = |line: &crate::fixtures::Line| -> Vec<(EventKind, String)> {
        events
            .iter()
            .filter(|event| event.record.scope() == &line.scope())
            .map(|event| (event.record.kind(), event.record.key().to_string()))
            .collect()
    };
    let (ours, theirs) = (told(bybit), told(other));
    assert_eq!(ours.len() + theirs.len(), events.len());
    assert_eq!(
        ours.iter().map(|(kind, _)| *kind).collect::<Vec<_>>(),
        [
            EventKind::SignalAdmitted,
            EventKind::SignalConflict,
            EventKind::OrderSubmitted,
            EventKind::OrderSettled
        ]
    );
    assert_eq!(
        theirs.iter().map(|(kind, _)| *kind).collect::<Vec<_>>(),
        [EventKind::SignalAdmitted, EventKind::OrderSubmitted]
    );
    assert_eq!(ours[0].1, theirs[0].1, "one key, two events");

    // And in the tables themselves, every row names its own venue.
    for table in [
        "signal_inbox",
        "signal_deliveries",
        "execution_operations",
        "execution_intents",
        "execution_orders",
        "margin_reservations",
        "outbox_events",
    ] {
        let venues = backend.select(&format!(
            "SELECT venue, CAST(COUNT(*) AS TEXT) FROM {table} GROUP BY venue ORDER BY venue"
        ));
        let named: Vec<&str> = venues
            .iter()
            .map(|row| row[0].as_deref().unwrap_or_default())
            .collect();
        let mut expected = pair
            .each_ref()
            .map(|line| line.identity.account_ref().venue().as_str());
        expected.sort_unstable();
        assert_eq!(named, expected, "{table}: {venues:?}");
    }
}

on_each_backend!(two_venues_profiles_with_one_name_one_symbol_and_one_alert_never_meet);
