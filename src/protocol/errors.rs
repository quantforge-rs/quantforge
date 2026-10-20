//! The error every surface answers with.

use serde::{Deserialize, Serialize};

use super::VersionRange;

/// The body of every failed HTTP response: an [`ErrorBody`] under `error`.
///
/// ```
/// use quantforge::protocol::{ErrorCode, ErrorEnvelope};
///
/// let json = r#"{"error":{"code":"not_found","message":"no job j-1"}}"#;
/// let envelope: ErrorEnvelope = serde_json::from_str(json)?;
/// assert_eq!(envelope.error.code, ErrorCode::NotFound);
/// assert_eq!(envelope.error.code.http_status(), Some(404));
/// assert_eq!(serde_json::to_string(&envelope)?, json);
/// # Ok::<(), serde_json::Error>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorEnvelope {
    /// What went wrong.
    pub error: ErrorBody,
}

/// What went wrong: a code to branch on, a message for people, and the
/// versions the server speaks when the version was the problem. The stream
/// sends the same fields as its `error` frame. Its shape is the same in every
/// version.
///
/// ```
/// use quantforge::protocol::{ErrorBody, ErrorCode, ProtoVersion, VersionRange};
///
/// let json = r#"{"code":"unsupported_version","message":"this server speaks protocol versions 1 to 1","supported":{"min_supported":1,"proto_version":1}}"#;
/// let body: ErrorBody = serde_json::from_str(json)?;
/// assert_eq!(body.code, ErrorCode::UnsupportedVersion);
/// assert_eq!(
///     body.supported,
///     Some(VersionRange { min_supported: ProtoVersion(1), proto_version: ProtoVersion(1) })
/// );
/// assert_eq!(serde_json::to_string(&body)?, json);
/// # Ok::<(), serde_json::Error>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorBody {
    /// What went wrong, for a program to branch on.
    pub code: ErrorCode,
    /// What went wrong, for a person to read. The wording may change at any
    /// time, so a program matches on `code` instead.
    pub message: String,
    /// With [`ErrorCode::UnsupportedVersion`], the versions the server speaks,
    /// so that the client can try one of them. Absent otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supported: Option<VersionRange>,
}

/// What went wrong, as a program branches on it.
///
/// The codes describe requests to a QuantForge server. They are not an
/// exchange's error kinds, even where the names match: a failure the
/// exchange reported to the server is described in the message, under the
/// code that fits the request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ErrorCode {
    /// The request is malformed or names something invalid, such as a market
    /// not in its canonical form, or a bundle that cannot run. Sending it
    /// again unchanged fails again. HTTP 400.
    InvalidRequest,
    /// The credentials are missing, unknown, or expired. HTTP 401.
    Unauthorized,
    /// The job, segment, bundle, or market the request names does not exist.
    /// HTTP 404.
    NotFound,
    /// The request conflicts with the current state, such as cancelling a job
    /// that has finished. HTTP 409.
    Conflict,
    /// The server speaks none of the versions the request offers;
    /// [`ErrorBody::supported`] lists the ones it does. HTTP 400.
    UnsupportedVersion,
    /// The request would take the account or session past a limit, such as
    /// its number of queued jobs or of subscriptions. It can succeed once
    /// usage drops. HTTP 429.
    LimitExceeded,
    /// Too many requests in too short a time; wait, then retry. HTTP 429.
    RateLimited,
    /// The server cannot serve the request right now; retry later. HTTP 503.
    Unavailable,
    /// The server failed while handling the request. HTTP 500.
    Internal,
    /// A code this build does not know, from a later revision of the
    /// protocol. Never sent: serializing it fails.
    #[serde(other, skip_serializing)]
    Unrecognized,
}

impl ErrorCode {
    /// The HTTP status a server answers with alongside this code, or `None`
    /// for [`ErrorCode::Unrecognized`], which no server sends.
    pub fn http_status(self) -> Option<u16> {
        match self {
            Self::InvalidRequest | Self::UnsupportedVersion => Some(400),
            Self::Unauthorized => Some(401),
            Self::NotFound => Some(404),
            Self::Conflict => Some(409),
            Self::LimitExceeded | Self::RateLimited => Some(429),
            Self::Internal => Some(500),
            Self::Unavailable => Some(503),
            Self::Unrecognized => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// Every code a server sends, with its wire name and HTTP status.
    const CODES: [(ErrorCode, &str, u16); 9] = [
        (ErrorCode::InvalidRequest, "invalid_request", 400),
        (ErrorCode::Unauthorized, "unauthorized", 401),
        (ErrorCode::NotFound, "not_found", 404),
        (ErrorCode::Conflict, "conflict", 409),
        (ErrorCode::UnsupportedVersion, "unsupported_version", 400),
        (ErrorCode::LimitExceeded, "limit_exceeded", 429),
        (ErrorCode::RateLimited, "rate_limited", 429),
        (ErrorCode::Unavailable, "unavailable", 503),
        (ErrorCode::Internal, "internal", 500),
    ];

    #[test]
    fn codes_cover_every_variant() {
        for (code, ..) in CODES {
            // Exhaustive on purpose: a new code stops this module from
            // compiling until it is added to CODES.
            match code {
                ErrorCode::InvalidRequest
                | ErrorCode::Unauthorized
                | ErrorCode::NotFound
                | ErrorCode::Conflict
                | ErrorCode::UnsupportedVersion
                | ErrorCode::LimitExceeded
                | ErrorCode::RateLimited
                | ErrorCode::Unavailable
                | ErrorCode::Internal
                | ErrorCode::Unrecognized => {}
            }
        }
        let names: HashSet<&str> = CODES.iter().map(|(_, name, _)| *name).collect();
        assert_eq!(names.len(), CODES.len());
    }

    #[test]
    fn every_code_has_one_wire_name_and_one_http_status() {
        for (code, name, status) in CODES {
            let json = format!("\"{name}\"");
            assert_eq!(serde_json::to_string(&code).expect("serialize"), json);
            assert_eq!(
                serde_json::from_str::<ErrorCode>(&json).expect("code"),
                code
            );
            assert_eq!(code.http_status(), Some(status), "{name}");
        }
        assert_eq!(ErrorCode::Unrecognized.http_status(), None);
    }

    #[test]
    fn a_code_this_build_does_not_know_is_unrecognized_and_keeps_the_message() {
        let body: ErrorBody =
            serde_json::from_str(r#"{"code":"quota_exhausted","message":"too many bundles"}"#)
                .expect("error body");
        assert_eq!(
            body,
            ErrorBody {
                code: ErrorCode::Unrecognized,
                message: "too many bundles".to_string(),
                supported: None,
            }
        );
    }

    #[test]
    fn an_unrecognized_code_is_never_sent() {
        assert!(serde_json::to_string(&ErrorCode::Unrecognized).is_err());
        let body = ErrorBody {
            code: ErrorCode::Unrecognized,
            message: "m".to_string(),
            supported: None,
        };
        assert!(serde_json::to_string(&ErrorEnvelope { error: body }).is_err());
    }

    #[test]
    fn the_supported_versions_are_left_out_unless_there_are_some() {
        let body = ErrorBody {
            code: ErrorCode::NotFound,
            message: "no such job".to_string(),
            supported: None,
        };
        let json = r#"{"code":"not_found","message":"no such job"}"#;
        assert_eq!(serde_json::to_string(&body).expect("serialize"), json);
        let with_null = r#"{"code":"not_found","message":"no such job","supported":null}"#;
        assert_eq!(
            serde_json::from_str::<ErrorBody>(with_null).expect("body"),
            body
        );
    }
}
