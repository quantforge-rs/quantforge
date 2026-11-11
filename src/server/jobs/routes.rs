//! The job routes: an upload, a submission, a job's status, the list, and
//! a cancel. Every answer says how things stand now, so no cache keeps it.

use axum::Json;
use axum::body::Bytes;
use axum::extract::rejection::{BytesRejection, JsonRejection, PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::header::{CONTENT_TYPE, LOCATION};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::Response;
use serde::Deserialize;

use super::status::status;
use super::{Jobs, submission};
use crate::model::{JobId, JobPhase, now_utc_ms};
use crate::protocol::bundle::BUNDLE_MEDIA_TYPE;
use crate::protocol::jobs::{self, BundleReceipt, JobList, JobSubmission};
use crate::protocol::{ContentHash, ProtocolError};
use crate::server::history::fresh;
use crate::server::refusal::Refusal;

/// How many jobs a list holds when its query names no `limit`.
const DEFAULT_LIMIT: u64 = 100;

/// The most jobs one list holds.
const MAX_LIMIT: u64 = 1_000;

/// `PUT /v1/bundles/{code_hash}`.
pub(in crate::server) async fn upload(
    State(jobs): State<Jobs>,
    name: Result<Path<String>, PathRejection>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Result<Response, Refusal> {
    let Path(text) = name.map_err(|rejection| Refusal::invalid(rejection.body_text()))?;
    let name: ContentHash = text
        .parse()
        .map_err(|failure: ProtocolError| Refusal::invalid(failure.to_string()))?;
    let media_type = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(str::trim);
    if !media_type.is_some_and(|found| found.eq_ignore_ascii_case(BUNDLE_MEDIA_TYPE)) {
        return Err(Refusal::invalid(format!(
            "an upload is a bundle's canonical tar, sent with Content-Type {BUNDLE_MEDIA_TYPE}"
        )));
    }
    let tar = body.map_err(|rejection| {
        if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
            Refusal::invalid(format!(
                "the upload holds more than {} bytes, the most a canonical tar within this \
                 server's bundle limits can hold",
                jobs.inner.limits.max_archive_bytes()
            ))
        } else {
            Refusal::invalid(format!(
                "the upload could not be read: {}",
                rejection.body_text()
            ))
        }
    })?;
    let stored = submission::upload(&jobs, name, tar.to_vec()).await?;
    let code = if stored.new {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    let receipt = BundleReceipt {
        code_hash: name,
        size_bytes: stored.size_bytes,
    };
    Ok(fresh((code, Json(receipt))))
}

/// `POST /v1/backtests`.
pub(in crate::server) async fn submit(
    State(jobs): State<Jobs>,
    uri: Uri,
    body: Result<Json<JobSubmission>, JsonRejection>,
) -> Result<Response, Refusal> {
    let Json(submission) = body.map_err(|rejection| {
        Refusal::invalid(format!(
            "the body is not a job submission: {}",
            rejection.body_text()
        ))
    })?;
    let record = submission::submit(&jobs, submission).await?;
    let status = status(&record)?;
    let location = format!("{}/{}", uri.path().trim_end_matches('/'), status.job_id);
    Ok(fresh((
        StatusCode::CREATED,
        [(LOCATION, location)],
        Json(status),
    )))
}

/// `GET /v1/backtests/{job_id}`.
pub(in crate::server) async fn job(
    State(jobs): State<Jobs>,
    id: Result<Path<String>, PathRejection>,
) -> Result<Response, Refusal> {
    let job_id = named(id)?;
    let id = job_id.clone();
    let record = jobs
        .inner
        .store
        .call(move |store| {
            store
                .job(&id)
                .map_err(|failure| Refusal::storage("reading a job", failure))
        })
        .await?
        .ok_or_else(|| no_job(job_id.as_str()))?;
    Ok(fresh(Json(status(&record)?)))
}

/// A list's query string.
#[derive(Debug, Deserialize)]
pub(in crate::server) struct ListQuery {
    limit: Option<u64>,
}

/// `GET /v1/backtests`.
pub(in crate::server) async fn list(
    State(jobs): State<Jobs>,
    query: Result<Query<ListQuery>, QueryRejection>,
) -> Result<Response, Refusal> {
    let Query(query) = query.map_err(|rejection| {
        Refusal::invalid(format!(
            "the query string is not a /v1/backtests query: {}",
            rejection.body_text()
        ))
    })?;
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT);
    if !(1..=MAX_LIMIT).contains(&limit) {
        return Err(Refusal::invalid(format!(
            "limit {limit} is not between 1 and {MAX_LIMIT}"
        )));
    }
    let limit = usize::try_from(limit).unwrap_or(usize::MAX);
    let records = jobs
        .inner
        .store
        .call(move |store| {
            store
                .jobs(limit)
                .map_err(|failure| Refusal::storage("listing the jobs", failure))
        })
        .await?;
    let jobs = records.iter().map(status).collect::<Result<_, _>>()?;
    Ok(fresh(Json(JobList { jobs })))
}

/// `POST /v1/backtests/{job_id}/cancel`.
pub(in crate::server) async fn cancel(
    State(jobs): State<Jobs>,
    id: Result<Path<String>, PathRejection>,
) -> Result<Response, Refusal> {
    let job_id = named(id)?;
    let id = job_id.clone();
    let record = jobs
        .inner
        .store
        .call(move |store| {
            store
                .cancel_job(&id, now_utc_ms())
                .map_err(|failure| Refusal::storage("cancelling a job", failure))
        })
        .await?
        .ok_or_else(|| no_job(job_id.as_str()))?;
    match record.phase {
        JobPhase::Cancelled => {
            jobs.signal(&job_id);
            Ok(fresh(Json(status(&record)?)))
        }
        JobPhase::Succeeded | JobPhase::Failed => Err(Refusal::conflict(format!(
            "job {job_id} has already {}; only a queued or running job can be cancelled",
            record.phase
        ))),
        JobPhase::Queued | JobPhase::Running => Err(Refusal::internal(format!(
            "the server's store did not cancel job {job_id}"
        ))),
    }
}

/// The job a path names. A path that is no job ID at all is refused; one
/// that is not an ID this server makes up names no job here.
fn named(id: Result<Path<String>, PathRejection>) -> Result<JobId, Refusal> {
    let Path(text) = id.map_err(|rejection| Refusal::invalid(rejection.body_text()))?;
    job_named(text)
}

/// The job `text` names, as [`named`] reads a path's.
pub(super) fn job_named(text: String) -> Result<JobId, Refusal> {
    let wire = jobs::JobId::new(text).map_err(|failure| Refusal::invalid(failure.to_string()))?;
    JobId::new(wire.as_str()).map_err(|_| no_job(wire.as_str()))
}

pub(super) fn no_job(job_id: &str) -> Refusal {
    Refusal::not_found(format!("the server holds no job {job_id}"))
}
