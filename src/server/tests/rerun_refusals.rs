//! The reruns refused, over the store of `backtesting`, every difference
//! named: when the store no longer holds what the job ran on as it was
//! recorded, when the server's runtime no longer admits the bundle or runs
//! no job over the job's range, and when the job never succeeded or
//! recorded nothing.

use sha2::Digest;

use super::backtesting::{
    Backtesting, END, Later, days, other_code, ran, running, stored, succeeded_without_provenance,
};
use super::jobs::{bundle, jobs_of, runtime, serve_jobs};
use super::reruns::{provenance, reran, rerun};
use super::*;
use crate::model::JobId;
use crate::protocol::bundle::Runtime;
use crate::protocol::jobs::{Provenance, RerunVerdict};
use crate::storage::segments;

/// The segments a job's provenance names, as stored: each name, payload
/// length and stored bytes.
fn named(bed: &Bed, provenance: &Provenance) -> Vec<(String, i64, Vec<u8>)> {
    let connection = bed.connection();
    provenance
        .inputs
        .data
        .segments
        .iter()
        .map(|segment| {
            let hash = segment.hash.to_string();
            connection
                .query_row(
                    "SELECT hash, payload_len, data FROM segments WHERE hash = ?1",
                    [&hash],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .expect("a stored segment")
        })
        .collect()
}

#[tokio::test]
async fn a_rerun_is_refused_naming_every_segment_or_bundle_not_held_as_recorded() {
    let bed = Bed::new();
    stored(&bed);
    let (address, _jobs) = running(&bed, Backtesting::new(Later::Same)).await;
    let (job, _) = ran(address).await;
    let recorded = provenance(address, &job).await;
    let segments_held = named(&bed, &recorded);
    let (first, last) = (&segments_held[0], &segments_held[2]);
    let connection = bed.connection();
    let other =
        segments::compress(&segments::encode(&days()[..2]).expect("a payload")).expect("packed");
    let other_name = segments::hash(&segments::encode(&days()[..2]).expect("a payload"));
    connection
        .execute(
            "UPDATE segments SET data = ?1 WHERE hash = ?2",
            rusqlite::params![other, first.0],
        )
        .expect("an altered segment");
    connection
        .execute("DELETE FROM segments WHERE hash = ?1", [&last.0])
        .expect("a lost segment");
    let refused = refusal(rerun(address, &job).await).await;
    assert_eq!(
        refused,
        (
            409,
            "conflict".to_string(),
            format!(
                "job {job} cannot be rerun: the server's copy of segment {} is altered: the \
                 payload hashes to {other_name}, not to {}; the server no longer holds segment \
                 {}, which held the bars from {} to {}",
                first.0,
                first.0,
                last.0,
                ms_to_rfc3339(T + 2 * DAY),
                ms_to_rfc3339(END)
            )
        )
    );

    // Each put back, the bundle altered instead.
    connection
        .execute(
            "UPDATE segments SET data = ?1 WHERE hash = ?2",
            rusqlite::params![first.2, first.0],
        )
        .expect("the segment put back");
    connection
        .execute(
            "INSERT INTO segments(hash, format, payload_len, data) VALUES (?1, 1, ?2, ?3)",
            rusqlite::params![last.0, last.1, last.2],
        )
        .expect("the segment stored again");
    assert_eq!(reran(address, &job).await.verdict, RerunVerdict::Verified);
    let code_hash = bundle().code_hash();
    let tar = other_code();
    let altered_name = crate::protocol::ContentHash::sha256(sha2::Sha256::digest(&tar).into());
    connection
        .execute(
            "UPDATE bundles SET data = ?1 WHERE code_hash = ?2",
            rusqlite::params![tar, code_hash.to_string()],
        )
        .expect("an altered bundle");
    assert_eq!(
        refusal(rerun(address, &job).await).await,
        (
            409,
            "conflict".to_string(),
            format!(
                "job {job} cannot be rerun: the server's copy of bundle {code_hash} is altered: \
                 it hashes to {altered_name}"
            )
        )
    );
    connection
        .execute("DELETE FROM bundles", [])
        .expect("the bundle lost");
    assert_eq!(
        refusal(rerun(address, &job).await).await.2,
        format!("job {job} cannot be rerun: the server no longer holds bundle {code_hash}")
    );
}

#[tokio::test]
async fn a_rerun_is_refused_for_a_job_that_never_succeeded_or_recorded_nothing() {
    let bed = Bed::new();
    stored(&bed);
    let legacy = succeeded_without_provenance(&bed);
    let (address, jobs) = running(&bed, Backtesting::new(Later::Same)).await;
    let (job, _) = ran(address).await;
    jobs.stop().await;
    let refused = |job: String| async move { refusal(rerun(address, &job).await).await };
    let unknown = JobId::generate().to_string();
    assert_eq!(refused(unknown.clone()).await.0, 404);
    assert_eq!(
        refused(legacy.to_string()).await,
        (
            409,
            "conflict".to_string(),
            format!(
                "job {legacy} has no provenance: it ended before the server read its inputs, or \
                 a build that recorded none ran it"
            )
        )
    );
    // A job that failed after it recorded its provenance has no result to
    // rerun against.
    bed.connection()
        .execute(
            "UPDATE jobs SET state = 'failed', results_json = NULL, failure_json = '{\"code\":\"invalid_request\",\"message\":\"x\"}' WHERE job_id = ?1",
            [&job],
        )
        .expect("a failed job");
    assert_eq!(
        refused(job.clone()).await,
        (
            409,
            "conflict".to_string(),
            format!("job {job} failed; only a job that has succeeded is rerun, against its result")
        )
    );
    let wrong = client()
        .get(format!("http://{address}/v1/backtests/{job}/rerun"))
        .send()
        .await
        .expect("a response");
    assert_eq!(refusal(wrong).await.0, 405);
}

/// A worker that runs like `Backtesting` but offers `runtime`.
fn offering(runtime: Runtime) -> Backtesting {
    Backtesting {
        runtime,
        ..Backtesting::new(Later::Same)
    }
}

#[tokio::test]
async fn a_rerun_is_refused_by_a_server_whose_runtime_no_longer_admits_the_bundle() {
    let bed = Bed::new();
    stored(&bed);
    let (address, jobs) = running(&bed, Backtesting::new(Later::Same)).await;
    let (job, _) = ran(address).await;
    jobs.stop().await;

    // Another server on the same store, its workers on another Python.
    let other = serve_jobs(
        &bed,
        jobs_of(&bed, offering(runtime("3.11")), DEFAULT_MAX_SPAN_BARS),
    )
    .await;
    let (status, code, message) = refusal(rerun(other, &job).await).await;
    assert_eq!((status, code.as_str()), (409, "conflict"));
    assert!(
        message.starts_with(&format!("job {job} cannot be rerun on this server: ")),
        "{message}"
    );
    assert!(
        message.contains("but this server runs Python 3.11"),
        "{message}"
    );
    assert!(
        message.ends_with("; the job ran on engine 0.3.0, Python 3.12"),
        "{message}"
    );

    // And one that runs no bundles at all.
    let none = serve_jobs(&bed, jobs_of(&bed, NoRuntime, DEFAULT_MAX_SPAN_BARS)).await;
    assert_eq!(
        refusal(rerun(none, &job).await).await,
        (
            503,
            "unavailable".to_string(),
            "this server runs no Python bundles: it was started without --job-python".to_string()
        )
    );
}

#[tokio::test]
async fn a_rerun_whose_range_spans_more_bars_than_the_server_runs_is_refused_before_any_read() {
    let bed = Bed::new();
    stored(&bed);
    let (address, jobs) = running(&bed, Backtesting::new(Later::Same)).await;
    let (job, _) = ran(address).await;
    let recorded = provenance(address, &job).await;
    jobs.stop().await;
    let last = named(&bed, &recorded).swap_remove(2);
    let connection = bed.connection();
    connection
        .execute("DELETE FROM segments WHERE hash = ?1", [&last.0])
        .expect("a lost segment");

    // The job's range, two days and 600 minutes long, spans 3,481 bars of
    // 1m: a server that runs one bar fewer refuses it on that alone, its
    // last segment lost, having read nothing.
    let fewer = serve_jobs(&bed, jobs_of(&bed, Backtesting::new(Later::Same), 3480)).await;
    assert_eq!(
        refusal(rerun(fewer, &job).await).await,
        (
            409,
            "conflict".to_string(),
            format!(
                "job {job} cannot be rerun on this server: the range spans 3481 bars of 1m, more \
                 than the 3480 this server runs one job on"
            )
        )
    );

    // Put back, it is rerun by a server that runs exactly as many.
    connection
        .execute(
            "INSERT INTO segments(hash, format, payload_len, data) VALUES (?1, 1, ?2, ?3)",
            rusqlite::params![last.0, last.1, last.2],
        )
        .expect("the segment stored again");
    let exactly = serve_jobs(&bed, jobs_of(&bed, Backtesting::new(Later::Same), 3481)).await;
    assert_eq!(reran(exactly, &job).await.verdict, RerunVerdict::Verified);
}
