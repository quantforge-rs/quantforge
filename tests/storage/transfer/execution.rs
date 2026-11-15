//! The execution store in a copy (`docs/storage-migration.md`): every
//! record the fixture holds reads from the target as it reads from the
//! source, and as the oracle wrote it, byte for byte; the copied inbox
//! still knows a delivery it has seen; nothing of the source's authority
//! comes along; and while a copy is unfinished the target takes no unit,
//! no operation, no activation and no lease.

use std::sync::atomic::AtomicUsize;

use quantforge::model::execution::{
    Change, Commit, DeliveryWrite, EventKey, EventKind, ExecutionUnit, Fence, FenceToken, Fenced,
    LeaseAnswer, LeaseHolder, LeaseRequest, NewOperation, OperationKey, OperationKind, OutboxEvent,
    Record, RecordKind, SignalRecord,
};
use quantforge::model::futures::{IntentState, PoolScope, SignalHash, SignalId};
use quantforge::model::{Asset, TransferValue};
use quantforge::{
    CopyStart, EngineError, ExecutionStore, SqliteStore, StoreActivation, StoreControl,
    TransferSource,
};

use super::postgres::{Bed, Cut, assert_refused, counts};
use crate::postgres::query;

/// 2026-10-20T00:00:00Z.
const T: i64 = 1_792_454_400_000;

/// The tables that hold records, each with its kind and its document.
const RECORDS: [(&str, RecordKind, &str); 9] = [
    ("signal_inbox", RecordKind::Signal, "signal_json"),
    (
        "execution_operations",
        RecordKind::Operation,
        "operation_json",
    ),
    ("execution_intents", RecordKind::Intent, "intent_json"),
    ("execution_orders", RecordKind::Order, "order_json"),
    ("execution_fills", RecordKind::Fill, "fill_json"),
    ("position_cycles", RecordKind::Cycle, "cycle_json"),
    (
        "margin_reservations",
        RecordKind::Reservation,
        "reservation_json",
    ),
    ("execution_incidents", RecordKind::Incident, "incident_json"),
    ("outbox_events", RecordKind::Event, "event_json"),
];

/// Every record of the fixture's execution tables, with the revision its
/// row stands at.
fn fixture_records(bed: &Bed) -> Vec<(&'static str, Record, u64)> {
    let mut out = Vec::new();
    for (table, kind, document) in RECORDS {
        let held = bed.cases.table(table);
        let at = |column: &str| held.columns.iter().position(|name| name == column);
        let document = at(document).expect("the document's column");
        for row in &held.rows {
            let TransferValue::Text(text) = &row[document] else {
                panic!("{table}: a document is text");
            };
            let record = Record::from_stored(kind, text)
                .unwrap_or_else(|error| panic!("{table}: {error}: {text}"));
            // The oracle's document is, byte for byte, the one this build
            // writes of the record it reads from it.
            assert_eq!(&record.to_stored().expect("stored"), text, "{table}");
            let revision = match at("revision").map(|index| &row[index]) {
                Some(TransferValue::Integer(revision)) => {
                    u64::try_from(*revision).expect("a revision")
                }
                // A fill is written once.
                None => 1,
                other => panic!("{table}: a revision is a number: {other:?}"),
            };
            out.push((table, record, revision));
        }
    }
    out
}

/// The record `named` names, as `store` reads it by what it is known by.
fn read_back(store: &impl ExecutionStore, named: &Record) -> Option<(Record, u64)> {
    match named {
        Record::Signal(record) => store
            .signal(record.scope(), record.id())
            .expect("a read")
            .map(|held| (Record::Signal(held.record), held.revision)),
        Record::Operation(record) => store
            .operation(record.id())
            .expect("a read")
            .map(|held| (Record::Operation(held.record), held.revision)),
        Record::Intent(record) => store
            .intent(record.scope(), record.signal())
            .expect("a read")
            .map(|held| (Record::Intent(held.record), held.revision)),
        Record::Order(record) => store
            .order(record.scope(), record.client_order_id())
            .expect("a read")
            .map(|held| (Record::Order(held.record), held.revision)),
        Record::Fill(record) => store
            .fill(record.scope(), &record.fill().instrument, record.trade_id())
            .expect("a read")
            .map(|held| (Record::Fill(held), 1)),
        Record::Cycle(record) => store
            .cycle(record.id())
            .expect("a read")
            .map(|held| (Record::Cycle(held.record), held.revision)),
        Record::Reservation(record) => store
            .reservation(record.operation())
            .expect("a read")
            .map(|held| (Record::Reservation(held.record), held.revision)),
        Record::Incident(record) => store
            .incident(record.scope(), record.key())
            .expect("a read")
            .map(|held| (Record::Incident(held.record), held.revision)),
        Record::Event(record) => store
            .event(record.scope(), record.key())
            .expect("a read")
            .map(|held| (Record::Event(held.record), held.revision)),
    }
}

/// The first signal of the fixture's inbox, as its store holds it.
fn first_signal(bed: &Bed) -> SignalRecord {
    fixture_records(bed)
        .into_iter()
        .find_map(|(_, record, _)| match record {
            Record::Signal(signal) => Some(signal),
            _ => None,
        })
        .expect("the fixture holds a signal")
}

/// A delivery of `payload` under the ID `id`, to the inbox `like`'s
/// signal is in, with the event that tells of it if it conflicts.
fn delivery(like: &SignalRecord, id: &str, payload: &str, at_ms: i64) -> ExecutionUnit {
    let mut signal = like.signal().clone();
    signal.id = SignalId::new(id).expect("a signal id");
    signal.payload_hash = SignalHash::of(payload.as_bytes());
    let record = SignalRecord::received(like.scope().clone(), signal, payload, at_ms)
        .expect("a signal record");
    let word = format!("conflict:{}", record.payload_hash());
    let conflict_event = OutboxEvent::pending(
        record.scope().clone(),
        EventKey::of_signal(record.profile(), &word, record.id()),
        EventKind::SignalConflict,
        at_ms,
    )
    .of_signal(record.id());
    let mut unit = ExecutionUnit::at(at_ms);
    unit.delivery = Some(DeliveryWrite {
        record,
        conflict_event,
    });
    unit
}

#[test]
fn every_execution_record_reads_from_the_copy_as_from_the_source_and_as_the_oracle_wrote_it() {
    let Some(bed) = Bed::new("") else {
        return;
    };
    bed.copy(3);
    let source = SqliteStore::new(&bed.path);
    let target = &bed.target;
    let records = fixture_records(&bed);
    assert_eq!(records.len(), 35, "the fixture's records");
    for (table, record, revision) in &records {
        let copied = read_back(target, record)
            .unwrap_or_else(|| panic!("{table}: the copy holds no {}", record.name()));
        assert_eq!(copied, (record.clone(), *revision), "{table}");
        assert_eq!(read_back(&source, record), Some(copied), "{table}");
    }

    // What a process that starts on the copy is handed to resume: the same
    // as on the source, in the same order.
    let unsettled = target.unsettled_intents().expect("the copy's");
    assert_eq!(unsettled, source.unsettled_intents().expect("the source's"));
    let states: Vec<IntentState> = unsettled
        .iter()
        .map(|held| held.record.intent().state())
        .collect();
    assert_eq!(
        states,
        [IntentState::OutcomeUnknown, IntentState::Submitting],
        "an exit not known to have arrived, and an entry still on its way"
    );
    let cycles = target.open_cycles().expect("the copy's");
    assert_eq!(cycles, source.open_cycles().expect("the source's"));
    assert_eq!(cycles.len(), 1);
    let incidents = target.open_incidents().expect("the copy's");
    assert_eq!(incidents, source.open_incidents().expect("the source's"));
    assert_eq!(incidents.len(), 1);
    let replacements = target.unsettled_replacements().expect("the copy's");
    assert_eq!(
        replacements,
        source.unsettled_replacements().expect("the source's")
    );
    assert_eq!(replacements.len(), 1);
    let received = target.received_signals(10).expect("the copy's");
    assert_eq!(received, source.received_signals(10).expect("the source's"));
    assert_eq!(received.len(), 1);
    let pending = target.pending_events(100).expect("the copy's");
    assert_eq!(pending, source.pending_events(100).expect("the source's"));
    assert_eq!(pending.len(), 6, "all but the one delivered");
    let first = first_signal(&bed);
    let noted = target
        .deliveries(first.scope(), first.id())
        .expect("the copy's");
    let theirs = source
        .deliveries(first.scope(), first.id())
        .expect("the source's");
    assert_eq!(noted, theirs);
    let dispositions: Vec<&str> = noted
        .iter()
        .map(|delivery| delivery.disposition.as_str())
        .collect();
    assert_eq!(dispositions, ["new", "duplicate", "conflict"]);
    for cycle in &cycles {
        let id = cycle.record.id();
        assert_eq!(
            target.fills(id).expect("the copy's"),
            source.fills(id).expect("the source's")
        );
        assert_eq!(target.fills(id).expect("the copy's").len(), 2);
    }
}

#[test]
fn a_copy_brings_the_inbox_and_the_ledger_and_none_of_the_sources_authority() {
    let Some(bed) = Bed::new("") else {
        return;
    };
    bed.copy(5000);
    let target = &bed.target;
    let identity = target.store_identity().expect("the record");
    assert_eq!(
        (identity.activation, identity.activation_epoch.0),
        (StoreActivation::Inactive, 0)
    );
    assert_eq!(target.control_events().expect("the log"), []);
    for kept in [
        "position_leases",
        "reservation_pools",
        "store_control_events",
    ] {
        let rows = bed.count(&format!("SELECT count(*) FROM {kept}"));
        assert_eq!(rows, "0", "{kept} is the target's own");
    }

    // The entry the source left on its way: its position was leased there,
    // and its margin held.
    let unsettled = target.unsettled_intents().expect("the copy's");
    let entry = unsettled
        .iter()
        .find(|held| held.record.blocks_entries())
        .expect("an unresolved entry");
    let key = entry.record.key().clone();
    let request = LeaseRequest {
        key: key.clone(),
        profile: entry.record.profile().clone(),
        holder: LeaseHolder::new("executor-t").expect("a holder"),
        ttl_ms: 30_000,
        now_ms: T + 200_000,
    };
    let refused = target
        .acquire_lease(&request)
        .expect_err("an inactive store grants no lease");
    assert!(
        refused
            .to_string()
            .contains("only an active store grants a lease"),
        "{refused}"
    );
    assert_eq!(target.lease(&key).expect("a read"), None);
    // The source's executor held token 1 there. Here it holds nothing.
    let mut unknown = entry.record.intent().clone();
    unknown
        .move_to(IntentState::OutcomeUnknown)
        .expect("an unknown outcome");
    let moved = entry.record.with_intent(unknown).expect("a successor");
    let theirs = Fence {
        key: key.clone(),
        holder: LeaseHolder::new("executor-c").expect("a holder"),
        token: FenceToken::new(1).expect("a token"),
    };
    let change = |fence: Fence| {
        ExecutionUnit::at(T + 200_000)
            .under(fence)
            .with(Change::replace(
                Record::Intent(moved.clone()),
                entry.revision,
            ))
    };
    assert_eq!(
        target.commit(&change(theirs.clone())).expect("an answer"),
        Commit::Fenced(Fenced::NotHeld)
    );

    // The inbox came along whole: a delivery the source recorded is a
    // duplicate here too, and names the signal as the source judged it.
    let first = first_signal(&bed);
    let again = delivery(&first, first.id().as_str(), first.payload(), T + 200_000);
    match target.commit(&again).expect("an answer") {
        Commit::Duplicate(named) => assert_eq!(named.record, first),
        other => panic!("a duplicate: {other:?}"),
    }
    // What the target numbers, it numbers past what was copied.
    let fresh = delivery(
        &first,
        "copy:entry:long:SOLUSDT.P:2026-10-20T00:09:00Z",
        r#"{"made":"on the target"}"#,
        T + 201_000,
    );
    assert_eq!(target.commit(&fresh).expect("an answer"), Commit::Applied);
    assert_eq!(bed.count("SELECT max(seq) FROM signal_inbox"), "8");
    assert_eq!(bed.count("SELECT count(*) FROM signal_inbox"), "8");
    let asked = NewOperation {
        scope: first.scope().clone(),
        kind: OperationKind::Stop,
        key: Some(OperationKey::new("made/on/the/target").expect("a key")),
        instrument: entry.record.key().instrument().clone(),
        signal: None,
        cycle: None,
        replacement: None,
    };
    // The entry's own scope, which trades its instrument.
    let asked = NewOperation {
        scope: entry.record.scope().clone(),
        ..asked
    };
    let numbered = target
        .begin_operation(&asked, T + 202_000)
        .expect("numbered");
    assert_eq!(numbered.record.id().get(), 6, "past the five copied");

    // Authority is the operator's to give, to this store: activated, it
    // starts at its own first epoch, and grants its own first token.
    let active = target.activate(T + 203_000).expect("activated");
    assert_eq!(active.activation_epoch.0, 1);
    assert_eq!(
        target.commit(&change(theirs)).expect("an answer"),
        Commit::Fenced(Fenced::NotHeld),
        "the source's lease is no lease here, active or not"
    );
    let LeaseAnswer::Granted(lease) = target.acquire_lease(&request).expect("an answer") else {
        panic!("the position is no one's");
    };
    assert_eq!(lease.fence.get(), 1);
    assert_eq!(
        target.commit(&change(lease.fence())).expect("an answer"),
        Commit::Applied
    );
    // The margin the source held is held here, against a pool whose
    // revision is this store's own.
    let pool = PoolScope::Asset(Asset::new("BTC").expect("an asset"));
    let held = target.pool(key.account(), &pool).expect("the pool");
    assert_eq!((held.revision, held.held.len()), (0, 1));
    assert_eq!(
        held.total(&pool).expect("a total").amount().to_string(),
        "0.00150000"
    );
}

#[test]
fn a_target_whose_copy_is_unfinished_takes_no_unit_no_operation_and_no_activation() {
    let Some(bed) = Bed::new("") else {
        return;
    };
    let store = &bed.target;
    let first = first_signal(&bed);
    let scope = first.scope().clone();
    let instrument = fixture_records(&bed)
        .into_iter()
        .find_map(|(_, record, _)| match record {
            Record::Intent(intent) if intent.scope() == &scope => {
                Some(intent.key().instrument().clone())
            }
            _ => None,
        })
        .expect("an intent of the first signal's profile");
    let request = LeaseRequest {
        key: scope.position_key(&instrument).expect("a position"),
        profile: first.profile().clone(),
        holder: LeaseHolder::new("executor-a").expect("a holder"),
        ttl_ms: 600_000,
        now_ms: T + 3_000,
    };

    let snapshot = bed.snapshot();
    let cut = Cut {
        inner: bed.target(),
        writes_left: AtomicUsize::new(29),
    };
    let stopped = bed.engine(&snapshot, &cut, 1).copy().expect_err("cut off");
    assert!(
        matches!(stopped, EngineError::CopyStopped { .. }),
        "{stopped:?}"
    );
    let source = snapshot.transfer_identity().expect("identity").store_id;
    let source = source.as_str();
    let before = counts(&bed);

    // Each of these would be taken by a store that held no unfinished
    // copy: the copy's beginning bound the store to the source's
    // environment, and none of the execution tables holds a row yet.
    let delivered = delivery(
        &first,
        "copy:entry:long:SOLUSDT.P:2026-10-20T00:09:00Z",
        r#"{"made":"while the copy was unfinished"}"#,
        T + 1_000,
    );
    assert_refused("a delivery", store.commit(&delivered), source);
    let asked = NewOperation {
        scope,
        kind: OperationKind::Stop,
        key: Some(OperationKey::new("made/while/copying").expect("a key")),
        instrument,
        signal: None,
        cycle: None,
        replacement: None,
    };
    assert_refused(
        "begin_operation",
        store.begin_operation(&asked, T + 1_000),
        source,
    );
    assert_refused("activate", store.activate(T + 1_000), source);
    // No lease either: of a store that is neither whole nor active.
    store
        .acquire_lease(&request)
        .expect_err("no lease of a store that is being copied into");
    assert_eq!(counts(&bed), before);
    assert_eq!(
        store.store_identity().expect("the record").activation,
        StoreActivation::Inactive
    );
    assert_eq!(bed.count("SELECT count(*) FROM position_leases"), "0");
    assert_eq!(bed.count("SELECT count(*) FROM store_control_events"), "0");

    // Even a store made active behind its own back, with a lease row to
    // match, grants no lease and extends none while its copy is
    // unfinished.
    query(
        &bed.database.url,
        "UPDATE store_control SET activation = 'active', activation_epoch = 1",
    );
    query(
        &bed.database.url,
        &format!(
            "INSERT INTO position_leases(venue, category, environment, account_kind, \
             account_id, symbol, fence, holder, profile_id, activation_epoch, acquired_at_ms, \
             expires_at_ms) VALUES ('bybit', 'linear', 'testnet', 'venue', '71002233', \
             'SOLUSDT', 1, 'executor-a', 'main', 1, {T}, {})",
            T + 600_000
        ),
    );
    let planted = store.lease(&request.key).expect("a read").expect("the row");
    assert_refused("acquire_lease", store.acquire_lease(&request), source);
    let renewed = store.renew_lease(&planted.fence(), 600_000, T + 3_000);
    assert_refused("renew_lease", renewed, source);
    query(&bed.database.url, "DELETE FROM position_leases");
    query(
        &bed.database.url,
        "UPDATE store_control SET activation = 'inactive', activation_epoch = 0",
    );

    assert_eq!(bed.copy(1).start, CopyStart::Resumed);
    assert_eq!(
        store.commit(&delivered).expect("an answer"),
        Commit::Applied
    );
    let numbered = store.begin_operation(&asked, T + 2_000).expect("numbered");
    assert_eq!(numbered.record.id().get(), 6, "past the five copied");
    store.activate(T + 2_000).expect("activated");
    let answer = store.acquire_lease(&request);
    assert!(matches!(answer, Ok(LeaseAnswer::Granted(_))), "{answer:?}");
}
