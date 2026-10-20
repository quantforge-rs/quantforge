//! The protocol version: which versions a build speaks, and how two peers
//! settle on one.

use serde::{Deserialize, Serialize};

use super::ProtocolError;

/// A protocol version. Versions count up from 1, one for each change a peer
/// of the version before could not cope with.
///
/// It has two spellings, and neither reads as the other: a JSON number in a
/// message, and `v` with the number as the first segment of an HTTP path
/// ([`path_segment`](Self::path_segment)). There is no `Display`, so that
/// neither can be written where the other belongs.
///
/// ```
/// use quantforge::protocol::ProtoVersion;
///
/// let version = ProtoVersion(1);
/// assert_eq!(serde_json::to_string(&version)?, "1");
/// assert_eq!(version.path_segment(), "v1");
/// assert_eq!(ProtoVersion::from_path_segment("v1")?, version);
///
/// assert!(serde_json::from_str::<ProtoVersion>(r#""v1""#).is_err());
/// assert!(ProtoVersion::from_path_segment("1").is_err());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProtoVersion(pub u32);

/// The newest protocol version this build speaks.
pub const PROTO_VERSION: ProtoVersion = ProtoVersion(1);

/// The oldest protocol version this build still speaks.
pub const MIN_SUPPORTED: ProtoVersion = ProtoVersion(1);

// A build always speaks the version it is.
const _: () = assert!(1 <= MIN_SUPPORTED.0 && MIN_SUPPORTED.0 <= PROTO_VERSION.0);

impl ProtoVersion {
    /// The version as the first segment of an HTTP path, such as `v1`.
    pub fn path_segment(self) -> String {
        format!("v{}", self.0)
    }

    /// Reads the first segment of an HTTP path, as
    /// [`path_segment`](Self::path_segment) writes it: `v` and a version
    /// number without leading zeros, and nothing else.
    pub fn from_path_segment(segment: &str) -> Result<Self, ProtocolError> {
        let invalid = || ProtocolError::InvalidPathVersion {
            input: segment.to_string(),
        };
        let digits = segment.strip_prefix('v').ok_or_else(invalid)?;
        let canonical = !digits.is_empty()
            && !digits.starts_with('0')
            && digits.bytes().all(|byte| byte.is_ascii_digit());
        if !canonical {
            return Err(invalid());
        }
        digits.parse().map(Self).map_err(|_| invalid())
    }
}

/// The versions one side speaks: every version from `min_supported` to
/// `proto_version`, both included. A [`Hello`](super::stream::Hello)
/// carries the client's as two fields of its own, and an
/// [`ErrorBody`](super::ErrorBody) refusing a version carries the server's.
/// Its shape is the same in every version.
///
/// ```
/// use quantforge::protocol::{ProtoVersion, VersionRange};
///
/// let range = VersionRange {
///     min_supported: ProtoVersion(1),
///     proto_version: ProtoVersion(2),
/// };
/// let json = r#"{"min_supported":1,"proto_version":2}"#;
/// assert_eq!(serde_json::to_string(&range)?, json);
/// assert_eq!(serde_json::from_str::<VersionRange>(json)?, range);
/// # Ok::<(), serde_json::Error>(())
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionRange {
    /// The oldest version spoken.
    pub min_supported: ProtoVersion,
    /// The newest version spoken.
    pub proto_version: ProtoVersion,
}

impl VersionRange {
    /// The versions this build speaks.
    pub const SUPPORTED: Self = Self {
        min_supported: MIN_SUPPORTED,
        proto_version: PROTO_VERSION,
    };

    /// Whether this range includes `version`: what a server checks before it
    /// answers a request whose path names `version`.
    pub fn supports(self, version: ProtoVersion) -> bool {
        self.holds_a_version() && self.min_supported <= version && version <= self.proto_version
    }

    /// The version a session between this side and `peer` runs at: the
    /// newest version both speak. It fails when no version is shared, or
    /// when either range holds no version at all.
    ///
    /// ```
    /// use quantforge::protocol::{ProtoVersion, ProtocolError, VersionRange};
    ///
    /// let range = |min_supported, proto_version| VersionRange {
    ///     min_supported: ProtoVersion(min_supported),
    ///     proto_version: ProtoVersion(proto_version),
    /// };
    /// let server = range(2, 4);
    /// assert_eq!(server.negotiate(range(1, 3)), Ok(ProtoVersion(3)));
    /// assert_eq!(server.negotiate(range(3, 7)), Ok(ProtoVersion(4)));
    /// assert_eq!(
    ///     server.negotiate(range(1, 1)),
    ///     Err(ProtocolError::PeerTooOld { peer_newest: 1, oldest_supported: 2 })
    /// );
    /// ```
    pub fn negotiate(self, peer: VersionRange) -> Result<ProtoVersion, ProtocolError> {
        for range in [self, peer] {
            if !range.holds_a_version() {
                return Err(ProtocolError::InvalidVersionRange {
                    min_supported: range.min_supported.0,
                    proto_version: range.proto_version.0,
                });
            }
        }
        if peer.proto_version < self.min_supported {
            return Err(ProtocolError::PeerTooOld {
                peer_newest: peer.proto_version.0,
                oldest_supported: self.min_supported.0,
            });
        }
        if peer.min_supported > self.proto_version {
            return Err(ProtocolError::PeerTooNew {
                peer_oldest: peer.min_supported.0,
                newest_supported: self.proto_version.0,
            });
        }
        Ok(self.proto_version.min(peer.proto_version))
    }

    fn holds_a_version(self) -> bool {
        ProtoVersion(1) <= self.min_supported && self.min_supported <= self.proto_version
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(min_supported: u32, proto_version: u32) -> VersionRange {
        VersionRange {
            min_supported: ProtoVersion(min_supported),
            proto_version: ProtoVersion(proto_version),
        }
    }

    #[test]
    fn peers_with_the_same_range_run_at_its_newest_version() {
        assert_eq!(range(1, 3).negotiate(range(1, 3)), Ok(ProtoVersion(3)));
        assert_eq!(range(2, 2).negotiate(range(2, 2)), Ok(ProtoVersion(2)));
    }

    #[test]
    fn an_older_peer_that_this_side_still_speaks_runs_at_its_own_newest_version() {
        assert_eq!(range(1, 5).negotiate(range(1, 3)), Ok(ProtoVersion(3)));
        assert_eq!(range(2, 5).negotiate(range(1, 2)), Ok(ProtoVersion(2)));
    }

    #[test]
    fn a_newer_peer_that_still_speaks_this_side_runs_at_this_sides_newest_version() {
        assert_eq!(range(1, 3).negotiate(range(2, 6)), Ok(ProtoVersion(3)));
        assert_eq!(range(1, 3).negotiate(range(3, 3)), Ok(ProtoVersion(3)));
    }

    #[test]
    fn a_peer_whose_newest_version_is_below_min_supported_is_too_old() {
        assert_eq!(
            range(3, 5).negotiate(range(1, 2)),
            Err(ProtocolError::PeerTooOld {
                peer_newest: 2,
                oldest_supported: 3
            })
        );
    }

    #[test]
    fn a_peer_whose_oldest_version_is_above_proto_version_is_too_new() {
        assert_eq!(
            range(1, 2).negotiate(range(3, 4)),
            Err(ProtocolError::PeerTooNew {
                peer_oldest: 3,
                newest_supported: 2
            })
        );
    }

    #[test]
    fn a_range_holding_no_version_is_refused_on_either_side() {
        for invalid in [range(0, 1), range(0, 0), range(3, 2)] {
            let refusal = Err(ProtocolError::InvalidVersionRange {
                min_supported: invalid.min_supported.0,
                proto_version: invalid.proto_version.0,
            });
            assert_eq!(
                invalid.negotiate(range(1, 3)),
                refusal,
                "{invalid:?} on this side"
            );
            assert_eq!(
                range(1, 3).negotiate(invalid),
                refusal,
                "{invalid:?} as the peer"
            );
        }
    }

    // Every pair of ranges over versions 0 to 4, invalid ones included, against
    // a search for the newest version both hold.
    #[test]
    fn negotiation_agrees_with_a_search_of_every_version() {
        let ranges: Vec<VersionRange> = (0..=4)
            .flat_map(|min| (0..=4).map(move |max| range(min, max)))
            .collect();
        let valid = |r: VersionRange| r.min_supported.0 >= 1 && r.min_supported <= r.proto_version;
        let holds = |r: VersionRange, v: u32| r.min_supported.0 <= v && v <= r.proto_version.0;
        for &ours in &ranges {
            for &theirs in &ranges {
                let outcome = ours.negotiate(theirs);
                let case = format!("{ours:?} with {theirs:?}");
                if !valid(ours) || !valid(theirs) {
                    assert!(
                        matches!(outcome, Err(ProtocolError::InvalidVersionRange { .. })),
                        "{case}: {outcome:?}"
                    );
                    continue;
                }
                let shared = (1..=4).rev().find(|&v| holds(ours, v) && holds(theirs, v));
                match shared {
                    Some(version) => assert_eq!(outcome, Ok(ProtoVersion(version)), "{case}"),
                    None if theirs.proto_version < ours.min_supported => assert_eq!(
                        outcome,
                        Err(ProtocolError::PeerTooOld {
                            peer_newest: theirs.proto_version.0,
                            oldest_supported: ours.min_supported.0,
                        }),
                        "{case}"
                    ),
                    None => assert_eq!(
                        outcome,
                        Err(ProtocolError::PeerTooNew {
                            peer_oldest: theirs.min_supported.0,
                            newest_supported: ours.proto_version.0,
                        }),
                        "{case}"
                    ),
                }
                // Both sides of a session must settle on the same version.
                assert_eq!(outcome.is_ok(), theirs.negotiate(ours).is_ok(), "{case}");
                if let Ok(version) = outcome {
                    assert_eq!(theirs.negotiate(ours), Ok(version), "{case}");
                }
            }
        }
    }

    #[test]
    fn a_range_supports_exactly_the_versions_it_holds() {
        let server = range(2, 4);
        let supported: Vec<u32> = (0..=6)
            .filter(|&v| server.supports(ProtoVersion(v)))
            .collect();
        assert_eq!(supported, [2, 3, 4]);
        assert!(!range(3, 2).supports(ProtoVersion(2)));
        assert!(!range(0, 2).supports(ProtoVersion(0)));
    }

    #[test]
    fn this_build_speaks_its_own_versions() {
        assert!(VersionRange::SUPPORTED.supports(PROTO_VERSION));
        assert!(VersionRange::SUPPORTED.supports(MIN_SUPPORTED));
        assert_eq!(
            VersionRange::SUPPORTED.negotiate(VersionRange::SUPPORTED),
            Ok(PROTO_VERSION)
        );
    }

    #[test]
    fn a_path_segment_is_v_and_a_version_number_and_nothing_else() {
        for (segment, version) in [("v1", 1), ("v2", 2), ("v42", 42), ("v4294967295", u32::MAX)] {
            assert_eq!(
                ProtoVersion::from_path_segment(segment),
                Ok(ProtoVersion(version))
            );
            assert_eq!(ProtoVersion(version).path_segment(), segment);
        }
        for segment in [
            "",
            "v",
            "1",
            "v0",
            "v01",
            "V1",
            "vv1",
            "v-1",
            "v+1",
            " v1",
            "v1 ",
            "v1/",
            "/v1",
            "v1.0",
            "v1a",
            "v١",
            "v4294967296",
        ] {
            assert_eq!(
                ProtoVersion::from_path_segment(segment),
                Err(ProtocolError::InvalidPathVersion {
                    input: segment.to_string()
                }),
                "{segment:?}"
            );
        }
    }

    // The JSON number and the path segment are two channels; neither may read
    // as the other.
    #[test]
    fn a_version_in_a_message_is_a_json_number_and_nothing_else() {
        assert_eq!(
            serde_json::from_str::<ProtoVersion>("3").expect("version"),
            ProtoVersion(3)
        );
        assert_eq!(
            serde_json::to_string(&ProtoVersion(3)).expect("serialize"),
            "3"
        );
        for json in [r#""v3""#, r#""3""#, "3.0", "-3", "null", "4294967296"] {
            assert!(
                serde_json::from_str::<ProtoVersion>(json).is_err(),
                "{json} must not read as a version"
            );
        }
    }
}
