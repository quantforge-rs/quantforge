//! What a job ran on, as the server recorded it before the job ran: the
//! job's provenance, and the identity of its inputs.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use super::super::bundle::{EngineVersion, PackageName, PackageVersion, PythonVersion};
use super::super::{ContentHash, ProtoVersion};
use super::{BacktestSettings, JobId, ResultRef};

/// The provenance format this build writes and reads: the layout of a
/// [`Provenance`], and how [`ProvenanceInputs::identity`] is taken. Any
/// change to either raises it.
pub const PROVENANCE_FORMAT: u32 = 1;

/// What `GET /v1/backtests/{job_id}/provenance` answers: everything a job
/// ran on, as the server recorded it before the job ran, and what it
/// produced, once it has succeeded.
///
/// `inputs` holds everything the job's result depends on, and `identity`
/// is their hash: two jobs given the same inputs have the same identity,
/// whichever server and store ran them. `strategy` repeats what the code
/// hash already fixes, for reading; `recorded` says where and when, and
/// changes with the store, never with what the job ran on.
///
/// ```
/// use quantforge::protocol::jobs::Provenance;
///
/// let json = r#"{"job_id":"7f3a0c1e9b2d4f6a8c0e1b3d5f7a9c2e","format":1,"identity":"sha256:4bfa1169e9e09789daa65271c4e3cee43848a460ce96aad6caee65022853fca5","inputs":{"code_hash":"sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855","data":{"exchange":"binance","category":"spot","symbol":"BTCUSDT","interval":"1m","start_open_ms":1792454400000,"end_open_ms":1792540740000,"bar_count":1440,"segments":[{"hash":"sha256:40f256656d99aa4df38ff7225d2846a06a5d164becf8f72d32015661914b211f","first_open_ms":1792454400000,"last_open_ms":1792540740000,"bar_count":1440}]},"backtest":{"initial_cash":"10000","fee_bps":"10","close_out_at_end":true},"runtime":{"engine":"0.3.0","python":"3.12","deps":{"numpy":"2.1.3"}},"proto_version":1},"strategy":{"entry":"sma_cross.py:SmaCross","params":{"fast":20,"slow":50}},"recorded":{"at_ms":1792540801500,"backend":"postgres","schema_version":11}}"#;
/// let provenance: Provenance = serde_json::from_str(json)?;
/// assert_eq!(provenance.inputs.data.bar_count, 1440);
/// assert_eq!(provenance.inputs.identity()?, provenance.identity);
/// assert!(provenance.results.is_none());
/// assert_eq!(serde_json::to_string(&provenance)?, json);
/// # Ok::<(), serde_json::Error>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    /// The job.
    pub job_id: JobId,
    /// [`PROVENANCE_FORMAT`].
    pub format: u32,
    /// The identity of the job's inputs: [`ProvenanceInputs::identity`].
    pub identity: ContentHash,
    /// Everything the job's result depends on, which the identity covers.
    pub inputs: ProvenanceInputs,
    /// The bundle's entry and parameters, as its manifest gives them: what
    /// the code hash fixes already, so no part of the identity.
    pub strategy: ProvenanceStrategy,
    /// Where and when the server recorded the job's provenance: no part of
    /// the identity.
    pub recorded: ProvenanceRecorded,
    /// What the job produced, once it has succeeded: its result files, as
    /// its status lists them. Absent until then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub results: Option<Vec<ResultRef>>,
}

/// Everything a job's result depends on: its bundle, its bars, how fills
/// were simulated, and what ran the bundle.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvenanceInputs {
    /// The bundle, by its code hash, which fixes its code, its manifest and
    /// so its parameters.
    pub code_hash: ContentHash,
    /// The bars the job ran on, and the segments that hold them.
    pub data: ProvenanceData,
    /// How fills were simulated.
    pub backtest: BacktestSettings,
    /// What ran the bundle.
    pub runtime: ProvenanceRuntime,
    /// The protocol version of the server that ran the job, which its
    /// submission and its result files were written in.
    pub proto_version: ProtoVersion,
}

impl ProvenanceInputs {
    /// The inputs' identity: the SHA-256 of their compact JSON as this type
    /// writes it, with no whitespace, its members in the order of its
    /// fields, `deps` in the order of their names, and strings escaped
    /// only where JSON requires. Python's `json.dumps(inputs,
    /// separators=(",", ":"), ensure_ascii=False)`, encoded as UTF-8, writes
    /// the same bytes from the members as they were received.
    ///
    /// ```
    /// use quantforge::protocol::jobs::ProvenanceInputs;
    ///
    /// let json = r#"{"code_hash":"sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855","data":{"exchange":"binance","category":"spot","symbol":"BTCUSDT","interval":"1m","start_open_ms":1792454400000,"end_open_ms":1792540740000,"bar_count":1440,"segments":[{"hash":"sha256:40f256656d99aa4df38ff7225d2846a06a5d164becf8f72d32015661914b211f","first_open_ms":1792454400000,"last_open_ms":1792540740000,"bar_count":1440}]},"backtest":{"initial_cash":"10000","fee_bps":"10","close_out_at_end":true},"runtime":{"engine":"0.3.0","python":"3.12","deps":{"numpy":"2.1.3"}},"proto_version":1}"#;
    /// let inputs: ProvenanceInputs = serde_json::from_str(json)?;
    /// assert_eq!(
    ///     inputs.identity()?.to_string(),
    ///     "sha256:4bfa1169e9e09789daa65271c4e3cee43848a460ce96aad6caee65022853fca5"
    /// );
    /// # Ok::<(), serde_json::Error>(())
    /// ```
    pub fn identity(&self) -> Result<ContentHash, serde_json::Error> {
        let text = serde_json::to_vec(self)?;
        Ok(ContentHash::sha256(Sha256::digest(&text).into()))
    }
}

/// The bars a job ran on: one market's bars whose open times lie from
/// `start_open_ms` to `end_open_ms`, both included, as the segments listed
/// hold them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvenanceData {
    /// The exchange, by the name QuantForge registers it under.
    pub exchange: String,
    /// The product family: `spot`, `linear` or `inverse`.
    pub category: String,
    /// The exchange's symbol, in uppercase.
    pub symbol: String,
    /// The bar interval, such as `1m`.
    pub interval: String,
    /// The open time of the first bar of the range.
    pub start_open_ms: i64,
    /// The open time of the last bar of the range.
    pub end_open_ms: i64,
    /// How many bars the job ran on: those of the segments within the
    /// range.
    pub bar_count: u64,
    /// The segments holding every bar the job ran on, oldest first, each
    /// fetched by its hash from `GET /v1/segments/{hash}`. The first and the
    /// last can hold bars outside the range, which the job did not run on.
    pub segments: Vec<ProvenanceSegment>,
}

/// One segment a job's bars came from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvenanceSegment {
    /// Its name: the SHA-256 of its payload.
    pub hash: ContentHash,
    /// The open time of its first bar.
    pub first_open_ms: i64,
    /// The open time of its last bar.
    pub last_open_ms: i64,
    /// How many bars it holds.
    pub bar_count: u32,
}

/// What ran a job's bundle: the engine, the Python version, and each
/// package the bundle declares, at the version that ran.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvenanceRuntime {
    /// The engine version.
    pub engine: EngineVersion,
    /// The Python version.
    pub python: PythonVersion,
    /// Each package the bundle's manifest declares, at the version the
    /// worker ran, which is the version the manifest pins.
    pub deps: BTreeMap<PackageName, PackageVersion>,
}

/// What a bundle's manifest says it runs: its entry and its parameters.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvenanceStrategy {
    /// The entry, `path/to/module.py:Class`.
    pub entry: String,
    /// The parameters the class was built with.
    pub params: Map<String, Value>,
}

/// Where and when a job's provenance was recorded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvenanceRecorded {
    /// When the server recorded it, as the job started.
    pub at_ms: i64,
    /// The backend of the store it was recorded in: `sqlite` or `postgres`.
    pub backend: String,
    /// The schema version of that store then.
    pub schema_version: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    const EMPTY: &str = "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    /// Inputs as a server writes them, with a non-ASCII symbol and a
    /// control character to escape: what an identity is taken over.
    fn inputs() -> ProvenanceInputs {
        serde_json::from_value(serde_json::json!({
            "code_hash": EMPTY,
            "data": {
                "exchange": "binance",
                "category": "spot",
                "symbol": "币安\u{1}USDT",
                "interval": "4h",
                "start_open_ms": 0,
                "end_open_ms": 14_400_000,
                "bar_count": 2,
                "segments": [
                    {"hash": EMPTY, "first_open_ms": 0, "last_open_ms": 14_400_000, "bar_count": 2}
                ]
            },
            "backtest": {"initial_cash": "100.50", "fee_bps": "0", "close_out_at_end": false},
            "runtime": {"engine": "0.2.0", "python": "3.11", "deps": {"pandas": "3.1.1", "numpy": "2.4.6"}},
            "proto_version": 1
        }))
        .expect("inputs")
    }

    #[test]
    fn inputs_have_their_exact_json_deps_by_name_and_decimals_with_their_scale() {
        let written = serde_json::to_string(&inputs()).expect("serialize");
        assert_eq!(
            written,
            format!(
                r#"{{"code_hash":"{EMPTY}","data":{{"exchange":"binance","category":"spot","symbol":"币安\u0001USDT","interval":"4h","start_open_ms":0,"end_open_ms":14400000,"bar_count":2,"segments":[{{"hash":"{EMPTY}","first_open_ms":0,"last_open_ms":14400000,"bar_count":2}}]}},"backtest":{{"initial_cash":"100.50","fee_bps":"0","close_out_at_end":false}},"runtime":{{"engine":"0.2.0","python":"3.11","deps":{{"numpy":"2.4.6","pandas":"3.1.1"}}}},"proto_version":1}}"#
            )
        );
    }

    // The expected hash is Python's: hashlib.sha256 of json.dumps(inputs,
    // separators=(",", ":"), ensure_ascii=False).encode() over the members
    // above, deps sorted by name.
    #[test]
    fn the_identity_is_the_hash_python_takes_of_the_same_members() {
        assert_eq!(
            inputs().identity().expect("an identity").to_string(),
            "sha256:da548b7f230e67191f1355fbed83125f08b17fbb8f3d24bcb3d087aba9bfebb2"
        );
    }

    #[test]
    fn any_input_changed_changes_the_identity() {
        let base = inputs().identity().expect("an identity");
        let mut changed = Vec::new();
        let mut one = inputs();
        one.data.end_open_ms += 1;
        changed.push(one);
        let mut one = inputs();
        one.data.segments[0].bar_count = 3;
        changed.push(one);
        let mut one = inputs();
        one.backtest.initial_cash = "100.5".parse().expect("a decimal");
        changed.push(one);
        let mut one = inputs();
        one.runtime.engine = "0.2.1".parse().expect("a version");
        changed.push(one);
        let mut one = inputs();
        one.runtime.deps.clear();
        changed.push(one);
        let mut one = inputs();
        one.proto_version = ProtoVersion(2);
        changed.push(one);
        for inputs in changed {
            assert_ne!(inputs.identity().expect("an identity"), base, "{inputs:?}");
        }
    }

    #[test]
    fn a_provenance_without_results_omits_them_and_reads_them_back_from_null() {
        let json = serde_json::json!({
            "job_id": "j-1",
            "format": 1,
            "identity": EMPTY,
            "inputs": inputs(),
            "strategy": {"entry": "s.py:S", "params": {}},
            "recorded": {"at_ms": 5, "backend": "sqlite", "schema_version": 11},
            "results": null
        });
        let provenance: Provenance = serde_json::from_value(json).expect("a provenance");
        assert_eq!(provenance.results, None);
        let written = serde_json::to_string(&provenance).expect("serialize");
        assert!(!written.contains("results"), "{written}");
        assert!(
            written.ends_with(r#""recorded":{"at_ms":5,"backend":"sqlite","schema_version":11}}"#)
        );
    }

    #[test]
    fn every_field_but_results_is_required() {
        let full = serde_json::json!({
            "job_id": "j-1",
            "format": 1,
            "identity": EMPTY,
            "inputs": inputs(),
            "strategy": {"entry": "s.py:S", "params": {}},
            "recorded": {"at_ms": 5, "backend": "sqlite", "schema_version": 11}
        });
        for field in [
            "job_id", "format", "identity", "inputs", "strategy", "recorded",
        ] {
            let mut without = full.clone();
            without.as_object_mut().expect("an object").remove(field);
            assert!(
                serde_json::from_value::<Provenance>(without).is_err(),
                "a provenance without {field} must be refused"
            );
        }
        let mut number = full;
        number["inputs"]["backtest"]["fee_bps"] = serde_json::json!(10);
        assert!(serde_json::from_value::<Provenance>(number).is_err());
    }
}
