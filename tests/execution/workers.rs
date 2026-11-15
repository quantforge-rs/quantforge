//! Two workers on one store, on each backend: a position's lease and the
//! fence it gives, a store's activation as the authority behind every
//! lease, and what racing writers leave behind.

use std::sync::Barrier;

use quantforge::automation::{ExecutionLedger, LedgerError};
use quantforge::model::execution::{Fenced, LeaseAnswer, Stale};
use quantforge::model::futures::{IntentState, OrderLeg, PoolScope};
use quantforge::model::{Asset, StoreActivation};
use quantforge::{ExecutionStore, StorageErrorKind, StoreControl};

use crate::fixtures::line;
use crate::orders::started;
use crate::support::{Backend, Manual, T, count, on_each_backend};

fn a_lease_that_ran_out_and_was_taken_fences_its_former_holder<B: Backend>(backend: &B) {
    let store = backend.open();
    store.activate(T).expect("activated");
    let line = line("bybit linear");
    let clock = Manual::at(T + 6_000);
    let ledger = ExecutionLedger::new(&store, &clock);
    let entry = started(&store, &ledger, &line, "entry", "trend", 0);

    // A holds the position; B is told so, and waits.
    let a = line.lease(&store, "executor-a", T + 7_000);
    assert_eq!((a.fence.get(), a.expires_at_ms), (1, T + 37_000));
    let other = backend.open();
    match other
        .acquire_lease(&line.lease_request("executor-b", T + 36_999))
        .expect("answered")
    {
        LeaseAnswer::Held(held) => assert_eq!(held, a),
        LeaseAnswer::Granted(lease) => panic!("granted over a live lease: {lease:?}"),
    }
    // A stalls past its lease. B takes the position, under the next token.
    let b = line.lease(&other, "executor-b", T + 37_000);
    assert_eq!((b.fence.get(), b.holder.as_str()), (2, "executor-b"));

    // A wakes and writes as if nothing had happened: it is fenced, and
    // nothing of it is written, whatever its own clock says.
    clock.set(T + 20_000);
    let stalled = ledger.record_attempt(&a.fence(), &entry.intent, &entry.id, None);
    match stalled {
        Err(LedgerError::Fenced(Fenced::Lost { holder, token })) => {
            assert_eq!((holder.as_str(), token.get()), ("executor-b", 2));
        }
        other => panic!("the former holder is fenced: {other:?}"),
    }
    assert_eq!(count(backend, "execution_orders"), 0);
    assert_eq!(
        store
            .renew_lease(&a.fence(), 30_000, T + 20_000)
            .expect("asked"),
        None
    );
    assert!(!store.release_lease(&a.fence()).expect("asked"));

    // B's own write goes through, and its renewal keeps its token.
    let later = ExecutionLedger::new(&other, &clock);
    clock.set(T + 38_000);
    let sent = later
        .record_attempt(&b.fence(), &entry.intent, &entry.id, None)
        .expect("recorded");
    assert_eq!(sent.record.intent().state(), IntentState::Submitting);
    let renewed = other
        .renew_lease(&b.fence(), 30_000, T + 50_000)
        .expect("asked")
        .expect("its own");
    assert_eq!(
        (renewed.fence, renewed.expires_at_ms),
        (b.fence, T + 80_000)
    );
    assert!(other.release_lease(&b.fence()).expect("asked"));
    // Given back, the position is anyone's again, under the token after.
    let c = line.lease(&store, "executor-a", T + 51_000);
    assert_eq!(c.fence.get(), 3);
}

fn a_store_that_is_not_active_grants_no_lease_and_applies_nothing_under_one<B: Backend>(
    backend: &B,
) {
    let store = backend.open();
    let line = line("bybit linear");
    let clock = Manual::at(T + 6_000);
    let ledger = ExecutionLedger::new(&store, &clock);

    // A store nobody activated executes nothing. It still records.
    let refused = store
        .acquire_lease(&line.lease_request("executor-a", T))
        .expect_err("inactive");
    assert_eq!(refused.kind(), StorageErrorKind::InvalidArgument);
    assert!(
        refused
            .to_string()
            .contains("is inactive, and only an active store grants a lease"),
        "{refused}"
    );
    let authority = store.store_identity().expect("identity").authority();
    assert!(authority.is_err(), "no authority to check either");

    let active = store.activate(T + 1).expect("activated");
    assert_eq!(active.authority().expect("active").0, 1);
    let entry = started(&store, &ledger, &line, "entry", "trend", 0);
    let lease = line.lease(&store, "executor-a", T + 7_000);
    assert_eq!(lease.epoch.0, 1);

    // The operator hands over to another store: this one stops at once,
    // lease or no lease.
    let handed_over = store.deactivate(T + 8_000).expect("deactivated");
    assert_eq!(handed_over.activation, StoreActivation::Deactivated);
    let stopped = ledger.record_attempt(&lease.fence(), &entry.intent, &entry.id, None);
    match stopped {
        Err(LedgerError::Fenced(Fenced::StoreNotActive { activation, epoch })) => {
            assert_eq!((activation, epoch.0), (StoreActivation::Deactivated, 1));
        }
        other => panic!("a deactivated store applies nothing: {other:?}"),
    }
    assert_eq!(count(backend, "execution_orders"), 0);
    assert_eq!(
        store
            .renew_lease(&lease.fence(), 30_000, T + 9_000)
            .expect("asked"),
        None
    );
    assert!(
        store
            .acquire_lease(&line.lease_request("executor-b", T + 9_000))
            .is_err()
    );

    // Taken back, it is active at a new epoch, and the activation voided
    // every lease: the position is no one's, and is leased again under
    // the token after its last.
    let back = store.activate(T + 10_000).expect("reactivated");
    assert_eq!(back.activation_epoch.0, 2);
    assert_eq!(store.lease(&line.key()).expect("a read"), None);
    let stale = ledger.record_attempt(&lease.fence(), &entry.intent, &entry.id, None);
    assert!(
        matches!(stale, Err(LedgerError::Fenced(Fenced::NotHeld))),
        "{stale:?}"
    );
    let again = line.lease(&store, "executor-a", T + 11_000);
    assert_eq!((again.fence.get(), again.epoch.0), (2, 2));
    // Its orders carry the epoch they are sent under.
    let id = line.client_order_id(back.activation_epoch, entry.operation, OrderLeg::Entry, 1);
    assert!(id.to_string().starts_with("qf1-k3x9p2-2-"), "{id}");
    ledger
        .record_attempt(&again.fence(), &entry.intent, &id, None)
        .expect("recorded");

    let log: Vec<(StoreActivation, StoreActivation, u32, i64)> = store
        .control_events()
        .expect("the log")
        .iter()
        .map(|event| (event.from, event.to, event.epoch.0, event.at_ms))
        .collect();
    assert_eq!(
        log,
        [
            (StoreActivation::Inactive, StoreActivation::Active, 1, T + 1),
            (
                StoreActivation::Active,
                StoreActivation::Deactivated,
                1,
                T + 8_000
            ),
            (
                StoreActivation::Deactivated,
                StoreActivation::Active,
                2,
                T + 10_000
            ),
        ]
    );
    let already = store.activate(T + 12_000).expect_err("once");
    assert!(already.to_string().contains("already active, at epoch 2"));
}

/// Every thread is a worker of its own, on a store handle of its own.
fn racing<B: Backend, T: Send>(
    backend: &B,
    workers: usize,
    work: impl Fn(usize, &B::Store) -> T + Sync,
) -> Vec<T> {
    let stores: Vec<B::Store> = (0..workers).map(|_| backend.open()).collect();
    let barrier = Barrier::new(workers);
    std::thread::scope(|scope| {
        let running: Vec<_> = stores
            .iter()
            .enumerate()
            .map(|(worker, store)| {
                let (barrier, work) = (&barrier, &work);
                scope.spawn(move || {
                    barrier.wait();
                    work(worker, store)
                })
            })
            .collect();
        running
            .into_iter()
            .map(|worker| worker.join().expect("a worker"))
            .collect()
    })
}

/// A call made again while another writer holds what it needs:
/// contention and a lost race both mean nothing changed.
fn patiently<T>(mut call: impl FnMut() -> Result<T, LedgerError>) -> Result<T, LedgerError> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        match call() {
            Err(LedgerError::Storage(busy))
                if matches!(
                    busy.kind(),
                    StorageErrorKind::Contention | StorageErrorKind::Conflict
                ) && std::time::Instant::now() < deadline => {}
            answer => return answer,
        }
    }
}

fn workers_that_race_leave_one_signal_one_attempt_and_one_reservation_behind<B: Backend + Sync>(
    backend: &B,
) {
    const WORKERS: usize = 6;
    let store = backend.open();
    store.activate(T).expect("activated");
    let line = line("bybit linear");
    let clock = Manual::at(T + 6_000);

    // The same delivery, six times at once: one is the first.
    let delivered = line.delivery("entry", "trend", 0, "234.23");
    let resolved = line.resolved(&delivered);
    let admissions = racing(backend, WORKERS, |_, store| {
        let ledger = ExecutionLedger::new(store, &clock);
        patiently(|| {
            ledger.admit(
                &line.identity,
                &delivered.signal,
                &delivered.payload,
                &resolved,
            )
        })
        .expect("answered")
        .delivery()
    });
    let first = admissions
        .iter()
        .filter(|delivery| **delivery == quantforge::model::futures::Delivery::New)
        .count();
    assert_eq!(first, 1, "{admissions:?}");
    assert_eq!(count(backend, "signal_inbox"), 1);
    assert_eq!(count(backend, "execution_intents"), 1);
    assert_eq!(count(backend, "outbox_events"), 1);
    assert_eq!(count(backend, "signal_deliveries"), WORKERS);

    // Its operation, asked for by all six: one number.
    let asked = line.operation(
        &delivered,
        quantforge::model::execution::OperationKind::Entry,
    );
    let numbers = racing(backend, WORKERS, |_, store| {
        patiently(|| Ok(store.begin_operation(&asked, T + 7_000)?))
            .expect("numbered")
            .record
            .id()
    });
    assert!(
        numbers.iter().all(|number| *number == numbers[0]),
        "{numbers:?}"
    );
    assert_eq!(count(backend, "execution_operations"), 1);

    // All six want the position: one gets it, and only that one's order
    // is recorded.
    let intent = store
        .intent(&line.scope(), &delivered.signal.id)
        .expect("a read")
        .expect("the intent");
    let epoch = store.store_identity().expect("identity").activation_epoch;
    let id = line.client_order_id(epoch, numbers[0], OrderLeg::Entry, 1);
    let recorded = racing(backend, WORKERS, |worker, store| {
        let ledger = ExecutionLedger::new(store, &clock);
        let request = line.lease_request(&format!("executor-{worker}"), T + 8_000);
        let answer = patiently(|| Ok(store.acquire_lease(&request)?)).expect("answered");
        let LeaseAnswer::Granted(lease) = answer else {
            return false;
        };
        patiently(|| ledger.record_attempt(&lease.fence(), &intent, &id, None)).is_ok()
    });
    assert_eq!(
        recorded.iter().filter(|done| **done).count(),
        1,
        "{recorded:?}"
    );
    assert_eq!(count(backend, "execution_orders"), 1);
    let held = store
        .intent(&line.scope(), &delivered.signal.id)
        .expect("a read")
        .expect("the intent");
    assert_eq!(held.record.intent().attempts().len(), 1);

    // Two entries on two more instruments would each reserve from the
    // pool the first drew on, each sized against it as it stood: one
    // reservation is made, and the other is told the pool has moved.
    let usdt = PoolScope::Asset(Asset::new("USDT").expect("an asset"));
    let account = line.identity.account_ref().clone();
    let sized = store.pool(&account, &usdt).expect("the pool").revision;
    // Two more instruments the line lists, both settled in USDT.
    let listed = ["BTCUSDT", "LUNAUSDT"];
    let entries: Vec<_> = listed
        .iter()
        .map(|symbol| {
            let other = line.trading(symbol);
            let ledger = ExecutionLedger::new(&store, &clock);
            let entry = started(&store, &ledger, &other, "entry", "trend", 0);
            let lease = other.lease(&store, "executor-a", T + 8_000);
            (other, entry, lease)
        })
        .collect();
    let reserved = racing(backend, entries.len(), |worker, store| {
        let (other, entry, lease) = &entries[worker];
        let ledger = ExecutionLedger::new(store, &clock);
        let held = other.reservation(entry.operation, "100", T + 9_000);
        patiently(|| {
            ledger.record_attempt(
                &lease.fence(),
                &entry.intent,
                &entry.id,
                Some((held.clone(), sized)),
            )
        })
        .map(|_| ())
    });
    let made = reserved.iter().filter(|outcome| outcome.is_ok()).count();
    assert_eq!(made, 1, "{reserved:?}");
    for outcome in reserved.iter().filter(|outcome| outcome.is_err()) {
        assert!(
            matches!(outcome, Err(LedgerError::Stale(Stale::Pool { .. }))),
            "{outcome:?}"
        );
    }
    let pool = store.pool(&account, &usdt).expect("the pool");
    assert_eq!((pool.revision, pool.held.len()), (sized + 1, 1));
}

on_each_backend!(
    a_lease_that_ran_out_and_was_taken_fences_its_former_holder,
    a_store_that_is_not_active_grants_no_lease_and_applies_nothing_under_one,
    workers_that_race_leave_one_signal_one_attempt_and_one_reservation_behind,
);
