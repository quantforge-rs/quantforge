//! Remote backtests, over HTTP.
//!
//! A job backtests a Python strategy bundle on candles the server stores.
//! It is the only kind of job: the strategies built into QuantForge are not
//! offered remotely. A client uploads its bundle, which is named by its
//! [`ContentHash`](super::ContentHash); submits a [`JobSubmission`]; reads
//! the job's [`JobStatus`] until the job has finished; and then fetches each
//! result by its [`ResultRef`]. A refused request is answered with an
//! [`ErrorEnvelope`](super::ErrorEnvelope). This module defines the bodies;
//! the routes come with the job API.

mod status;
mod submission;

use std::fmt;

use serde::{Deserialize, Serialize};

use super::{ProtocolError, opaque_id};

pub use status::{JobState, JobStatus, ResultRef};
pub use submission::{BacktestSettings, DataSelection, JobSubmission};

/// The server's name for a job: 1 to 64 ASCII letters, digits, `-`, or `_`,
/// so that it can sit in a URL path as it is. Only the server makes one up.
///
/// ```
/// use quantforge::protocol::jobs::JobId;
///
/// let id: JobId = serde_json::from_str(r#""j-7f3a""#)?;
/// assert_eq!(id.as_str(), "j-7f3a");
/// assert!(serde_json::from_str::<JobId>(r#""j 7f3a""#).is_err());
/// # Ok::<(), serde_json::Error>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String")]
pub struct JobId(String);

impl JobId {
    /// Checks and wraps an id.
    pub fn new(id: impl Into<String>) -> Result<Self, ProtocolError> {
        opaque_id("job id", id.into()).map(Self)
    }

    /// The id's text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for JobId {
    type Error = ProtocolError;

    fn try_from(id: String) -> Result<Self, Self::Error> {
        Self::new(id)
    }
}

impl fmt::Display for JobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_job_id_is_read_through_the_same_check_as_new() {
        let id: JobId = serde_json::from_str(r#""job_42""#).expect("job id");
        assert_eq!(id, JobId::new("job_42").expect("job id"));
        assert_eq!(id.to_string(), "job_42");
        assert_eq!(
            serde_json::to_string(&id).expect("serialize"),
            r#""job_42""#
        );

        let error = serde_json::from_str::<JobId>(r#""../etc""#).expect_err("refused");
        assert!(error.to_string().contains("invalid job id"), "{error}");
        assert!(serde_json::from_str::<JobId>(r#""""#).is_err());
        assert!(serde_json::from_str::<JobId>("42").is_err());
    }
}
