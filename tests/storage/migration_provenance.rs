//! Schema 10 to schema 11: a store the build before the reproducibility
//! ledger wrote (`tests/fixtures/sqlite/schema-10.sql`) gains the table of
//! each job's provenance, and keeps every row it held: its jobs, their
//! results and logs, and its execution store's activation log, lease and
//! operation among them. No job that build ran has a provenance, and a job
//! the store holds can be given one once.

use quantforge::model::provenance::{InputsIdentity, NewProvenance};
use quantforge::storage::sqlite::SCHEMA_VERSION;
use quantforge::{CandleStore, JobId, JobPhase, JobStore, StoreActivation, StoreControl};

use crate::migration::{everything, load, shape, tables, version};

#[test]
fn a_schema_10_store_gains_the_provenance_table_and_keeps_every_row() {
    let fresh_dir = tempfile::tempdir().expect("tempdir");
    let fresh = quantforge::SqliteStore::new(fresh_dir.path().join("market.sqlite"));
    CandleStore::init(&fresh).expect("fresh");
    let (_tempdir, store) = load(10);
    assert_eq!(version(&store), 10);
    let mut kept = tables(&store);
    assert!(!kept.iter().any(|held| held == "job_provenance"));
    kept.push("sqlite_sequence".to_string());
    let before: Vec<_> = kept.iter().map(|table| everything(&store, table)).collect();
    assert_eq!(everything(&store, "store_control_events").len(), 2);
    assert_eq!(everything(&store, "position_leases").len(), 1);
    assert_eq!(everything(&store, "execution_operations").len(), 1);

    CandleStore::init(&store).expect("schema 11");

    assert_eq!(version(&store), SCHEMA_VERSION);
    assert_eq!(shape(&store), shape(&fresh), "not the fresh schema");
    for (table, rows_before) in kept.iter().zip(&before) {
        let after = everything(&store, table);
        match table.as_str() {
            "meta" => assert_eq!(
                after,
                [[
                    Some("schema_version".to_string()),
                    Some(SCHEMA_VERSION.to_string())
                ]]
            ),
            "schema_migrations" => {
                assert_eq!(after[..rows_before.len()], rows_before[..], "the log kept");
                assert_eq!(after.len(), rows_before.len() + 1, "one step logged");
            }
            _ => assert_eq!(&after, rows_before, "{table}"),
        }
    }
    let steps: Vec<(u32, u32)> = store
        .migrations()
        .expect("log")
        .iter()
        .map(|step| (step.from_version, step.to_version))
        .collect();
    assert_eq!(
        steps,
        [(4, 5), (5, 6), (6, 7), (7, 8), (8, 9), (9, 10), (10, 11)]
    );
    assert_eq!(
        everything(&store, "job_provenance"),
        Vec::<Vec<Option<String>>>::new()
    );

    // No job the earlier build ran has a provenance, however it ended.
    let jobs = store.jobs(100).expect("the jobs");
    assert_eq!(jobs.len(), 11);
    assert!(jobs.iter().any(|job| job.phase == JobPhase::Succeeded));
    for job in &jobs {
        assert_eq!(store.provenance(&job.job_id).expect("a read"), None);
    }

    // The store's record stays as the earlier build left it: deactivated
    // at the epoch it was activated at.
    let identity = store.store_identity().expect("identity");
    assert_eq!(
        (identity.activation, identity.activation_epoch.0),
        (StoreActivation::Deactivated, 1)
    );
    assert_eq!(store.control_events().expect("the log").len(), 2);

    // A job the store holds is given a provenance once, which names this
    // store and its schema.
    let job: &JobId = &jobs[0].job_id;
    let provenance = NewProvenance {
        identity: InputsIdentity::of(b"{}"),
        provenance_json: "{}".to_string(),
        segments: Vec::new(),
        packed: Vec::new(),
    };
    assert!(
        store
            .record_provenance(job, &provenance, 1_792_900_000_000)
            .expect("recorded")
    );
    assert!(
        !store
            .record_provenance(job, &provenance, 1_792_900_000_001)
            .expect("again")
    );
    let recorded = store.provenance(job).expect("a read").expect("the record");
    assert_eq!(
        (
            recorded.backend.as_str(),
            recorded.schema_version,
            recorded.recorded_at_ms
        ),
        ("sqlite", SCHEMA_VERSION, 1_792_900_000_000)
    );

    CandleStore::init(&store).expect("second init");
    assert_eq!(store.migrations().expect("log").len(), 7);
}
