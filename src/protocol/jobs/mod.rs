//! Remote backtests, over HTTP.
//!
//! A job backtests a Python strategy bundle on candles the server stores.
//! It is the only kind of job: the strategies built into QuantForge are not
//! offered remotely. A client uploads its bundle, which is named by its
//! [`ContentHash`](super::ContentHash); submits a [`JobSubmission`]; reads
//! the job's [`JobStatus`] until the job has finished, or follows its log
//! as it runs; and then fetches what it produced: a [`JobResult`], and the
//! files it lists, which [`results`](super::results) specifies.
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
//! | `GET /v1/backtests/{job_id}/logs` | | `200` and a [`LogPage`]: the lines of the job's log after the query's cursor, and its progress. Asked as a WebSocket: `101`, then a stream of [`LogFrame`]s that follows the log to its end |
//! | `GET /v1/backtests/{job_id}/result` | | `200` and the job's [`JobResult`]: its metrics, and each result file's hash, size and path |
//! | `GET /v1/backtests/{job_id}/result/metrics.json` | | `200` and the job's metrics: a [`Metrics`](super::results::Metrics), as JSON |
//! | `GET /v1/backtests/{job_id}/result/trades.arrow` | | `200` and the job's closed trades, as an Arrow IPC file |
//! | `GET /v1/backtests/{job_id}/result/equity.arrow` | | `200` and the job's equity curve, as an Arrow IPC file |
//!
//! None of the routes needs credentials yet. A refused request is answered
//! with an [`ErrorEnvelope`](super::ErrorEnvelope):
//!
//! | Status and code | When |
//! | --- | --- |
//! | 400 `invalid_request` | A log query whose `after` or `limit` is not a number, or whose `limit` is not from 1 to 1,000; a WebSocket request for a log that is not a well-formed upgrade. An upload not sent as `application/x-tar`, larger than any bundle within the server's limits, whose bytes hash to another name, that is not exactly a canonical tar, breaks the [bundle spec], or cannot run on this server: from the third on, the message is the bundle's refusal, each problem's message verbatim. A submission that is not in its canonical form, whose settings are out of range, whose market without `category` is ambiguous, whose range spans more bars than the server runs one job on, that names a bundle the server does not hold or cannot run, or whose range is not within the bars the server stores, from the first to the last |
//! | 404 `not_found` | No job of that ID; a result file of another name |
//! | 409 `conflict` | Cancelling a job that has finished; asking for the result of a job that has not succeeded, which the message says is queued, running, failed or cancelled |
//! | 503 `unavailable` | The server's store could not answer in time; `Retry-After` says how many seconds to wait |
//! | 500 `internal` | The server failed: its store refused, or its copy of a result does not match the result's hash |
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
//! 6. A job runs within the server's limits on its CPU time, its memory and
//!    its wall-clock time. A job past one fails with `invalid_request`, its
//!    message naming the limit; a job whose worker stops before the job
//!    finishes fails with `internal`, its message saying how it stopped.
//!
//! A server without a worker runtime fails every job as it starts, with
//! `unavailable`.
//!
//! # A job's log
//!
//! A job's log is what the job wrote while it ran, line by line, and how
//! far it has come. The server numbers a job's lines from 1 as it takes
//! them ([`LogLine::seq`]) and stamps each with its own clock, so a line's
//! number is a cursor: `after=<seq>` asks for what follows that line.
//!
//! - **A page.** `GET /v1/backtests/{job_id}/logs`, with a [`LogQuery`] as
//!   its query string, answers a [`LogPage`]: the lines after the cursor,
//!   oldest first, the job's [`JobProgress`], and the cursor to ask with
//!   next. A client asks again until the page says it is
//!   [`complete`](LogPage::complete).
//! - **A stream.** The same path asked as a WebSocket upgrade is answered
//!   with [`LogFrame`]s: a `line` for each line after the cursor, first the
//!   ones the server already holds and then each new one as the job writes
//!   it; a `progress` whenever the job has come further than the server
//!   last said; and `end` once the job has ended and its last line was
//!   sent, after which the server closes the connection. A stream of a
//!   queued job stays open, and starts with the job. A server pings a
//!   stream it has had nothing to send on for a while. A client sends
//!   nothing but what WebSocket itself asks of it, such as the answer to
//!   a ping, and one that lost its connection asks again with the number
//!   of the last line it received.
//! - **Bounds.** A server keeps the newest lines of a job that writes more
//!   than it keeps, and the start of a line longer than it keeps of one:
//!   this one 10,000 lines and 1 MiB of text a job, and 8 KiB a line. What
//!   it left out is counted, never passed over in silence: a page's
//!   [`dropped_lines`](LogPage::dropped_lines), a stream's `truncated`
//!   frame, and a line's [`dropped_bytes`](LogLine::dropped_bytes).
//! - **Afterwards.** A job's log is kept with the job once the job has
//!   ended, however it ended. The server stores it before it says
//!   `complete` or `end`, so the server that ran the job and every server
//!   that runs on its store afterwards answer both ways with the same
//!   lines; a log its store would not take is lost once the server that
//!   ran the job no longer holds it, and the job ends all the same. The
//!   log of a job that never started is empty, and that of a
//!   job whose server was killed while it ran is lost with the server:
//!   only a job whose run ends has its log kept.
//!
//! A server that stops closes the stream of a job that has not ended with
//! WebSocket's close code 1001 and no `end`. A stream the server cannot go
//! on with ends with an `error` frame.
//!
//! # A job's result
//!
//! A job that has succeeded has three result files, which its
//! [`JobState::Succeeded`] lists as [`ResultRef`]s and its [`JobResult`]
//! with their paths: `metrics.json`, `trades.arrow` and `equity.arrow`.
//! [`results`](super::results) is their contract: the metrics' fields, each
//! table's columns and types, and how a decimal is written. A job's result
//! never changes, and it outlives the server that ran the job: it is kept
//! in the server's store, with the job.
//!
//! Each file is answered with its media type, the entity tag `"<hash>"`,
//! quotes included, and
//! [`RESULT_CACHE_CONTROL`](super::results::RESULT_CACHE_CONTROL), which
//! lets the client's own cache keep it for a year. A request whose
//! `If-None-Match` names that tag, weakly or not, or is `*`, gets
//! `304 Not Modified` and no body. The server checks a file against its
//! hash before it sends it, and a client that wants to be sure of what it
//! holds hashes the bytes it received and compares.
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

mod log_stream;
mod logs;
mod result;
mod status;
mod submission;

use std::fmt;

use serde::{Deserialize, Serialize};

use super::{ProtocolError, opaque_id};

pub use log_stream::LogFrame;
pub use logs::{JobProgress, LOG_PAGE_LINES, LogLevel, LogLine, LogPage, LogQuery};
pub use result::{JobResult, ResultLink};
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
