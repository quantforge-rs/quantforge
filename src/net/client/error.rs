//! Why a request to a QuantForge server did not get its answer.

use thiserror::Error;

use crate::protocol::{ErrorBody, ErrorCode, VersionRange};

/// Why a [`RemoteClient`](super::RemoteClient) call failed. Every variant
/// names the server by its URL, never with the credentials a URL can hold,
/// and none ever holds the client's token.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ClientError {
    /// A server's URL that is not one: see
    /// [`server_url`](super::server_url).
    #[error("invalid server URL: {reason}")]
    InvalidUrl {
        /// What is wrong with it.
        reason: String,
    },

    /// A token that cannot travel in a header: one that is empty or holds
    /// anything but printable ASCII. It is never shown.
    #[error("the token is empty or holds a character other than printable ASCII without spaces")]
    InvalidToken,

    /// No answer came: the server could not be reached, or the connection
    /// broke or timed out before its answer arrived.
    #[error("could not reach the server at {url}: {cause}")]
    Unreachable {
        /// The server, as the client was given it.
        url: String,
        /// What failed, with every cause the transport gave.
        cause: String,
    },

    /// An answer came that a QuantForge server does not give: the URL
    /// names something else, or a proxy answered in its place.
    #[error("the server at {url} {what}; is it a QuantForge server?")]
    NotQuantForge {
        /// The server.
        url: String,
        /// What it answered.
        what: String,
    },

    /// The server speaks no protocol version this build speaks.
    #[error(
        "the server at {url} runs QuantForge {release}, which speaks protocol versions {} to {}, \
         and this build speaks {} to {}; upgrade the older of the two",
        server.min_supported.0,
        server.proto_version.0,
        ours.min_supported.0,
        ours.proto_version.0
    )]
    Incompatible {
        /// The server.
        url: String,
        /// Its release, as it names it.
        release: String,
        /// The versions it speaks.
        server: VersionRange,
        /// The versions this build speaks.
        ours: VersionRange,
    },

    /// The server refused the request, with its error envelope. Its message
    /// is the server's own, word for word.
    #[error("{}", .body.message)]
    Refused {
        /// The HTTP status.
        status: u16,
        /// The envelope's code, message and supported versions.
        body: ErrorBody,
    },

    /// A file the server sent is not the file it listed: another size,
    /// another hash, or bytes the result format does not read.
    #[error("the server at {url} sent {what}")]
    Damaged {
        /// The server.
        url: String,
        /// What arrived, and how it differs from what was listed.
        what: String,
    },
}

impl ClientError {
    /// The code of a refusal, or `None` for any other failure.
    pub fn code(&self) -> Option<ErrorCode> {
        match self {
            Self::Refused { body, .. } => Some(body.code),
            _ => None,
        }
    }
}
