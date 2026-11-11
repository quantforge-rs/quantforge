//! The job store's steps, and the checks coded here: stores claiming at
//! once from many queued jobs each claim jobs of their own, so every job is
//! claimed once, none twice and none lost; and an end whose results cannot
//! be stored ends nothing.

use std::collections::BTreeSet;
use std::str::FromStr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use quantforge::{
    BundleHash, JobId, JobOutcome, JobPhase, JobStore, NewJob, ResultHash, StorageError,
    StorageErrorKind, StoredResult,
};

use super::cases::JobText;
use super::{Coded, Fixture, Step, expect_refused};

/// The checks coded here: id and statement.
pub const CHECKS: &[(&str, &str)] = &[
    (
        "jobs.claims_racing",
        "stores claiming at once from many queued jobs claim each exactly once, none twice and none lost",
    ),
    (
        "jobs.failed_end_changes_nothing",
        "an end whose results cannot all be stored changes nothing: the job still runs and holds no result, and ends once they can",
    ),
];

pub fn check<S: JobStore + Send>(id: &str) -> Coded<S> {
    match id {
        "jobs.claims_racing" => claims_racing,
        "jobs.failed_end_changes_nothing" => failed_end_changes_nothing,
        other => panic!("no job check {other}"),
    }
}

fn job_id(text: &str) -> JobId {
    JobId::new(text).expect("a job id")
}

fn hash(text: &str) -> BundleHash {
    BundleHash::from_str(text).expect("a bundle hash")
}

/// Runs a job store step; `false` for a step of another port.
pub fn step<S: JobStore + ?Sized>(store: &S, step: &Step) -> bool {
    match step {
        Step::StoreBundle {
            hash: name,
            data,
            expect,
        } => {
            let bytes = hex::decode(data).expect("hex");
            let stored = store.store_bundle(&hash(name), &bytes, 0).expect("store");
            assert_eq!(&stored, expect, "stored {name} anew");
        }
        Step::LoadBundle { hash: name, expect } => {
            let loaded = store
                .load_bundle(&hash(name))
                .expect("load")
                .map(hex::encode);
            assert_eq!(&loaded, expect, "bundle {name}");
        }
        Step::LoadResult { hash: name, expect } => {
            let name: ResultHash = name.parse().expect("a result hash");
            let loaded = store.load_result(&name).expect("load").map(hex::encode);
            assert_eq!(&loaded, expect, "result {name}");
        }
        Step::SubmitJob { job, refused } => {
            let outcome = store.submit_job(&job.new_job());
            match refused {
                Some(refused) => expect_refused(outcome, refused, "submit"),
                None => outcome.expect("submit"),
            }
        }
        Step::Job { job_id: id, expect } => {
            let found = store.job(&job_id(id)).expect("job");
            assert_eq!(&found.as_ref().map(JobText::of), expect, "job {id}");
        }
        Step::Jobs { limit, expect } => {
            let listed: Vec<JobText> = store
                .jobs(*limit)
                .expect("jobs")
                .iter()
                .map(JobText::of)
                .collect();
            assert_eq!(&listed, expect, "jobs {limit}");
        }
        Step::ClaimJob { at, expect } => {
            let claimed = store.claim_job(*at).expect("claim");
            assert_eq!(&claimed.as_ref().map(JobText::of), expect, "claim at {at}");
        }
        Step::FinishJob {
            job_id: id,
            outcome,
            at,
            expect,
        } => {
            let finished = store
                .finish_job(&job_id(id), &outcome.outcome(), *at)
                .expect("finish");
            assert_eq!(&finished, expect, "finish {id}");
        }
        Step::CancelJob {
            job_id: id,
            at,
            expect,
        } => {
            let cancelled = store.cancel_job(&job_id(id), *at).expect("cancel");
            assert_eq!(&cancelled.as_ref().map(JobText::of), expect, "cancel {id}");
        }
        Step::FailRunningJobs {
            failure,
            at,
            expect,
        } => {
            let failed = store.fail_running_jobs(failure, *at).expect("fail running");
            assert_eq!(&failed, expect, "failed on restart");
        }
        _ => return false,
    }
    true
}

/// A call made again while another writer holds what it needs.
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

fn claims_racing<S: JobStore + Send>(fixture: &Fixture<S>) {
    const JOBS: u64 = 40;
    const CLAIMERS: usize = 4;
    let bundle = hash(&format!("sha256:{}", "ab".repeat(32)));
    fixture
        .store
        .store_bundle(&bundle, b"tar", 0)
        .expect("a bundle");
    for n in 1..=JOBS {
        fixture
            .store
            .submit_job(&NewJob {
                job_id: job_id(&format!("{n:032x}")),
                bundle,
                request_json: "{}".to_string(),
                submitted_at_ms: i64::try_from(n).expect("n"),
            })
            .expect("a job");
    }
    let claimers: Vec<S> = (0..CLAIMERS).map(|_| (fixture.reopen)()).collect();
    let claimed = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for store in claimers {
            let claimed = &claimed;
            scope.spawn(move || {
                while let Some(job) = past_contention(|| store.claim_job(1)).expect("a claim") {
                    claimed.lock().expect("the claims").push(job.job_id);
                }
            });
        }
    });
    let claimed = claimed.into_inner().expect("the claims");
    let distinct: BTreeSet<&JobId> = claimed.iter().collect();
    assert_eq!(claimed.len(), distinct.len(), "a job claimed twice");
    assert_eq!(distinct.len() as u64, JOBS, "a job never claimed");
    assert_eq!(fixture.store.claim_job(2).expect("a claim"), None);
}

/// With the results table refusing every new row, a succeeded end fails
/// whole: the job is still running, its record shows no results, and the
/// first result, which the table took before it refused, is gone with the
/// rest. Once the table takes rows again, the same end goes through.
fn failed_end_changes_nothing<S: JobStore + Send>(fixture: &Fixture<S>) {
    let (store, raw) = (&fixture.store, &fixture.raw);
    let bundle = hash(&format!("sha256:{}", "cd".repeat(32)));
    store.store_bundle(&bundle, b"tar", 0).expect("a bundle");
    let id = job_id(&format!("{:032x}", 1));
    store
        .submit_job(&NewJob {
            job_id: id.clone(),
            bundle,
            request_json: "{}".to_string(),
            submitted_at_ms: 1,
        })
        .expect("a job");
    store.claim_job(2).expect("a claim").expect("the job");
    let result = |fill: u8| StoredResult {
        hash: ResultHash::from_digest([fill; 32]),
        size_bytes: 3,
        data: vec![fill; 3],
    };
    let (first, second) = (result(0x11), result(0x22));
    let outcome = JobOutcome::Succeeded {
        results_json: "[]".to_string(),
        results: vec![first.clone(), second.clone()],
    };
    let count = || raw.select("SELECT CAST(COUNT(*) AS TEXT) FROM job_results");
    let none = [[Some("0".to_string())]];

    let [refuse, allow] = raw.write_refusal("job_results");
    raw.execute(&refuse);
    store
        .finish_job(&id, &outcome, 3)
        .expect_err("the results table refuses the rows");
    let record = store.job(&id).expect("a read").expect("the job");
    assert_eq!(record.phase, JobPhase::Running, "the job still runs");
    assert_eq!(
        (record.finished_at_ms, record.results_json),
        (None, None),
        "nothing of the end is recorded"
    );
    assert_eq!(count(), none, "no result without the end that names it");
    assert_eq!(store.load_result(&first.hash).expect("a read"), None);

    raw.execute(&allow);
    assert!(store.finish_job(&id, &outcome, 4).expect("the end"));
    let record = store.job(&id).expect("a read").expect("the job");
    assert_eq!(
        (record.phase, record.finished_at_ms),
        (JobPhase::Succeeded, Some(4))
    );
    for kept in [first, second] {
        assert_eq!(
            store.load_result(&kept.hash).expect("a read"),
            Some(kept.data)
        );
    }
}
