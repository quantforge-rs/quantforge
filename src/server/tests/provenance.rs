//! A job's provenance in process, over the store of `backtesting`: two
//! packed days, one of them since corrected, and a third day no segment
//! holds. A job reads each from where it is, packs what no segment holds,
//! runs on exactly the bars its provenance names, and records it before its
//! worker runs; the route answers it, checked against its identity, and
//! refuses a job without one.

use super::backtesting::{
    Backtesting, END, Later, START, days, ended, found, get, other_code, ran, running, stored,
    succeeded_without_provenance,
};
use super::jobs::{bundle, data, jobs_of, queued, serve_jobs, submission, submit, upload};
use super::*;
use crate::model::JobId;
use crate::ports::JobStore;
use crate::protocol::ContentHash;
use crate::protocol::jobs::{Provenance, ProvenanceSegment};
use crate::storage::segments;

/// The bars of `candles` from `first` to `last`, packed as a segment is,
/// as a provenance lists them.
fn listed(candles: &[Candle], first: TimestampMs, last: TimestampMs) -> ProvenanceSegment {
    let held: Vec<Candle> = candles
        .iter()
        .filter(|candle| (first..=last).contains(&candle.open_time_ms))
        .cloned()
        .collect();
    let hash = segments::hash(&segments::encode(&held).expect("a payload"));
    ProvenanceSegment {
        hash: ContentHash::sha256(*hash.digest()),
        first_open_ms: first,
        last_open_ms: last,
        bar_count: u32::try_from(held.len()).expect("a count"),
    }
}

#[tokio::test]
async fn a_job_runs_on_exactly_the_bars_of_the_segments_its_provenance_names() {
    let bed = Bed::new();
    stored(&bed);
    let worker = Backtesting::new(Later::Same);
    let handed = Arc::clone(&worker.handed);
    let (address, _jobs) = running(&bed, worker).await;
    let (job, status) = ran(address).await;
    assert_eq!(status["state"]["kind"], "succeeded", "{status}");

    let response = get(address, &format!("/v1/backtests/{job}/provenance")).await;
    assert_eq!(response.status(), 200);
    assert_eq!(header(&response, CACHE_CONTROL), Some("no-cache"));
    let provenance: Provenance = response.json().await.expect("a provenance");
    // The first day from its segment, packed before the correction; the
    // corrected day and the part of the third the job reads, packed for it.
    let (original, found) = (days(), found());
    let expected = vec![
        listed(&original, T, T + DAY - MINUTE),
        listed(&found, T + DAY, T + 2 * DAY - MINUTE),
        listed(&found, T + 2 * DAY, END),
    ];
    let data = &provenance.inputs.data;
    assert_eq!(data.segments, expected);
    assert_eq!((data.start_open_ms, data.end_open_ms), (START, END));
    assert_eq!(data.bar_count, 1340 + 1440 + 701);
    assert_eq!(
        (
            data.exchange.as_str(),
            data.category.as_str(),
            data.symbol.as_str(),
            data.interval.as_str()
        ),
        (VENUE.as_str(), "spot", "BTCUSDT", "1m")
    );
    assert_eq!(
        provenance.identity,
        provenance.inputs.identity().expect("an identity")
    );
    assert_eq!(provenance.inputs.code_hash, bundle().code_hash());
    assert_eq!(
        serde_json::to_value(&provenance.inputs.runtime).expect("json"),
        serde_json::json!({"engine": "0.3.0", "python": "3.12", "deps": {}})
    );
    assert_eq!(provenance.inputs.proto_version, PROTO_VERSION);
    assert_eq!(provenance.strategy.entry, "strategy.py:Hold");
    assert!(provenance.strategy.params.is_empty());
    assert_eq!(
        (
            provenance.recorded.backend.as_str(),
            provenance.recorded.schema_version
        ),
        ("sqlite", crate::storage::sqlite::SCHEMA_VERSION)
    );
    assert_eq!(
        serde_json::to_value(&provenance.results).expect("json"),
        status["state"]["results"]
    );

    // The worker ran on those bars and no others, every decimal as stored.
    let ran_on: Vec<String> = handed.lock().expect("the runs")[0]
        .iter()
        .map(text)
        .collect();
    let in_range: Vec<String> = found
        .iter()
        .filter(|candle| (START..=END).contains(&candle.open_time_ms))
        .map(text)
        .collect();
    assert_eq!(ran_on, in_range);

    // Every segment is served under its name, and holds what it lists; a
    // later correction changes none of them.
    let market = market(Category::Spot);
    let corrected = [
        candle(T + 200 * MINUTE, "1.10", None),
        candle(END, "1.10", None),
    ];
    bed.store
        .upsert_candles(&market, &corrected)
        .expect("corrections");
    for (segment, from) in expected.iter().zip([&original, &found, &found]) {
        let response = get(address, &format!("/v1/segments/{}", segment.hash)).await;
        assert_eq!(response.status(), 200, "{}", segment.hash);
        let bytes = response.bytes().await.expect("a body");
        let name = SegmentHash::from_digest(*segment.hash.digest());
        let opened: Vec<String> = segments::open(&bytes, &name)
            .expect("a segment")
            .iter()
            .map(text)
            .collect();
        let held: Vec<String> = from
            .iter()
            .filter(|candle| {
                (segment.first_open_ms..=segment.last_open_ms).contains(&candle.open_time_ms)
            })
            .map(text)
            .collect();
        assert_eq!(opened, held, "{}", segment.hash);
    }
    let again: Provenance = get(address, &format!("/v1/backtests/{job}/provenance"))
        .await
        .json()
        .await
        .expect("a provenance");
    assert_eq!(again, provenance, "a provenance never changes");
}

#[tokio::test]
async fn the_route_refuses_a_job_without_a_provenance_and_one_altered_in_the_store() {
    let bed = Bed::new();
    stored(&bed);
    let legacy = succeeded_without_provenance(&bed);
    let (address, jobs) = running(&bed, Backtesting::new(Later::Same)).await;
    let (job, _) = ran(address).await;
    jobs.stop().await;
    // A job queued once the queue has stopped stays queued.
    let hash = upload(address, &bundle()).await;
    let waiting = queued(
        submit(
            address,
            &submission(&hash, data(Some(Category::Spot), START, END)),
        )
        .await,
    )
    .await;
    let waiting = waiting["job_id"].as_str().expect("a job id");

    let refused = |path: String| async move { refusal(get(address, &path).await).await };
    let unknown = JobId::generate();
    assert_eq!(
        refused(format!("/v1/backtests/{unknown}/provenance")).await,
        (
            404,
            "not_found".to_string(),
            format!("the server holds no job {unknown}")
        )
    );
    assert_eq!(
        refused(format!("/v1/backtests/{waiting}/provenance")).await,
        (
            409,
            "conflict".to_string(),
            format!("job {waiting} is still queued; its provenance is recorded as it starts")
        )
    );
    assert_eq!(
        refused(format!("/v1/backtests/{legacy}/provenance")).await,
        (
            409,
            "conflict".to_string(),
            format!(
                "job {legacy} has no provenance: it ended before the server read its inputs, or \
                 a build that recorded none ran it"
            )
        )
    );

    // A record that no longer matches its identity is never shown.
    let damaged = (
        500,
        "internal".to_string(),
        format!("the server's copy of job {job}'s provenance is damaged"),
    );
    let connection = bed.connection();
    let original: String = connection
        .query_row(
            "SELECT provenance_json FROM job_provenance WHERE job_id = ?1",
            [&job],
            |row| row.get(0),
        )
        .expect("the record");
    for altered in [
        original.replace("\"fee_bps\":\"10\"", "\"fee_bps\":\"1\""),
        original.replace("\"format\":1", "\"format\":2"),
        "not json".to_string(),
    ] {
        assert_ne!(altered, original);
        connection
            .execute(
                "UPDATE job_provenance SET provenance_json = ?1 WHERE job_id = ?2",
                rusqlite::params![altered, job],
            )
            .expect("the damage");
        assert_eq!(
            refused(format!("/v1/backtests/{job}/provenance")).await,
            damaged
        );
    }
    connection
        .execute(
            "UPDATE job_provenance SET provenance_json = ?1, identity = ?2 WHERE job_id = ?3",
            rusqlite::params![original, format!("sha256:{}", "0".repeat(64)), job],
        )
        .expect("another identity");
    assert_eq!(
        refused(format!("/v1/backtests/{job}/provenance")).await,
        damaged
    );
}

#[tokio::test]
async fn a_job_whose_bars_cannot_be_read_or_packed_fails_before_it_runs() {
    let bed = Bed::new();
    stored(&bed);
    let market = market(Category::Spot);
    // The first day's segment, altered behind the server's back.
    let connection = bed.connection();
    let first_day: String = connection
        .query_row(
            "SELECT hash FROM segment_index ORDER BY window_start_ms LIMIT 1",
            [],
            |row| row.get(0),
        )
        .expect("the first day's segment");
    let other =
        segments::compress(&segments::encode(&days()[..3]).expect("a payload")).expect("packed");
    connection
        .execute(
            "UPDATE segments SET data = ?1 WHERE hash = ?2",
            rusqlite::params![other, first_day],
        )
        .expect("the damage");
    let worker = Backtesting::new(Later::Same);
    let handed = Arc::clone(&worker.handed);
    let (address, _jobs) = running(&bed, worker).await;
    let (job, status) = ran(address).await;
    assert_eq!(status["state"]["kind"], "failed", "{status}");
    assert_eq!(
        status["state"]["error"],
        serde_json::json!({
            "code": "internal",
            "message": format!("the server's copy of segment {first_day} is damaged"),
        })
    );
    // The segment as it was, and a bar half a minute after each of the
    // corrected day's: 2,880 bars in one window, which no segment holds.
    let first_day_bars: Vec<Candle> = days()[..1440].to_vec();
    let original =
        segments::compress(&segments::encode(&first_day_bars).expect("a payload")).expect("packed");
    connection
        .execute(
            "UPDATE segments SET data = ?1 WHERE hash = ?2",
            rusqlite::params![original, first_day],
        )
        .expect("the repair");
    let crowded: Vec<Candle> = (0..1440)
        .map(|i| candle(T + DAY + i * MINUTE + 30_000, "1", None))
        .collect();
    bed.store
        .upsert_candles(&market, &crowded)
        .expect("crowded bars");
    let (crowded_job, status) = ran(address).await;
    assert_eq!(status["state"]["kind"], "failed", "{status}");
    assert_eq!(
        status["state"]["error"],
        serde_json::json!({
            "code": "invalid_request",
            "message": format!(
                "the bars of {} spot BTCUSDT 1m from {} cannot be packed into a segment, so the \
                 job cannot be recorded: a segment holds 1 to 1440 candles, not 2880; \
                 `quantforge data validate` shows what is wrong with them",
                VENUE.as_str(),
                ms_to_rfc3339(T + DAY)
            ),
        })
    );
    // Neither job ran, nor recorded a provenance.
    assert!(handed.lock().expect("the runs").is_empty());
    for job in [job, crowded_job] {
        let job = JobId::new(job.as_str()).expect("a job id");
        assert_eq!(bed.store.provenance(&job).expect("a read"), None);
    }
}

#[tokio::test]
async fn a_job_whose_stored_bundle_was_altered_fails_before_it_runs() {
    let bed = Bed::new();
    stored(&bed);
    let worker = Backtesting::new(Later::Same);
    let handed = Arc::clone(&worker.handed);
    let jobs = jobs_of(&bed, worker, DEFAULT_MAX_SPAN_BARS);
    let address = serve_jobs(&bed, jobs.clone()).await;
    let hash = upload(address, &bundle()).await;
    let request = submission(&hash, data(Some(Category::Spot), START, END));
    let job = queued(submit(address, &request).await).await["job_id"]
        .as_str()
        .expect("a job id")
        .to_string();
    // Other code stored under the bundle's name while its job waits: it
    // reads back as a bundle, and is still not the one the job names.
    bed.connection()
        .execute("UPDATE bundles SET data = ?1", [other_code()])
        .expect("the alteration");
    assert_eq!(jobs.start().await.expect("a started queue"), 0);
    let status = ended(address, &job).await;
    assert_eq!(
        status["state"]["error"],
        serde_json::json!({
            "code": "internal",
            "message": "the server cannot read the job's bundle",
        })
    );
    assert!(handed.lock().expect("the runs").is_empty());
    let job = JobId::new(job.as_str()).expect("a job id");
    assert_eq!(bed.store.provenance(&job).expect("a read"), None);
}
