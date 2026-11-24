//! What a server runs bundles on, as it tells its clients.

use serde::{Deserialize, Serialize};

use super::super::bundle::Runtime;

/// What `GET /v1/runtime` answers: the runtime the server's workers run
/// bundles on, which every upload is admitted against, or no runtime when
/// the server runs no bundles and fails every job as it starts. A client
/// reads it to write a manifest the server admits, and to check a bundle
/// before it uploads it.
///
/// ```
/// use quantforge::protocol::jobs::JobRuntime;
///
/// let json = r#"{"runtime":{"engine":"0.3.0","python":"3.12","packages":{"numpy":["2.1.3"]}}}"#;
/// let answer: JobRuntime = serde_json::from_str(json)?;
/// let runtime = answer.runtime.as_ref().expect("a runtime");
/// assert_eq!(runtime.python.to_string(), "3.12");
/// assert_eq!(serde_json::to_string(&answer)?, json);
///
/// let none: JobRuntime = serde_json::from_str("{}")?;
/// assert!(none.runtime.is_none());
/// assert_eq!(serde_json::to_string(&none)?, "{}");
/// # Ok::<(), serde_json::Error>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobRuntime {
    /// The engine, the Python version and the packages the workers run, or
    /// none, when the server runs no bundles.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<Runtime>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_runtime_and_no_runtime_have_their_exact_json() {
        let json = r#"{"runtime":{"engine":"0.2.0","python":"3.11","packages":{}}}"#;
        let answer: JobRuntime = serde_json::from_str(json).expect("an answer");
        assert!(answer.runtime.is_some());
        assert_eq!(serde_json::to_string(&answer).expect("serialize"), json);
        let none = JobRuntime { runtime: None };
        assert_eq!(serde_json::to_string(&none).expect("serialize"), "{}");
        assert_eq!(
            serde_json::from_str::<JobRuntime>(r#"{"runtime":null}"#).expect("an answer"),
            none
        );
    }

    #[test]
    fn a_runtime_that_is_not_one_is_refused() {
        for json in [
            r#"{"runtime":{"engine":"0.2","python":"3.11","packages":{}}}"#,
            r#"{"runtime":{"engine":"0.2.0","python":"3","packages":{}}}"#,
            r#"{"runtime":{"engine":"0.2.0","python":"3.11"}}"#,
            r#"{"runtime":"3.11"}"#,
        ] {
            assert!(serde_json::from_str::<JobRuntime>(json).is_err(), "{json}");
        }
    }
}
