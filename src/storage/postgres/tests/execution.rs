//! The execution store on PostgreSQL where only its own locks can show
//! it: what a unit waits for before it reads, and what it must never
//! wait for. Each test holds a transaction open at a known statement, as
//! another writer in flight would, and sees who waits. The units are the
//! shared cases' (`tests/fixtures/execution_cases.json`), read from the
//! text a store keeps each record as.

use super::scratch::{Scratch, rows};
use super::segments::{Held, wait_for_a_lock};
use super::*;
use crate::model::execution::{
    Change, Commit, DeliveryWrite, ExecutionUnit, Expect, Fence, FenceToken, Fenced, Found,
    LeaseAnswer, LeaseHolder, LeaseRequest, NewOperation, OperationKey, OperationKind, PoolGuard,
    Record, RecordKind, Stale,
};
use crate::model::futures::{PositionKey, ProfileId, SignalId};
use crate::model::{Environment, StoreActivation, TRANSFER_TABLES};
use crate::ports::{ExecutionStore, StorageErrorKind, StoreControl};
use crate::storage::execution::plan::Plan;
use serde_json::Value;
use std::thread;

/// The race the tests take their store and their units from: an admitted
/// entry, its operation numbered, its position leased, and the change
/// that records its first order.
const RACE: &str = "execution.same_change_racing";

pub(super) struct Cases(pub(super) Value);

impl Cases {
    pub(super) fn load() -> Self {
        let text = include_str!("../../../../tests/fixtures/execution_cases.json");
        Self(serde_json::from_str(text).expect("the execution cases"))
    }

    fn text<'v>(value: &'v Value, member: &str) -> &'v str {
        value[member]
            .as_str()
            .unwrap_or_else(|| panic!("{member} of {value}"))
    }

    fn record(&self, name: &str) -> Record {
        let held = &self.0["records"][name];
        let kind = RecordKind::ALL
            .into_iter()
            .find(|kind| kind.as_str() == Self::text(held, "kind"))
            .expect("a kind of record");
        Record::from_stored(kind, Self::text(held, "stored")).expect("a record")
    }

    fn position(&self, name: &str) -> PositionKey {
        serde_json::from_value(self.0["positions"][name].clone()).expect("a position")
    }

    fn fence(&self, fence: &Value) -> Fence {
        Fence {
            key: self.position(Self::text(fence, "position")),
            holder: LeaseHolder::new(Self::text(fence, "holder")).expect("a holder"),
            token: FenceToken::new(fence["token"].as_u64().expect("a token")).expect("a token"),
        }
    }

    pub(super) fn unit(&self, unit: &Value) -> ExecutionUnit {
        let mut made = ExecutionUnit::at(unit["at_ms"].as_i64().expect("a time"));
        made.fence = (!unit["fence"].is_null()).then(|| self.fence(&unit["fence"]));
        made.pool = unit["pool"].as_u64().map(|revision| PoolGuard { revision });
        if !unit["delivery"].is_null() {
            let delivery = &unit["delivery"];
            let (Record::Signal(record), Record::Event(conflict_event)) = (
                self.record(Self::text(delivery, "record")),
                self.record(Self::text(delivery, "conflict_event")),
            ) else {
                panic!("a delivery is a signal and an event");
            };
            made.delivery = Some(DeliveryWrite {
                record,
                conflict_event,
            });
        }
        for change in unit["changes"].as_array().expect("the changes") {
            made.changes.push(Change {
                record: self.record(Self::text(change, "record")),
                expect: change["expect"]
                    .as_u64()
                    .map_or(Expect::Absent, Expect::Revision),
            });
        }
        made
    }

    fn race(&self) -> &Value {
        self.0["races"]
            .as_array()
            .expect("the races")
            .iter()
            .find(|race| race["id"] == RACE)
            .expect("the race")
    }
}

/// A store set up as the race's writers find it, and what they write.
pub(super) struct Raced {
    pub(super) scratch: Scratch,
    pub(super) store: PostgresStore,
    /// The unit that recorded the entry's delivery and admitted it.
    pub(super) admission: ExecutionUnit,
    /// The change that records the entry's first order, under its lease.
    pub(super) change: ExecutionUnit,
}

pub(super) fn raced() -> Option<Raced> {
    let scratch = super::migrate::scratch()?;
    let store = scratch.store();
    store.init().expect("a schema");
    let cases = Cases::load();
    let race = cases.race();
    let mut admission = None;
    for step in race["setup"].as_array().expect("the setup") {
        match Cases::text(step, "op") {
            "bind" => store
                .bind_environment(Environment::Demo)
                .expect("the store is bound"),
            "activate" => drop(
                store
                    .activate(step["at_ms"].as_i64().expect("a time"))
                    .expect("the store is activated"),
            ),
            "commit" => {
                let unit = cases.unit(&step["unit"]);
                assert_eq!(store.commit(&unit).expect("an answer"), Commit::Applied);
                admission = Some(unit);
            }
            "begin_operation" => {
                let asked = &step["operation"];
                let scope = &cases.0["scopes"][Cases::text(asked, "scope")];
                let operation = NewOperation {
                    scope: serde_json::from_value(scope.clone()).expect("a scope"),
                    kind: OperationKind::Entry,
                    key: Some(OperationKey::new(Cases::text(asked, "key")).expect("a key")),
                    instrument: serde_json::from_value(asked["instrument"].clone())
                        .expect("an instrument"),
                    signal: Some(SignalId::new(Cases::text(asked, "signal")).expect("a signal")),
                    cycle: None,
                    replacement: None,
                };
                let at_ms = step["at_ms"].as_i64().expect("a time");
                store.begin_operation(&operation, at_ms).expect("numbered");
            }
            "acquire_lease" => {
                let request = LeaseRequest {
                    key: cases.position(Cases::text(step, "position")),
                    profile: ProfileId::new(Cases::text(step, "profile")).expect("a profile"),
                    holder: LeaseHolder::new(Cases::text(step, "holder")).expect("a holder"),
                    ttl_ms: step["ttl_ms"].as_i64().expect("a duration"),
                    now_ms: step["now_ms"].as_i64().expect("a time"),
                };
                let answer = store.acquire_lease(&request).expect("an answer");
                assert!(matches!(answer, LeaseAnswer::Granted(_)), "{answer:?}");
            }
            other => panic!("the setup has no step {other}"),
        }
    }
    let writers = race["writers"].as_array().expect("the writers");
    Some(Raced {
        change: cases.unit(&writers[0]["unit"]),
        admission: admission.expect("the setup admits a signal"),
        store,
        scratch,
    })
}

fn count(store: &PostgresStore, table: &str) -> String {
    rows(store, &format!("SELECT COUNT(*) FROM {table}"))[0][0]
        .clone()
        .expect("a count")
}

/// A unit holds every key it may write a first row under before it reads
/// anything: while another transaction holds one of them, as another
/// delivery of the same signal would, the unit waits, and decides on what
/// that transaction left. A delivery that read the inbox before the other
/// had written, and the intent after, would answer that its intent was
/// stale where it was a duplicate.
#[test]
fn a_unit_waits_for_every_key_it_may_first_write_before_it_reads() {
    let Some(raced) = raced() else {
        return;
    };
    let later = ExecutionUnit {
        at_ms: raced.admission.at_ms + 1_000,
        ..raced.admission.clone()
    };
    let plan = Plan::of(&later).expect("a plan");
    // The signal, its conflict event, its intent and its event.
    assert_eq!(plan.guards.len(), 4, "{:?}", plan.guards);
    for guard in &plan.guards {
        let held = Held::start(
            &raced.scratch,
            format!("SELECT pg_advisory_xact_lock({guard})"),
            String::new(),
        );
        let answer = thread::scope(|scope| {
            let committing = scope.spawn(|| raced.store.commit(&later));
            wait_for_a_lock(&raced.store);
            assert!(!committing.is_finished(), "it waits for key {guard}");
            held.finish();
            committing.join().expect("the unit")
        });
        assert!(
            matches!(answer, Ok(Commit::Duplicate(_))),
            "decided once the key was free: {answer:?}"
        );
    }
    assert_eq!(count(&raced.store, "signal_inbox"), "1");
    assert_eq!(count(&raced.store, "execution_intents"), "1");
}

/// A change of the store's activation waits for a unit that holds its
/// position's lease row, and a unit that comes while the change is under
/// way waits for it and sees it: no unit is ever applied under a lease on
/// a store that stopped being active before the unit read its lease.
#[test]
fn a_change_of_activation_and_a_unit_under_a_lease_never_pass_each_other() {
    let Some(raced) = raced() else {
        return;
    };
    let at_ms = raced.change.at_ms;

    // A unit in flight: its lease row is locked, and nothing is written yet.
    let unit_in_flight = Held::start(
        &raced.scratch,
        "SELECT fence FROM position_leases FOR UPDATE".to_string(),
        String::new(),
    );
    thread::scope(|scope| {
        let deactivating = scope.spawn(|| raced.store.deactivate(at_ms));
        wait_for_a_lock(&raced.store);
        assert!(!deactivating.is_finished(), "it waits for the unit");
        assert_eq!(
            raced.store.store_identity().expect("the record").activation,
            StoreActivation::Active
        );
        unit_in_flight.finish();
        let after = deactivating
            .join()
            .expect("the change")
            .expect("deactivated");
        assert_eq!(after.activation, StoreActivation::Deactivated);
    });
    raced.store.activate(at_ms).expect("activated again");
    let fence = raced.change.fence.clone().expect("a fence");
    let request = LeaseRequest {
        key: fence.key.clone(),
        profile: ProfileId::new("main").expect("a profile"),
        holder: fence.holder.clone(),
        ttl_ms: 600_000,
        now_ms: at_ms,
    };
    let LeaseAnswer::Granted(lease) = raced.store.acquire_lease(&request).expect("an answer")
    else {
        panic!("the lease is free after an activation");
    };
    let change = ExecutionUnit {
        fence: Some(lease.fence()),
        ..raced.change.clone()
    };

    // A deactivation in flight: the leases are locked and the record
    // changed, and neither is committed yet.
    let change_in_flight = Held::start(
        &raced.scratch,
        "LOCK TABLE position_leases IN EXCLUSIVE MODE; \
         UPDATE store_control SET activation = 'deactivated'"
            .to_string(),
        String::new(),
    );
    let answer = thread::scope(|scope| {
        let committing = scope.spawn(|| raced.store.commit(&change));
        wait_for_a_lock(&raced.store);
        assert!(!committing.is_finished(), "it waits for the change");
        change_in_flight.finish();
        committing.join().expect("the unit").expect("an answer")
    });
    assert_eq!(
        answer,
        Commit::Fenced(Fenced::StoreNotActive {
            activation: StoreActivation::Deactivated,
            epoch: lease.epoch,
        }),
        "it read the store's record once it had its lease row"
    );
    assert_eq!(count(&raced.store, "execution_orders"), "0");
}

/// A copy that begins locks every transferred table and then the store's
/// record. A unit under a lease that waits for one of those tables holds
/// no lock on the record meanwhile, so the copy's beginning never waits
/// for the unit while the unit waits for it: both end, in order.
#[test]
fn a_copy_that_begins_and_a_unit_under_a_lease_never_wait_for_each_other() {
    let Some(raced) = raced() else {
        return;
    };
    let tables: Vec<&str> = TRANSFER_TABLES.iter().map(|table| table.name).collect();
    // What `copying::hold_off_writers` runs, in its order, with the unit
    // arriving between its two statements.
    let copy_beginning = Held::start(
        &raced.scratch,
        format!("LOCK TABLE {} IN SHARE MODE", tables.join(", ")),
        "SELECT id FROM store_control WHERE id = 1 FOR UPDATE".to_string(),
    );
    let answer = thread::scope(|scope| {
        let committing = scope.spawn(|| raced.store.commit(&raced.change));
        wait_for_a_lock(&raced.store);
        assert!(!committing.is_finished(), "its write waits for the copy");
        // The copy's second statement runs now. Were the unit holding the
        // record's row, each would wait for the other until the server
        // ended one of them.
        copy_beginning.finish();
        committing.join().expect("the unit")
    });
    assert_eq!(answer.expect("no deadlock"), Commit::Applied);
    assert_eq!(count(&raced.store, "execution_orders"), "1");
}

/// Ends the sessions of the store's database that wait on a lock.
const END_THE_WAITERS: &str = "SELECT pg_terminate_backend(pid) FROM pg_stat_activity \
     WHERE datname = current_database() AND wait_event_type = 'Lock'";

/// A unit whose connection ends while it is under way has an unknown
/// outcome, and says so. Its writer asks again, as it does whenever an
/// answer never came, and learns what became of the unit: here nothing
/// was written, so it is applied; asked once more, as if that answer were
/// lost too, it finds its own record at the revision its write left.
#[test]
fn a_unit_cut_off_under_way_has_an_unknown_outcome_and_is_asked_again() {
    let Some(raced) = raced() else {
        return;
    };
    let another_writer = Held::start(
        &raced.scratch,
        "LOCK TABLE execution_orders IN ACCESS EXCLUSIVE MODE".to_string(),
        String::new(),
    );
    let writer = raced.scratch.store();
    let cut_off = thread::scope(|scope| {
        let committing = scope.spawn(|| writer.commit(&raced.change));
        wait_for_a_lock(&raced.store);
        rows(&raced.store, END_THE_WAITERS);
        committing.join().expect("the unit")
    });
    let refused = cut_off.expect_err("its session is gone");
    assert_eq!(
        refused.kind(),
        StorageErrorKind::OutcomeUnknown,
        "{refused}"
    );
    another_writer.finish();
    assert_eq!(count(&raced.store, "execution_orders"), "0");

    assert_eq!(
        writer.commit(&raced.change).expect("an answer"),
        Commit::Applied,
        "the store went on without the session, and nothing was half written"
    );
    let again = writer.commit(&raced.change).expect("an answer");
    let Commit::Stale(Stale::Record { kind, found, .. }) = &again else {
        panic!("stale: {again:?}");
    };
    assert_eq!(
        (*kind, *found),
        (RecordKind::Intent, Found::Present { revision: 2 })
    );
    assert_eq!(count(&raced.store, "execution_orders"), "1", "written once");
}
