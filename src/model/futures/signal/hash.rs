//! The name of a signal's payload.

use crate::model::ModelError;
use crate::model::job::sha256_name;
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
