//! Where a job stands, and what it produced.

use serde::{Deserialize, Serialize};

use super::super::{ContentHash, ErrorBody};
use super::JobId;

/// A job and where it stands.
///
/// ```
/// use quantforge::protocol::jobs::{JobState, JobStatus};
///
/// let json = r#"{"job_id":"j-7f3a","submitted_at_ms":1792454400000,"state":{"kind":"running","started_at_ms":1792454401500}}"#;
/// let status: JobStatus = serde_json::from_str(json)?;
/// assert_eq!(status.state, JobState::Running { started_at_ms: 1792454401500 });
/// assert_eq!(serde_json::to_string(&status)?, json);
/// # Ok::<(), serde_json::Error>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobStatus {
    /// The job.
    pub job_id: JobId,
    /// When the server accepted the submission.
    pub submitted_at_ms: i64,
    /// Where it stands now.
    pub state: JobState,
}

/// Where a job stands, named by `kind`. A job is queued, then running, and
/// then succeeded, failed, or cancelled; it can also fail or be cancelled
/// before it starts. The last three are final.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum JobState {
    /// Accepted, and waiting for a worker.
    Queued,
    /// Running on a worker.
    Running {
        /// When the worker started it.
        started_at_ms: i64,
    },
    /// Finished, with results.
    Succeeded {
        /// When the worker started it.
        started_at_ms: i64,
        /// When it finished.
        finished_at_ms: i64,
        /// What it produced.
        results: Vec<ResultRef>,
    },
    /// Finished without results. `error.code` is `invalid_request` when the
    /// bundle or its data is at fault, so running it again unchanged fails
    /// again, and `internal` or `unavailable` when the server is.
    Failed {
        /// When the worker started it; absent when it failed before starting.
        #[serde(skip_serializing_if = "Option::is_none")]
        started_at_ms: Option<i64>,
        /// When it failed.
        finished_at_ms: i64,
        /// Why.
        error: ErrorBody,
    },
    /// Stopped at the client's request.
    Cancelled {
        /// When the worker started it; absent when it was cancelled before
        /// starting.
        #[serde(skip_serializing_if = "Option::is_none")]
        started_at_ms: Option<i64>,
        /// When it stopped.
        finished_at_ms: i64,
    },
    /// A state this build does not know, from a later revision of the
    /// protocol. Never sent: serializing it fails.
    #[serde(other, skip_serializing)]
    Unrecognized,
}

/// One result of a succeeded job, to fetch by its hash.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResultRef {
    /// What the result holds, such as `trades`, `equity`, or `metrics`.
    pub name: String,
    /// Its media type, such as `application/json`.
    pub media_type: String,
    /// Its name as stored content: the hash of the bytes a download returns.
    pub hash: ContentHash,
    /// How many bytes a download returns.
    pub size_bytes: u64,
}

#[cfg(test)]
mod tests {
    use super::super::super::ErrorCode;
    use super::*;

    const EMPTY: &str = "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    fn status(state: JobState) -> JobStatus {
        JobStatus {
            job_id: JobId::new("j-1").expect("job id"),
            submitted_at_ms: 100,
            state,
        }
    }

    /// Every state a server sends, with its exact JSON under `state`.
    fn states() -> Vec<(JobState, String)> {
        let failure = ErrorBody {
            code: ErrorCode::InvalidRequest,
            message: "the strategy raised ValueError".to_string(),
            supported: None,
        };
        vec![
            (JobState::Queued, r#"{"kind":"queued"}"#.to_string()),
            (
                JobState::Running { started_at_ms: 200 },
                r#"{"kind":"running","started_at_ms":200}"#.to_string(),
            ),
            (
                JobState::Succeeded {
                    started_at_ms: 200,
                    finished_at_ms: 300,
                    results: vec![ResultRef {
                        name: "metrics".to_string(),
                        media_type: "application/json".to_string(),
                        hash: EMPTY.parse().expect("hash"),
                        size_bytes: 0,
                    }],
                },
                format!(
                    r#"{{"kind":"succeeded","started_at_ms":200,"finished_at_ms":300,"results":[{{"name":"metrics","media_type":"application/json","hash":"{EMPTY}","size_bytes":0}}]}}"#
                ),
            ),
            (
                JobState::Failed {
                    started_at_ms: Some(200),
                    finished_at_ms: 250,
                    error: failure.clone(),
                },
                r#"{"kind":"failed","started_at_ms":200,"finished_at_ms":250,"error":{"code":"invalid_request","message":"the strategy raised ValueError"}}"#.to_string(),
            ),
            (
                JobState::Failed {
                    started_at_ms: None,
                    finished_at_ms: 150,
                    error: failure,
                },
                r#"{"kind":"failed","finished_at_ms":150,"error":{"code":"invalid_request","message":"the strategy raised ValueError"}}"#.to_string(),
            ),
            (
                JobState::Cancelled {
                    started_at_ms: Some(200),
                    finished_at_ms: 220,
                },
                r#"{"kind":"cancelled","started_at_ms":200,"finished_at_ms":220}"#.to_string(),
            ),
            (
                JobState::Cancelled {
                    started_at_ms: None,
                    finished_at_ms: 120,
                },
                r#"{"kind":"cancelled","finished_at_ms":120}"#.to_string(),
            ),
        ]
    }

    #[test]
    fn every_state_has_its_exact_json() {
        for (state, json) in states() {
            // Exhaustive on purpose: a new state stops this module from
            // compiling until it is added to `states`.
            match &state {
                JobState::Queued
                | JobState::Running { .. }
                | JobState::Succeeded { .. }
                | JobState::Failed { .. }
                | JobState::Cancelled { .. }
                | JobState::Unrecognized => {}
            }
            let status = status(state);
            let full = format!(r#"{{"job_id":"j-1","submitted_at_ms":100,"state":{json}}}"#);
            assert_eq!(serde_json::to_string(&status).expect("serialize"), full);
            assert_eq!(
                serde_json::from_str::<JobStatus>(&full).expect("status"),
                status
            );
        }
    }

    #[test]
    fn an_absent_start_reads_the_same_from_null() {
        let json = r#"{"kind":"cancelled","started_at_ms":null,"finished_at_ms":120}"#;
        assert_eq!(
            serde_json::from_str::<JobState>(json).expect("state"),
            JobState::Cancelled {
                started_at_ms: None,
                finished_at_ms: 120
            }
        );
    }

    #[test]
    fn a_state_this_build_does_not_know_is_unrecognized_and_keeps_the_job() {
        let json =
            r#"{"job_id":"j-1","submitted_at_ms":100,"state":{"kind":"paused","since_ms":5}}"#;
        assert_eq!(
            serde_json::from_str::<JobStatus>(json).expect("status"),
            status(JobState::Unrecognized)
        );
    }

    // A tag read from buffered content would also take a variant's index.
    #[test]
    fn a_state_named_by_a_number_is_refused() {
        for kind in ["0", "1", "null"] {
            let json =
                format!(r#"{{"job_id":"j-1","submitted_at_ms":100,"state":{{"kind":{kind}}}}}"#);
            assert!(
                serde_json::from_str::<JobStatus>(&json).is_err(),
                "kind {kind} must be refused"
            );
        }
    }

    #[test]
    fn an_unrecognized_state_is_never_sent() {
        assert!(serde_json::to_string(&status(JobState::Unrecognized)).is_err());
    }
}
