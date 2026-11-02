//! The store record's steps: its identity, the migration log, the
//! environment it is bound to for life, its legacy runs and their
//! adoption; and the check that a store keeps its ID.

use quantforge::{RunJournalStore, StoreControl};

use super::cases::{
    Cases, LegacyRunText, MigrationText, StoreRecordText, environment, execution_mode,
};
use super::{Coded, Fixture, Step, expect_refused};

/// The checks coded here: id and statement.
pub const CHECKS: &[(&str, &str)] = &[(
    "control.store_id_kept",
    "a store's ID is 32 lowercase hex digits and stays the same when the store is opened again",
)];

pub fn check<S: RunJournalStore + StoreControl>(id: &str) -> Coded<S> {
    match id {
        "control.store_id_kept" => store_id_kept,
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
