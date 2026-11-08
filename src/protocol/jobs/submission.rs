//! What a client sends to start a job.

use serde::{Deserialize, Serialize};

use super::super::{ContentHash, ExactDecimal};

/// What `PUT /v1/bundles/{code_hash}` answers: the bundle the server holds.
///
/// ```
/// use quantforge::protocol::jobs::BundleReceipt;
///
/// let json = r#"{"code_hash":"sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855","size_bytes":3072}"#;
/// let receipt: BundleReceipt = serde_json::from_str(json)?;
/// assert_eq!(receipt.size_bytes, 3072);
/// assert_eq!(serde_json::to_string(&receipt)?, json);
/// # Ok::<(), serde_json::Error>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleReceipt {
    /// The bundle's code hash, which a [`JobSubmission`] names it by.
    pub code_hash: ContentHash,
    /// The size of its canonical tar.
    pub size_bytes: u64,
}

/// A backtest to run: the bundle, the candles it runs on, and how fills are
/// simulated. Strategy parameters belong to the bundle, so the bundle's hash
/// and these fields together say everything the client asks for. A bundle
/// accepts a range of engine versions, so which engine, Python, and packages
/// actually ran the job is the server's to record.
///
/// ```
/// use quantforge::protocol::jobs::JobSubmission;
///
/// let json = r#"{"bundle":"sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855","data":{"exchange":"binance","category":"spot","symbol":"BTCUSDT","interval":"1m","start_open_ms":1792454400000,"end_open_ms":1792540740000},"backtest":{"initial_cash":"10000","fee_bps":"10","close_out_at_end":true}}"#;
/// let submission: JobSubmission = serde_json::from_str(json)?;
/// assert_eq!(submission.data.exchange, "binance");
/// assert_eq!(submission.data.category.as_deref(), Some("spot"));
/// assert_eq!(submission.backtest.fee_bps.to_string(), "10");
/// assert_eq!(serde_json::to_string(&submission)?, json);
/// # Ok::<(), serde_json::Error>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobSubmission {
    /// The strategy bundle, by its code hash
    /// ([`Bundle::code_hash`](super::super::bundle::Bundle::code_hash)). The
    /// server must hold it already.
    pub bundle: ContentHash,
    /// The candles to run it on.
    pub data: DataSelection,
    /// How fills are simulated.
    pub backtest: BacktestSettings,
}

/// The candles a job runs on: one market's closed bars whose open times lie
/// from `start_open_ms` to `end_open_ms`, both included.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataSelection {
    /// The exchange, by the name QuantForge registers it under.
    pub exchange: String,
    /// The product family: `spot`, `linear` or `inverse`. With `exchange` it
    /// names the product line. Left out only in the form a selection took
    /// before markets carried one, which a server reads as `spot` only while
    /// that is unambiguous (see the [module docs](super)).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// The exchange's symbol, in uppercase.
    pub symbol: String,
    /// The bar interval, such as `1m`.
    pub interval: String,
    /// The open time of the first bar.
    pub start_open_ms: i64,
    /// The open time of the last bar.
    pub end_open_ms: i64,
}

/// How a backtest simulates fills, under the same execution model as a local
/// backtest: a position set on one bar fills at the next bar's open.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BacktestSettings {
    /// The quote currency to start with; above zero.
    pub initial_cash: ExactDecimal,
    /// The fee on every fill, in basis points of its notional, so `10` is
    /// 0.1%; zero or more.
    pub fee_bps: ExactDecimal,
    /// Whether a position still open after the last bar is sold at that
    /// bar's close and recorded as a trade.
    pub close_out_at_end: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    const JSON: &str = r#"{"bundle":"sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855","data":{"exchange":"binance","category":"spot","symbol":"BTCUSDT","interval":"4h","start_open_ms":1792454400000,"end_open_ms":1792540800000},"backtest":{"initial_cash":"2500.00","fee_bps":"7.5","close_out_at_end":false}}"#;

    #[test]
    fn a_submission_has_its_exact_json() {
        let submission: JobSubmission = serde_json::from_str(JSON).expect("submission");
        assert_eq!(submission.data.interval, "4h");
        assert_eq!(submission.backtest.initial_cash.to_string(), "2500.00");
        assert!(!submission.backtest.close_out_at_end);
        assert_eq!(serde_json::to_string(&submission).expect("serialize"), JSON);
    }

    #[test]
    fn every_amount_must_be_a_decimal_string() {
        for (field, number) in [
            (r#""initial_cash":"2500.00""#, r#""initial_cash":2500.00"#),
            (r#""fee_bps":"7.5""#, r#""fee_bps":7.5"#),
        ] {
            let json = JSON.replace(field, number);
            assert_ne!(json, JSON, "{field} is in the fixture");
            assert!(
                serde_json::from_str::<JobSubmission>(&json).is_err(),
                "{number} must be refused"
            );
        }
    }

    #[test]
    fn every_field_is_required_but_category() {
        for field in [
            r#""bundle":"sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855","#,
            r#""start_open_ms":1792454400000,"#,
            r#","close_out_at_end":false"#,
        ] {
            let json = JSON.replace(field, "");
            assert_ne!(json, JSON, "{field} is in the fixture");
            assert!(
                serde_json::from_str::<JobSubmission>(&json).is_err(),
                "a submission without {field} must be refused"
            );
        }
    }

    #[test]
    fn a_selection_without_category_is_read_and_sent_without_one() {
        let legacy = JSON.replace(r#""category":"spot","#, "");
        let submission: JobSubmission = serde_json::from_str(&legacy).expect("submission");
        assert_eq!(submission.data.category, None);
        assert_eq!(
            serde_json::to_string(&submission).expect("serialize"),
            legacy
        );
    }

    #[test]
    fn a_receipt_has_its_exact_json() {
        let receipt = BundleReceipt {
            code_hash: "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
                .parse()
                .expect("hash"),
            size_bytes: 1024,
        };
        let json = r#"{"code_hash":"sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855","size_bytes":1024}"#;
        assert_eq!(serde_json::to_string(&receipt).expect("serialize"), json);
        assert_eq!(
            serde_json::from_str::<BundleReceipt>(json).expect("receipt"),
            receipt
        );
    }
}
