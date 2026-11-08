//! The job routes' submissions, statuses, list and cancel: a submission
//! queued with its category named, each one refused before anything is
//! queued, and what the other routes answer of the jobs the store holds.
//! The queue never starts here; `tests/backtest_jobs.rs` runs jobs.

use serde_json::json;

use super::jobs::{
    Admitting, bundle, data, jobs_of, queued, runtime, serve_jobs, submission, submit, upload,
};
use super::*;
use crate::model::{JobId, JobOutcome, NewJob};
use crate::ports::JobStore;
use crate::protocol::ContentHash;

/// A bed holding Spot BTCUSDT `1m` from `T` for an hour, 60 bars.
fn hour() -> Bed {
    let bed = Bed::new();
    let candles: Vec<Candle> = (0..60).map(|i| candle(T + i * MINUTE, "1", None)).collect();
    bed.store
        .upsert_candles(&market(Category::Spot), &candles)
        .expect("the candles");
    bed
}

async fn get(address: SocketAddr, path: &str) -> reqwest::Response {
    client()
        .get(format!("http://{address}{path}"))
        .send()
        .await
        .expect("a response")
}

async fn post(address: SocketAddr, path: &str) -> reqwest::Response {
    client()
        .post(format!("http://{address}{path}"))
        .send()
        .await
        .expect("a response")
}

#[tokio::test]
async fn a_submission_is_queued_with_its_category_named_and_read_back_by_its_location() {
    let bed = hour();
    let address = serve_jobs(&bed, jobs_of(&bed, Admitting(runtime("3.12")), 100)).await;
    let hash = upload(address, &bundle()).await;
    let body = submission(&hash, data(None, T, T + 59 * MINUTE));
    let status = queued(submit(address, &body).await).await;

    let id = status["job_id"].as_str().expect("a job id").to_string();
    assert_eq!(id.len(), 32, "{id}");
    assert!(
        id.bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    );
    assert_eq!(status["state"], json!({"kind": "queued"}));
    let mut named = body.clone();
    named["data"]["category"] = json!("spot");
    assert_eq!(status["submission"], named);
    assert!(status["submitted_at_ms"].as_i64().is_some_and(|ms| ms > T));

    let response = get(address, &format!("/v1/backtests/{id}")).await;
    assert_eq!(response.status(), 200);
    assert_eq!(header(&response, CACHE_CONTROL), Some("no-cache"));
    let read: Value = response.json().await.expect("a status");
    assert_eq!(read, status);
    let list: Value = get(address, "/v1/backtests")
        .await
        .json()
        .await
        .expect("a list");
    assert_eq!(list, json!({"jobs": [status]}));
}

#[tokio::test]
async fn a_submission_is_refused_before_anything_is_queued() {
    let bed = hour();
    bed.store
        .upsert_candles(
            &market_of(VENUE, Category::Linear, "ETHUSDT", Interval::M1),
            &[candle(T, "1", None)],
        )
        .expect("a linear candle");
    bed.store
        .upsert_candles(
            &market_of(VENUE, Category::Spot, "ETHUSDT", Interval::M1),
            &[candle(T, "1", None)],
        )
        .expect("a spot candle");
    let address = serve_jobs(&bed, jobs_of(&bed, Admitting(runtime("3.12")), 30)).await;
    let hash = upload(address, &bundle()).await;
    let missing = ContentHash::sha256([7; 32]);
    let end = T + 29 * MINUTE;
    let with = |change: &dyn Fn(&mut Value)| {
        let mut body = submission(&hash, data(Some(Category::Spot), T, end));
        change(&mut body);
        body
    };
    let cases: Vec<(Value, String)> = vec![
        (
            with(&|body| body["data"]["symbol"] = json!("btcusdt")),
            r#"symbol "btcusdt" is not in its canonical form; write "BTCUSDT""#.to_string(),
        ),
        (
            with(&|body| body["data"]["exchange"] = json!("nyse")),
            format!(
                r#"exchange "nyse" is not one QuantForge names; write one of {}"#,
                Venue::ALL.map(|venue| venue.as_str()).join(", ")
            ),
        ),
        (
            with(&|body| body["data"]["category"] = json!("futures")),
            r#"category "futures" is not spot, linear or inverse"#.to_string(),
        ),
        (
            with(&|body| body["data"]["interval"] = json!("1M")),
            r#"interval "1M" is not one of QuantForge's intervals, such as 1m or 1h"#.to_string(),
        ),
        (
            with(&|body| body["data"]["start_open_ms"] = json!(end + MINUTE)),
            format!("start_open_ms {} is after end_open_ms {end}", end + MINUTE),
        ),
        (
            with(&|body| body["data"]["end_open_ms"] = json!(T + 30 * MINUTE)),
            "the range spans 31 bars of 1m, more than the 30 this server runs one job on; ask \
             for a shorter range"
                .to_string(),
        ),
        (
            with(&|body| body["backtest"]["initial_cash"] = json!("0")),
            "initial_cash is 0; a backtest starts with cash above zero".to_string(),
        ),
        (
            with(&|body| body["backtest"]["fee_bps"] = json!("-1")),
            "fee_bps is -1; a fee is zero or more basis points".to_string(),
        ),
        (
            with(&|body| body["bundle"] = json!(missing.to_string())),
            format!(
                "the server holds no bundle {missing}; upload it first with PUT /v1/bundles/{missing}"
            ),
        ),
        (
            with(&|body| {
                body["data"]["symbol"] = json!("ETHUSDT");
                body["data"]
                    .as_object_mut()
                    .expect("an object")
                    .remove("category");
            }),
            format!(
                "{} stores ETHUSDT 1m as spot and linear; a market without category reads spot \
                 only when no other family holds the symbol and interval, so name the category",
                VENUE.as_str()
            ),
        ),
        (
            with(&|body| body["data"]["symbol"] = json!("SOLUSDT")),
            format!(
                "the server stores no bars of {} spot SOLUSDT 1m; a job runs only on candles the \
                 server holds",
                VENUE.as_str()
            ),
        ),
        (
            with(&|body| {
                body["data"]["start_open_ms"] = json!(T - MINUTE);
                body["data"]["end_open_ms"] = json!(end - MINUTE);
            }),
            format!(
                "the server stores {} spot BTCUSDT 1m from {} to {}, which does not cover the \
                 job's range from {} to {}; ask for a range inside it",
                VENUE.as_str(),
                ms_to_rfc3339(T),
                ms_to_rfc3339(T + 59 * MINUTE),
                ms_to_rfc3339(T - MINUTE),
                ms_to_rfc3339(end - MINUTE)
            ),
        ),
    ];
    for (body, expected) in cases {
        let (status, code, message) = refusal(submit(address, &body).await).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "invalid_request"),
            "{expected}"
        );
        assert_eq!(message, expected);
    }

    let mut incomplete = with(&|_| {});
    incomplete["data"]
        .as_object_mut()
        .expect("an object")
        .remove("symbol");
    let (status, code, message) = refusal(submit(address, &incomplete).await).await;
    assert_eq!((status, code.as_str()), (400, "invalid_request"));
    assert!(
        message.starts_with("the body is not a job submission: ") && message.contains("symbol"),
        "{message}"
    );
    let list: Value = get(address, "/v1/backtests")
        .await
        .json()
        .await
        .expect("a list");
    assert_eq!(list, json!({"jobs": []}));
}

#[tokio::test]
async fn a_bundle_the_runtime_cannot_run_is_refused_at_submission_in_its_own_words() {
    let bed = hour();
    let address = serve_jobs(&bed, jobs_of(&bed, Admitting(runtime("3.11")), 100)).await;
    // Stored before this server's runtime said otherwise.
    let bundle = bundle();
    let hash = crate::model::BundleHash::from_digest(*bundle.code_hash().digest());
    bed.store
        .store_bundle(&hash, &bundle.canonical_tar(), T)
        .expect("the bundle");
    let body = submission(&bundle.code_hash(), data(None, T, T + MINUTE));
    let (status, code, message) = refusal(submit(address, &body).await).await;
    assert_eq!((status, code.as_str()), (400, "invalid_request"));
    let expected = bundle
        .admit(&runtime("3.11"))
        .expect_err("refused")
        .to_string();
    assert_eq!(message, expected);
}

#[tokio::test]
async fn a_job_no_id_names_is_refused_and_one_the_store_lacks_is_not_found() {
    let bed = Bed::new();
    let address = serve_jobs(&bed, jobs_of(&bed, NoRuntime, 100)).await;
    let absent = "0".repeat(32);
    for (path, status, code, message) in [
        (
            "/v1/backtests/j-7f3a",
            404,
            "not_found",
            "the server holds no job j-7f3a".to_string(),
        ),
        (
            &format!("/v1/backtests/{absent}"),
            404,
            "not_found",
            format!("the server holds no job {absent}"),
        ),
        (
            "/v1/backtests/a.b",
            400,
            "invalid_request",
            "invalid job id \"a.b\"".to_string(),
        ),
    ] {
        for response in [
            get(address, path).await,
            post(address, &format!("{path}/cancel")).await,
        ] {
            let answer = refusal(response).await;
            assert_eq!((answer.0, answer.1.as_str()), (status, code), "{path}");
            assert!(answer.2.starts_with(&message), "{path}: {}", answer.2);
        }
    }
}

#[tokio::test]
async fn the_list_holds_the_last_submitted_first_and_at_most_its_limit() {
    let bed = hour();
    let address = serve_jobs(&bed, jobs_of(&bed, Admitting(runtime("3.12")), 100)).await;
    let hash = upload(address, &bundle()).await;
    let mut ids = Vec::new();
    for _ in 0..3 {
        let status = queued(submit(address, &submission(&hash, data(None, T, T))).await).await;
        ids.push(status["job_id"].as_str().expect("an id").to_string());
    }
    let listed = |list: Value| -> Vec<String> {
        list["jobs"]
            .as_array()
            .expect("jobs")
            .iter()
            .map(|job| job["job_id"].as_str().expect("an id").to_string())
            .collect()
    };
    let all = get(address, "/v1/backtests")
        .await
        .json()
        .await
        .expect("a list");
    assert_eq!(
        listed(all),
        [ids[2].clone(), ids[1].clone(), ids[0].clone()]
    );
    let two = get(address, "/v1/backtests?limit=2")
        .await
        .json()
        .await
        .expect("a list");
    assert_eq!(listed(two), [ids[2].clone(), ids[1].clone()]);

    for (query, expected) in [
        ("limit=0", "limit 0 is not between 1 and 1000"),
        ("limit=1001", "limit 1001 is not between 1 and 1000"),
        (
            "limit=many",
            "the query string is not a /v1/backtests query: ",
        ),
    ] {
        let (status, code, message) =
            refusal(get(address, &format!("/v1/backtests?{query}")).await).await;
        assert_eq!((status, code.as_str()), (400, "invalid_request"), "{query}");
        assert!(message.starts_with(expected), "{query}: {message}");
    }
}

#[tokio::test]
async fn a_queued_job_is_cancelled_once_and_a_finished_one_cannot_be() {
    let bed = hour();
    let address = serve_jobs(&bed, jobs_of(&bed, Admitting(runtime("3.12")), 100)).await;
    let hash = upload(address, &bundle()).await;
    let status = queued(submit(address, &submission(&hash, data(None, T, T))).await).await;
    let id = status["job_id"].as_str().expect("an id").to_string();

    let first: Value = post(address, &format!("/v1/backtests/{id}/cancel"))
        .await
        .json()
        .await
        .expect("a status");
    assert_eq!(first["state"]["kind"], "cancelled");
    assert!(first["state"].get("started_at_ms").is_none(), "{first}");
    assert!(first["state"]["finished_at_ms"].as_i64().is_some());
    let again = post(address, &format!("/v1/backtests/{id}/cancel")).await;
    assert_eq!(again.status(), 200);
    assert_eq!(again.json::<Value>().await.expect("a status"), first);

    // A job another server ran to its end.
    let done = JobId::generate();
    let bundle_hash = crate::model::BundleHash::from_digest(*hash.digest());
    let request_json =
        serde_json::to_string(&submission(&hash, data(Some(Category::Spot), T, T))).expect("json");
    bed.store
        .submit_job(&NewJob {
            job_id: done.clone(),
            bundle: bundle_hash,
            request_json,
            submitted_at_ms: T,
        })
        .expect("a job");
    let claimed = bed
        .store
        .claim_job(T + 1)
        .expect("a claim")
        .expect("the job");
    assert_eq!(claimed.job_id, done);
    let outcome = JobOutcome::Succeeded {
        results_json: "[]".to_string(),
    };
    assert!(
        bed.store
            .finish_job(&done, &outcome, T + 2)
            .expect("an end")
    );
    let (status, code, message) =
        refusal(post(address, &format!("/v1/backtests/{done}/cancel")).await).await;
    assert_eq!((status, code.as_str()), (409, "conflict"));
    assert_eq!(
        message,
        format!("job {done} has already succeeded; only a queued or running job can be cancelled")
    );
    let read: Value = get(address, &format!("/v1/backtests/{done}"))
        .await
        .json()
        .await
        .expect("a status");
    assert_eq!(
        read["state"],
        json!({"kind": "succeeded", "started_at_ms": T + 1, "finished_at_ms": T + 2, "results": []})
    );
}
