//! `docs/execution-store.md` held to the code and to the shared cases: the
//! kinds of record with their tables and formats, the worked example, the
//! words a column spells a state with, a successor rule for every kind,
//! the order a unit's checks answer in, the answers, and what a store
//! keeps for good. The lists sit between `<!-- begin: name -->` and
//! `<!-- end: name -->`.

use std::path::Path;

use quantforge::automation::ExecutionLedger;
use quantforge::model::execution::{
    CYCLE_RECORD_FORMAT, Change, Commit, DeliveryState, DeliveryWrite, EVENT_FORMAT, EventKey,
    EventKind, ExecutionScope, ExecutionUnit, FILL_RECORD_FORMAT, Fence, FenceToken,
    INCIDENT_FORMAT, INTENT_RECORD_FORMAT, IncidentKind, IncidentState, OPERATION_FORMAT,
    ORDER_RECORD_FORMAT, OperationKey, OperationKind, OrderPhase, OutboxEvent, PoolGuard,
    RESERVATION_FORMAT, Record, RecordKind, ReleaseReason, ReservationState, SIGNAL_RECORD_FORMAT,
    SignalState, Stale,
};
use quantforge::model::futures::{OwnerTag, PoolScope, ProfileId, ProfileIdentity};
use quantforge::model::{
    Asset, Environment, ExchangeId, KEPT_BY_EACH_STORE, TRANSFER_TABLES, VenueAccountId,
};
use quantforge::{ExecutionStore, StorageError, StoreControl};
use serde_json::Value;

use crate::fixtures::{Delivered, Line, line, submitting};
use crate::orders::started;
use crate::support::{Backend, Manual, T, count, on_each_backend};

const PAGE: &str = "docs/execution-store.md";

fn read(path: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(path);
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// The lines of the page between a list's markers.
fn section(name: &str) -> Vec<String> {
    let doc = read(PAGE);
    let lines: Vec<&str> = doc.lines().collect();
    let at = |edge: &str| {
        let marker = format!("<!-- {edge}: {name} -->");
        lines
            .iter()
            .position(|line| line.trim() == marker)
            .unwrap_or_else(|| panic!("{PAGE} has no {marker}"))
    };
    lines[at("begin") + 1..at("end")]
        .iter()
        .map(|line| line.to_string())
        .collect()
}

/// The items of a list, each joined onto one line: a line that starts
/// with `- ` or with a number begins one.
fn items(name: &str) -> Vec<String> {
    let mut items: Vec<String> = Vec::new();
    for line in section(name) {
        let text = line.trim();
        if text.is_empty() {
            continue;
        }
        let numbered = text
            .split_once(". ")
            .is_some_and(|(number, _)| number.parse::<u32>().is_ok());
        if text.starts_with("- ") || numbered {
            items.push(text.to_string());
        } else {
            let item = items
                .last_mut()
                .unwrap_or_else(|| panic!("a line before the first item: {line}"));
            item.push(' ');
            item.push_str(text);
        }
    }
    items
}

/// The backticked words of a line, in order.
fn words(line: &str) -> Vec<String> {
    line.split('`')
        .skip(1)
        .step_by(2)
        .map(str::to_string)
        .collect()
}

fn fixture() -> Value {
    serde_json::from_str(&read("tests/fixtures/execution_cases.json")).expect("the fixture")
}

#[test]
fn the_page_lists_every_kind_of_record_with_its_table_and_the_format_this_build_writes() {
    let listed: Vec<Vec<String>> = items("record kinds")
        .iter()
        .map(|item| words(item)[..3].to_vec())
        .collect();
    let formats = [
        (RecordKind::Signal, SIGNAL_RECORD_FORMAT),
        (RecordKind::Operation, OPERATION_FORMAT),
        (RecordKind::Intent, INTENT_RECORD_FORMAT),
        (RecordKind::Order, ORDER_RECORD_FORMAT),
        (RecordKind::Fill, FILL_RECORD_FORMAT),
        (RecordKind::Cycle, CYCLE_RECORD_FORMAT),
        (RecordKind::Reservation, RESERVATION_FORMAT),
        (RecordKind::Incident, INCIDENT_FORMAT),
        (RecordKind::Event, EVENT_FORMAT),
    ];
    assert_eq!(
        formats.map(|(kind, _)| kind),
        RecordKind::ALL,
        "a format for every kind, in the kinds' order"
    );
    assert_eq!(listed.len(), formats.len(), "{listed:?}");
    for (listed, (kind, format)) in listed.iter().zip(formats) {
        let [name, table, version] = listed.as_slice() else {
            panic!("a kind, its table and its format: {listed:?}");
        };
        assert_eq!(name, kind.as_str());
        assert_eq!(version, &format.to_string(), "the format of {kind}");
        // The table keeps the kind's document in a column named for it.
        let held = TRANSFER_TABLES
            .iter()
            .find(|held| held.name == table)
            .unwrap_or_else(|| panic!("{table} is no table a store keeps"));
        let document = format!("{kind}_json");
        assert!(
            held.columns.iter().any(|column| column.name == document),
            "{table} holds no {document}"
        );
    }
    // Every record of the shared cases is of one of them, at its format.
    let fixture = fixture();
    for (name, held) in fixture["records"].as_object().expect("the records") {
        let kind = held["kind"].as_str().expect("a kind");
        let format = formats
            .iter()
            .find(|(known, _)| known.as_str() == kind)
            .unwrap_or_else(|| panic!("{name} is of no listed kind"))
            .1;
        let stored: Value =
            serde_json::from_str(held["stored"].as_str().expect("text")).expect("a document");
        assert_eq!(stored["format"], format, "{name}");
    }
}

#[test]
fn the_pages_example_is_a_record_of_the_shared_cases() {
    let block: Vec<String> = section("record example")
        .into_iter()
        .filter(|line| !line.starts_with("```"))
        .collect();
    let shown: Value = serde_json::from_str(&block.join("\n")).expect("the example is JSON");
    let fixture = fixture();
    let held = &fixture["records"]["reservation-1"];
    assert_eq!(held["kind"], "reservation");
    let text = held["stored"].as_str().expect("text");
    let stored: Value = serde_json::from_str(text).expect("a document");
    assert_eq!(shown, stored, "the example is the fixture's reservation-1");
    let record = Record::from_stored(RecordKind::Reservation, text).expect("a reservation");
    assert_eq!(record.to_stored().expect("stored"), text);
    // However the page wraps its lines, and whatever ends them.
    let prose = read(PAGE).split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(prose.contains("the fixture's `reservation-1`"));
}

#[test]
fn the_page_spells_every_word_a_column_holds() {
    fn spelled<T: Copy>(
        all: impl IntoIterator<Item = T>,
        word: fn(T) -> &'static str,
    ) -> Vec<String> {
        all.into_iter().map(|one| word(one).to_string()).collect()
    }
    let expected = [
        (
            "signal states",
            spelled(SignalState::ALL, SignalState::as_str),
        ),
        ("order phases", spelled(OrderPhase::ALL, OrderPhase::as_str)),
        (
            "reservation states",
            spelled(ReservationState::ALL, ReservationState::as_str),
        ),
        (
            "release reasons",
            spelled(ReleaseReason::ALL, ReleaseReason::as_str),
        ),
        (
            "incident states",
            spelled(IncidentState::ALL, IncidentState::as_str),
        ),
        (
            "incident kinds",
            spelled(IncidentKind::ALL, IncidentKind::as_str),
        ),
        (
            "delivery states",
            spelled(DeliveryState::ALL, DeliveryState::as_str),
        ),
        (
            "operation kinds",
            spelled(OperationKind::ALL, OperationKind::as_str),
        ),
        ("event kinds", spelled(EventKind::ALL, EventKind::as_str)),
    ];
    let listed = items("state words");
    assert_eq!(listed.len(), expected.len(), "{listed:?}");
    for (item, (name, spelling)) in listed.iter().zip(expected) {
        assert!(item.starts_with(&format!("- {name}: ")), "{item}");
        assert_eq!(words(item), spelling, "{name}");
    }
}

#[test]
fn the_page_gives_every_kind_of_record_its_rule_of_succession() {
    let listed: Vec<String> = items("successor rules")
        .iter()
        .map(|item| words(item).remove(0))
        .collect();
    let kinds: Vec<&str> = RecordKind::ALL
        .into_iter()
        .map(RecordKind::as_str)
        .collect();
    assert_eq!(listed, kinds);
}

#[test]
fn nothing_in_the_execution_store_is_deleted() {
    let listed = items("retention");
    let [kept, own] = listed.as_slice() else {
        panic!("what is kept for good, and what is the store's own: {listed:?}");
    };
    assert!(kept.starts_with("- kept for good: "), "{kept}");
    assert!(
        own.starts_with("- the store's own, rewritten in place: "),
        "{own}"
    );
    // The tables this story added: the transferred ones after the policy
    // tables, and the three each store keeps for itself.
    let policies = TRANSFER_TABLES
        .iter()
        .position(|table| table.name == "policy_revisions")
        .expect("the policy revisions");
    let mut tables: Vec<&str> = TRANSFER_TABLES[policies + 1..]
        .iter()
        .map(|table| table.name)
        .collect();
    tables.extend(&KEPT_BY_EACH_STORE[3..]);
    let mut listed: Vec<String> = words(kept);
    listed.extend(words(own));
    tables.sort_unstable();
    listed.sort_unstable();
    assert_eq!(listed, tables, "every table of the execution store");
    assert_eq!(words(own), ["position_leases", "reservation_pools"]);

    // And no statement of either backend's store removes a row.
    let mut scanned = 0;
    for directory in [
        "src/storage/execution",
        "src/storage/sqlite/execution",
        "src/storage/postgres/execution",
    ] {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join(directory);
        for entry in std::fs::read_dir(&directory).expect("a directory") {
            let path = entry.expect("an entry").path();
            if path.extension().is_none_or(|extension| extension != "rs") {
                continue;
            }
            let source = std::fs::read_to_string(&path).expect("a source file");
            for word in ["DELETE", "TRUNCATE", "DROP TABLE"] {
                assert!(!source.contains(word), "{} holds {word}", path.display());
            }
            scanned += 1;
        }
    }
    assert!(scanned >= 12, "the stores' sources: {scanned}");
}

#[test]
fn the_page_names_the_keys_and_the_layout_the_code_builds() {
    let page = read(PAGE);
    let line = line("bybit linear");
    let delivered = line.delivery("entry", "trend", 0, "234.23");
    let key = OperationKey::of_signal(line.identity.id(), &delivered.signal.id, 1);
    assert_eq!(key.as_str(), format!("sig/main/1/{}", delivered.signal.id));
    assert!(page.contains("`sig/<profile>/<ordinal>/<signal ID>`"));
    let id = line.client_order_id(
        quantforge::model::futures::ActivationEpoch(1),
        quantforge::model::OperationId::new(1).expect("an operation id"),
        quantforge::model::futures::OrderLeg::Entry,
        1,
    );
    assert_eq!(id.to_string(), "qf1-k3x9p2-1-1-e1");
    assert!(page.contains("`qf1-<owner tag>-<epoch>-<operation>-<leg><attempt>`"));
}

/// What a unit came to, as the page names it.
fn answer(outcome: &Result<Commit, StorageError>) -> &'static str {
    match outcome {
        Ok(Commit::Applied) => "applied",
        Ok(Commit::Duplicate(_)) => "duplicate",
        Ok(Commit::Conflict(_)) => "conflict",
        Ok(Commit::Stale(_)) => "stale",
        Ok(Commit::Fenced(_)) => "fenced",
        Err(StorageError::InvalidArgument(_)) => "refused",
        Err(other) => panic!("an answer or a refusal: {other:?}"),
    }
}

/// One unit that fails every check the page lists, mended one check at a
/// time: each time it answers with the next check of the list, and
/// nothing of it is written until none is left.
fn a_units_checks_answer_in_the_order_the_page_lists<B: Backend>(backend: &B) {
    let store = backend.open();
    store.activate(T).expect("activated");
    let clock = Manual::at(T + 6_000);
    let ledger = ExecutionLedger::new(&store, &clock);
    let main = line("bybit linear");
    let other = Line {
        identity: ProfileIdentity::new(
            ProfileId::new("second").expect("a profile id"),
            main.identity.account_ref().clone(),
            main.identity.category(),
            OwnerTag::new("a1b2c3").expect("an owner tag"),
        )
        .expect("an identity"),
        ..main.clone()
    };
    // Two profiles' entries admitted on one position. The first is sent,
    // with its margin reserved: it holds the position's entry slot, its
    // order stands at revision 1, and the pool at revision 1.
    let first = started(&store, &ledger, &main, "entry", "trend", 0);
    let second = started(&store, &ledger, &other, "entry", "other", 0);
    let lease = main.lease(&store, "executor-a", T + 7_000);
    clock.set(T + 8_000);
    let held = main.reservation(first.operation, "205.18548", T + 8_000);
    ledger
        .record_attempt(
            &lease.fence(),
            &first.intent,
            &first.id,
            Some((held.clone(), 0)),
        )
        .expect("recorded");
    let order = store
        .order(&main.scope(), &first.id.to_string())
        .expect("a read")
        .expect("the order");
    let at_ms = T + 9_000;

    let delivery = |delivered: &Delivered| {
        let record = main.received(delivered, at_ms);
        DeliveryWrite {
            conflict_event: main.conflict_event(&record),
            record,
        }
    };
    let recorded = delivery(&main.delivery("entry", "trend", 0, "234.23"));
    let altered = delivery(&main.delivery("entry", "trend", 0, "999.99"));
    let fresh = delivery(&main.delivery("entry", "fresh", 5, "234.23"));
    let event = |scope: ExecutionScope| {
        let key = EventKey::new("docs/the-order-of-the-checks").expect("a key");
        Record::Event(OutboxEvent::pending(
            scope,
            key,
            EventKind::IncidentOpened,
            at_ms,
        ))
    };
    let elsewhere = ExecutionScope::of_account(
        ExchangeId::BybitLinear,
        Environment::Testnet,
        VenueAccountId::issued("71002233").expect("an account id"),
    );
    let not_its_writers = Fence {
        token: FenceToken::new(9).expect("a token"),
        ..lease.fence()
    };
    let reserving = Record::Reservation(other.reservation(second.operation, "100.5", at_ms));
    let acknowledged = Record::Order(
        order
            .record
            .acknowledged("8389765519")
            .expect("a successor"),
    );
    let released = held
        .release(ReleaseReason::NotOpened, at_ms)
        .expect("a successor");
    let taking_the_slot = Change::replace(
        Record::Intent(submitting(&second.intent.record, &second.id, at_ms)),
        second.intent.revision,
    );
    // Everything wrong at once; each `mend` below puts one thing right.
    struct Mended {
        environment: bool,
        fence: bool,
        delivery: u8,
        pool: bool,
        revision: bool,
        successor: bool,
        slot: bool,
    }
    let unit = |mended: &Mended| {
        let mut unit = ExecutionUnit::at(at_ms).under(if mended.fence {
            lease.fence()
        } else {
            not_its_writers.clone()
        });
        unit.delivery = Some(match mended.delivery {
            0 => recorded.clone(),
            1 => altered.clone(),
            _ => fresh.clone(),
        });
        unit.pool = Some(PoolGuard {
            revision: u64::from(mended.pool),
        });
        unit.changes = vec![
            Change::new(event(if mended.environment {
                main.scope()
            } else {
                elsewhere.clone()
            })),
            Change::new(reserving.clone()),
            Change::replace(
                acknowledged.clone(),
                if mended.revision { order.revision } else { 5 },
            ),
            Change::replace(
                Record::Reservation(if mended.successor {
                    released.clone()
                } else {
                    held.clone()
                }),
                1,
            ),
        ];
        if !mended.slot {
            unit.changes.push(taking_the_slot.clone());
        }
        unit
    };
    let mut mended = Mended {
        environment: false,
        fence: false,
        delivery: 0,
        pool: false,
        revision: false,
        successor: false,
        slot: false,
    };
    let mut answers: Vec<&'static str> = Vec::new();
    let mut said: Vec<String> = Vec::new();
    let mut commit = |mended: &Mended| {
        let outcome = store.commit(&unit(mended));
        answers.push(answer(&outcome));
        said.push(match &outcome {
            Ok(Commit::Stale(Stale::Pool { .. })) => "the pool".to_string(),
            Ok(Commit::Stale(Stale::Record { kind, .. })) => format!("the {kind}"),
            Ok(other) => format!("{other:?}"),
            Err(refused) => refused.to_string(),
        });
    };
    commit(&mended);
    mended.environment = true;
    commit(&mended);
    mended.fence = true;
    commit(&mended);
    mended.delivery = 1;
    commit(&mended);
    mended.delivery = 2;
    commit(&mended);
    mended.pool = true;
    commit(&mended);
    mended.revision = true;
    commit(&mended);
    mended.successor = true;
    commit(&mended);
    // Nothing of the unit is written yet but the notes of the two
    // deliveries the inbox knew, and the one conflict it was told of.
    assert_eq!(count(backend, "signal_inbox"), 2);
    assert_eq!(count(backend, "margin_reservations"), 1);
    assert_eq!(count(backend, "signal_deliveries"), 4);
    assert_eq!(
        store
            .order(&main.scope(), &first.id.to_string())
            .expect("a read"),
        Some(order.clone())
    );
    mended.slot = true;
    commit(&mended);
    assert_eq!(count(backend, "signal_inbox"), 3);
    assert_eq!(count(backend, "margin_reservations"), 2);

    // The page's list, in its order: each item's answers.
    let known = [
        "applied",
        "duplicate",
        "conflict",
        "stale",
        "fenced",
        "refused",
    ];
    let listed: Vec<String> = items("decision order")
        .iter()
        .flat_map(|item| words(item))
        .collect();
    assert!(
        listed.iter().all(|word| known.contains(&word.as_str())),
        "{listed:?}"
    );
    assert_eq!(answers, listed, "{said:#?}");
    // And each answer is of the check the item names.
    assert!(
        said[0].contains("is for testnet, and the store is bound to demo"),
        "{}",
        said[0]
    );
    assert_eq!(said[4], "the pool");
    assert_eq!(said[5], "the order");
    assert!(
        said[6].contains("cannot follow the one recorded"),
        "{}",
        said[6]
    );
    assert!(
        said[7].contains("cannot take its position's slot"),
        "{}",
        said[7]
    );
    let pool = PoolScope::Asset(Asset::new("USDT").expect("an asset"));
    let held = store
        .pool(main.identity.account_ref(), &pool)
        .expect("the pool");
    assert_eq!((held.revision, held.held.len()), (2, 1));
}

on_each_backend!(a_units_checks_answer_in_the_order_the_page_lists);

#[test]
fn the_page_lists_the_answers_a_unit_comes_to() {
    let listed: Vec<String> = items("answers")
        .iter()
        .map(|item| words(item).remove(0))
        .collect();
    let stale = Commit::Stale(Stale::Pool { revision: 1 });
    let fenced = Commit::Fenced(quantforge::model::execution::Fenced::NotHeld);
    let named = [
        answer(&Ok(Commit::Applied)),
        "duplicate",
        "conflict",
        answer(&Ok(stale)),
        answer(&Ok(fenced)),
    ];
    assert_eq!(listed, named);
}
