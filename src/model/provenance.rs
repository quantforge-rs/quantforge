//! A job's provenance as the job store keeps it: the server's record of
//! everything a job ran on, as the server's JSON, which a store keeps as
//! text and never reads, under the job and the identity of the job's
//! inputs; the segments the record names, which the store must hold; and
//! the segments the server packed for the job from bars no segment held
//! yet, which the store keeps beside the others. The protocol's
//! `Provenance` gives the JSON its meaning.

use std::{fmt, str::FromStr};

use sha2::{Digest, Sha256};

use super::job::sha256_name;
use super::{JobId, ModelError, SegmentHash, TimestampMs};

const PREFIX: &str = "sha256:";

sha256_name!(
    /// The identity of a job's inputs: the SHA-256 of the inputs as the
    /// server writes them, written `sha256:` and 64 lowercase hex digits.
    /// Jobs given the same inputs have the same identity, whichever store
    /// ran them.
    InputsIdentity,
    InvalidInputsIdentity
);

impl InputsIdentity {
    /// The identity of inputs written as `text`.
    pub fn of(text: &[u8]) -> Self {
        Self::from_digest(Sha256::digest(text).into())
    }
}

/// A segment the server packed for a job, from bars no segment held yet,
/// as a store keeps it beside every other segment: its name, the size of
/// its payload, and its stored bytes, the payload as one zstd frame.
#[derive(Clone, PartialEq, Eq)]
pub struct PackedSegment {
    /// The SHA-256 of its payload.
    pub hash: SegmentHash,
    /// The size of its payload.
    pub payload_len: u64,
    /// What the store keeps: the payload as one zstd frame.
    pub data: Vec<u8>,
}

impl fmt::Debug for PackedSegment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PackedSegment")
            .field("hash", &self.hash)
            .field("payload_len", &self.payload_len)
            .field("stored_bytes", &self.data.len())
            .finish()
    }
}

/// A job's provenance as the server hands it to its store as the job
/// starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewProvenance {
    /// The identity of the job's inputs.
    pub identity: InputsIdentity,
    /// The record, as the server's JSON.
    pub provenance_json: String,
    /// Every segment the record names, in its order: each one the store
    /// holds already, or one of `packed`.
    pub segments: Vec<SegmentHash>,
    /// The segments the server packed for the job, which the store keeps in
    /// the change that records the provenance. One it holds already stays
    /// as it is.
    pub packed: Vec<PackedSegment>,
}

/// A job's provenance as a store keeps it: the record and the identity of
/// the job's inputs, and the store that recorded it and when, which are no
/// part of that identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProvenanceRecord {
    pub job_id: JobId,
    pub identity: InputsIdentity,
    /// The record, as the server's JSON.
    pub provenance_json: String,
    /// The backend of the store that recorded it: `sqlite` or `postgres`.
    pub backend: String,
    /// The schema version of the store that recorded it.
    pub schema_version: u32,
    pub recorded_at_ms: TimestampMs,
}

#[cfg(test)]
mod tests {
    use super::*;

    const EMPTY: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    #[test]
    fn an_identity_is_the_sha256_of_the_inputs_and_reads_back_only_its_written_form() {
        let identity = InputsIdentity::of(b"");
        assert_eq!(identity.to_string(), format!("sha256:{EMPTY}"));
        assert_eq!(
            identity
                .to_string()
                .parse::<InputsIdentity>()
                .expect("read"),
            identity
        );
        assert_eq!(
            format!("{identity:?}"),
            format!("InputsIdentity(sha256:{EMPTY})")
        );
        for refused in [
            EMPTY.to_string(),
            format!("sha256:{}", EMPTY.to_uppercase()),
        ] {
            assert!(
                matches!(refused.parse::<InputsIdentity>(), Err(ModelError::InvalidInputsIdentity(input)) if input == refused),
                "{refused:?}"
            );
        }
    }

    #[test]
    fn a_packed_segment_shows_its_name_and_sizes_and_never_its_bytes() {
        let packed = PackedSegment {
            hash: SegmentHash::from_digest([0xab; 32]),
            payload_len: 137,
            data: vec![7; 64],
        };
        let shown = format!("{packed:?}");
        assert!(
            shown.contains("payload_len: 137") && shown.contains("stored_bytes: 64"),
            "{shown}"
        );
        assert!(!shown.contains("data"), "{shown}");
    }
}
