//! The result routes: what a succeeded job produced, and each of its
//! files. A result never changes, so a file may be cached for a year; the
//! server turns what its store keeps back into the file and checks it
//! against the file's hash before it sends a byte.

use axum::Json;
use axum::extract::rejection::PathRejection;
use axum::extract::{Path, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE, ETAG};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use tracing::error;

use super::Jobs;
use super::routes::{job_named, no_job};
use crate::model::{JobId, JobPhase, JobRecord, ResultHash};
use crate::protocol::jobs::{self, JobResult, ResultLink, ResultRef};
use crate::protocol::results::{Metrics, RESULT_CACHE_CONTROL, RESULT_FORMAT, ResultKind};
use crate::server::history::fresh;
use crate::server::refusal::Refusal;
use crate::server::results::unpack;
use crate::server::segment::names_tag;
use crate::server::store::ServedStore;

/// `GET /v1/backtests/{job_id}/result`.
pub(in crate::server) async fn result(
    State(jobs): State<Jobs>,
    uri: Uri,
    id: Result<Path<String>, PathRejection>,
) -> Result<Response, Refusal> {
    let Path(text) = id.map_err(|rejection| Refusal::invalid(rejection.body_text()))?;
    let job_id = job_named(text)?;
    let id = job_id.clone();
    let (references, metrics) = jobs
        .inner
        .store
        .call(move |store| {
            let references = produced(store, &id)?;
            let metrics = file(store, &id, &references, ResultKind::Metrics)?;
            let metrics = Metrics::decode(&metrics).map_err(|failure| {
                damaged(&id, ResultKind::Metrics.name(), &failure.to_string())
            })?;
            Ok((references, metrics))
        })
        .await?;
    let base = uri.path().trim_end_matches('/');
    let results = ResultKind::ALL
        .into_iter()
        .filter_map(|kind| {
            let reference = references.iter().find(|one| one.name == kind.name())?;
            Some(ResultLink {
                name: reference.name.clone(),
                media_type: reference.media_type.clone(),
                hash: reference.hash,
                size_bytes: reference.size_bytes,
                path: format!("{base}/{}", kind.file_name()),
            })
        })
        .collect();
    let job_id = jobs::JobId::new(job_id.as_str())
        .map_err(|_| Refusal::internal(format!("the server cannot read job {job_id}")))?;
    Ok(fresh(Json(JobResult {
        job_id,
        format: RESULT_FORMAT,
        metrics,
        results,
    })))
}

/// `GET /v1/backtests/{job_id}/result/{file}`.
pub(in crate::server) async fn result_file(
    State(jobs): State<Jobs>,
    path: Result<Path<(String, String)>, PathRejection>,
    headers: HeaderMap,
) -> Result<Response, Refusal> {
    let Path((text, file_name)) =
        path.map_err(|rejection| Refusal::invalid(rejection.body_text()))?;
    let job_id = job_named(text)?;
    let Some(kind) = ResultKind::from_file_name(&file_name) else {
        return Err(Refusal::not_found(format!(
            "a job's result has no file {file_name}; its files are metrics.json, trades.arrow \
             and equity.arrow"
        )));
    };
    let id = job_id.clone();
    let (reference, bytes) = jobs
        .inner
        .store
        .call(move |store| {
            let references = produced(store, &id)?;
            let reference = named(&id, &references, kind)?.clone();
            // Bytes a client already holds are not sent, so not read.
            if names_tag(&headers, &tag(&reference)) {
                return Ok((reference, None));
            }
            let bytes = file(store, &id, &references, kind)?;
            Ok((reference, Some(bytes)))
        })
        .await?;
    let cached = [
        (ETAG, tag(&reference)),
        (CACHE_CONTROL, RESULT_CACHE_CONTROL.to_string()),
    ];
    Ok(match bytes {
        None => (StatusCode::NOT_MODIFIED, cached).into_response(),
        Some(bytes) => (cached, [(CONTENT_TYPE, reference.media_type)], bytes).into_response(),
    })
}

/// A file's entity tag: its hash, in quotes.
fn tag(reference: &ResultRef) -> String {
    format!("\"{}\"", reference.hash)
}

/// The results a job lists, once it has succeeded.
fn produced(store: &dyn ServedStore, job_id: &JobId) -> Result<Vec<ResultRef>, Refusal> {
    let record = store
        .job(job_id)
        .map_err(|failure| Refusal::storage("reading a job", failure))?
        .ok_or_else(|| no_job(job_id.as_str()))?;
    let stands = match record.phase {
        JobPhase::Succeeded => return listed(&record),
        JobPhase::Queued => "is still queued",
        JobPhase::Running => "is still running",
        JobPhase::Failed => "failed",
        JobPhase::Cancelled => "was cancelled",
    };
    Err(Refusal::conflict(format!(
        "job {job_id} {stands}; only a job that has succeeded has a result"
    )))
}

fn listed(record: &JobRecord) -> Result<Vec<ResultRef>, Refusal> {
    serde_json::from_str(record.results_json.as_deref().unwrap_or_default()).map_err(|failure| {
        error!(job = %record.job_id, %failure, "a stored job's results cannot be read");
        Refusal::internal(format!("the server cannot read job {}", record.job_id))
    })
}

/// The reference a succeeded job lists for `kind`.
fn named<'a>(
    job_id: &JobId,
    references: &'a [ResultRef],
    kind: ResultKind,
) -> Result<&'a ResultRef, Refusal> {
    references
        .iter()
        .find(|reference| reference.name == kind.name())
        .ok_or_else(|| {
            error!(job = %job_id, result = kind.name(), "a succeeded job lists no such result");
            Refusal::internal(format!(
                "the server holds no {} result of job {job_id}",
                kind.name()
            ))
        })
}

/// One of a job's files, from what the store keeps of it, checked against
/// its hash.
fn file(
    store: &dyn ServedStore,
    job_id: &JobId,
    references: &[ResultRef],
    kind: ResultKind,
) -> Result<Vec<u8>, Refusal> {
    let reference = named(job_id, references, kind)?;
    let stored = store
        .load_result(&ResultHash::from_digest(*reference.hash.digest()))
        .map_err(|failure| Refusal::storage("loading a result", failure))?
        .ok_or_else(|| {
            damaged(
                job_id,
                kind.name(),
                "the store holds nothing under its name",
            )
        })?;
    unpack(&stored, reference).map_err(|why| damaged(job_id, kind.name(), &why))
}

/// The refusal of a result whose stored copy is not the file its job
/// lists: nothing of it is sent.
fn damaged(job_id: &JobId, result: &str, why: &str) -> Refusal {
    error!(job = %job_id, result, why, "a stored result does not match its name, so it was not sent");
    Refusal::internal(format!(
        "the server's copy of job {job_id}'s {result} result is damaged"
    ))
}
