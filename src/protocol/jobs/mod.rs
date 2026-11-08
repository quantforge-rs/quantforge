//! Remote backtests, over HTTP.
//!
//! A job backtests a Python strategy bundle on candles the server stores.
//! It is the only kind of job: the strategies built into QuantForge are not
//! offered remotely. A client uploads its bundle, which is named by its
//! [`ContentHash`](super::ContentHash); submits a [`JobSubmission`]; reads
//! the job's [`JobStatus`] until the job has finished; and then fetches each
//! result by its [`ResultRef`].
//!
//! # Routes
//!
//! | Route | Body | Answer |
//! | --- | --- | --- |
//! | `PUT /v1/bundles/{code_hash}` | The bundle's canonical tar, as [`BUNDLE_MEDIA_TYPE`](super::bundle::BUNDLE_MEDIA_TYPE) | `201` and a [`BundleReceipt`] when the server stores it, `200` and the same when it held it already |
//! | `POST /v1/backtests` | A [`JobSubmission`] | `201` and the job's [`JobStatus`], queued, with its path in `Location` |
//! | `GET /v1/backtests/{job_id}` | | `200` and the job's [`JobStatus`] |
//! | `GET /v1/backtests` | | `200` and a [`JobList`], newest first: at most `limit` jobs, a query parameter of 1 to 1,000, 100 when it is left out |
//! | `POST /v1/backtests/{job_id}/cancel` | | `200` and the job's [`JobStatus`], cancelled |
//!
//! None of the routes needs credentials yet. A refused request is answered
//! with an [`ErrorEnvelope`](super::ErrorEnvelope):
//!
//! | Status and code | When |
//! | --- | --- |
//! | 400 `invalid_request` | An upload not sent as `application/x-tar`, larger than any bundle within the server's limits, whose bytes hash to another name, that is not exactly a canonical tar, breaks the [bundle spec], or cannot run on this server: from the third on, the message is the bundle's refusal, each problem's message verbatim. A submission that is not in its canonical form, whose settings are out of range, whose market without `category` is ambiguous, whose range spans more bars than the server runs one job on, that names a bundle the server does not hold or cannot run, or whose range is not within the bars the server stores, from the first to the last |
//! | 404 `not_found` | No job of that ID |
//! | 409 `conflict` | Cancelling a job that has finished |
//! | 503 `unavailable` | The server's store could not answer in time; `Retry-After` says how many seconds to wait |
//! | 500 `internal` | The server failed |
//!
//! # A job's life
//!
//! 1. The server takes an upload's SHA-256, reads it back strictly, checks
//!    the bundle, admits it against its runtime, and stores it once.
//! 2. It checks a submission before it queues it: the market and range in
//!    their canonical forms, the settings, the bundle, and the candles,
//!    which must cover the whole range.
//! 3. Jobs start in the order they were submitted, as many at once as the
//!    server runs, each on the candles the server read for it as it
//!    started.
//! 4. A queued or running job can be cancelled; a running one's worker is
//!    stopped and freed for the next job.
//! 5. A server that stops fails the jobs it runs, with `internal` and the
//!    reason, so that their clients submit them again; one that was killed
//!    fails them as it starts again. Queued jobs wait for it either way.
//!
//! A server without a worker runtime fails every job as it starts, with
//! `unavailable`.
//!
//! # A market without `category`
//!
//! A [`DataSelection`] may leave out `category`: the form a market took
//! before it carried one, when every market was `spot`. The server reads it
//! as `spot` unless the exchange holds the symbol and interval in another
//! family as well, and then refuses it, naming the families, rather than
//! guess. The job's [`JobStatus::submission`] always names the category.
//!
//! [bundle spec]: <https://github.com/quantforge-rs/quantforge/blob/main/docs/bundle-spec.md>

mod status;
mod submission;

use std::fmt;

use serde::{Deserialize, Serialize};

use super::{ProtocolError, opaque_id};

pub use status::{JobList, JobState, JobStatus, ResultRef};
pub use submission::{BacktestSettings, BundleReceipt, DataSelection, JobSubmission};

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
