//! `GET /v1/segments/{hash}`: a segment's bytes as stored, which never
//! change, so any cache may keep them for a year. Before the server sends
//! them it checks them against their name, so that no cache keeps a damaged
//! copy for that long.

use axum::extract::rejection::PathRejection;
use axum::extract::{Path, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE, ETAG, IF_NONE_MATCH};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use tracing::error;

use super::refusal::Refusal;
use super::store::ServerStore;
use crate::model::SegmentHash;
use crate::protocol::ContentHash;
use crate::protocol::segments::{SEGMENT_CACHE_CONTROL, SEGMENT_MEDIA_TYPE};
use crate::storage::segments;

/// `GET /v1/segments/{hash}`.
pub(super) async fn segment(
    State(store): State<ServerStore>,
    hash: Result<Path<String>, PathRejection>,
    headers: HeaderMap,
) -> Result<Response, Refusal> {
    let Path(text) = hash.map_err(|rejection| Refusal::invalid(rejection.body_text()))?;
    let hash: ContentHash = text
        .parse()
        .map_err(|failure: crate::protocol::ProtocolError| Refusal::invalid(failure.to_string()))?;
    let tag = format!("\"{hash}\"");
    let cached = names_tag(&headers, &tag);
    let name = SegmentHash::from_digest(*hash.digest());
    let found = store
        .call(move |store| {
            let Some(bytes) = store
                .load_segment(&name)
                .map_err(|failure| Refusal::storage("loading a segment", failure))?
            else {
                return Ok(None);
            };
            // Bytes a client already holds are not sent, so not checked.
            if !cached {
                check(&bytes, &name)?;
            }
            Ok(Some(bytes))
        })
        .await?;
    let Some(bytes) = found else {
        return Err(Refusal::not_found(format!(
            "the server holds no segment {hash}"
        )));
    };
    let headers = [
        (ETAG, tag),
        (CACHE_CONTROL, SEGMENT_CACHE_CONTROL.to_string()),
    ];
    if cached {
        return Ok((StatusCode::NOT_MODIFIED, headers).into_response());
    }
    Ok((headers, [(CONTENT_TYPE, SEGMENT_MEDIA_TYPE)], bytes).into_response())
}

/// Whether an `If-None-Match` field names `tag`, or is `*`. Entity tags
/// compare weakly there, so `W/"…"` names it too.
pub(super) fn names_tag(headers: &HeaderMap, tag: &str) -> bool {
    headers
        .get_all(IF_NONE_MATCH)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .any(|candidate| {
            candidate == "*" || candidate.strip_prefix("W/").unwrap_or(candidate) == tag
        })
}

/// Refuses stored bytes that are not one zstd frame of a payload named
/// `name`, as every reader would.
fn check(bytes: &[u8], name: &SegmentHash) -> Result<(), Refusal> {
    let damaged = |why: String| {
        error!(segment = %name, %why, "a stored segment does not match its name, so it was not sent");
        Refusal::internal(format!("the server's copy of segment {name} is damaged"))
    };
    let payload = segments::decompress(bytes).map_err(|failure| damaged(failure.to_string()))?;
    let found = segments::hash(&payload);
    if found != *name {
        return Err(damaged(format!("its payload hashes to {found}")));
    }
    Ok(())
}
