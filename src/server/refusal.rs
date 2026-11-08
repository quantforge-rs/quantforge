//! Every failed answer, as the protocol's error envelope: what a request
//! got wrong, what the server does not answer, and what a storage failure
//! may tell a client. A client learns whether to retry, never the store's
//! own words, which can name its host and database; the operator reads
//! those in the log.

use std::error::Error;

use axum::Json;
use axum::http::header::{CACHE_CONTROL, RETRY_AFTER};
use axum::http::{HeaderValue, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use tracing::{error, warn};

use crate::ports::{StorageError, StorageErrorKind};
use crate::protocol::{ErrorBody, ErrorCode, ErrorEnvelope, ProtoVersion, VersionRange};

/// How many seconds a client told to retry waits first.
const RETRY_AFTER_SECONDS: &str = "1";

/// A failed answer: a code to branch on and a message for people.
#[derive(Debug)]
pub(super) struct Refusal {
    pub(super) code: ErrorCode,
    pub(super) message: String,
    supported: Option<VersionRange>,
}

impl Refusal {
    fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            supported: None,
        }
    }

    /// The request is malformed or names something invalid.
    pub(super) fn invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidRequest, message)
    }

    /// What the request names does not exist.
    pub(super) fn not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::NotFound, message)
    }

    /// The request contradicts the state of what it names.
    pub(super) fn conflict(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Conflict, message)
    }

    /// The server cannot answer now, and a retry can succeed.
    pub(super) fn unavailable(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Unavailable, message)
    }

    /// The server failed.
    pub(super) fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Internal, message)
    }

    /// What a storage failure while `doing` something tells a client: to
    /// retry when nothing was changed and time ran out, and otherwise that
    /// the server failed.
    pub(super) fn storage(doing: &str, failure: StorageError) -> Self {
        let kind = failure.kind();
        let causes = causes(&failure);
        match kind {
            StorageErrorKind::Unavailable
            | StorageErrorKind::Contention
            | StorageErrorKind::Conflict => {
                warn!(%kind, error = %causes, "the store could not answer in time while {doing}");
                Self::unavailable("the server's store could not answer in time; retry later")
            }
            _ => {
                error!(%kind, error = %causes, "the store failed while {doing}");
                Self::internal("the server's store failed to answer")
            }
        }
    }
}

/// An error and every cause under it, as one line.
fn causes(failure: &dyn Error) -> String {
    let mut line = failure.to_string();
    let mut cause = failure.source();
    while let Some(next) = cause {
        line.push_str(": ");
        line.push_str(&next.to_string());
        cause = next.source();
    }
    line
}

impl IntoResponse for Refusal {
    fn into_response(self) -> Response {
        let status = self
            .code
            .http_status()
            .and_then(|status| StatusCode::from_u16(status).ok())
            .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let retry = self.code == ErrorCode::Unavailable;
        let envelope = ErrorEnvelope {
            error: ErrorBody {
                code: self.code,
                message: self.message,
                supported: self.supported,
            },
        };
        // A refusal says how things stood at the time: no cache keeps it.
        let mut response = (
            status,
            [(CACHE_CONTROL, HeaderValue::from_static("no-store"))],
            Json(envelope),
        )
            .into_response();
        if retry {
            response
                .headers_mut()
                .insert(RETRY_AFTER, HeaderValue::from_static(RETRY_AFTER_SECONDS));
        }
        response
    }
}

/// A path no route answers: under a version the server does not speak,
/// the versions it does; otherwise not found.
pub(super) async fn no_route(method: Method, uri: Uri) -> Refusal {
    let first = uri.path().trim_start_matches('/').split('/').next();
    if let Some(Ok(version)) = first.map(ProtoVersion::from_path_segment) {
        let supported = VersionRange::SUPPORTED;
        if !supported.supports(version) {
            return Refusal {
                supported: Some(supported),
                ..Refusal::new(
                    ErrorCode::UnsupportedVersion,
                    format!(
                        "this server speaks protocol versions {} to {}, not {}",
                        supported.min_supported.0, supported.proto_version.0, version.0
                    ),
                )
            };
        }
    }
    Refusal::not_found(format!("this server answers no {method} {}", uri.path()))
}

/// A known path asked with a method it does not take. The router adds the
/// `Allow` header that lists the ones it does.
pub(super) async fn wrong_method(method: Method, uri: Uri) -> Refusal {
    Refusal::new(
        ErrorCode::MethodNotAllowed,
        format!(
            "{} does not take {method}; the Allow header lists what it takes",
            uri.path()
        ),
    )
}
