//! The frames that open a session.

use std::fmt;

use serde::{Deserialize, Serialize};

use super::super::{ProtoVersion, ProtocolError, VersionRange, opaque_id};

/// The client's first frame: the versions it speaks, its token, and the
/// optional features it supports. Its `type`, `proto_version`, and
/// `min_supported` are the same in every version.
///
/// ```
/// use quantforge::protocol::ProtoVersion;
/// use quantforge::protocol::stream::{AuthToken, ClientFrame, Hello};
///
/// let hello = ClientFrame::Hello(Hello {
///     proto_version: ProtoVersion(1),
///     min_supported: ProtoVersion(1),
///     token: AuthToken::new("example-token"),
///     capabilities: Vec::new(),
/// });
/// let json = r#"{"type":"hello","proto_version":1,"min_supported":1,"token":"example-token","capabilities":[]}"#;
/// assert_eq!(serde_json::to_string(&hello)?, json);
/// # Ok::<(), serde_json::Error>(())
/// ```
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Hello {
    /// The newest version the client speaks.
    pub proto_version: ProtoVersion,
    /// The oldest version the client speaks.
    pub min_supported: ProtoVersion,
    /// The token the server issued to the client.
    pub token: AuthToken,
    /// The optional features the client supports, by name. Version 1
    /// defines none, and a server ignores names it does not know.
    pub capabilities: Vec<String>,
}

impl Hello {
    /// The versions the client speaks, to
    /// [`negotiate`](VersionRange::negotiate) with.
    pub fn versions(&self) -> VersionRange {
        VersionRange {
            min_supported: self.min_supported,
            proto_version: self.proto_version,
        }
    }
}

/// A bearer token, sent as a plain JSON string. It is never printed: its
/// `Debug` hides it, and it has no `Display`. Nor does it have `==`, because
/// a server checks a token with a comparison that takes as long whatever the
/// token, and comparing strings does not.
#[derive(Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AuthToken(String);

impl AuthToken {
    /// Wraps a token.
    pub fn new(token: impl Into<String>) -> Self {
        Self(token.into())
    }

    /// The token itself, for the server to check. Never log it.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for AuthToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AuthToken(<redacted>)")
    }
}

/// The server's answer to a [`Hello`] it accepts: the version the session
/// runs at, the session's id, the server's clock, and the session's limits.
///
/// ```
/// use quantforge::protocol::ProtoVersion;
/// use quantforge::protocol::stream::ServerFrame;
///
/// let json = r#"{"type":"welcome","negotiated_version":1,"session_id":"s-91c2","server_time_ms":1792454400123,"limits":{"max_subscriptions":16,"max_resume_bars":1000,"ping_interval_ms":15000,"idle_timeout_ms":45000}}"#;
/// let ServerFrame::Welcome(welcome) = serde_json::from_str(json)? else {
///     panic!("a welcome frame");
/// };
/// assert_eq!(welcome.negotiated_version, ProtoVersion(1));
/// assert_eq!(welcome.limits.max_subscriptions, 16);
/// # Ok::<(), serde_json::Error>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Welcome {
    /// The newest version both sides speak, which every later frame of the
    /// session follows. Its name and meaning are the same in every version.
    pub negotiated_version: ProtoVersion,
    /// The server's name for the session.
    pub session_id: SessionId,
    /// The server's clock as it sent the frame, for the client to compare
    /// its own clock with.
    pub server_time_ms: i64,
    /// What the session may do.
    pub limits: SessionLimits,
}

/// The server's name for a session: 1 to 64 ASCII letters, digits, `-`, or
/// `_`. Only the server makes one up.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String")]
pub struct SessionId(String);

impl SessionId {
    /// Checks and wraps an id.
    pub fn new(id: impl Into<String>) -> Result<Self, ProtocolError> {
        opaque_id("session id", id.into()).map(Self)
    }

    /// The id's text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for SessionId {
    type Error = ProtocolError;

    fn try_from(id: String) -> Result<Self, Self::Error> {
        Self::new(id)
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// What a session may do. These limits are provisional until the stream's
/// server is built.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionLimits {
    /// The most markets the session may subscribe to at once. The server
    /// refuses a `subscribe` beyond it as `limit_exceeded`.
    pub max_subscriptions: u32,
    /// The most stored bars one `subscribe` replays. Bars before those are
    /// reported in a [`GapNotice`](super::GapNotice) as `beyond_resume`.
    pub max_resume_bars: u32,
    /// How often the server pings a session it has nothing else to send to.
    pub ping_interval_ms: u32,
    /// How long the server waits for any frame from the client before it
    /// closes the session.
    pub idle_timeout_ms: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    const HELLO: &str =
        r#"{"proto_version":3,"min_supported":2,"token":"test-token","capabilities":[]}"#;

    #[test]
    fn debug_never_shows_the_token() {
        let hello: Hello = serde_json::from_str(HELLO).expect("hello");
        let printed = format!("{hello:?} {:?}", hello.token);
        assert!(!printed.contains("test-token"), "{printed}");
        assert!(printed.contains("AuthToken(<redacted>)"), "{printed}");
        assert_eq!(hello.token.expose(), "test-token");
    }

    #[test]
    fn a_hello_states_the_range_to_negotiate_with() {
        let hello: Hello = serde_json::from_str(HELLO).expect("hello");
        assert_eq!(
            hello.versions(),
            VersionRange {
                min_supported: ProtoVersion(2),
                proto_version: ProtoVersion(3),
            }
        );
        assert_eq!(serde_json::to_string(&hello).expect("serialize"), HELLO);
    }

    #[test]
    fn a_hello_needs_every_field() {
        for field in [
            r#""proto_version":3,"#,
            r#""min_supported":2,"#,
            r#""token":"test-token","#,
            r#","capabilities":[]"#,
        ] {
            let json = HELLO.replace(field, "");
            assert_ne!(json, HELLO, "{field} is in the fixture");
            assert!(
                serde_json::from_str::<Hello>(&json).is_err(),
                "a hello without {field} must be refused"
            );
        }
    }

    #[test]
    fn a_session_id_is_read_through_the_same_check_as_new() {
        let id: SessionId = serde_json::from_str(r#""s-91c2""#).expect("session id");
        assert_eq!(id, SessionId::new("s-91c2").expect("session id"));
        assert_eq!(id.as_str(), "s-91c2");
        let error = serde_json::from_str::<SessionId>(r#""s 1""#).expect_err("refused");
        assert!(error.to_string().contains("invalid session id"), "{error}");
    }
}
