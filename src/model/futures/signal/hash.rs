//! The name of a signal's payload.

use crate::model::ModelError;
use crate::model::job::sha256_name;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error};
use sha2::{Digest, Sha256};
use std::{fmt, str::FromStr};

const PREFIX: &str = "sha256:";

sha256_name!(
    /// The hash of a signal's canonical payload: the SHA-256 of its bytes,
    /// written `sha256:` and 64 lowercase hex digits, as the payload format
    /// defines it. A store keeps it beside the signal's ID; two deliveries
    /// with one ID and one hash are one signal.
    SignalHash,
    InvalidSignalHash
);

impl SignalHash {
    /// The name of `payload`, a signal's canonical payload.
    pub fn of(payload: &[u8]) -> Self {
        Self::from_digest(Sha256::digest(payload).into())
    }
}

/// A stored hash is its written form, read back as strictly as it parses.
impl Serialize for SignalHash {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for SignalHash {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(D::Error::custom)
    }
}
