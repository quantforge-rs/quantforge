//! The one order a unit of the execution store takes its locks in on
//! PostgreSQL, whatever order it lists its changes in: the rows it finds
//! and the tables it writes, each in the order a copy takes its tables.
//! Two writers that lock in one order never hold a lock each that the
//! other waits for, so no unit is ever ended by the server for a wait
//! that could not end. Each test lists a unit's changes the other way
//! round, holds one lock as another writer would, and sees what the unit
//! holds while it waits.

use super::execution::{Cases, raced};
use super::scratch::rows;
use super::segments::{Held, wait_for_a_lock};
use crate::model::execution::{Change, Commit, ExecutionUnit, Found, RecordKind, Stale};
use crate::model::{TRANSFER_TABLES, TransferValue};
use crate::ports::ExecutionStore;
use crate::storage::execution::plan::Plan;
use serde_json::Value;
use std::thread;

/// `unit` with its changes listed the other way round.
fn backwards(unit: &ExecutionUnit) -> ExecutionUnit {
    ExecutionUnit {
        changes: unit.changes.iter().rev().cloned().collect(),
        ..unit.clone()
    }
}

/// Every unit of the shared cases: of each scenario's steps, and of each
/// race's setup and writers.
fn units(cases: &Cases) -> Vec<ExecutionUnit> {
    let steps = |of: &Value, member: &str| -> Vec<Value> {
        of[member].as_array().cloned().unwrap_or_default()
    };
    let mut found = Vec::new();
    for scenario in steps(&cases.0, "scenarios") {
        found.extend(steps(&scenario, "steps"));
    }
    for race in steps(&cases.0, "races") {
        found.extend(steps(&race, "setup"));
        found.extend(steps(&race, "writers"));
    }
    found
        .iter()
        .filter(|step| step["unit"].is_object())
        .map(|step| cases.unit(&step["unit"]))
        .collect()
}

/// The rows a plan's unit reads, and locks where it finds them, in the
/// order it does: each as its table and the values that name it.
fn locked(plan: &Plan) -> Vec<(&'static str, Vec<TransferValue>)> {
    plan.lock_order()
        .into_iter()
        .map(|index| {
            let lookup = &plan.lookups[index];
            let named = lookup.equals.iter().map(|(_, value)| value.clone());
            (lookup.table.name(), named.collect())
        })
        .collect()
}

/// Of every unit of the shared cases: listed the other way round it reads
/// the same rows in the same order, table by table as a copy takes them.
#[test]
fn a_unit_reads_its_rows_in_one_order_however_it_lists_its_changes() {
    let cases = Cases::load();
    let rank = |name: &str| TRANSFER_TABLES.iter().position(|table| table.name == name);
    let mut reordered = 0;
    for unit in units(&cases) {
        let other = backwards(&unit);
        let (plan, other) = match (Plan::of(&unit), Plan::of(&other)) {
            (Ok(plan), Ok(other)) => (plan, other),
            (Err(_), Err(_)) => continue,
            (one, other) => panic!("one order is refused and not the other: {one:?} {other:?}"),
        };
        let order = locked(&plan);
        assert_eq!(order, locked(&other), "{unit:?}");
        let ranks: Vec<_> = order.iter().map(|(table, _)| rank(table)).collect();
        assert!(ranks.iter().all(Option::is_some), "{order:?}");
        assert!(ranks.windows(2).all(|pair| pair[0] <= pair[1]), "{order:?}");
        let listed: Vec<_> = plan
            .lookups
            .iter()
            .map(|lookup| lookup.table.name())
            .collect();
        let sorted: Vec<_> = order.iter().map(|(table, _)| *table).collect();
        if listed != sorted {
            reordered += 1;
        }
    }
    assert!(
        reordered >= 10,
        "the cases list their changes in another order than a copy's {reordered} times"
    );
}

/// The rows of `table` no transaction holds.
fn unlocked(store: &super::PostgresStore, table: &str) -> String {
    let sql =
        format!("SELECT COUNT(*) FROM (SELECT 1 FROM {table} FOR UPDATE SKIP LOCKED) AS unlocked");
    rows(store, &sql)[0][0].clone().expect("a count")
}

/// A unit that finds two rows locks the one whose table a copy takes
/// first, though it lists the other first: while another writer holds the
/// order's row, the unit waits for it with the intent's row already
/// locked. Its answer is still the first check to fail in the order the
/// unit lists its changes, which is the contract's.
#[test]
fn a_unit_locks_the_rows_it_finds_in_one_order_whatever_order_it_lists_them_in() {
    let Some(raced) = raced() else {
        return;
    };
    let recorded = raced.store.commit(&raced.change).expect("an answer");
    assert_eq!(recorded, Commit::Applied);
    let [intent, order] = [&raced.change.changes[0], &raced.change.changes[1]];
    assert_eq!(
        (intent.record.kind(), order.record.kind()),
        (RecordKind::Intent, RecordKind::Order)
    );
    // Both rows are found, and neither stands at the revision asked for.
    let unit = ExecutionUnit {
        changes: vec![
            Change::replace(order.record.clone(), 9),
            Change::replace(intent.record.clone(), 9),
        ],
        ..raced.change.clone()
    };

    let another_writer = Held::start(
        &raced.scratch,
        "SELECT seq FROM execution_orders FOR UPDATE".to_string(),
        String::new(),
    );
    let answer = thread::scope(|scope| {
        let committing = scope.spawn(|| raced.store.commit(&unit));
        wait_for_a_lock(&raced.store);
        assert!(!committing.is_finished(), "it waits for the order's row");
        assert_eq!(
            unlocked(&raced.store, "execution_intents"),
            "0",
            "the intent's row is locked first, though the unit lists it last"
        );
        another_writer.finish();
        committing.join().expect("the unit").expect("an answer")
    });
    let Commit::Stale(Stale::Record { kind, found, .. }) = &answer else {
        panic!("stale: {answer:?}");
    };
    assert_eq!(
        (*kind, *found),
        (RecordKind::Order, Found::Present { revision: 1 }),
        "the first change the unit lists answers"
    );
}

/// The tables the session that waits on a lock has written to or waits to
/// write to, and whether it holds each.
const TABLES_OF_THE_WAITER: &str = "SELECT c.relname, l.granted FROM pg_locks l \
     JOIN pg_class c ON c.oid = l.relation \
     WHERE l.locktype = 'relation' AND l.mode = 'RowExclusiveLock' AND c.relkind = 'r' \
     AND l.pid IN (SELECT pid FROM pg_stat_activity \
     WHERE datname = current_database() AND wait_event_type = 'Lock') \
     ORDER BY c.relname";

/// A unit writes its tables in the order a copy takes them, though it
/// lists its changes the other way round: with the orders' table held as
/// a copy that begins holds it, the unit waits for that table having
/// written its intent, and never the other way about. A copy holds every
/// table before the one it waits for, so a unit that wrote a later table
/// first and then waited for an earlier one would wait for the copy while
/// the copy waited for it.
#[test]
fn a_unit_writes_its_tables_in_the_order_a_copy_takes_them() {
    let Some(raced) = raced() else {
        return;
    };
    let unit = backwards(&raced.change);
    assert_eq!(unit.changes[0].record.kind(), RecordKind::Order);

    let holding_the_later_table = Held::start(
        &raced.scratch,
        "LOCK TABLE execution_orders IN SHARE MODE".to_string(),
        String::new(),
    );
    let answer = thread::scope(|scope| {
        let committing = scope.spawn(|| raced.store.commit(&unit));
        wait_for_a_lock(&raced.store);
        assert!(!committing.is_finished(), "its order waits for the table");
        let text = |value: &str| Some(value.to_string());
        assert_eq!(
            rows(&raced.store, TABLES_OF_THE_WAITER),
            [
                [text("execution_intents"), text("t")],
                [text("execution_orders"), text("f")]
            ],
            "the intent is written first, though the unit lists it last"
        );
        holding_the_later_table.finish();
        committing.join().expect("the unit").expect("an answer")
    });
    assert_eq!(answer, Commit::Applied);
}
