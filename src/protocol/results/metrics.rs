//! The `metrics` result: a backtest's summary.

use serde::{Deserialize, Serialize};

use super::super::ExactDecimal;
use super::{ResultError, refused};

const RESULT: &str = "metrics";

/// A backtest's summary: `metrics.json`, and the `metrics` of a
/// [`JobResult`](super::super::jobs::JobResult). Amounts are in the quote
/// asset.
///
/// ```
/// use quantforge::protocol::results::Metrics;
///
/// let json = r#"{"initial_cash":"10000","final_equity":"10250.50","total_return_pct":"2.5050","max_drawdown_pct":"1.2","trade_count":3}"#;
/// let metrics: Metrics = serde_json::from_str(json)?;
/// assert_eq!(metrics.final_equity.to_string(), "10250.50");
/// assert_eq!(metrics.trade_count, 3);
/// assert_eq!(serde_json::to_string(&metrics)?, json);
///
/// // `metrics.json` holds exactly those bytes.
/// assert_eq!(metrics.encode()?, json.as_bytes());
/// assert_eq!(Metrics::decode(json.as_bytes())?, metrics);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Metrics {
    /// The cash the backtest started with.
    pub initial_cash: ExactDecimal,
    /// Cash plus any position still open, valued at the last bar's close,
    /// after a close-out at the end and its fee.
    pub final_equity: ExactDecimal,
    /// `(final_equity - initial_cash) / initial_cash`, in percent.
    pub total_return_pct: ExactDecimal,
    /// The largest fall of the equity curve from an earlier peak, in
    /// percent.
    pub max_drawdown_pct: ExactDecimal,
    /// How many trades closed: the rows of the `trades` table.
    pub trade_count: u64,
}

impl Metrics {
    /// The bytes of `metrics.json`: the object in UTF-8, its fields in the
    /// order above, with no whitespace.
    pub fn encode(&self) -> Result<Vec<u8>, ResultError> {
        serde_json::to_vec(self).map_err(|failure| refused(RESULT, failure.to_string()))
    }

    /// The metrics of a `metrics.json`. Refuses bytes that are not exactly
    /// what [`Metrics::encode`] writes for them.
    pub fn decode(file: &[u8]) -> Result<Self, ResultError> {
        let metrics: Self =
            serde_json::from_slice(file).map_err(|failure| refused(RESULT, failure.to_string()))?;
        super::exactly(RESULT, file, metrics, Self::encode)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const JSON: &str = r#"{"initial_cash":"10010","final_equity":"10999.989","total_return_pct":"9.89","max_drawdown_pct":"10","trade_count":2}"#;

    #[test]
    fn metrics_have_their_exact_json_and_file() {
        let metrics: Metrics = serde_json::from_str(JSON).expect("metrics");
        assert_eq!(metrics.initial_cash.to_string(), "10010");
        assert_eq!(metrics.trade_count, 2);
        assert_eq!(serde_json::to_string(&metrics).expect("serialize"), JSON);
        assert_eq!(metrics.encode().expect("a file"), JSON.as_bytes());
        assert_eq!(Metrics::decode(JSON.as_bytes()).expect("metrics"), metrics);
    }

    #[test]
    fn every_amount_must_be_a_decimal_string_and_every_field_is_required() {
        for (field, changed) in [
            (
                r#""final_equity":"10999.989""#,
                r#""final_equity":10999.989"#,
            ),
            (r#""max_drawdown_pct":"10""#, r#""max_drawdown_pct":10"#),
            (r#""trade_count":2"#, r#""trade_count":"2""#),
            (r#""trade_count":2"#, r#""trade_count":-2"#),
            (r#","trade_count":2"#, ""),
            (r#""initial_cash":"10010","#, ""),
        ] {
            let json = JSON.replace(field, changed);
            assert_ne!(json, JSON, "{field} is in the fixture");
            assert!(
                serde_json::from_str::<Metrics>(&json).is_err(),
                "{changed:?} must be refused"
            );
        }
    }

    #[test]
    fn a_file_that_is_not_exactly_the_one_written_is_refused() {
        let spaced = JSON.replace(r#","trade_count""#, r#", "trade_count""#);
        let reordered = r#"{"trade_count":2,"initial_cash":"10010","final_equity":"10999.989","total_return_pct":"9.89","max_drawdown_pct":"10"}"#;
        let extra = JSON.replace(r#""trade_count":2"#, r#""trade_count":2,"bars":15"#);
        for file in [
            spaced.as_str(),
            reordered,
            extra.as_str(),
            &format!("{JSON}\n"),
        ] {
            assert!(
                serde_json::from_str::<Metrics>(file).is_ok(),
                "{file} is the object"
            );
            assert_eq!(
                Metrics::decode(file.as_bytes())
                    .expect_err("not the file")
                    .to_string(),
                "not a format 1 metrics file: it is not the file its contents are written as"
            );
        }
        let refusal = Metrics::decode(b"[]").expect_err("no object");
        assert!(
            refusal
                .to_string()
                .starts_with("not a format 1 metrics file: invalid length"),
            "{refusal}"
        );
    }
}
