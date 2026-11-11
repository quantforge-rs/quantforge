//! What a succeeded job produced, and where each file of it is.

use serde::{Deserialize, Serialize};

use super::super::ContentHash;
use super::super::results::Metrics;
use super::JobId;

/// What `GET /v1/backtests/{job_id}/result` answers: the job's metrics, and
/// where each of its result files is.
///
/// ```
/// use quantforge::protocol::jobs::JobResult;
///
/// let json = r#"{"job_id":"7f3a0c1e9b2d4f6a8c0e1b3d5f7a9c2e","format":1,"metrics":{"initial_cash":"10000","final_equity":"10250.50","total_return_pct":"2.5050","max_drawdown_pct":"1.2","trade_count":3},"results":[{"name":"trades","media_type":"application/vnd.apache.arrow.file","hash":"sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855","size_bytes":1882,"path":"/v1/backtests/7f3a0c1e9b2d4f6a8c0e1b3d5f7a9c2e/result/trades.arrow"}]}"#;
/// let result: JobResult = serde_json::from_str(json)?;
/// assert_eq!(result.metrics.final_equity.to_string(), "10250.50");
/// assert_eq!(result.results[0].path, "/v1/backtests/7f3a0c1e9b2d4f6a8c0e1b3d5f7a9c2e/result/trades.arrow");
/// assert_eq!(serde_json::to_string(&result)?, json);
/// # Ok::<(), serde_json::Error>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobResult {
    /// The job.
    pub job_id: JobId,
    /// The result format of its files:
    /// [`RESULT_FORMAT`](super::super::results::RESULT_FORMAT).
    pub format: u32,
    /// The summary, which `metrics.json` holds too.
    pub metrics: Metrics,
    /// The job's result files: `metrics`, `trades` and `equity`, in that
    /// order.
    pub results: Vec<ResultLink>,
}

/// One result file of a job: its [`ResultRef`](super::ResultRef), and where
/// to fetch it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResultLink {
    /// What the result holds: `metrics`, `trades`, or `equity`.
    pub name: String,
    /// The file's media type.
    pub media_type: String,
    /// The hash of the bytes a download returns.
    pub hash: ContentHash,
    /// How many bytes a download returns.
    pub size_bytes: u64,
    /// The path to `GET` it at, on the server that answered, from the
    /// root: the path of this answer's request, then the file's name.
    pub path: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    const EMPTY: &str = "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    fn json() -> String {
        format!(
            r#"{{"job_id":"j-1","format":1,"metrics":{{"initial_cash":"10010","final_equity":"10999.989","total_return_pct":"9.89","max_drawdown_pct":"10","trade_count":2}},"results":[{{"name":"metrics","media_type":"application/json","hash":"{EMPTY}","size_bytes":117,"path":"/v1/backtests/j-1/result/metrics.json"}},{{"name":"equity","media_type":"application/vnd.apache.arrow.file","hash":"{EMPTY}","size_bytes":1178,"path":"/v1/backtests/j-1/result/equity.arrow"}}]}}"#
        )
    }

    #[test]
    fn a_result_has_its_exact_json() {
        let json = json();
        let result: JobResult = serde_json::from_str(&json).expect("a result");
        assert_eq!(result.job_id.as_str(), "j-1");
        assert_eq!(result.format, 1);
        assert_eq!(result.metrics.trade_count, 2);
        let names: Vec<&str> = result
            .results
            .iter()
            .map(|link| link.name.as_str())
            .collect();
        assert_eq!(names, ["metrics", "equity"]);
        assert_eq!(result.results[1].size_bytes, 1178);
        assert_eq!(serde_json::to_string(&result).expect("serialize"), json);
    }

    #[test]
    fn every_field_is_required_and_the_metrics_stay_exact() {
        let json = json();
        for field in [
            r#""format":1,"#,
            r#","path":"/v1/backtests/j-1/result/metrics.json""#,
            r#""size_bytes":117,"#,
            r#""max_drawdown_pct":"10","#,
        ] {
            let without = json.replace(field, "");
            assert_ne!(without, json, "{field} is in the fixture");
            assert!(
                serde_json::from_str::<JobResult>(&without).is_err(),
                "a result without {field} must be refused"
            );
        }
        let number = json.replace(
            r#""final_equity":"10999.989""#,
            r#""final_equity":10999.989"#,
        );
        assert!(serde_json::from_str::<JobResult>(&number).is_err());
    }
}
