//! The shared storage conformance suite, `docs/storage-conformance.md`: the
//! scenarios of `tests/fixtures/storage_conformance_cases.json`, each step
//! with the answer a conforming store gives, plus a few checks coded here
//! that read the identity and segment cases. Expected answers come from the
//! fixtures, never from another backend.
//!
//! A backend joins by implementing [`Backend`] and [`Raw`], then calling
//! the group of each port it implements: [`candles`], [`journal`],
//! [`segments`], [`control`], [`jobs`], and, with all of them,
//! [`durability`]. Every
//! scenario and check runs on a fresh, empty store, and a group fails
//! listing every one that failed.

pub mod candles;
pub mod cases;
pub mod control;
pub mod durability;
pub mod jobs;
pub mod journal;
pub mod segments;

use std::any::Any;
use std::panic::{self, AssertUnwindSafe};
use std::time::Duration;

use quantforge::{
    CandleStore, JobStore, RunJournalStore, SegmentStore, StorageError, StoreControl,
};

pub use cases::{Cases, Refusal, Scenario, Step};

/// A storage backend under test: how to open a fresh, empty, initialized
/// store of it, isolated from every other store the suite opens.
pub trait Backend {
    /// The store, which implements the ports of the groups it runs.
    type Store;
    /// Named in failure reports.
    const NAME: &'static str;
    fn fresh() -> Fixture<Self::Store>;
}

/// One fresh store, a raw view of its database, and a way to open the same
/// database again as a restarted process would.
pub struct Fixture<S> {
    pub store: S,
    pub raw: Box<dyn Raw>,
    pub reopen: Box<dyn Fn() -> S>,
    /// Whatever keeps the database alive, dropped last: a temporary
    /// directory, an isolated database.
    pub guard: Box<dyn Any>,
}

/// The backend's own view of its tables, for what the ports do not show.
/// The scenarios' SQL is portable: plain `SELECT`, `INSERT` and `UPDATE`
/// over the tables of `docs/storage-schema.md`, every selected column text
/// or `CAST(… AS TEXT)`.
pub trait Raw {
    /// Every row the query returns, each value as its text, NULL as `None`.
    fn select(&self, sql: &str) -> Vec<Vec<Option<String>>>;
    /// Runs a statement behind the store's back, as an older or foreign
    /// writer might.
    fn execute(&self, sql: &str);
    /// The backend's names for the types of the non-null values in a
    /// column, distinct and sorted: SQLite's `typeof`, PostgreSQL's
    /// `pg_typeof`.
    fn value_types(&self, table: &str, column: &str) -> Vec<String>;
    /// `bytes` as a literal of the backend's SQL.
    fn blob(&self, bytes: &[u8]) -> String;
    /// The statements that make `table` refuse every new row, and that let
    /// it take them again.
    fn write_refusal(&self, table: &str) -> [String; 2];
    /// Holds the lock a writer of `table` needs, as another writer would,
    /// until the guard drops. Readers pass it.
    fn hold_writes(&self, table: &str) -> Box<dyn Any>;
    /// Ends every connection a store of this database holds. A store that
    /// holds none between calls has nothing to lose.
    fn end_connections(&self);
}

/// A store whose calls can be bounded.
pub trait Bounded: Sized {
    fn within(&self, budget: Duration) -> Self;
}

/// A check coded here rather than written in the fixture.
pub type Coded<S> = fn(&Fixture<S>);

/// The candle store's scenarios and checks.
pub fn candles<B: Backend>()
where
    B::Store: CandleStore,
{
    let cases = cases::load();
    run_group::<B, _>(
        "candles",
        &cases,
        &cases.candles,
        candles::CHECKS,
        candles::check,
        |cases, raw, reopen, store, step| {
            shared_step(raw, reopen, store, step) || candles::step(cases, store, step)
        },
    );
}

/// The run journal's scenarios and checks.
pub fn journal<B: Backend>()
where
    B::Store: RunJournalStore,
{
    let cases = cases::load();
    run_group::<B, _>(
        "journal",
        &cases,
        &cases.journal,
        journal::CHECKS,
        journal::check,
        |cases, raw, reopen, store, step| {
            shared_step(raw, reopen, store, step) || journal::step(cases, store, step)
        },
    );
}

/// The store record's scenarios and checks; they write runs too.
pub fn control<B: Backend>()
where
    B::Store: RunJournalStore + StoreControl,
{
    let cases = cases::load();
    run_group::<B, _>(
        "control",
        &cases,
        &cases.control,
        control::CHECKS,
        control::check,
        |cases, raw, reopen, store, step| {
            shared_step(raw, reopen, store, step)
                || journal::step(cases, store, step)
                || control::step(cases, store, step)
        },
    );
}

/// The segment store's scenarios and checks; they write candles too.
pub fn segments<B: Backend>()
where
    B::Store: CandleStore + SegmentStore + Send,
{
    let cases = cases::load();
    run_group::<B, _>(
        "segments",
        &cases,
        &cases.segments,
        segments::CHECKS,
        segments::check,
        |cases, raw, reopen, store, step| {
            shared_step(raw, reopen, store, step)
                || candles::step(cases, store, step)
                || segments::step(cases, store, step)
        },
    );
}

/// The job store's scenarios and checks.
pub fn jobs<B: Backend>()
where
    B::Store: JobStore + Send,
{
    let cases = cases::load();
    run_group::<B, _>(
        "jobs",
        &cases,
        &cases.jobs,
        jobs::CHECKS,
        jobs::check,
        |_cases, raw, reopen, store, step| {
            shared_step(raw, reopen, store, step) || jobs::step(store, step)
        },
    );
}

/// What a store's acknowledgement means, and how long it may take; its
/// checks are all coded.
pub fn durability<B: Backend>()
where
    B::Store: CandleStore + RunJournalStore + StoreControl + Bounded,
{
    let cases = cases::load();
    run_group::<B, _>(
        "durability",
        &cases,
        &[],
        durability::CHECKS,
        durability::check,
        |_, _, _, _, _| false,
    );
}

/// Runs every scenario of a group, then every coded check, each on a fresh
/// store, and fails once at the end listing all that failed.
fn run_group<B, F>(
    group: &str,
    cases: &Cases,
    scenarios: &[Scenario],
    checks: &[(&str, &str)],
    check: fn(&str) -> Coded<B::Store>,
    step: F,
) where
    B: Backend,
    F: Fn(&Cases, &dyn Raw, &dyn Fn() -> B::Store, &mut B::Store, &Step) -> bool,
{
    let mut failures = Vec::new();
    for scenario in scenarios {
        let outcome = panic::catch_unwind(AssertUnwindSafe(|| {
            let Fixture {
                mut store,
                raw,
                reopen,
                guard,
            } = B::fresh();
            for (index, current) in scenario.steps.iter().enumerate() {
                let handled = step(cases, &*raw, &*reopen, &mut store, current);
                assert!(handled, "step {index} is no {group} step: {current:?}");
            }
            // The store goes before what keeps its database.
            drop(store);
            drop(guard);
        }));
        if let Err(payload) = outcome {
            failures.push(format!("{}: {}", scenario.id, message(&payload)));
        }
    }
    for (id, _) in checks {
        let run = check(id);
        let outcome = panic::catch_unwind(AssertUnwindSafe(|| run(&B::fresh())));
        if let Err(payload) = outcome {
            failures.push(format!("{id}: {}", message(&payload)));
        }
    }
    let total = scenarios.len() + checks.len();
    assert!(
        failures.is_empty(),
        "{} of {total} {group} checks failed on {}:\n{}",
        failures.len(),
        B::NAME,
        failures.join("\n")
    );
}

fn message(payload: &Box<dyn Any + Send>) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|text| text.to_string()))
        .unwrap_or_else(|| "a panic without a message".to_string())
}

/// The steps every group shares: the raw view, and opening the database
/// again, after which the scenario goes on with the new store.
fn shared_step<S>(raw: &dyn Raw, reopen: &dyn Fn() -> S, store: &mut S, step: &Step) -> bool {
    match step {
        Step::RawSelect { sql, expect } => {
            assert_eq!(&raw.select(sql), expect, "{sql}");
        }
        Step::RawContains { sql, contains } => {
            let rows = raw.select(sql);
            let [row] = rows.as_slice() else {
                panic!("one row expected from {sql}: {rows:?}");
            };
            let [Some(value)] = row.as_slice() else {
                panic!("one value expected from {sql}: {row:?}");
            };
            for part in contains {
                assert!(value.contains(part.as_str()), "{part} is not in {value}");
            }
        }
        Step::RawExecute { sql } => raw.execute(sql),
        Step::ValueTypes {
            table,
            column,
            expect,
        } => {
            assert_eq!(&raw.value_types(table, column), expect, "{table}.{column}");
        }
        Step::Reopen => *store = reopen(),
        _ => return false,
    }
    true
}

/// Holds an answer to what the step said: refused with that kind and text,
/// or answered.
pub fn expect_refused<T: std::fmt::Debug>(
    outcome: Result<T, StorageError>,
    refused: &Refusal,
    what: &str,
) {
    let error = match outcome {
        Ok(answer) => panic!("{what}: refused expected, got {answer:?}"),
        Err(error) => error,
    };
    match refused.kind.as_str() {
        "invalid_argument" => assert!(
            matches!(error, StorageError::InvalidArgument(_)),
            "{what}: {error:?}"
        ),
        other => panic!("no refusal kind {other}"),
    }
    let shown = error.to_string();
    assert!(
        shown.contains(&refused.contains),
        "{what}: {shown:?} does not say {:?}",
        refused.contains
    );
}
