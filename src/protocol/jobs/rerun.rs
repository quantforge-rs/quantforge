//! What a rerun of a job found: whether the job's result came out again,
//! and, where a file did not, the first place it differs.

use serde::{Deserialize, Serialize};

use super::super::{ContentHash, ErrorBody};
use super::{JobId, ProvenanceRuntime};

/// What `POST /v1/backtests/{job_id}/rerun` answers once the rerun has run:
/// the job run again from its provenance, and its result compared with the
/// job's, file by file.
///
/// ```
/// use quantforge::protocol::jobs::{RerunReport, RerunVerdict};
///
/// let json = r#"{"job_id":"7f3a0c1e9b2d4f6a8c0e1b3d5f7a9c2e","identity":"sha256:4bfa1169e9e09789daa65271c4e3cee43848a460ce96aad6caee65022853fca5","verdict":"diverged","runtime":{"engine":"0.3.1","python":"3.12","deps":{"numpy":"2.1.3"}},"results":[{"name":"metrics","recorded":"sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855","rerun":"sha256:40f256656d99aa4df38ff7225d2846a06a5d164becf8f72d32015661914b211f","divergence":{"field":"final_equity","recorded":"10250.50","rerun":"10250.51"}},{"name":"equity","recorded":"sha256:4bfa1169e9e09789daa65271c4e3cee43848a460ce96aad6caee65022853fca5","rerun":"sha256:9b80ffd85cbcbb3c03e5e7ba06b6ad4a7778e10b4845d7f263f518383106778a","divergence":{"row":1437,"field":"equity","recorded":"10249.00","rerun":"10249.01"}}]}"#;
/// let report: RerunReport = serde_json::from_str(json)?;
/// assert_eq!(report.verdict, RerunVerdict::Diverged);
/// let first = report.results[1].divergence.as_ref().expect("a divergence");
/// assert_eq!((first.row, first.field.as_str()), (Some(1437), "equity"));
/// assert_eq!(serde_json::to_string(&report)?, json);
/// # Ok::<(), serde_json::Error>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RerunReport {
    /// The job that was run again.
    pub job_id: JobId,
    /// The identity of the inputs it was run again on: the job's own, from
    /// its [`Provenance`](super::Provenance).
    pub identity: ContentHash,
    /// Whether the job's result came out again.
    pub verdict: RerunVerdict,
    /// What ran the rerun, which may be a later engine or package than ran
    /// the job, wherever the bundle's manifest allows one.
    pub runtime: ProvenanceRuntime,
    /// Each of the job's result files, in the order the job lists them,
    /// against the file the rerun produced; then each file the rerun
    /// produced that the job does not list.
    pub results: Vec<ResultComparison>,
    /// Why the rerun failed, when it did: it produced no file, and its
    /// verdict is `diverged`. A failure of the server's own is refused
    /// instead, as no verdict.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<ErrorBody>,
}

/// Whether a job's result came out again.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum RerunVerdict {
    /// Every result file came out byte for byte as the job's, and so every
    /// value in it, each decimal with its scale; and the rerun produced no
    /// other file.
    Verified,
    /// A file came out otherwise or not at all, the rerun produced a file
    /// the job does not list, or the rerun failed.
    Diverged,
    /// A verdict this build does not know, from a later revision of the
    /// protocol. Never sent: serializing it fails.
    #[serde(other, skip_serializing)]
    Unrecognized,
}

/// One of a job's result files against the rerun's file of the same name,
/// or a file only the rerun produced.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResultComparison {
    /// The file: `metrics`, `trades` or `equity` for every file a job
    /// lists.
    pub name: String,
    /// The hash the job lists for it; absent for a file the job does not
    /// list, which only the rerun produced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recorded: Option<ContentHash>,
    /// The hash of the rerun's file; absent when the rerun produced none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rerun: Option<ContentHash>,
    /// Where the two first differ; absent when they are the same file, or
    /// the rerun produced none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub divergence: Option<Divergence>,
}

/// The first place two result files of one name differ, reading `metrics`
/// field by field and the tables row by row, each row column by column,
/// every value as its exact text: a decimal with its scale, so `1.10` and
/// `1.1` differ.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Divergence {
    /// The row, counted from 1: of `trades` or `equity`; absent for
    /// `metrics`, which is one record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub row: Option<u64>,
    /// The column or metric that differs first; where one file has no such
    /// row, the table's first column.
    pub field: String,
    /// Its value in the job's file; absent where that file has no such row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recorded: Option<String>,
    /// Its value in the rerun's file; absent where that file has no such
    /// row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rerun: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::super::super::ErrorCode;
    use super::*;

    const EMPTY: &str = "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    fn report(verdict: RerunVerdict) -> RerunReport {
        RerunReport {
            job_id: JobId::new("j-1").expect("a job id"),
            identity: EMPTY.parse().expect("a hash"),
            verdict,
            runtime: serde_json::from_str(r#"{"engine":"0.2.0","python":"3.11","deps":{}}"#)
                .expect("a runtime"),
            results: vec![ResultComparison {
                name: "trades".to_string(),
                recorded: Some(EMPTY.parse().expect("a hash")),
                rerun: None,
                divergence: None,
            }],
            failure: None,
        }
    }

    #[test]
    fn a_verified_report_and_a_failed_rerun_have_their_exact_json() {
        let verified = report(RerunVerdict::Verified);
        assert_eq!(
            serde_json::to_string(&verified).expect("serialize"),
            format!(
                r#"{{"job_id":"j-1","identity":"{EMPTY}","verdict":"verified","runtime":{{"engine":"0.2.0","python":"3.11","deps":{{}}}},"results":[{{"name":"trades","recorded":"{EMPTY}"}}]}}"#
            )
        );
        let mut failed = report(RerunVerdict::Diverged);
        failed.failure = Some(ErrorBody {
            code: ErrorCode::InvalidRequest,
            message: "the strategy raised".to_string(),
            supported: None,
        });
        let json = serde_json::to_string(&failed).expect("serialize");
        assert!(
            json.ends_with(
                r#""failure":{"code":"invalid_request","message":"the strategy raised"}}"#
            ),
            "{json}"
        );
        assert_eq!(
            serde_json::from_str::<RerunReport>(&json).expect("a report"),
            failed
        );
    }

    #[test]
    fn a_divergence_past_the_end_of_one_file_omits_that_side() {
        let divergence = Divergence {
            row: Some(4),
            field: "symbol".to_string(),
            recorded: None,
            rerun: Some("BTCUSDT".to_string()),
        };
        let json = serde_json::to_string(&divergence).expect("serialize");
        assert_eq!(json, r#"{"row":4,"field":"symbol","rerun":"BTCUSDT"}"#);
        assert_eq!(
            serde_json::from_str::<Divergence>(&json).expect("a divergence"),
            divergence
        );
    }

    #[test]
    fn a_file_only_the_rerun_produced_has_no_recorded_hash() {
        let extra = ResultComparison {
            name: "notes".to_string(),
            recorded: None,
            rerun: Some(EMPTY.parse().expect("a hash")),
            divergence: None,
        };
        let json = serde_json::to_string(&extra).expect("serialize");
        assert_eq!(json, format!(r#"{{"name":"notes","rerun":"{EMPTY}"}}"#));
        assert_eq!(
            serde_json::from_str::<ResultComparison>(&json).expect("a comparison"),
            extra
        );
    }

    #[test]
    fn a_verdict_this_build_does_not_know_is_unrecognized_and_never_sent() {
        let json = serde_json::to_string(&report(RerunVerdict::Verified))
            .expect("serialize")
            .replace(r#""verified""#, r#""inconclusive""#);
        let read: RerunReport = serde_json::from_str(&json).expect("a report");
        assert_eq!(read.verdict, RerunVerdict::Unrecognized);
        assert!(serde_json::to_string(&read).is_err());
        assert!(serde_json::from_str::<RerunVerdict>("0").is_err());
    }
}
