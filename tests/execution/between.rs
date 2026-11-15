//! What changes between a look and a write, on each backend. Admission
//! looks at a position without its lease, in two reads, and then writes;
//! the tests here let another writer act exactly between those steps, as a
//! fill arriving at that moment would, and hold the ledger to what the
//! store holds at the end. And an entry is held to the store's rules once
//! more when its order is recorded, under the lease, whatever admission
//! saw.

use std::sync::Mutex;

use quantforge::automation::{Admission, Evidence, ExecutionLedger, LedgerError};
use quantforge::model::execution::{
    Change, Commit, CycleRecord, EventKey, ExecutionScope, ExecutionUnit, Fence, FillRecord, Halt,
    Incident, IncidentKey, IncidentKind, IntentRecord, Lease, LeaseAnswer, LeaseRequest,
    NewOperation, OperationRecord, OrderRecord, OutboxEvent, PoolReservations, Record, Reservation,
    SignalDelivery, SignalRecord, SignalState, Stored,
};
use quantforge::model::futures::{
    FuturesOrderStatus, IntentState, PoolScope, PositionCycleId, PositionKey, SignalId,
};
use quantforge::model::{AccountRef, InstrumentId, OperationId, Refusal, VenueTradeId};
use quantforge::ports::StorageError;
use quantforge::{ExecutionStore, StoreControl};

use crate::fixtures::{Line, line, reported};
use crate::orders::{Started, started};
use crate::support::{Backend, Manual, T, count, on_each_backend};

/// One of the two reads admission judges a position by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Read {
    PositionIntents,
    OpenCycle,
}

/// A store in which something happens once, just before one of its reads:
/// what another writer does between two looks of the one under test.
struct Meanwhile<'s, S> {
    store: &'s S,
    before: Read,
    happens: Mutex<Option<Box<dyn FnOnce() + Send + 's>>>,
}

impl<'s, S: ExecutionStore> Meanwhile<'s, S> {
    fn new(store: &'s S, before: Read, happens: impl FnOnce() + Send + 's) -> Self {
        Self {
            store,
            before,
            happens: Mutex::new(Some(Box::new(happens))),
        }
    }

    fn now(&self, read: Read) {
        if read != self.before {
            return;
        }
        let happens = self.happens.lock().expect("what happens").take();
        if let Some(happens) = happens {
            happens();
        }
    }

    fn happened(&self) -> bool {
        self.happens.lock().expect("what happens").is_none()
    }
}

/// Every call of the port passed on as it is.
macro_rules! passed_on {
    ($($name:ident($($arg:ident: $kind:ty),*) -> $answer:ty;)*) => {$(
        fn $name(&self, $($arg: $kind),*) -> Result<$answer, StorageError> {
            self.store.$name($($arg),*)
        }
    )*};
}

impl<S: ExecutionStore> ExecutionStore for Meanwhile<'_, S> {
    fn position_intents(
        &self,
        key: &PositionKey,
    ) -> Result<Vec<Stored<IntentRecord>>, StorageError> {
        self.now(Read::PositionIntents);
        self.store.position_intents(key)
    }

    fn open_cycle(&self, key: &PositionKey) -> Result<Option<Stored<CycleRecord>>, StorageError> {
        self.now(Read::OpenCycle);
        self.store.open_cycle(key)
    }

    passed_on! {
        commit(unit: &ExecutionUnit) -> Commit;
        begin_operation(operation: &NewOperation, at_ms: i64) -> Stored<OperationRecord>;
        acquire_lease(request: &LeaseRequest) -> LeaseAnswer;
        renew_lease(fence: &Fence, ttl_ms: i64, now_ms: i64) -> Option<Lease>;
        release_lease(fence: &Fence) -> bool;
        lease(key: &PositionKey) -> Option<Lease>;
        signal(scope: &ExecutionScope, signal: &SignalId) -> Option<Stored<SignalRecord>>;
        received_signals(limit: usize) -> Vec<Stored<SignalRecord>>;
        deliveries(scope: &ExecutionScope, signal: &SignalId) -> Vec<SignalDelivery>;
        intent(scope: &ExecutionScope, signal: &SignalId) -> Option<Stored<IntentRecord>>;
        unsettled_intents() -> Vec<Stored<IntentRecord>>;
        operation(id: OperationId) -> Option<Stored<OperationRecord>>;
        unsettled_replacements() -> Vec<Stored<OperationRecord>>;
        order(scope: &ExecutionScope, client_order_id: &str) -> Option<Stored<OrderRecord>>;
        orders(operation: OperationId) -> Vec<Stored<OrderRecord>>;
        fill(scope: &ExecutionScope, instrument: &InstrumentId, trade: &VenueTradeId)
            -> Option<FillRecord>;
        fills(cycle: PositionCycleId) -> Vec<FillRecord>;
        cycle(id: PositionCycleId) -> Option<Stored<CycleRecord>>;
        open_cycles() -> Vec<Stored<CycleRecord>>;
        pool(account: &AccountRef, pool: &PoolScope) -> PoolReservations;
        reservation(operation: OperationId) -> Option<Stored<Reservation>>;
        incident(scope: &ExecutionScope, key: &IncidentKey) -> Option<Stored<Incident>>;
        open_incidents() -> Vec<Stored<Incident>>;
        event(scope: &ExecutionScope, key: &EventKey) -> Option<Stored<OutboxEvent>>;
        pending_events(limit: usize) -> Vec<Stored<OutboxEvent>>;
    }
}

/// An entry on `line`, recorded as sent under its position's lease.
struct Sent {
    entry: Started,
    lease: Lease,
    intent: Stored<IntentRecord>,
}

fn sent(
    store: &(impl ExecutionStore + StoreControl),
    ledger: &ExecutionLedger<'_>,
    line: &Line,
) -> Sent {
    let entry = started(store, ledger, line, "entry", "trend", 0);
    let lease = line.lease(store, "executor-a", T + 7_000);
    let intent = ledger
        .record_attempt(&lease.fence(), &entry.intent, &entry.id, None)
        .expect("recorded");
    Sent {
        entry,
        lease,
        intent,
    }
}

/// Delivers a `kind` signal of `source` to `line`'s profile through
/// `ledger`, and answers the signal as it was recorded.
fn deliver(ledger: &ExecutionLedger<'_>, line: &Line, kind: &str, source: &str) -> SignalRecord {
    let delivered = line.delivery(kind, source, 1, "234.23");
    let admission = ledger
        .admit(
            &line.identity,
            &delivered.signal,
            &delivered.payload,
            &line.resolved(&delivered),
        )
        .expect("answered");
    assert!(matches!(admission, Admission::Recorded(_)), "{admission:?}");
    admission.record().clone()
}

/// An exit that arrives just as its entry fills closes the position that
/// fill opened, whichever of admission's two reads the fill lands before.
/// Read the open cycle first and the unsettled intents second, and a fill
/// between the two is seen by neither: the exit is recorded as having
/// nothing to do, for good, and the position stays open.
fn an_exit_that_arrives_as_its_entry_fills_closes_the_position_it_opened<B: Backend>(backend: &B) {
    let store = backend.open();
    store.activate(T).expect("activated");
    let clock = Manual::at(T + 6_000);
    let ledger = ExecutionLedger::new(&store, &clock);
    let symbols = [
        (Read::PositionIntents, "SOLUSDT"),
        (Read::OpenCycle, "BTCUSDT"),
    ];
    for (before, symbol) in symbols {
        clock.set(T + 6_000);
        let line = line("bybit linear").trading(symbol);
        let sent = sent(&store, &ledger, &line);
        let id = &sent.entry.id;
        let filled = reported(&line, id, FuturesOrderStatus::Filled, "4.38", "4.38");
        let fills = [line
            .fill("t-1", id, "4.38", "234.25", sent.entry.operation, None)
            .fill()
            .clone()];
        let fence = sent.lease.fence();
        let racing = Meanwhile::new(&store, before, || {
            let evidence = Evidence::Order {
                order: &filled,
                fills: &fills,
            };
            ledger
                .settle(&fence, &sent.intent, evidence)
                .expect("the entry fills");
        });

        clock.set(T + 70_000);
        let exit = deliver(
            &ExecutionLedger::new(&racing, &clock),
            &line,
            "exit",
            "trend",
        );
        assert!(racing.happened(), "the fill landed before {before:?}");
        assert_eq!(exit.state(), SignalState::Admitted, "{before:?}: {exit:?}");
        let open = store
            .open_cycle(&line.key())
            .expect("a read")
            .expect("the cycle the fill opened");
        let closing = store
            .intent(&line.scope(), exit.id())
            .expect("a read")
            .expect("the exit's intent");
        assert_eq!(closing.record.cycle(), Some(open.record.id()), "{before:?}");
    }
    assert_eq!(count(backend, "position_cycles"), 2);
}

/// An entry that arrives just as another fills is refused for the position
/// that fill opened, whichever read the fill lands before.
fn an_entry_that_arrives_as_another_fills_is_refused_for_the_position_it_opened<B: Backend>(
    backend: &B,
) {
    let store = backend.open();
    store.activate(T).expect("activated");
    let clock = Manual::at(T + 6_000);
    let ledger = ExecutionLedger::new(&store, &clock);
    let symbols = [
        (Read::PositionIntents, "SOLUSDT"),
        (Read::OpenCycle, "BTCUSDT"),
    ];
    for (before, symbol) in symbols {
        clock.set(T + 6_000);
        let line = line("bybit linear").trading(symbol);
        let sent = sent(&store, &ledger, &line);
        let id = &sent.entry.id;
        let filled = reported(&line, id, FuturesOrderStatus::Filled, "4.38", "4.38");
        let fills = [line
            .fill("t-1", id, "4.38", "234.25", sent.entry.operation, None)
            .fill()
            .clone()];
        let fence = sent.lease.fence();
        let racing = Meanwhile::new(&store, before, || {
            let evidence = Evidence::Order {
                order: &filled,
                fills: &fills,
            };
            ledger
                .settle(&fence, &sent.intent, evidence)
                .expect("the entry fills");
        });

        let late = deliver(
            &ExecutionLedger::new(&racing, &clock),
            &line,
            "entry",
            "other",
        );
        assert!(racing.happened(), "the fill landed before {before:?}");
        assert_eq!(late.state(), SignalState::Rejected, "{before:?}: {late:?}");
        assert_eq!(
            late.refusal().map(|refused| refused.refusal),
            Some(Refusal::Pyramiding),
            "{before:?}"
        );
    }
    assert_eq!(
        count(backend, "execution_intents"),
        2,
        "one for each entry that was sent"
    );
}

fn incident(line: &Line, key: &str) -> Incident {
    Incident::open(
        line.scope(),
        IncidentKey::new(key).expect("a key"),
        IncidentKind::ProtectionFailure,
        Halt::Position(line.instrument()),
        None,
        None,
        "the stop of the position could not be placed",
        T + 7_500,
    )
    .expect("an incident")
}

fn refused(answer: &Result<Stored<IntentRecord>, LedgerError>) -> Option<Refusal> {
    match answer {
        Err(LedgerError::Refused(refusal)) => Some(refusal.refusal),
        _ => None,
    }
}

/// Admission looks without a lease, so what it saw may have changed by
/// the time an entry is sent. `record_attempt` looks again under the
/// lease: an incident opened since halts the entry, another entry sent
/// since blocks it, and a position opened since refuses it; nothing is
/// written, and the entry stays admitted. An exit is held up by none of
/// it.
fn an_entry_is_held_to_what_the_store_holds_again_before_its_order_is_recorded<B: Backend>(
    backend: &B,
) {
    let store = backend.open();
    store.activate(T).expect("activated");
    let clock = Manual::at(T + 6_000);
    let ledger = ExecutionLedger::new(&store, &clock);
    let line = line("bybit linear");
    // Two entries admitted while the position is flat and nothing halts it.
    let first = started(&store, &ledger, &line, "entry", "trend", 0);
    let second = started(&store, &ledger, &line, "entry", "other", 1);
    let lease = line.lease(&store, "executor-a", T + 7_000);
    let fence = lease.fence();
    let attempt = |entry: &Started| ledger.record_attempt(&fence, &entry.intent, &entry.id, None);

    // An incident opened after the admission halts the entry.
    let raised = incident(&line, "op/1/protection_failure");
    let raise = ExecutionUnit::at(T + 7_500).with(Change::new(Record::Incident(raised.clone())));
    assert_eq!(store.commit(&raise).expect("a commit"), Commit::Applied);
    let halted = attempt(&first);
    assert_eq!(refused(&halted), Some(Refusal::IncidentOpen), "{halted:?}");
    assert_eq!(
        count(backend, "execution_orders"),
        0,
        "nothing of it written"
    );
    let still = store
        .intent(&line.scope(), first.intent.record.signal())
        .expect("a read")
        .expect("the intent");
    assert_eq!(still, first.intent, "the entry stays as admission left it");

    // Cleared, the first entry is recorded; the second is then blocked by
    // it, and once it has filled, by the position it opened.
    let cleared = raised
        .clear("protected by hand", T + 7_800)
        .expect("cleared");
    let clear = ExecutionUnit::at(T + 7_800).with(Change::replace(Record::Incident(cleared), 1));
    assert_eq!(store.commit(&clear).expect("a commit"), Commit::Applied);
    let sent = attempt(&first).expect("recorded");
    assert_eq!(sent.record.intent().state(), IntentState::Submitting);
    let blocked = attempt(&second);
    assert_eq!(
        refused(&blocked),
        Some(Refusal::UnresolvedEntry),
        "{blocked:?}"
    );
    let filled = reported(&line, &first.id, FuturesOrderStatus::Filled, "4.38", "4.38");
    let fills = [line
        .fill("t-1", &first.id, "4.38", "234.25", first.operation, None)
        .fill()
        .clone()];
    let evidence = Evidence::Order {
        order: &filled,
        fills: &fills,
    };
    ledger.settle(&fence, &sent, evidence).expect("settled");
    let pyramiding = attempt(&second);
    assert_eq!(
        refused(&pyramiding),
        Some(Refusal::Pyramiding),
        "{pyramiding:?}"
    );
    assert_eq!(
        count(backend, "execution_orders"),
        1,
        "the first entry's alone"
    );

    // An exit goes on while an incident halts entries.
    let again = incident(&line, "op/2/protection_failure");
    let raise = ExecutionUnit::at(T + 9_000).with(Change::new(Record::Incident(again)));
    assert_eq!(store.commit(&raise).expect("a commit"), Commit::Applied);
    clock.set(T + 70_000);
    let exit = started(&store, &ledger, &line, "exit", "trend", 2);
    let closing = attempt(&exit).expect("an exit is not halted");
    assert_eq!(closing.record.intent().state(), IntentState::Submitting);
    assert_eq!(count(backend, "execution_orders"), 2);
}

on_each_backend!(
    an_exit_that_arrives_as_its_entry_fills_closes_the_position_it_opened,
    an_entry_that_arrives_as_another_fills_is_refused_for_the_position_it_opened,
    an_entry_is_held_to_what_the_store_holds_again_before_its_order_is_recorded,
);
