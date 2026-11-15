//! The execution store's checks that are coded rather than written as
//! steps: the races of `tests/fixtures/execution_cases.json`, where the
//! fixture sets a store up and names what each of several writers asks at
//! once, and what holds whoever wins is checked here; and the checks that
//! hold locks, refuse writes or change rows behind the store's back.

use std::sync::Barrier;
use std::time::{Duration, Instant};

use quantforge::model::execution::{
    Commit, ExecutionUnit, Found, LeaseAnswer, PoolGuard, Record, RecordKind, Stale, Stored,
};
use quantforge::model::futures::Delivery;
use quantforge::{ExecutionStore, StorageError, StorageErrorKind, StoreActivation, StoreControl};

use super::execution::{Cases, Race, Step, refusal, step};
use super::{Bounded, Fixture, Raw};

/// A check coded here: it reads the fixture, and runs on a fresh store.
pub type Coded<S> = fn(&Cases, &Fixture<S>);

/// The checks coded here that no race of the fixture names: id and
/// statement.
pub const CHECKS: &[(&str, &str)] = &[
    (
        "execution.records_keep_their_text",
        "every record of the fixture is read from the text a store keeps it as and written again byte for byte",
    ),
    (
        "execution.failed_unit_changes_nothing",
        "a unit one of whose writes fails leaves nothing of itself, whichever write it was: no signal, no note of its delivery, no intent and no event; once the write goes through, the same unit is applied whole",
    ),
    (
        "execution.waits_are_bounded",
        "a unit and a lease call held up by another writer fail as contention within their budget and change nothing, and go through once the writer is done",
    ),
    (
        "execution.lost_answer_asked_again",
        "a writer that never got its answer asks again and learns what became of its write: a delivery sent again is a duplicate that names the signal recorded, and a change made again finds its own record at the revision it left, by a store whose connections are gone too",
    ),
    (
        "execution.row_changed_behind_the_store_is_refused",
        "a row whose columns no longer say what its document says is refused where it is read and where a unit would replace it, and is read again once it is put right",
    ),
    (
        "execution.lease_row_gives_no_authority",
        "a lease row written behind the back of a store that was never activated fences nothing, and the store's activation voids it: a unit written under it is applied neither before nor after, and its position's next grant takes the token after the row's",
    ),
];

pub fn check<S: ExecutionStore + StoreControl + Bounded>(id: &str) -> Coded<S> {
    match id {
        "execution.first_deliveries_racing" => first_deliveries_racing,
        "execution.conflicting_deliveries_racing" => conflicting_deliveries_racing,
        "execution.same_change_racing" => same_change_racing,
        "execution.leases_racing" => leases_racing,
        "execution.reservations_racing" => reservations_racing,
        "execution.operations_racing" => operations_racing,
        "execution.records_keep_their_text" => records_keep_their_text,
        "execution.failed_unit_changes_nothing" => failed_unit_changes_nothing,
        "execution.waits_are_bounded" => waits_are_bounded,
        "execution.lost_answer_asked_again" => lost_answer_asked_again,
        "execution.row_changed_behind_the_store_is_refused" => changed_row_refused,
        "execution.lease_row_gives_no_authority" => lease_row_gives_no_authority,
        other => panic!("no execution check {other}"),
    }
}

const BUDGET: Duration = Duration::from_secs(2);

/// What a loaded machine may add to a budget before a call is seen to end.
const SCHEDULING: Duration = Duration::from_secs(1);

/// A call made again while another writer holds what it needs: contention
/// means nothing changed, and a conflict that the transaction was rolled
/// back whole.
fn past_contention<T>(
    mut call: impl FnMut() -> Result<T, StorageError>,
) -> Result<T, StorageError> {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        match call() {
            Err(busy)
                if matches!(
                    busy.kind(),
                    StorageErrorKind::Contention | StorageErrorKind::Conflict
                ) && Instant::now() < deadline => {}
            answer => return answer,
        }
    }
}

fn count(raw: &dyn Raw, table: &str) -> usize {
    raw.select(&format!("SELECT CAST(COUNT(*) AS TEXT) FROM {table}"))[0][0]
        .as_deref()
        .expect("a count")
        .parse()
        .expect("a number")
}

/// One column of every row of `table`, in the order of `order`.
fn column(raw: &dyn Raw, table: &str, column: &str, order: &str) -> Vec<String> {
    raw.select(&format!(
        "SELECT CAST({column} AS TEXT) FROM {table} ORDER BY {order}"
    ))
    .into_iter()
    .map(|row| row[0].clone().unwrap_or_default())
    .collect()
}

/// Runs a race's setup on the fixture's store, each step held to its
/// answer, and answers the store it was run on.
fn set_up<S: ExecutionStore + StoreControl>(cases: &Cases, fixture: &Fixture<S>, race: &Race) -> S {
    let mut store = (fixture.reopen)();
    for current in &race.setup {
        step(cases, &*fixture.raw, &*fixture.reopen, &mut store, current);
    }
    store
}

/// The units a race's writers commit.
fn units(cases: &Cases, race: &Race) -> Vec<ExecutionUnit> {
    race.writers
        .iter()
        .map(|writer| match writer {
            Step::Commit { unit, .. } => cases.unit(unit),
            other => panic!("{}: a writer commits a unit: {other:?}", race.id),
        })
        .collect()
}

/// Every writer's work run at once, each on a store of its own opened on
/// the fixture's database, started together.
fn at_once<S, W, T>(fixture: &Fixture<S>, work: &[W], run: impl Fn(&S, &W) -> T + Sync) -> Vec<T>
where
    S: Send + Sync,
    W: Sync,
    T: Send,
{
    let stores: Vec<S> = work.iter().map(|_| (fixture.reopen)()).collect();
    let barrier = Barrier::new(work.len());
    std::thread::scope(|scope| {
        let running: Vec<_> = stores
            .iter()
            .zip(work)
            .map(|(store, work)| {
                let (barrier, run) = (&barrier, &run);
                scope.spawn(move || {
                    barrier.wait();
                    run(store, work)
                })
            })
            .collect();
        running
            .into_iter()
            .map(|writer| writer.join().expect("a writer"))
            .collect()
    })
}

fn committed<S: ExecutionStore>(fixture: &Fixture<S>, units: &[ExecutionUnit]) -> Vec<Commit> {
    at_once(fixture, units, |store, unit| {
        past_contention(|| store.commit(unit)).expect("an answer")
    })
}

fn first_deliveries_racing<S: ExecutionStore + StoreControl>(cases: &Cases, fixture: &Fixture<S>) {
    let race = cases.race("execution.first_deliveries_racing");
    let store = set_up(cases, fixture, race);
    let units = units(cases, race);
    let answers = committed(fixture, &units);

    let arrived = &units[0].delivery.as_ref().expect("a delivery").record;
    let recorded = store
        .signal(arrived.scope(), arrived.id())
        .expect("a read")
        .expect("the signal");
    let applied = answers
        .iter()
        .filter(|answer| **answer == Commit::Applied)
        .count();
    assert_eq!(applied, 1, "{answers:?}");
    for answer in &answers {
        match answer {
            Commit::Applied => {}
            Commit::Duplicate(named) => assert_eq!(named, &recorded),
            other => panic!("applied or a duplicate: {other:?}"),
        }
    }
    let raw = &*fixture.raw;
    assert_eq!(count(raw, "signal_inbox"), 1);
    assert_eq!(count(raw, "execution_intents"), 1);
    assert_eq!(count(raw, "outbox_events"), 1);
    let mut noted = column(raw, "signal_deliveries", "disposition", "seq");
    noted.sort();
    let mut expected = vec!["duplicate".to_string(); units.len() - 1];
    expected.push("new".to_string());
    assert_eq!(noted, expected);
}

fn conflicting_deliveries_racing<S: ExecutionStore + StoreControl>(
    cases: &Cases,
    fixture: &Fixture<S>,
) {
    let race = cases.race("execution.conflicting_deliveries_racing");
    let store = set_up(cases, fixture, race);
    let units = units(cases, race);
    let answers = committed(fixture, &units);

    let arrived = |unit: &ExecutionUnit| unit.delivery.clone().expect("a delivery").record;
    let first = arrived(&units[0]);
    let recorded = store
        .signal(first.scope(), first.id())
        .expect("a read")
        .expect("the signal");
    let winner = recorded.record.payload_hash();
    let mut seen = [0usize; 3];
    for (unit, answer) in units.iter().zip(&answers) {
        let same_payload = arrived(unit).payload_hash() == winner;
        match answer {
            Commit::Applied => {
                assert!(same_payload, "the payload recorded is the applied unit's");
                seen[0] += 1;
            }
            Commit::Duplicate(named) => {
                assert!(same_payload, "a duplicate brought the recorded payload");
                assert_eq!(named, &recorded);
                seen[1] += 1;
            }
            Commit::Conflict(named) => {
                assert!(!same_payload, "a conflict brought another payload");
                assert_eq!(named, &recorded);
                seen[2] += 1;
            }
            other => panic!("applied, a duplicate or a conflict: {other:?}"),
        }
    }
    assert_eq!(seen, [1, 3, 4], "{answers:?}");
    let raw = &*fixture.raw;
    assert_eq!(count(raw, "signal_inbox"), 1);
    assert_eq!(count(raw, "execution_intents"), 1);
    let mut told = column(raw, "outbox_events", "kind", "seq");
    told.sort();
    assert_eq!(told, ["signal_admitted", "signal_conflict"], "told once");
    let mut noted = column(raw, "signal_deliveries", "disposition", "seq");
    noted.sort();
    let expected: Vec<&str> = [["conflict"; 4].as_slice(), &["duplicate"; 3], &["new"]].concat();
    assert_eq!(noted, expected);
    // Every conflicting delivery kept the payload it brought.
    let kept = fixture.raw.select(
        "SELECT CAST(COUNT(*) AS TEXT) FROM signal_deliveries WHERE disposition = 'conflict' \
         AND payload IS NOT NULL",
    );
    assert_eq!(kept[0][0].as_deref(), Some("4"));
    let deliveries = store
        .deliveries(first.scope(), first.id())
        .expect("the deliveries");
    assert_eq!(deliveries.len(), units.len());
    assert_eq!(deliveries[0].disposition, Delivery::New);
}

fn same_change_racing<S: ExecutionStore + StoreControl>(cases: &Cases, fixture: &Fixture<S>) {
    let race = cases.race("execution.same_change_racing");
    let store = set_up(cases, fixture, race);
    let units = units(cases, race);
    let answers = committed(fixture, &units);

    let applied = answers
        .iter()
        .filter(|answer| **answer == Commit::Applied)
        .count();
    assert_eq!(applied, 1, "{answers:?}");
    for answer in &answers {
        match answer {
            Commit::Applied => {}
            Commit::Stale(Stale::Record { kind, found, .. }) => {
                assert_eq!(*kind, RecordKind::Intent, "the first change that is stale");
                assert_eq!(*found, Found::Present { revision: 2 });
            }
            other => panic!("applied or stale: {other:?}"),
        }
    }
    assert_eq!(count(&*fixture.raw, "execution_orders"), 1);
    let unsettled = store.unsettled_intents().expect("a read");
    assert_eq!(unsettled.len(), 1);
    assert_eq!(unsettled[0].revision, 2);
    assert_eq!(
        Record::Intent(unsettled[0].record.clone()),
        units[0].changes[0].record
    );
}

fn leases_racing<S: ExecutionStore + StoreControl>(cases: &Cases, fixture: &Fixture<S>) {
    let race = cases.race("execution.leases_racing");
    let store = set_up(cases, fixture, race);
    let requests: Vec<_> = race
        .writers
        .iter()
        .map(|writer| match writer {
            Step::AcquireLease {
                position,
                profile,
                holder,
                ttl_ms,
                now_ms,
                ..
            } => cases.lease_request(position, profile, holder, *ttl_ms, *now_ms),
            other => panic!("a holder asks for a lease: {other:?}"),
        })
        .collect();
    let answers = at_once(fixture, &requests, |store, request| {
        past_contention(|| store.acquire_lease(request)).expect("an answer")
    });

    let held = store
        .lease(&requests[0].key)
        .expect("a read")
        .expect("the lease");
    assert_eq!(held.fence.get(), 1);
    let mut granted = 0;
    for (request, answer) in requests.iter().zip(&answers) {
        match answer {
            LeaseAnswer::Granted(lease) => {
                assert_eq!(lease, &held);
                assert_eq!(lease.holder, request.holder);
                granted += 1;
            }
            LeaseAnswer::Held(lease) => {
                assert_eq!(lease, &held);
                assert_ne!(lease.holder, request.holder);
            }
        }
    }
    assert_eq!(granted, 1, "{answers:?}");
}

fn reservations_racing<S: ExecutionStore + StoreControl>(cases: &Cases, fixture: &Fixture<S>) {
    let race = cases.race("execution.reservations_racing");
    let store = set_up(cases, fixture, race);
    let units = units(cases, race);
    let reserved = |unit: &ExecutionUnit| match &unit.changes[0].record {
        Record::Reservation(held) => held.clone(),
        other => panic!("a writer reserves: {other:?}"),
    };
    let first = reserved(&units[0]);
    let key = first
        .scope()
        .position_key(first.instrument())
        .expect("a position");
    let (account, pool) = (key.account(), first.pool());

    // Each writer sizes against the pool as it reads it, and sizes again
    // when the pool has moved by the time it writes.
    let tries = at_once(fixture, &units, |store, unit| {
        for tried in 1..=100 {
            let revision = past_contention(|| store.pool(account, pool))
                .expect("the pool")
                .revision;
            let mut sized = unit.clone();
            sized.pool = Some(PoolGuard { revision });
            match past_contention(|| store.commit(&sized)).expect("an answer") {
                Commit::Applied => return tried,
                Commit::Stale(Stale::Pool { .. }) => {}
                other => panic!("applied, or the pool moved: {other:?}"),
            }
        }
        panic!("the pool kept moving");
    });
    assert_eq!(tries.len(), units.len());

    let held = store.pool(account, pool).expect("the pool");
    assert_eq!(
        held.revision,
        units.len() as u64,
        "one revision a reservation"
    );
    let mut expected: Vec<_> = units.iter().map(reserved).collect();
    let mut found = held.held.clone();
    let by_operation = |held: &quantforge::model::execution::Reservation| held.operation().get();
    expected.sort_by_key(by_operation);
    found.sort_by_key(by_operation);
    assert_eq!(found, expected);
    assert_eq!(
        held.total(pool).expect("a total").amount().to_string(),
        "1002.00"
    );
}

fn operations_racing<S: ExecutionStore + StoreControl>(cases: &Cases, fixture: &Fixture<S>) {
    let race = cases.race("execution.operations_racing");
    let store = set_up(cases, fixture, race);
    let asked: Vec<_> = race
        .writers
        .iter()
        .map(|writer| match writer {
            Step::BeginOperation {
                operation, at_ms, ..
            } => (cases.operation(operation), *at_ms),
            other => panic!("an asker begins an operation: {other:?}"),
        })
        .collect();
    let numbered = at_once(fixture, &asked, |store, (operation, at_ms)| {
        past_contention(|| store.begin_operation(operation, *at_ms)).expect("an operation")
    });

    let shared: Vec<&Stored<_>> = asked
        .iter()
        .zip(&numbered)
        .filter(|((operation, _), _)| operation.key == asked[0].0.key)
        .map(|(_, numbered)| numbered)
        .collect();
    assert_eq!(shared.len(), 8);
    assert!(shared.iter().all(|one| *one == shared[0]), "{shared:?}");
    let mut numbers: Vec<u64> = numbered.iter().map(|one| one.record.id().get()).collect();
    numbers.sort_unstable();
    numbers.dedup();
    assert_eq!(numbers.len(), 5, "one number a key: {numbers:?}");
    assert_eq!(count(&*fixture.raw, "execution_operations"), 5);
    for one in &numbered {
        let held = store
            .operation(one.record.id())
            .expect("a read")
            .expect("the operation");
        assert_eq!(&held, one);
    }
}

fn records_keep_their_text<S>(cases: &Cases, _: &Fixture<S>) {
    assert!(cases.records.len() > 100, "the fixture's records");
    let mut kinds: Vec<&str> = Vec::new();
    for (name, text) in &cases.records {
        let record = cases.record(name);
        assert_eq!(record.kind().as_str(), text.kind, "{name}");
        assert_eq!(record.to_stored().expect("stored"), text.stored, "{name}");
        if !kinds.contains(&text.kind.as_str()) {
            kinds.push(&text.kind);
        }
    }
    assert_eq!(kinds.len(), RecordKind::ALL.len(), "every kind: {kinds:?}");
}

/// The admission of the first-deliveries race: a signal, the note of its
/// delivery, its intent and its event, in one unit.
fn admission(cases: &Cases) -> ExecutionUnit {
    units(cases, cases.race("execution.first_deliveries_racing")).remove(0)
}

const ADMISSION_TABLES: [&str; 4] = [
    "signal_inbox",
    "signal_deliveries",
    "execution_intents",
    "outbox_events",
];

fn failed_unit_changes_nothing<S: ExecutionStore + StoreControl>(
    cases: &Cases,
    fixture: &Fixture<S>,
) {
    let race = cases.race("execution.first_deliveries_racing");
    let store = set_up(cases, fixture, race);
    let raw = &*fixture.raw;
    let unit = admission(cases);
    // Whichever of the unit's four writes fails, the first or the last.
    for refusing in ADMISSION_TABLES {
        let [refuse, allow] = raw.write_refusal(refusing);
        raw.execute(&refuse);
        let failed = refusal(store.commit(&unit), refusing);
        for table in ADMISSION_TABLES {
            assert_eq!(
                count(raw, table),
                0,
                "{table}, with {refusing} refusing: {failed}"
            );
        }
        raw.execute(&allow);
    }
    assert_eq!(store.commit(&unit).expect("an answer"), Commit::Applied);
    for table in ADMISSION_TABLES {
        assert_eq!(count(raw, table), 1, "{table}");
    }
}

fn waits_are_bounded<S: ExecutionStore + StoreControl + Bounded>(
    cases: &Cases,
    fixture: &Fixture<S>,
) {
    let race = cases.race("execution.leases_racing");
    let store = set_up(cases, fixture, race);
    let raw = &*fixture.raw;
    let bounded = store.within(BUDGET);
    let unit = admission(cases);
    let Step::AcquireLease {
        position,
        profile,
        holder,
        ttl_ms,
        now_ms,
        ..
    } = &race.writers[0]
    else {
        panic!("a holder asks for a lease");
    };
    let request = cases.lease_request(position, profile, holder, *ttl_ms, *now_ms);

    let in_time = |started: Instant, refused: &StorageError| {
        let took = started.elapsed();
        assert_eq!(refused.kind(), StorageErrorKind::Contention, "{refused}");
        assert!(took < BUDGET + SCHEDULING, "it took {took:?} of {BUDGET:?}");
        assert!(
            took > BUDGET / 2,
            "it gave up after {took:?}, without waiting"
        );
    };

    let held = raw.hold_writes("signal_inbox");
    let started = Instant::now();
    let refused = refusal(bounded.commit(&unit), "another writer holds the inbox");
    in_time(started, &refused);
    drop(held);
    assert_eq!(count(raw, "signal_inbox"), 0, "no answer, no signal");
    assert_eq!(count(raw, "signal_deliveries"), 0);
    assert_eq!(bounded.commit(&unit).expect("an answer"), Commit::Applied);

    let held = raw.hold_writes("position_leases");
    let started = Instant::now();
    let refused = refusal(
        bounded.acquire_lease(&request),
        "another writer holds the leases",
    );
    in_time(started, &refused);
    drop(held);
    assert_eq!(store.lease(&request.key).expect("a read"), None);
    let answer = bounded.acquire_lease(&request).expect("an answer");
    assert!(matches!(answer, LeaseAnswer::Granted(_)), "{answer:?}");
}

fn lost_answer_asked_again<S: ExecutionStore + StoreControl>(cases: &Cases, fixture: &Fixture<S>) {
    let race = cases.race("execution.same_change_racing");
    let store = set_up(cases, fixture, race);
    let raw = &*fixture.raw;
    let change = units(cases, race).remove(0);
    let delivered = race
        .setup
        .iter()
        .find_map(|step| match step {
            Step::Commit { unit, .. } if unit.delivery.is_some() => Some(cases.unit(unit)),
            _ => None,
        })
        .expect("the setup admits a signal");

    // The delivery's answer never arrived: sent again, it names the signal
    // that was recorded, and writes nothing but the note of it.
    let arrived = &delivered.delivery.as_ref().expect("a delivery").record;
    let recorded = store
        .signal(arrived.scope(), arrived.id())
        .expect("a read")
        .expect("the signal");
    raw.end_connections();
    assert_eq!(
        store.commit(&delivered).expect("an answer"),
        Commit::Duplicate(recorded)
    );
    assert_eq!(count(raw, "signal_inbox"), 1);
    assert_eq!(count(raw, "execution_intents"), 1);

    // The change's answer never arrived either: made again, it finds the
    // intent at the revision its own write left, holding what it wrote.
    assert_eq!(store.commit(&change).expect("an answer"), Commit::Applied);
    raw.end_connections();
    let again = store.commit(&change).expect("an answer");
    let Commit::Stale(Stale::Record { kind, found, .. }) = &again else {
        panic!("stale: {again:?}");
    };
    assert_eq!(
        (*kind, *found),
        (RecordKind::Intent, Found::Present { revision: 2 })
    );
    let unsettled = store.unsettled_intents().expect("a read");
    assert_eq!(
        Record::Intent(unsettled[0].record.clone()),
        change.changes[0].record,
        "its own write is what stands"
    );
    assert_eq!(count(raw, "execution_orders"), 1, "written once");
}

fn changed_row_refused<S: ExecutionStore + StoreControl>(cases: &Cases, fixture: &Fixture<S>) {
    let race = cases.race("execution.same_change_racing");
    let store = set_up(cases, fixture, race);
    let raw = &*fixture.raw;
    let change = units(cases, race).remove(0);
    let Record::Intent(intent) = &change.changes[0].record else {
        panic!("the race changes an intent");
    };
    let read = || store.intent(intent.scope(), intent.signal());
    assert!(read().expect("a read").is_some());

    for (edit, undo) in [
        // A column that no longer says what the document says.
        (
            "UPDATE execution_intents SET symbol = 'ETHUSDT'",
            "UPDATE execution_intents SET symbol = 'SOLUSDT'",
        ),
        (
            "UPDATE execution_intents SET state = 'submitting'",
            "UPDATE execution_intents SET state = 'admitted'",
        ),
        // A document that no longer says what its columns say.
        (
            "UPDATE execution_intents SET intent_json = \
             replace(intent_json, '\"state\":\"Admitted\"', '\"state\":\"Expired\"')",
            "UPDATE execution_intents SET intent_json = \
             replace(intent_json, '\"state\":\"Expired\"', '\"state\":\"Admitted\"')",
        ),
        // A document of a format this build does not read.
        (
            "UPDATE execution_intents SET intent_json = \
             replace(intent_json, '{\"format\":1,\"scope\"', '{\"format\":2,\"scope\"')",
            "UPDATE execution_intents SET intent_json = \
             replace(intent_json, '{\"format\":2,\"scope\"', '{\"format\":1,\"scope\"')",
        ),
    ] {
        raw.execute(edit);
        let refused = refusal(read(), edit);
        assert!(
            matches!(refused, StorageError::InvalidArgument(_)),
            "{edit}: {refused:?}"
        );
        let refused = refusal(store.commit(&change), edit);
        assert!(
            matches!(refused, StorageError::InvalidArgument(_)),
            "{edit}: {refused:?}"
        );
        assert_eq!(count(raw, "execution_orders"), 0, "{edit}");
        raw.execute(undo);
        assert!(read().expect("a read").is_some(), "{undo}");
    }
    assert_eq!(store.commit(&change).expect("an answer"), Commit::Applied);
}

fn lease_row_gives_no_authority<S: ExecutionStore + StoreControl>(
    cases: &Cases,
    fixture: &Fixture<S>,
) {
    // The race's setup with its activation left out: a bound store that
    // holds an admitted signal, and was never activated.
    let race = cases.race("execution.same_change_racing");
    let mut store = (fixture.reopen)();
    let mut asked = None;
    for current in &race.setup {
        match current {
            Step::Activate { .. } => {}
            Step::AcquireLease {
                position,
                profile,
                holder,
                ttl_ms,
                now_ms,
                ..
            } => asked = Some(cases.lease_request(position, profile, holder, *ttl_ms, *now_ms)),
            other => step(cases, &*fixture.raw, &*fixture.reopen, &mut store, other),
        }
    }
    let request = asked.expect("the setup asks for a lease");
    let change = units(cases, race).remove(0);
    let fence = change.fence.clone().expect("a fence");

    // The row a lease granted at epoch 1 leaves, written behind the store's back.
    fixture.raw.execute(&format!(
        "INSERT INTO position_leases(venue, category, environment, account_kind, account_id, \
         symbol, fence, holder, profile_id, activation_epoch, acquired_at_ms, expires_at_ms) \
         VALUES ('bybit', 'linear', 'demo', 'venue', '71002233', 'SOLUSDT', {}, '{}', 'main', \
         1, {}, {})",
        fence.token,
        fence.holder,
        request.now_ms,
        request.now_ms + request.ttl_ms
    ));
    let planted = store.lease(&fence.key).expect("a read").expect("the row");
    assert_eq!(planted.fence(), fence);
    let standing = store.store_identity().expect("the record");
    assert_eq!(standing.activation, StoreActivation::Inactive);
    match store.commit(&change).expect("an answer") {
        Commit::Fenced(quantforge::model::execution::Fenced::StoreNotActive {
            activation,
            epoch,
        }) => assert_eq!((activation, epoch.0), (StoreActivation::Inactive, 0)),
        other => panic!("fenced: {other:?}"),
    }
    assert_eq!(
        store
            .renew_lease(&fence, 30_000, request.now_ms)
            .expect("asked"),
        None
    );
    let refused = refusal(
        store.acquire_lease(&request),
        "a lease of an inactive store",
    );
    assert!(
        refused
            .to_string()
            .contains("only an active store grants a lease"),
        "{refused}"
    );

    // Activated at the very epoch the row names, the store holds the
    // lease to be no one's: it grants its own, under the next token.
    let active = store.activate(request.now_ms).expect("activated");
    assert_eq!(active.activation_epoch.0, 1);
    assert_eq!(store.lease(&fence.key).expect("a read"), None);
    assert_eq!(
        store.commit(&change).expect("an answer"),
        Commit::Fenced(quantforge::model::execution::Fenced::NotHeld)
    );
    assert_eq!(count(&*fixture.raw, "execution_orders"), 0);
    match store.acquire_lease(&request).expect("an answer") {
        LeaseAnswer::Granted(lease) => assert_eq!(lease.fence.get(), fence.token.get() + 1),
        LeaseAnswer::Held(lease) => panic!("held by {}", lease.holder),
    }
}
