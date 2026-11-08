//! A job's record as the protocol shows it. A record no build writes, such
//! as a succeeded job without a start, fails the request, logged, rather
//! than show the job as something it was not.

use tracing::error;

use crate::model::{JobPhase, JobRecord};
use crate::protocol::ErrorBody;
use crate::protocol::jobs::{JobId, JobState, JobStatus, JobSubmission, ResultRef};
use crate::server::refusal::Refusal;

/// The job's status, as the protocol shows it.
pub(super) fn status(record: &JobRecord) -> Result<JobStatus, Refusal> {
    let unreadable = |what: &str, why: String| {
        error!(job = %record.job_id, %why, "a stored job's {what} cannot be read");
        Refusal::internal(format!("the server cannot read job {}", record.job_id))
    };
    let job_id = JobId::new(record.job_id.as_str())
        .map_err(|failure| unreadable("id", failure.to_string()))?;
    let submission: JobSubmission = serde_json::from_str(&record.request_json)
        .map_err(|failure| unreadable("submission", failure.to_string()))?;
    let started = || {
        record
            .started_at_ms
            .ok_or_else(|| unreadable("start", "it has none".to_string()))
    };
    let finished = || {
        record
            .finished_at_ms
            .ok_or_else(|| unreadable("end", "it has none".to_string()))
    };
    let state = match record.phase {
        JobPhase::Queued => JobState::Queued,
        JobPhase::Running => JobState::Running {
            started_at_ms: started()?,
        },
        JobPhase::Succeeded => {
            let results: Vec<ResultRef> =
                serde_json::from_str(record.results_json.as_deref().unwrap_or_default())
                    .map_err(|failure| unreadable("results", failure.to_string()))?;
            JobState::Succeeded {
                started_at_ms: started()?,
                finished_at_ms: finished()?,
                results,
            }
        }
        JobPhase::Failed => {
            let error: ErrorBody =
                serde_json::from_str(record.failure_json.as_deref().unwrap_or_default())
                    .map_err(|failure| unreadable("failure", failure.to_string()))?;
            JobState::Failed {
                started_at_ms: record.started_at_ms,
                finished_at_ms: finished()?,
                error,
            }
        }
        JobPhase::Cancelled => JobState::Cancelled {
            started_at_ms: record.started_at_ms,
            finished_at_ms: finished()?,
        },
    };
    Ok(JobStatus {
        job_id,
        submitted_at_ms: record.submitted_at_ms,
        submission,
        state,
    })
}
