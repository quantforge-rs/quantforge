//! What a server says about itself before any version is agreed.

use serde::{Deserialize, Serialize};

use super::{ProtoVersion, VersionRange};

/// The answer to `GET /version`: the server's release and the protocol
/// versions it speaks. The path names no version and the shape is the same in
/// every version, so a client of any version reads it first, then sends its
/// requests under the newest version both sides speak
/// ([`VersionRange::negotiate`]).
///
/// ```
/// use quantforge::protocol::{ProtoVersion, ServerVersion, VersionRange};
///
/// let json = r#"{"version":"0.3.0","proto_version":1,"min_supported":1}"#;
/// let server: ServerVersion = serde_json::from_str(json)?;
/// assert_eq!(server.version, "0.3.0");
/// let version = VersionRange::SUPPORTED.negotiate(server.versions());
/// assert_eq!(version.map(ProtoVersion::path_segment), Ok("v1".to_string()));
/// assert_eq!(serde_json::to_string(&server)?, json);
/// # Ok::<(), serde_json::Error>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerVersion {
    /// The server's QuantForge release, such as `0.3.0`, for people to read.
    /// A client chooses what to send by the protocol versions, never by this.
    pub version: String,
    /// The newest protocol version the server speaks.
    pub proto_version: ProtoVersion,
    /// The oldest protocol version the server speaks.
    pub min_supported: ProtoVersion,
}

impl ServerVersion {
    /// The versions the server speaks, to
    /// [`negotiate`](VersionRange::negotiate) with.
    pub fn versions(&self) -> VersionRange {
        VersionRange {
            min_supported: self.min_supported,
            proto_version: self.proto_version,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_versions_are_the_range_the_server_states() {
        let server: ServerVersion =
            serde_json::from_str(r#"{"version":"0.4.1","proto_version":3,"min_supported":2}"#)
                .expect("server version");
        assert_eq!(
            server.versions(),
            VersionRange {
                min_supported: ProtoVersion(2),
                proto_version: ProtoVersion(3),
            }
        );
    }

    #[test]
    fn fields_a_later_release_adds_are_ignored() {
        let json = r#"{"version":"0.9.0","proto_version":4,"min_supported":2,"runtime":{"python":"3.13"}}"#;
        assert_eq!(
            serde_json::from_str::<ServerVersion>(json).expect("server version"),
            ServerVersion {
                version: "0.9.0".to_string(),
                proto_version: ProtoVersion(4),
                min_supported: ProtoVersion(2),
            }
        );
    }

    #[test]
    fn every_field_is_required() {
        for json in [
            r#"{"proto_version":1,"min_supported":1}"#,
            r#"{"version":"0.3.0","min_supported":1}"#,
            r#"{"version":"0.3.0","proto_version":1}"#,
        ] {
            assert!(
                serde_json::from_str::<ServerVersion>(json).is_err(),
                "{json}"
            );
        }
    }
}
