//! The execution store's scenarios (`docs/execution-store.md`): the steps
//! of `tests/fixtures/execution_cases.json`, each with the answer a
//! conforming store gives. The fixture is written by
//! `spikes/storage-contract/execution_cases.py`, never by the Rust code
//! under test, and names every record by the exact text a store keeps it
//! as: a step's record is read from that text, and a record a store hands
//! back is held to it byte for byte.

use std::collections::BTreeMap;
use std::panic::{self, AssertUnwindSafe};

use quantforge::model::execution::{
    Change, Commit, DeliveryWrite, ExecutionScope, ExecutionUnit, Expect, Fence, FenceToken,
    Fenced, Found, Lease, LeaseAnswer, LeaseHolder, LeaseRequest, NewOperation, OperationKey,
    OperationKind, PoolGuard, Record, RecordKind, Replacement, Stale, Stored,
};
use quantforge::model::futures::{
    ActivationEpoch, PoolScope, PositionCycleId, PositionKey, ProfileId, SignalId,
};
use quantforge::model::{AccountRef, InstrumentId, OperationId, VenueTradeId};
use quantforge::{ExecutionStore, StorageError, StoreControl};
use serde::Deserialize;

use super::cases::{self, environment};
use super::execution_checks as checks;
use super::{Backend, Bounded, Fixture, Raw, Refusal, expect_refused, message};

/// The fixture, as the oracle wrote it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cases {
    pub format: u32,
    pub doc: String,
    pub scopes: BTreeMap<String, ExecutionScope>,
    pub accounts: BTreeMap<String, AccountRef>,
    pub positions: BTreeMap<String, PositionKey>,
    pub records: BTreeMap<String, RecordText>,
    pub scenarios: Vec<Scenario>,
    pub races: Vec<Race>,
}

/// One record: its kind, and the text a store keeps it as.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordText {
    pub kind: String,
    pub stored: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    pub id: String,
    pub doc: String,
    pub steps: Vec<Step>,
}

/// A store set up step by step, and what each of several writers then
/// asks of it at once.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Race {
    pub id: String,
    pub doc: String,
    pub setup: Vec<Step>,
    pub writers: Vec<Step>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredText {
    pub record: String,
    pub revision: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FenceText {
    pub position: String,
    pub holder: String,
    pub token: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeaseText {
    pub position: String,
    pub profile: String,
    pub holder: String,
    pub fence: u64,
    pub epoch: u32,
    pub acquired_at_ms: i64,
    pub expires_at_ms: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeaseAnswerText {
    Granted(LeaseText),
    Held(LeaseText),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeText {
    pub record: String,
    /// No record, or the revision of the one it replaces.
    pub expect: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryWriteText {
    pub record: String,
    pub conflict_event: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnitText {
    pub at_ms: i64,
    pub fence: Option<FenceText>,
    pub pool: Option<u64>,
    pub delivery: Option<DeliveryWriteText>,
    pub changes: Vec<ChangeText>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StaleText {
    Record {
        kind: String,
        name: String,
        found: Option<u64>,
    },
    Pool(u64),
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FencedText {
    NotHeld,
    Lost { holder: String, token: u64 },
    StoreNotActive { activation: String, epoch: u32 },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommitText {
    Applied,
    Duplicate(StoredText),
    Conflict(StoredText),
    Stale(StaleText),
    Fenced(FencedText),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperationText {
    pub scope: String,
    pub kind: String,
    pub key: Option<String>,
    pub instrument: InstrumentId,
    pub signal: Option<String>,
    pub replacement: Option<Replacement>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryText {
    pub disposition: String,
    pub payload_hash: String,
    pub payload: Option<String>,
    pub fire_time_ms: i64,
    pub received_at_ms: i64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PoolText {
    pub revision: u64,
    pub held: Vec<String>,
    /// The margin held against the pool, where any is.
    pub total: Option<String>,
}

/// One step of a scenario.
#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Step {
    Bind {
        environment: String,
    },
    Activate {
        at_ms: i64,
    },
    Deactivate {
        at_ms: i64,
    },
    Reopen,
    RawSelect {
        sql: String,
        expect: Vec<Vec<Option<String>>>,
    },
    AcquireLease {
        position: String,
        profile: String,
        holder: String,
        ttl_ms: i64,
        now_ms: i64,
        #[serde(default)]
        expect: Option<LeaseAnswerText>,
        #[serde(default)]
        refused: Option<Refusal>,
    },
    RenewLease {
        fence: FenceText,
        ttl_ms: i64,
        now_ms: i64,
        #[serde(default)]
        expect: Option<LeaseText>,
        #[serde(default)]
        refused: Option<Refusal>,
    },
    ReleaseLease {
        fence: FenceText,
        expect: bool,
    },
    Lease {
        position: String,
        expect: Option<LeaseText>,
    },
    BeginOperation {
        operation: OperationText,
        at_ms: i64,
        #[serde(default)]
        expect: Option<StoredText>,
        #[serde(default)]
        refused: Option<Refusal>,
    },
    Commit {
        unit: UnitText,
        #[serde(default)]
        expect: Option<CommitText>,
        #[serde(default)]
        refused: Option<Refusal>,
    },
    Signal {
        scope: String,
        signal: String,
        expect: Option<StoredText>,
    },
    ReceivedSignals {
        limit: usize,
        expect: Vec<StoredText>,
    },
    Deliveries {
        scope: String,
        signal: String,
        expect: Vec<DeliveryText>,
    },
    Intent {
        scope: String,
        signal: String,
        expect: Option<StoredText>,
    },
    PositionIntents {
        position: String,
        expect: Vec<StoredText>,
    },
    UnsettledIntents {
        expect: Vec<StoredText>,
    },
    Operation {
        id: u64,
        expect: Option<StoredText>,
    },
    UnsettledReplacements {
        expect: Vec<StoredText>,
    },
    Order {
        scope: String,
        client_order_id: String,
        expect: Option<StoredText>,
    },
    Orders {
        operation: u64,
        expect: Vec<StoredText>,
    },
    Fill {
        scope: String,
        instrument: InstrumentId,
        trade: String,
        expect: Option<String>,
    },
    Fills {
        cycle: u64,
        expect: Vec<String>,
    },
    Cycle {
        id: u64,
        expect: Option<StoredText>,
    },
    OpenCycle {
        position: String,
        expect: Option<StoredText>,
    },
    OpenCycles {
        expect: Vec<StoredText>,
    },
    Pool {
        account: String,
        pool: PoolScope,
        expect: PoolText,
    },
    Reservation {
        operation: u64,
        expect: Option<StoredText>,
    },
    Incident {
        scope: String,
        key: String,
        expect: Option<StoredText>,
    },
    OpenIncidents {
        expect: Vec<StoredText>,
    },
    Event {
        scope: String,
        key: String,
        expect: Option<StoredText>,
    },
    PendingEvents {
        limit: usize,
        expect: Vec<StoredText>,
    },
}

pub fn load() -> Cases {
    let cases: Cases = cases::read("tests/fixtures/execution_cases.json");
    assert_eq!(cases.format, 1, "the fixture's format");
    cases
}

fn kind_of(text: &str) -> RecordKind {
    RecordKind::ALL
        .into_iter()
        .find(|kind| kind.as_str() == text)
        .unwrap_or_else(|| panic!("no kind of record {text}"))
}

fn operation_kind(text: &str) -> OperationKind {
    OperationKind::ALL
        .into_iter()
        .find(|kind| kind.as_str() == text)
        .unwrap_or_else(|| panic!("no kind of operation {text}"))
}

impl Cases {
    pub fn scope(&self, name: &str) -> &ExecutionScope {
        self.scopes
            .get(name)
            .unwrap_or_else(|| panic!("the fixture names no scope {name}"))
    }

    pub fn account(&self, name: &str) -> &AccountRef {
        self.accounts
            .get(name)
            .unwrap_or_else(|| panic!("the fixture names no account {name}"))
    }

    pub fn position(&self, name: &str) -> &PositionKey {
        self.positions
            .get(name)
            .unwrap_or_else(|| panic!("the fixture names no position {name}"))
    }

    pub fn text(&self, name: &str) -> &RecordText {
        self.records
            .get(name)
            .unwrap_or_else(|| panic!("the fixture names no record {name}"))
    }

    /// The record `name` names, read from the text a store keeps it as.
    pub fn record(&self, name: &str) -> Record {
        let text = self.text(name);
        Record::from_stored(kind_of(&text.kind), &text.stored)
            .unwrap_or_else(|error| panic!("{name} is no {}: {error}", text.kind))
    }

    pub fn race(&self, id: &str) -> &Race {
        self.races
            .iter()
            .find(|race| race.id == id)
            .unwrap_or_else(|| panic!("the fixture has no race {id}"))
    }

    pub fn fence(&self, text: &FenceText) -> Fence {
        Fence {
            key: self.position(&text.position).clone(),
            holder: LeaseHolder::new(&text.holder).expect("a holder"),
            token: FenceToken::new(text.token).expect("a token"),
        }
    }

    pub fn lease(&self, text: &LeaseText) -> Lease {
        Lease {
            key: self.position(&text.position).clone(),
            profile: ProfileId::new(&text.profile).expect("a profile id"),
            holder: LeaseHolder::new(&text.holder).expect("a holder"),
            fence: FenceToken::new(text.fence).expect("a token"),
            epoch: ActivationEpoch(text.epoch),
            acquired_at_ms: text.acquired_at_ms,
            expires_at_ms: text.expires_at_ms,
        }
    }

    pub fn unit(&self, text: &UnitText) -> ExecutionUnit {
        let mut unit = ExecutionUnit::at(text.at_ms);
        unit.fence = text.fence.as_ref().map(|fence| self.fence(fence));
        unit.pool = text.pool.map(|revision| PoolGuard { revision });
        unit.delivery = text.delivery.as_ref().map(|delivery| {
            let (Record::Signal(record), Record::Event(conflict_event)) = (
                self.record(&delivery.record),
                self.record(&delivery.conflict_event),
            ) else {
                panic!("a delivery is a signal and an event: {delivery:?}");
            };
            DeliveryWrite {
                record,
                conflict_event,
            }
        });
        unit.changes = text
            .changes
            .iter()
            .map(|change| Change {
                record: self.record(&change.record),
                expect: change.expect.map_or(Expect::Absent, Expect::Revision),
            })
            .collect();
        unit
    }

    pub fn operation(&self, text: &OperationText) -> NewOperation {
        NewOperation {
            scope: self.scope(&text.scope).clone(),
            kind: operation_kind(&text.kind),
            key: text
                .key
                .as_ref()
                .map(|key| OperationKey::new(key).expect("an operation key")),
            instrument: text.instrument.clone(),
            signal: text
                .signal
                .as_ref()
                .map(|signal| SignalId::new(signal).expect("a signal id")),
            cycle: None,
            replacement: text.replacement.clone(),
        }
    }

    pub fn lease_request(
        &self,
        position: &str,
        profile: &str,
        holder: &str,
        ttl_ms: i64,
        now_ms: i64,
    ) -> LeaseRequest {
        LeaseRequest {
            key: self.position(position).clone(),
            profile: ProfileId::new(profile).expect("a profile id"),
            holder: LeaseHolder::new(holder).expect("a holder"),
            ttl_ms,
            now_ms,
        }
    }

    /// Holds a record a store handed back to the one the fixture names:
    /// the same record, at the same revision, and the same text when it is
    /// written again.
    pub fn same(&self, found: Option<(Record, u64)>, expect: Option<&StoredText>, what: &str) {
        match (found, expect) {
            (None, None) => {}
            (Some((record, revision)), Some(expect)) => {
                let name = &expect.record;
                assert_eq!(
                    record.to_stored().expect("its stored form"),
                    self.text(name).stored,
                    "{what}: {name}"
                );
                assert_eq!(record, self.record(name), "{what}: {name}");
                assert_eq!(revision, expect.revision, "{what}: the revision of {name}");
            }
            (found, expect) => panic!("{what}: found {found:?}, and the fixture says {expect:?}"),
        }
    }

    fn same_list(&self, found: Vec<(Record, u64)>, expect: &[StoredText], what: &str) {
        let names: Vec<String> = found.iter().map(|(record, _)| record.name()).collect();
        assert_eq!(found.len(), expect.len(), "{what}: {names:?}");
        for (index, (found, expect)) in found.into_iter().zip(expect).enumerate() {
            self.same(Some(found), Some(expect), &format!("{what} #{index}"));
        }
    }
}

/// A stored record of one kind, as any kind's.
fn any<T>(stored: Stored<T>, wrap: fn(T) -> Record) -> (Record, u64) {
    (wrap(stored.record), stored.revision)
}

fn each<T>(stored: Vec<Stored<T>>, wrap: fn(T) -> Record) -> Vec<(Record, u64)> {
    stored.into_iter().map(|one| any(one, wrap)).collect()
}

fn operation_id(number: u64) -> OperationId {
    OperationId::new(number).expect("an operation id")
}

fn cycle_id(number: u64) -> PositionCycleId {
    PositionCycleId::new(number).expect("a cycle id")
}

fn signal_id(text: &str) -> SignalId {
    SignalId::new(text).expect("a signal id")
}

/// Holds what a unit came to against what the fixture says of it.
pub fn same_commit(cases: &Cases, answer: Commit, expect: &CommitText, what: &str) {
    match (answer, expect) {
        (Commit::Applied, CommitText::Applied) => {}
        (Commit::Duplicate(held), CommitText::Duplicate(expect))
        | (Commit::Conflict(held), CommitText::Conflict(expect)) => {
            cases.same(Some(any(held, Record::Signal)), Some(expect), what);
        }
        (
            Commit::Stale(Stale::Record { kind, name, found }),
            CommitText::Stale(StaleText::Record {
                kind: expected_kind,
                name: expected_name,
                found: expected,
            }),
        ) => {
            let found = match found {
                Found::Absent => None,
                Found::Present { revision } => Some(revision),
            };
            assert_eq!(
                (kind.as_str(), &name, found),
                (expected_kind.as_str(), expected_name, *expected),
                "{what}"
            );
        }
        (Commit::Stale(Stale::Pool { revision }), CommitText::Stale(StaleText::Pool(expect))) => {
            assert_eq!(revision, *expect, "{what}: the pool's revision");
        }
        (Commit::Fenced(Fenced::NotHeld), CommitText::Fenced(FencedText::NotHeld)) => {}
        (
            Commit::Fenced(Fenced::Lost { holder, token }),
            CommitText::Fenced(FencedText::Lost {
                holder: expected_holder,
                token: expected,
            }),
        ) => {
            assert_eq!(
                (holder.as_str(), token.get()),
                (expected_holder.as_str(), *expected),
                "{what}"
            );
        }
        (
            Commit::Fenced(Fenced::StoreNotActive { activation, epoch }),
            CommitText::Fenced(FencedText::StoreNotActive {
                activation: expected_activation,
                epoch: expected,
            }),
        ) => {
            assert_eq!(
                (activation.as_str(), epoch.0),
                (expected_activation.as_str(), *expected),
                "{what}"
            );
        }
        (answer, expect) => {
            panic!("{what}: it came to {answer:?}, and the fixture says {expect:?}")
        }
    }
}

/// Runs one step on `store` and holds its answer to the fixture's.
pub fn step<S: ExecutionStore + StoreControl>(
    cases: &Cases,
    raw: &dyn Raw,
    reopen: &dyn Fn() -> S,
    store: &mut S,
    step: &Step,
) {
    match step {
        Step::Bind { environment: bound } => store
            .bind_environment(environment(bound))
            .expect("the store is bound"),
        Step::Activate { at_ms } => drop(store.activate(*at_ms).expect("the store is activated")),
        Step::Deactivate { at_ms } => {
            drop(store.deactivate(*at_ms).expect("the store is deactivated"));
        }
        Step::Reopen => *store = reopen(),
        Step::RawSelect { sql, expect } => assert_eq!(&raw.select(sql), expect, "{sql}"),
        Step::AcquireLease {
            position,
            profile,
            holder,
            ttl_ms,
            now_ms,
            expect,
            refused,
        } => {
            let request = cases.lease_request(position, profile, holder, *ttl_ms, *now_ms);
            let outcome = store.acquire_lease(&request);
            let what = format!("the lease of {position} asked by {holder}");
            match (refused, expect) {
                (Some(refused), _) => expect_refused(outcome, refused, &what),
                (None, Some(LeaseAnswerText::Granted(lease))) => assert_eq!(
                    outcome.expect("an answer"),
                    LeaseAnswer::Granted(cases.lease(lease)),
                    "{what}"
                ),
                (None, Some(LeaseAnswerText::Held(lease))) => assert_eq!(
                    outcome.expect("an answer"),
                    LeaseAnswer::Held(cases.lease(lease)),
                    "{what}"
                ),
                (None, None) => panic!("{what}: the step names no answer"),
            }
        }
        Step::RenewLease {
            fence,
            ttl_ms,
            now_ms,
            expect,
            refused,
        } => {
            let outcome = store.renew_lease(&cases.fence(fence), *ttl_ms, *now_ms);
            let what = format!("the renewal by {}", fence.holder);
            match refused {
                Some(refused) => expect_refused(outcome, refused, &what),
                None => assert_eq!(
                    outcome.expect("an answer"),
                    expect.as_ref().map(|lease| cases.lease(lease)),
                    "{what}"
                ),
            }
        }
        Step::ReleaseLease { fence, expect } => {
            let released = store.release_lease(&cases.fence(fence)).expect("an answer");
            assert_eq!(released, *expect, "the release by {}", fence.holder);
        }
        Step::Lease { position, expect } => {
            let held = store.lease(cases.position(position)).expect("a read");
            let expect = expect.as_ref().map(|lease| cases.lease(lease));
            assert_eq!(held, expect, "the lease of {position}");
        }
        Step::BeginOperation {
            operation,
            at_ms,
            expect,
            refused,
        } => {
            let outcome = store.begin_operation(&cases.operation(operation), *at_ms);
            let what = format!("the {} under {:?}", operation.kind, operation.key);
            match refused {
                Some(refused) => expect_refused(outcome, refused, &what),
                None => {
                    let numbered = outcome.expect("an operation");
                    cases.same(
                        Some(any(numbered, Record::Operation)),
                        expect.as_ref(),
                        &what,
                    );
                }
            }
        }
        Step::Commit {
            unit,
            expect,
            refused,
        } => {
            let outcome = store.commit(&cases.unit(unit));
            let what = format!("the unit at {}", unit.at_ms);
            match (refused, expect) {
                (Some(refused), _) => expect_refused(outcome, refused, &what),
                (None, Some(expect)) => {
                    same_commit(cases, outcome.expect("an answer"), expect, &what);
                }
                (None, None) => panic!("{what}: the step names no answer"),
            }
        }
        Step::Signal {
            scope,
            signal,
            expect,
        } => {
            let found = store
                .signal(cases.scope(scope), &signal_id(signal))
                .expect("a read");
            let found = found.map(|stored| any(stored, Record::Signal));
            cases.same(
                found,
                expect.as_ref(),
                &format!("signal {signal} of {scope}"),
            );
        }
        Step::ReceivedSignals { limit, expect } => {
            let found = store.received_signals(*limit).expect("a read");
            cases.same_list(each(found, Record::Signal), expect, "the received signals");
        }
        Step::Deliveries {
            scope,
            signal,
            expect,
        } => {
            let found = store
                .deliveries(cases.scope(scope), &signal_id(signal))
                .expect("a read");
            assert_eq!(found.len(), expect.len(), "the deliveries of {signal}");
            for (found, expect) in found.iter().zip(expect) {
                assert_eq!(&found.scope, cases.scope(scope));
                assert_eq!(found.signal.as_str(), signal);
                assert_eq!(
                    (
                        found.disposition.as_str(),
                        found.payload_hash.to_string(),
                        &found.payload,
                        found.fire_time_ms,
                        found.received_at_ms,
                    ),
                    (
                        expect.disposition.as_str(),
                        expect.payload_hash.clone(),
                        &expect.payload,
                        expect.fire_time_ms,
                        expect.received_at_ms,
                    ),
                    "a delivery of {signal}"
                );
            }
        }
        Step::Intent {
            scope,
            signal,
            expect,
        } => {
            let found = store
                .intent(cases.scope(scope), &signal_id(signal))
                .expect("a read");
            let found = found.map(|stored| any(stored, Record::Intent));
            cases.same(
                found,
                expect.as_ref(),
                &format!("intent {signal} of {scope}"),
            );
        }
        Step::PositionIntents { position, expect } => {
            let found = store
                .position_intents(cases.position(position))
                .expect("a read");
            let what = format!("the unsettled intents of {position}");
            cases.same_list(each(found, Record::Intent), expect, &what);
        }
        Step::UnsettledIntents { expect } => {
            let found = store.unsettled_intents().expect("a read");
            cases.same_list(each(found, Record::Intent), expect, "the unsettled intents");
        }
        Step::Operation { id, expect } => {
            let found = store.operation(operation_id(*id)).expect("a read");
            let found = found.map(|stored| any(stored, Record::Operation));
            cases.same(found, expect.as_ref(), &format!("operation {id}"));
        }
        Step::UnsettledReplacements { expect } => {
            let found = store.unsettled_replacements().expect("a read");
            let what = "the unsettled replacements";
            cases.same_list(each(found, Record::Operation), expect, what);
        }
        Step::Order {
            scope,
            client_order_id,
            expect,
        } => {
            let found = store
                .order(cases.scope(scope), client_order_id)
                .expect("a read");
            let found = found.map(|stored| any(stored, Record::Order));
            let what = format!("order {client_order_id} of {scope}");
            cases.same(found, expect.as_ref(), &what);
        }
        Step::Orders { operation, expect } => {
            let found = store.orders(operation_id(*operation)).expect("a read");
            let what = format!("the orders of operation {operation}");
            cases.same_list(each(found, Record::Order), expect, &what);
        }
        Step::Fill {
            scope,
            instrument,
            trade,
            expect,
        } => {
            let trade_id = VenueTradeId::new(trade).expect("a trade id");
            let found = store
                .fill(cases.scope(scope), instrument, &trade_id)
                .expect("a read");
            let expect = expect.as_ref().map(|name| StoredText {
                record: name.clone(),
                revision: 1,
            });
            let found = found.map(|fill| (Record::Fill(fill), 1));
            cases.same(found, expect.as_ref(), &format!("fill {trade} of {scope}"));
        }
        Step::Fills { cycle, expect } => {
            let found = store.fills(cycle_id(*cycle)).expect("a read");
            let expect: Vec<StoredText> = expect
                .iter()
                .map(|name| StoredText {
                    record: name.clone(),
                    revision: 1,
                })
                .collect();
            let found = found
                .into_iter()
                .map(|fill| (Record::Fill(fill), 1))
                .collect();
            cases.same_list(found, &expect, &format!("the fills of cycle {cycle}"));
        }
        Step::Cycle { id, expect } => {
            let found = store.cycle(cycle_id(*id)).expect("a read");
            let found = found.map(|stored| any(stored, Record::Cycle));
            cases.same(found, expect.as_ref(), &format!("cycle {id}"));
        }
        Step::OpenCycle { position, expect } => {
            let found = store.open_cycle(cases.position(position)).expect("a read");
            let found = found.map(|stored| any(stored, Record::Cycle));
            cases.same(
                found,
                expect.as_ref(),
                &format!("the open cycle of {position}"),
            );
        }
        Step::OpenCycles { expect } => {
            let found = store.open_cycles().expect("a read");
            cases.same_list(each(found, Record::Cycle), expect, "the open cycles");
        }
        Step::Pool {
            account,
            pool,
            expect,
        } => {
            let found = store.pool(cases.account(account), pool).expect("a read");
            assert_eq!(found.revision, expect.revision, "the pool's revision");
            let total = found.total(pool).expect("a total").amount().to_string();
            let held: Vec<StoredText> = expect
                .held
                .iter()
                .map(|name| StoredText {
                    record: name.clone(),
                    revision: 0,
                })
                .collect();
            assert_eq!(found.held.len(), held.len(), "what the pool holds");
            for (found, expect) in found.held.into_iter().zip(&held) {
                assert_eq!(
                    Record::Reservation(found),
                    cases.record(&expect.record),
                    "a reservation of the pool"
                );
            }
            if let Some(expect) = &expect.total {
                assert_eq!(&total, expect, "the margin held against the pool");
            }
        }
        Step::Reservation { operation, expect } => {
            let found = store.reservation(operation_id(*operation)).expect("a read");
            let found = found.map(|stored| any(stored, Record::Reservation));
            let what = format!("the reservation of operation {operation}");
            cases.same(found, expect.as_ref(), &what);
        }
        Step::Incident { scope, key, expect } => {
            let incident = quantforge::model::execution::IncidentKey::new(key).expect("a key");
            let found = store
                .incident(cases.scope(scope), &incident)
                .expect("a read");
            let found = found.map(|stored| any(stored, Record::Incident));
            cases.same(
                found,
                expect.as_ref(),
                &format!("incident {key} of {scope}"),
            );
        }
        Step::OpenIncidents { expect } => {
            let found = store.open_incidents().expect("a read");
            cases.same_list(each(found, Record::Incident), expect, "the open incidents");
        }
        Step::Event { scope, key, expect } => {
            let event = quantforge::model::execution::EventKey::new(key).expect("a key");
            let found = store.event(cases.scope(scope), &event).expect("a read");
            let found = found.map(|stored| any(stored, Record::Event));
            cases.same(found, expect.as_ref(), &format!("event {key} of {scope}"));
        }
        Step::PendingEvents { limit, expect } => {
            let found = store.pending_events(*limit).expect("a read");
            cases.same_list(each(found, Record::Event), expect, "the pending events");
        }
    }
}

/// Runs every scenario, then every race and every coded check, each on a
/// fresh store, and fails once at the end listing all that failed.
pub fn run<B: Backend>()
where
    B::Store: ExecutionStore + StoreControl + Bounded,
{
    let cases = load();
    let mut failures = Vec::new();
    for scenario in &cases.scenarios {
        let outcome = panic::catch_unwind(AssertUnwindSafe(|| {
            let Fixture {
                mut store,
                raw,
                reopen,
                guard,
            } = B::fresh();
            for (index, current) in scenario.steps.iter().enumerate() {
                let stepped = panic::catch_unwind(AssertUnwindSafe(|| {
                    step(&cases, &*raw, &*reopen, &mut store, current);
                }));
                if let Err(payload) = stepped {
                    panic!("step {index}: {}", message(&payload));
                }
            }
            // The store goes before what keeps its database.
            drop(store);
            drop(guard);
        }));
        if let Err(payload) = outcome {
            failures.push(format!("{}: {}", scenario.id, message(&payload)));
        }
    }
    let coded = cases
        .races
        .iter()
        .map(|race| race.id.as_str())
        .chain(checks::CHECKS.iter().map(|(id, _)| *id));
    for id in coded {
        let run = checks::check::<B::Store>(id);
        let outcome = panic::catch_unwind(AssertUnwindSafe(|| run(&cases, &B::fresh())));
        if let Err(payload) = outcome {
            failures.push(format!("{id}: {}", message(&payload)));
        }
    }
    let total = cases.scenarios.len() + cases.races.len() + checks::CHECKS.len();
    assert!(
        failures.is_empty(),
        "{} of {total} execution checks failed on {}:\n{}",
        failures.len(),
        B::NAME,
        failures.join("\n")
    );
}

/// A call's refusal, for a check that expects one.
pub fn refusal<T: std::fmt::Debug>(outcome: Result<T, StorageError>, what: &str) -> StorageError {
    match outcome {
        Ok(answer) => panic!("{what}: refused expected, got {answer:?}"),
        Err(error) => error,
    }
}
