//! The store record's steps: its identity, the migration log, the
//! environment it is bound to for life, its activation and the log of
//! it, its legacy runs and their adoption; and the checks that a store
//! keeps its ID and that of several activations at once one is made.

use std::sync::Barrier;
use std::time::{Duration, Instant};

use quantforge::model::Environment;
use quantforge::{RunJournalStore, StorageError, StorageErrorKind, StoreActivation, StoreControl};

use super::cases::{
    Cases, ControlEventText, LegacyRunText, MigrationText, StoreRecordText, environment,
    execution_mode,
};
use super::{Coded, Fixture, Step, expect_refused};

/// The checks coded here: id and statement.
pub const CHECKS: &[(&str, &str)] = &[
    (
        "control.store_id_kept",
        "a store's ID is 32 lowercase hex digits and stays the same when the store is opened again",
    ),
    (
        "control.activations_racing",
        "of eight activations of one store at once, one is made and seven find it done: the store is active at epoch 1, and its log holds one row",
    ),
];

pub fn check<S: RunJournalStore + StoreControl>(id: &str) -> Coded<S> {
    match id {
        "control.store_id_kept" => store_id_kept,
        "control.activations_racing" => activations_racing,
        other => panic!("no control check {other}"),
    }
}

/// Runs a store-record step; `false` for a step of another port.
pub fn step<S: StoreControl + ?Sized>(cases: &Cases, store: &S, step: &Step) -> bool {
    match step {
        Step::StoreRecord { expect } => {
            let found = store.store_identity().expect("the store's record");
            let found = StoreRecordText {
                schema_version: found.schema_version,
                environment: found.environment.map(|bound| bound.as_str().to_string()),
                activation: found.activation.as_str().to_string(),
                activation_epoch: found.activation_epoch.0,
            };
            assert_eq!(&found, expect, "the store's record");
        }
        Step::Migrations { expect } => {
            let log = store.migrations().expect("the migration log");
            let found: Vec<MigrationText> = log
                .iter()
                .map(|step| MigrationText {
                    to_version: step.to_version,
                    from_version: step.from_version,
                })
                .collect();
            assert_eq!(&found, expect, "the migration log");
            // A new store's steps were all run by this build.
            for step in &log {
                assert_eq!(step.quantforge_version, env!("CARGO_PKG_VERSION"));
                assert!(step.applied_at_ms > 0, "{step:?}");
            }
        }
        Step::BindEnvironment {
            environment: bound,
            refused,
        } => {
            let outcome = store.bind_environment(environment(bound));
            match refused {
                None => outcome.expect("bind"),
                Some(refused) => expect_refused(outcome, refused, &format!("bind {bound}")),
            }
        }
        Step::Activate { at_ms, refused } => {
            let outcome = store.activate(*at_ms);
            match refused {
                None => drop(outcome.expect("activate")),
                Some(refused) => expect_refused(outcome, refused, "activate"),
            }
        }
        Step::Deactivate { at_ms, refused } => {
            let outcome = store.deactivate(*at_ms);
            match refused {
                None => drop(outcome.expect("deactivate")),
                Some(refused) => expect_refused(outcome, refused, "deactivate"),
            }
        }
        Step::ControlEvents { expect } => {
            let found: Vec<ControlEventText> = store
                .control_events()
                .expect("the activation log")
                .iter()
                .map(|event| ControlEventText {
                    from: event.from.as_str().to_string(),
                    to: event.to.as_str().to_string(),
                    epoch: event.epoch.0,
                    at_ms: event.at_ms,
                })
                .collect();
            assert_eq!(&found, expect, "the activation log");
        }
        Step::LegacyRuns { expect } => {
            let found = store.legacy_runs().expect("legacy runs");
            assert_eq!(found.len(), expect.len(), "legacy runs: {found:?}");
            for (run, text) in found.iter().zip(expect) {
                let LegacyRunText {
                    run_id,
                    market,
                    status,
                    execution_mode: mode,
                    started_at_ms,
                } = text;
                assert_eq!(&run.run_id, run_id);
                assert_eq!(run.market, cases.market(market), "{run_id}");
                assert_eq!(run.status.as_str(), status, "{run_id}");
                assert_eq!(
                    run.execution_mode,
                    mode.as_deref().map(execution_mode),
                    "{run_id}"
                );
                assert_eq!(&run.started_at_ms, started_at_ms, "{run_id}");
            }
        }
        Step::AdoptRun {
            run_id,
            identity,
            execution_mode: mode,
            refused,
        } => {
            let outcome = store.adopt_run(
                run_id,
                &identity.identity(),
                mode.as_deref().map(execution_mode),
            );
            match refused {
                None => outcome.expect("adopt"),
                Some(refused) => expect_refused(outcome, refused, &format!("adopt {run_id}")),
            }
        }
        _ => return false,
    }
    true
}

/// A store's ID is 32 lowercase hex digits, and a store opened again on the
/// same database has the same one.
fn store_id_kept<S: RunJournalStore + StoreControl>(fixture: &Fixture<S>) {
    let first = fixture.store.store_identity().expect("the record");
    let id = first.store_id.as_str();
    assert!(
        id.len() == 32 && id.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')),
        "{id}"
    );
    let reopened = (fixture.reopen)();
    let again = reopened.store_identity().expect("the record");
    assert_eq!(again.store_id, first.store_id);
}

/// A call made again while another writer holds what it needs: contention
/// means nothing changed.
fn past_contention<T>(
    mut call: impl FnMut() -> Result<T, StorageError>,
) -> Result<T, StorageError> {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        match call() {
            Err(busy)
                if busy.kind() == StorageErrorKind::Contention && Instant::now() < deadline => {}
            answer => return answer,
        }
    }
}

/// Eight processes activate one store at once. One activation is made:
/// the others find the store active at the epoch it gave, and say so.
fn activations_racing<S: RunJournalStore + StoreControl>(fixture: &Fixture<S>) {
    const OPERATORS: usize = 8;
    let at_ms = 1_792_454_400_000;
    fixture
        .store
        .bind_environment(Environment::Demo)
        .expect("bound");
    let operators: Vec<S> = (0..OPERATORS).map(|_| (fixture.reopen)()).collect();
    let barrier = Barrier::new(OPERATORS);
    let answers: Vec<Result<_, StorageError>> = std::thread::scope(|scope| {
        let asking: Vec<_> = operators
            .iter()
            .map(|operator| {
                let barrier = &barrier;
                scope.spawn(move || {
                    barrier.wait();
                    past_contention(|| operator.activate(at_ms))
                })
            })
            .collect();
        asking
            .into_iter()
            .map(|operator| operator.join().expect("an operator"))
            .collect()
    });
    let made: Vec<_> = answers
        .iter()
        .filter_map(|answer| answer.as_ref().ok())
        .collect();
    let [identity] = made.as_slice() else {
        panic!("one activation is made: {answers:?}");
    };
    assert_eq!(
        (identity.activation, identity.activation_epoch.0),
        (StoreActivation::Active, 1)
    );
    for refused in answers.iter().filter_map(|answer| answer.as_ref().err()) {
        assert!(
            matches!(refused, StorageError::InvalidArgument(_))
                && refused
                    .to_string()
                    .contains("is already active, at epoch 1"),
            "{refused}"
        );
    }
    let now = fixture.store.store_identity().expect("the record");
    assert_eq!(
        (now.activation, now.activation_epoch.0),
        (StoreActivation::Active, 1)
    );
    let log = fixture.store.control_events().expect("the activation log");
    assert_eq!(log.len(), 1, "{log:?}");
    assert_eq!((log[0].epoch.0, log[0].at_ms), (1, at_ms));
}
