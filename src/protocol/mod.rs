//! The wire format between QuantForge instances: a server that ingests and
//! stores market data and runs backtests, and the clients that use it.
//! These types are the whole contract, and these pages are its
//! specification, written for the authors of clients and servers in any
//! language. Every JSON example in them is a doctest, so each one is exactly
//! what the types send and accept.
//!
//! The module uses nothing else from QuantForge, so that it can become the
//! `quantforge-protocol` crate; `tests/protocol_boundary.rs` holds it to
//! that.
//!
//! # Surfaces
//!
//! - [`jobs`]: remote backtests of a Python strategy bundle, over HTTP.
//! - [`bundle`]: the strategy bundle a job runs, uploaded as its canonical
//!   tar and named by its code hash; its rules are the [bundle spec].
//! - [`segments`]: stored candles, as immutable segments named by their
//!   content hash, over HTTP; a segment's bytes are the [segment spec].
//! - [`stream`]: bars as they close, over a WebSocket.
//!
//! [bundle spec]: <https://github.com/quantforge-rs/quantforge/blob/main/docs/bundle-spec.md>
//! [segment spec]: <https://github.com/quantforge-rs/quantforge/blob/main/docs/segment-spec.md>
//!
//! # Encoding
//!
//! - Every message is one JSON object in UTF-8: an HTTP body, or a WebSocket
//!   text frame. A list is never sent bare but inside an object, such as
//!   [`segments::SegmentIndex`], so that fields can join it later. Two
//!   bodies are not JSON: a bundle upload, which is its canonical tar, as
//!   [`bundle::BUNDLE_MEDIA_TYPE`], and the plain-text `ok` of
//!   `GET /healthz`.
//! - Field names and enum values are `snake_case`. In Rust, the serde name
//!   of each field and variant here is its wire name.
//! - Every integer is below 2^53 in magnitude, so JavaScript reads it
//!   exactly.
//! - A time is a count of milliseconds since the Unix epoch, UTC, in a field
//!   whose name ends in `_ms`. A window of bars includes both its bounds,
//!   which are bar open times.
//! - A price, quantity, or amount is an [`ExactDecimal`]: a JSON string,
//!   never a number.
//! - Stored content is named by its [`ContentHash`].
//! - A market is four strings, each in its one canonical form. `exchange` is
//!   the name QuantForge registers the exchange under, such as `binance`.
//!   `category` is the product family it sells the symbol in: `spot`,
//!   `linear` or `inverse`; the two together name the product line, and a
//!   candle strategy's market is always `spot`.
//!   `symbol` is the exchange's symbol in uppercase, such as `BTCUSDT`.
//!   `interval` is one of QuantForge's bar intervals, whatever the exchange
//!   calls them: `1s`, `1m`, `3m`, `5m`, `15m`, `30m`, `1h`, `2h`, `4h`,
//!   `6h`, `8h`, `12h`, `1d`, `3d`, or `1w`. A server refuses any other
//!   spelling as [`ErrorCode::InvalidRequest`] instead of correcting it.
//! - An optional field is left out when it has no value. A receiver reads a
//!   missing field and a `null` alike.
//!
//! # Versions
//!
//! One protocol version covers all three surfaces. A build speaks every
//! version from [`MIN_SUPPORTED`] to [`PROTO_VERSION`], its
//! [`VersionRange::SUPPORTED`].
//!
//! - An HTTP request names its version as the first segment of its path, as
//!   in `/v1/jobs` ([`ProtoVersion::path_segment`]). A server that does not
//!   speak that version answers [`ErrorCode::UnsupportedVersion`], with the
//!   versions it does speak in [`ErrorBody::supported`].
//! - Two HTTP paths name no version, and every version answers them alike.
//!   `GET /version` answers a [`ServerVersion`]: the server's release and
//!   the versions it speaks, so that a client can choose the version for its
//!   paths before its first request under one. `GET /healthz` answers `ok`
//!   while the server runs, for liveness probes.
//! - The stream's URL names no version. The client's [`stream::Hello`] states
//!   the versions it speaks, and the server's [`stream::Welcome`] names the
//!   newest one both sides speak ([`VersionRange::negotiate`]), or an `error`
//!   frame refuses the session.
//!
//! A few shapes are the same in every version, so that peers of any two
//! versions can still tell each other where they stand: the `hello` frame's
//! `type`, `proto_version`, and `min_supported`; the `welcome` frame's
//! `negotiated_version`; the `error` frame and [`ErrorBody`];
//! [`ServerVersion`]; and [`VersionRange`].
//!
//! # Compatibility
//!
//! Within a version, a sender may add an optional field, a frame type, an
//! enum value, or a capability. A receiver ignores fields it does not know,
//! and reads a frame type or enum value it does not know as that enum's
//! `Unrecognized` variant. `Unrecognized` is never sent: serializing it fails.
//! Any other change needs a new version: a field removed, renamed, or
//! retyped, a new required field, or a field whose meaning changes. Unlike
//! the run state QuantForge stores, which is never read across schema
//! versions, the wire grows by optional fields. A bundle's manifest is the
//! exception: it refuses keys it does not know, and grows only by a new
//! bundle format.
//!
//! Version 1 may still change until QuantForge 0.3.0 is released.
//!
//! # Decoding in Rust
//!
//! A frame's fields are buffered while its `type` is found, and with
//! `serde_json`'s `arbitrary_precision` feature a buffered integer no longer
//! decodes. A consumer must not enable that feature.
//!
//! ```
//! use quantforge::protocol::stream::ServerFrame;
//!
//! let frame: ServerFrame = serde_json::from_str(r#"{"type":"ping","nonce":7}"#)?;
//! assert!(matches!(frame, ServerFrame::Ping(ping) if ping.nonce == 7));
//! # Ok::<(), serde_json::Error>(())
//! ```

// Repeated from the crate root so that the module keeps them on its own.
#![forbid(unsafe_code)]
#![deny(missing_docs, missing_debug_implementations)]

pub mod bundle;
mod decimal;
mod discovery;
mod errors;
mod hash;
pub mod jobs;
pub mod segments;
pub mod stream;
mod version;

use thiserror::Error;

pub use decimal::ExactDecimal;
pub use discovery::ServerVersion;
pub use errors::{ErrorBody, ErrorCode, ErrorEnvelope};
pub use hash::ContentHash;
pub use version::{MIN_SUPPORTED, PROTO_VERSION, ProtoVersion, VersionRange};

/// Why a value was refused, or why two peers share no version.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum ProtocolError {
    /// A decimal that is not in its exact form, or that a decimal cannot hold
    /// without rounding.
    #[error("invalid decimal {input:?}: {reason}")]
    InvalidDecimal {
        /// The text that was refused.
        input: String,
        /// What is wrong with it.
        reason: &'static str,
    },

    /// A content hash that is not `sha256:` and 64 lowercase hex digits.
    #[error("invalid content hash {input:?}: expected sha256: and 64 lowercase hex digits")]
    InvalidContentHash {
        /// The text that was refused.
        input: String,
    },

    /// An id that is not 1 to 64 letters, digits, `-`, or `_`.
    #[error("invalid {kind} {input:?}: expected 1 to 64 ASCII letters, digits, '-', or '_'")]
    InvalidId {
        /// Which id it was meant to be, such as `job id`.
        kind: &'static str,
        /// The text that was refused.
        input: String,
    },

    /// An HTTP path segment that is not `v` and a version number.
    #[error("invalid version path segment {input:?}: expected v and a version number, such as v1")]
    InvalidPathVersion {
        /// The segment that was refused.
        input: String,
    },

    /// A version range that holds no version: one that starts below 1, or
    /// whose `min_supported` is above its `proto_version`.
    #[error(
        "invalid protocol version range {min_supported} to {proto_version}: versions start at 1 \
         and min_supported may not exceed proto_version"
    )]
    InvalidVersionRange {
        /// The range's oldest version.
        min_supported: u32,
        /// The range's newest version.
        proto_version: u32,
    },

    /// An engine version that is not `MAJOR.MINOR.PATCH`.
    #[error("invalid engine version {input:?}: expected MAJOR.MINOR.PATCH, such as \"0.3.0\"")]
    InvalidEngineVersion {
        /// The text that was refused.
        input: String,
    },

    /// An engine requirement that is not comparators on engine versions.
    #[error(
        "invalid engine requirement {input:?}: {reason}; write comparators on MAJOR.MINOR.PATCH \
         versions, such as \">=0.3.0, <0.4.0\""
    )]
    InvalidEngineRequirement {
        /// The text that was refused.
        input: String,
        /// What is wrong with it.
        reason: String,
    },

    /// A Python version that is not `MAJOR.MINOR`.
    #[error("invalid python version {input:?}: expected MAJOR.MINOR, such as \"3.12\"")]
    InvalidPythonVersion {
        /// The text that was refused.
        input: String,
    },

    /// A package name that is not in its normalized form.
    #[error("invalid package name {input:?}: {reason}")]
    InvalidPackageName {
        /// The text that was refused.
        input: String,
        /// What is wrong with it, and the normalized spelling when there is
        /// one.
        reason: String,
    },

    /// A package version that is not in its canonical form.
    #[error(
        "invalid package version {input:?}: expected a canonical version such as \"2.1.3\", \
         \"1.0rc1\", or \"2.0.post1\""
    )]
    InvalidPackageVersion {
        /// The text that was refused.
        input: String,
    },

    /// A bundle entry that does not name an importable module and a class.
    #[error("invalid entry {input:?}: {reason}")]
    InvalidEntry {
        /// The text that was refused.
        input: String,
        /// What is wrong with it.
        reason: String,
    },

    /// The peer's newest version is older than the oldest this side speaks.
    #[error(
        "the peer speaks protocol versions up to {peer_newest}, older than the oldest this side \
         speaks, {oldest_supported}"
    )]
    PeerTooOld {
        /// The newest version the peer speaks.
        peer_newest: u32,
        /// The oldest version this side speaks.
        oldest_supported: u32,
    },

    /// The peer's oldest version is newer than the newest this side speaks.
    #[error(
        "the peer speaks protocol versions from {peer_oldest}, newer than the newest this side \
         speaks, {newest_supported}"
    )]
    PeerTooNew {
        /// The oldest version the peer speaks.
        peer_oldest: u32,
        /// The newest version this side speaks.
        newest_supported: u32,
    },
}

/// Checks an id a server assigns, such as a job's: 1 to 64 ASCII letters,
/// digits, `-`, or `_`, so that it can sit in a URL path as it is.
fn opaque_id(kind: &'static str, id: String) -> Result<String, ProtocolError> {
    let allowed = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_';
    if (1..=64).contains(&id.len()) && id.bytes().all(allowed) {
        Ok(id)
    } else {
        Err(ProtocolError::InvalidId { kind, input: id })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_id_is_one_to_sixty_four_url_safe_characters() {
        let longest = "a".repeat(64);
        for id in [
            "j",
            "job-1",
            "A_z-09",
            "550e8400-e29b-41d4-a716-446655440000",
            &longest,
        ] {
            assert_eq!(opaque_id("job id", id.to_string()), Ok(id.to_string()));
        }
        let too_long = "a".repeat(65);
        for id in ["", "a b", "a/b", "a.b", "a%20b", "jöb", "a\n", &too_long] {
            assert_eq!(
                opaque_id("job id", id.to_string()),
                Err(ProtocolError::InvalidId {
                    kind: "job id",
                    input: id.to_string()
                }),
                "{id:?}"
            );
        }
    }
}
