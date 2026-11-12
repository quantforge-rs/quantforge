//! A job's log on PostgreSQL: stored once under its job's ID, behind the
//! copy guard like every write, and read back as it was stored.

use tokio_postgres::types::Type;

use super::bridge::Effect;
use super::{PostgresStore, copying};
use crate::model::{JobId, TimestampMs};
use crate::ports::StorageError;
use crate::storage::jobs::no_job;

const STORE_LOG: &str = concat!(
    r#"
    INSERT INTO job_logs(job_id, log_json, stored_at_ms)
    SELECT $1, $2, $3
    WHERE EXISTS (SELECT 1 FROM jobs WHERE job_id = $1) AND "#,
    copying::not_copying!(),
    r#"
    ON CONFLICT (job_id) DO NOTHING
    "#
);

const JOB_KNOWN: &str = "SELECT EXISTS (SELECT 1 FROM jobs WHERE job_id = $1)";

const LOAD_LOG: &str = "SELECT log_json FROM job_logs WHERE job_id = $1";

impl PostgresStore {
    /// `JobStore::store_job_log`: once, for a job the store holds.
    pub(super) fn store_log(
        &self,
        job_id: &JobId,
        log_json: &str,
        stored_at_ms: TimestampMs,
    ) -> Result<bool, StorageError> {
        let (job, log_json) = (job_id.clone(), log_json.to_string());
        self.bridge.execute(Effect::Write, move |client| {
            Box::pin(async move {
                let id = job.as_str();
                let stored = client
                    .execute_typed(
                        STORE_LOG,
                        &[
                            (&id, Type::TEXT),
                            (&log_json, Type::TEXT),
                            (&stored_at_ms, Type::INT8),
                        ],
                    )
                    .await?;
                if stored > 0 {
                    return Ok(Ok(true));
                }
                // Nothing written: held off by a copy, stored already, or
                // there is no such job.
                if let Some(refused) = copying::refusal(&**client).await? {
                    return Ok(Err(refused));
                }
                let known: bool = client
                    .query_typed(JOB_KNOWN, &[(&id, Type::TEXT)])
                    .await?
                    .first()
                    .map(|row| row.try_get(0))
                    .transpose()?
                    .unwrap_or(false);
                Ok(if known { Ok(false) } else { Err(no_job(&job)) })
            })
        })?
    }

    /// `JobStore::job_log`.
    pub(super) fn load_log(&self, job_id: &JobId) -> Result<Option<String>, StorageError> {
        self.keyed(LOAD_LOG, job_id.as_str().to_string())
    }
}
