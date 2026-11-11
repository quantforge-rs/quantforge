//! Remote backtests as the job store keeps them: a job's name, where it
//! stands, the bundle it runs, and its record. What was asked, what a job
//! produced and why it failed are the server's JSON, which a store keeps as
//! text and never reads: the protocol, not the store, gives them meaning.

use super::identity::text_id;
use super::{ModelError, TimestampMs};
use std::{fmt, str::FromStr};

/// Whether `text` is exactly 32 lowercase hex digits.
fn hex32(text: &str) -> bool {
    text.len() == 32 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

text_id!(
    /// A job's name: a random UUID without hyphens, which only the server
    /// makes up.
    JobId,
    "job id",
    "32 lowercase hex digits",
    hex32
);

impl JobId {
    /// A fresh random job ID.
    pub fn generate() -> Self {
        Self(uuid::Uuid::new_v4().simple().to_string())
    }
}

const PREFIX: &str = "sha256:";

/// A name that is the SHA-256 of what it names, written `sha256:` and 64
/// lowercase hex digits, the protocol's content hash.
macro_rules! sha256_name {
    ($(#[$doc:meta])* $name:ident, $invalid:ident) => {
        $(#[$doc])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name([u8; 32]);

        impl $name {
            /// The name of bytes whose SHA-256 digest is `digest`.
            pub const fn from_digest(digest: [u8; 32]) -> Self {
                Self(digest)
            }

            /// The SHA-256 digest.
            pub const fn digest(&self) -> &[u8; 32] {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(PREFIX)?;
                self.0.iter().try_for_each(|byte| write!(f, "{byte:02x}"))
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!(stringify!($name), "({})"), self)
            }
        }

        /// Reads the written form back; uppercase hex is refused, so that
        /// each name has one spelling.
        impl FromStr for $name {
            type Err = ModelError;

            fn from_str(text: &str) -> Result<Self, Self::Err> {
                let invalid = || ModelError::$invalid(text.to_string());
                let hex = text.strip_prefix(PREFIX).ok_or_else(invalid)?;
                let lowercase = |b: u8| matches!(b, b'0'..=b'9' | b'a'..=b'f');
                if !(hex.len() == 64 && hex.bytes().all(lowercase)) {
                    return Err(invalid());
                }
                let mut digest = [0; 32];
                hex::decode_to_slice(hex, &mut digest).map_err(|_| invalid())?;
                Ok(Self(digest))
            }
        }
    };
}

sha256_name!(
    /// A bundle's name: the SHA-256 of its canonical tar, written `sha256:`
    /// and 64 lowercase hex digits, the protocol's code hash.
    BundleHash,
    InvalidBundleHash
);

sha256_name!(
    /// A result's name: the SHA-256 of the file a client downloads, written
    /// `sha256:` and 64 lowercase hex digits, as the job's results list it.
    ResultHash,
    InvalidResultHash
);

/// Where a job stands. A job is queued, then running, then succeeded,
/// failed or cancelled; it can also be cancelled before it starts, and the
/// last three are final.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum JobPhase {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

impl JobPhase {
    /// Every phase, in the order a job goes through them.
    pub const ALL: [Self; 5] = [
        Self::Queued,
        Self::Running,
        Self::Succeeded,
        Self::Failed,
        Self::Cancelled,
    ];

    /// The phase as a store writes it, and the wire names it: `queued`,
    /// `running`, `succeeded`, `failed` or `cancelled`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    /// Whether the job has ended, so that nothing changes it again.
    pub const fn is_final(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }
}

impl fmt::Display for JobPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for JobPhase {
    type Err = ModelError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|phase| phase.as_str() == text)
            .ok_or_else(|| ModelError::InvalidIdentifier {
                what: "job phase",
                value: text.to_string(),
                rule: "queued, running, succeeded, failed or cancelled",
            })
    }
}

/// A job the server accepts: its name, the bundle it runs, the submission
/// as the server's JSON, and when it was accepted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewJob {
    pub job_id: JobId,
    pub bundle: BundleHash,
    pub request_json: String,
    pub submitted_at_ms: TimestampMs,
}

/// Everything a store keeps of a job.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobRecord {
    pub job_id: JobId,
    pub phase: JobPhase,
    pub bundle: BundleHash,
    /// The submission as the server accepted it, as its JSON.
    pub request_json: String,
    pub submitted_at_ms: TimestampMs,
    /// When a worker started it; absent for a job that never started.
    pub started_at_ms: Option<TimestampMs>,
    /// When it ended; absent until it has.
    pub finished_at_ms: Option<TimestampMs>,
    /// What a succeeded job produced, as the server's JSON.
    pub results_json: Option<String>,
    /// Why a failed job failed, as the server's JSON.
    pub failure_json: Option<String>,
}

/// One file a job produced, as a store keeps it under its name.
#[derive(Clone, PartialEq, Eq)]
pub struct StoredResult {
    /// The file's name: the SHA-256 of its bytes as a client downloads
    /// them.
    pub hash: ResultHash,
    /// How many bytes a client downloads.
    pub size_bytes: u64,
    /// What the store keeps: the file as the server packed it.
    pub data: Vec<u8>,
}

impl fmt::Debug for StoredResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StoredResult")
            .field("hash", &self.hash)
            .field("size_bytes", &self.size_bytes)
            .field("stored_bytes", &self.data.len())
            .finish()
    }
}

/// How a run ended, as the server writes it into the job's record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobOutcome {
    /// Finished, with what it produced: the list the job's record shows,
    /// as the server's JSON, and the files it names, which the store keeps
    /// in the same change.
    Succeeded {
        results_json: String,
        results: Vec<StoredResult>,
    },
    /// Finished without results, with why.
    Failed { failure_json: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    const EMPTY: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    #[test]
    fn a_job_id_is_32_lowercase_hex_digits_and_a_generated_one_is_new_each_time() {
        let first = JobId::generate();
        assert_eq!(JobId::new(first.as_str()).expect("job id"), first);
        assert_ne!(JobId::generate(), first);
        for text in [
            "",
            "J0123456789abcdef0123456789abcde",
            &"a".repeat(33),
            "j-7f3a",
        ] {
            assert!(JobId::new(text).is_err(), "{text:?}");
        }
    }

    #[test]
    fn a_bundle_hash_reads_back_only_its_written_form() {
        let text = format!("sha256:{EMPTY}");
        let hash: BundleHash = text.parse().expect("hash");
        assert_eq!(hash.to_string(), text);
        assert_eq!(BundleHash::from_digest(*hash.digest()), hash);
        for refused in [
            EMPTY.to_string(),
            format!("sha256:{}", EMPTY.to_uppercase()),
            format!("SHA256:{EMPTY}"),
            format!("sha256:{EMPTY}0"),
        ] {
            assert!(
                matches!(refused.parse::<BundleHash>(), Err(ModelError::InvalidBundleHash(input)) if input == refused),
                "{refused:?}"
            );
        }
    }

    #[test]
    fn a_result_hash_reads_back_only_its_written_form_and_a_stored_result_hides_its_bytes() {
        let text = format!("sha256:{EMPTY}");
        let hash: ResultHash = text.parse().expect("hash");
        assert_eq!(hash.to_string(), text);
        assert_eq!(format!("{hash:?}"), format!("ResultHash({text})"));
        assert!(
            matches!(EMPTY.parse::<ResultHash>(), Err(ModelError::InvalidResultHash(input)) if input == EMPTY)
        );
        let stored = StoredResult {
            hash,
            size_bytes: 9,
            data: vec![7; 4],
        };
        assert_eq!(
            format!("{stored:?}"),
            format!("StoredResult {{ hash: ResultHash({text}), size_bytes: 9, stored_bytes: 4 }}")
        );
    }

    #[test]
    fn a_phase_has_one_name_and_only_the_last_three_are_final() {
        for phase in JobPhase::ALL {
            assert_eq!(phase.as_str().parse::<JobPhase>().expect("phase"), phase);
        }
        let finals: Vec<bool> = JobPhase::ALL.iter().map(|phase| phase.is_final()).collect();
        assert_eq!(finals, [false, false, true, true, true]);
        assert!("Queued".parse::<JobPhase>().is_err());
        assert!("canceled".parse::<JobPhase>().is_err());
    }
}
