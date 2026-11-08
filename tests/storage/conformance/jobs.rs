//! The job store's steps, and the check coded here: stores claiming at once
//! from many queued jobs each claim jobs of their own, so every job is
//! claimed once, none twice and none lost.

use std::collections::BTreeSet;
use std::str::FromStr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use quantforge::{BundleHash, JobId, JobStore, NewJob, StorageError, StorageErrorKind};

use super::cases::JobText;
use super::{Coded, Fixture, Step, expect_refused};

/// The checks coded here: id and statement.
pub const CHECKS: &[(&str, &str)] = &[(
    "jobs.claims_racing",
    "stores claiming at once from many queued jobs claim each exactly once, none twice and none lost",
)];

pub fn check<S: JobStore + Send>(id: &str) -> Coded<S> {
    match id {
        "jobs.claims_racing" => claims_racing,
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
