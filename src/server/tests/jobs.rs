//! The job routes' uploads: a bundle stored once under its code hash, and
//! every upload refused before anything is stored, a bundle's own refusal
//! word for word. The helpers here serve `submissions` as well.

use reqwest::header::LOCATION;
use serde_json::json;
use sha2::{Digest, Sha256};

use super::*;
use crate::model::BundleHash;
use crate::ports::JobStore;
use crate::protocol::ContentHash;
use crate::protocol::bundle::{Bundle, BundleLimits, Runtime};

pub(super) const MANIFEST: &str = "format = 1\nentry = \"strategy.py:Hold\"\nengine = \">=0.3.0, \
     <0.4.0\"\npython = \"3.12\"\n";
pub(super) const STRATEGY: &str = "class Hold:\n    pass\n";

/// The spec's smallest bundle: a manifest and a strategy that holds.
pub(super) fn bundle() -> Bundle {
    Bundle::from_files(
        [("quantforge.toml", MANIFEST), ("strategy.py", STRATEGY)],
        &BundleLimits::default(),
    )
    .expect("a bundle")
}

/// A runtime of engine 0.3.0 and `python`, offering no packages.
pub(super) fn runtime(python: &str) -> Runtime {
    serde_json::from_str(&format!(
        r#"{{"engine":"0.3.0","python":"{python}","packages":{{}}}}"#
    ))
    .expect("a runtime")
}

/// A worker that offers a runtime and is never asked to run a job: its
/// queue is never started.
pub(super) struct Admitting(pub(super) Runtime);

impl JobWorker for Admitting {
    fn runtime(&self) -> Option<Runtime> {
        Some(self.0.clone())
    }

    fn run(&self, input: JobInput, _cancel: &Cancel, _log: &JobLog) -> WorkerOutcome {
        panic!("job {} ran, but the queue never started", input.job_id)
    }
}

/// The job routes over `bed`'s store, offering `worker`'s runtime, their
/// queue never started.
pub(super) fn jobs_of<W: JobWorker + 'static>(bed: &Bed, worker: W, max_span_bars: u64) -> Jobs {
    Jobs::new(
        bed.server_store(),
        worker,
        JobLimits {
            concurrency: 1,
            max_span_bars,
            log: LogLimits::default(),
        },
    )
}

/// Serves the history routes and `jobs` over `bed`'s store.
pub(super) async fn serve_jobs(bed: &Bed, jobs: Jobs) -> SocketAddr {
    start_with(bed.history(), jobs).await
}

/// The name a bundle with these bytes is uploaded under.
pub(super) fn name_of(bytes: &[u8]) -> ContentHash {
    ContentHash::sha256(Sha256::digest(bytes).into())
}

/// `PUT /v1/bundles/{name}` with `body`, sent as `media_type`.
pub(super) async fn put_bundle(
    address: SocketAddr,
    name: &str,
    media_type: Option<&str>,
    body: Vec<u8>,
) -> reqwest::Response {
    let mut request = client()
        .put(format!("http://{address}/v1/bundles/{name}"))
        .body(body);
    if let Some(media_type) = media_type {
        request = request.header(CONTENT_TYPE, media_type);
    }
    request.send().await.expect("a response")
}

/// Uploads `bundle` as a client does, and checks that it was stored.
pub(super) async fn upload(address: SocketAddr, bundle: &Bundle) -> ContentHash {
    let response = put_bundle(
        address,
        &bundle.code_hash().to_string(),
        Some("application/x-tar"),
        bundle.canonical_tar(),
    )
    .await;
    assert!(response.status().is_success(), "{}", response.status());
    bundle.code_hash()
}

/// A canonical tar of `files`, sorted by path, written field by field from
/// the spec's table: what a client that skipped the bundle checks sends.
fn canonical_tar(files: &[(&str, &str)]) -> Vec<u8> {
    let mut tar = Vec::new();
    for (path, text) in files {
        let mut header = [0_u8; 512];
        header[..path.len()].copy_from_slice(path.as_bytes());
        header[100..108].copy_from_slice(b"0000644\0");
        header[108..116].copy_from_slice(b"0000000\0");
        header[116..124].copy_from_slice(b"0000000\0");
        header[124..136].copy_from_slice(format!("{:011o}\0", text.len()).as_bytes());
        header[136..148].copy_from_slice(b"00000000000\0");
        header[148..156].copy_from_slice(b"        ");
        header[156] = b'0';
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");
        let checksum: u32 = header.iter().map(|&byte| u32::from(byte)).sum();
        header[148..156].copy_from_slice(format!("{checksum:06o}\0 ").as_bytes());
        tar.extend_from_slice(&header);
        tar.extend_from_slice(text.as_bytes());
        tar.resize(tar.len().div_ceil(512) * 512, 0);
    }
    tar.resize(tar.len() + 1024, 0);
    tar
}

#[tokio::test]
async fn an_upload_is_stored_once_under_its_code_hash() {
    let bed = Bed::new();
    let address = serve_jobs(&bed, jobs_of(&bed, NoRuntime, 100)).await;
    let bundle = bundle();
    let tar = bundle.canonical_tar();
    let name = bundle.code_hash().to_string();
    let receipt = format!(r#"{{"code_hash":"{name}","size_bytes":{}}}"#, tar.len());

    for (round, status) in [(1, 201), (2, 200)] {
        let response = put_bundle(address, &name, Some("application/x-tar"), tar.clone()).await;
        assert_eq!(response.status(), status, "upload {round}");
        assert_eq!(header(&response, CONTENT_TYPE), Some("application/json"));
        assert_eq!(header(&response, CACHE_CONTROL), Some("no-cache"));
        assert_eq!(
            response.text().await.expect("a body"),
            receipt,
            "upload {round}"
        );
    }
    let hash = BundleHash::from_digest(*bundle.code_hash().digest());
    let stored = bed.store.load_bundle(&hash).expect("a read");
    assert_eq!(stored.as_deref(), Some(tar.as_slice()));
}

/// An upload refused: what it is, the name it is sent under, its media
/// type, its bytes, and the refusal's message.
type Case = (&'static str, String, Option<&'static str>, Vec<u8>, String);

#[tokio::test]
async fn an_upload_is_refused_unless_it_is_exactly_a_bundle_sent_under_its_hash() {
    let bed = Bed::new();
    let address = serve_jobs(&bed, jobs_of(&bed, Admitting(runtime("3.11")), 100)).await;
    let tar = bundle().canonical_tar();
    let other = name_of(b"another bundle");
    let mut extended = tar.clone();
    extended.extend_from_slice(&[0; 512]);
    let unchecked = canonical_tar(&[("strategy.py", STRATEGY)]);
    let refused = |bytes: &[u8]| {
        Bundle::from_canonical_tar(bytes, &BundleLimits::default())
            .expect_err("refused")
            .to_string()
    };
    let cases: [Case; 6] = [
        (
            "sent under another name",
            other.to_string(),
            Some("application/x-tar"),
            tar.clone(),
            format!(
                "the upload hashes to {}, not to {other}, the name it was sent under; send a \
                 bundle's canonical tar under its code hash",
                name_of(&tar)
            ),
        ),
        (
            "without a media type",
            name_of(&tar).to_string(),
            None,
            tar.clone(),
            "an upload is a bundle's canonical tar, sent with Content-Type application/x-tar"
                .to_string(),
        ),
        (
            "as another media type",
            name_of(&tar).to_string(),
            Some("application/octet-stream"),
            tar.clone(),
            "an upload is a bundle's canonical tar, sent with Content-Type application/x-tar"
                .to_string(),
        ),
        (
            "not exactly a canonical tar",
            name_of(&extended).to_string(),
            Some("application/x-tar"),
            extended.clone(),
            refused(&extended),
        ),
        (
            "breaking the spec",
            name_of(&unchecked).to_string(),
            Some("application/x-tar"),
            unchecked.clone(),
            refused(&unchecked),
        ),
        (
            "for another runtime",
            name_of(&tar).to_string(),
            Some("application/x-tar; charset=binary"),
            tar.clone(),
            bundle()
                .admit(&runtime("3.11"))
                .expect_err("refused")
                .to_string(),
        ),
    ];
    for (case, name, media_type, body, expected) in cases {
        let response = put_bundle(address, &name, media_type, body).await;
        let (status, code, message) = refusal(response).await;
        assert_eq!((status, code.as_str()), (400, "invalid_request"), "{case}");
        assert_eq!(message, expected, "{case}");
    }
    assert!(
        refused(&unchecked).contains("has no quantforge.toml"),
        "{}",
        refused(&unchecked)
    );
    for bytes in [&tar, &extended, &unchecked] {
        let hash = BundleHash::from_digest(*name_of(bytes).digest());
        let stored = bed.store.load_bundle(&hash).expect("a read");
        assert!(stored.is_none(), "a refused upload was stored");
    }

    let (status, code, message) = refusal(
        put_bundle(
            address,
            "sha256:nothex",
            Some("application/x-tar"),
            tar.clone(),
        )
        .await,
    )
    .await;
    assert_eq!((status, code.as_str()), (400, "invalid_request"));
    let expected = "sha256:nothex"
        .parse::<ContentHash>()
        .expect_err("refused")
        .to_string();
    assert_eq!(message, expected);
}

#[tokio::test]
async fn an_upload_of_megabytes_within_the_limits_is_stored() {
    // Larger than a request body may be anywhere else on the server.
    let bed = Bed::new();
    let address = serve_jobs(&bed, jobs_of(&bed, NoRuntime, 100)).await;
    let data = "# padding\n".repeat(90_000);
    let files: Vec<(String, String)> = (0..4)
        .map(|i| (format!("data_{i}.py"), data.clone()))
        .chain([
            ("quantforge.toml".to_string(), MANIFEST.to_string()),
            ("strategy.py".to_string(), STRATEGY.to_string()),
        ])
        .collect();
    let bundle = Bundle::from_files(files, &BundleLimits::default()).expect("a bundle");
    let tar = bundle.canonical_tar();
    assert!(tar.len() > 3 * 1024 * 1024, "{}", tar.len());
    let name = bundle.code_hash().to_string();
    let response = put_bundle(address, &name, Some("application/x-tar"), tar).await;
    assert_eq!(response.status(), 201);
}

#[tokio::test]
async fn an_upload_larger_than_any_bundle_within_the_limits_is_refused_unread() {
    let bed = Bed::new();
    let address = serve_jobs(&bed, jobs_of(&bed, NoRuntime, 100)).await;
    // 256 headers and paddings, 8 MiB of files and the closing blocks.
    let most = 256 * 1023 + 8 * 1024 * 1024 + 1024;
    assert_eq!(BundleLimits::default().max_archive_bytes(), most);
    let body = vec![0; usize::try_from(most + 1).expect("a size")];
    let name = name_of(&body).to_string();
    let response = put_bundle(address, &name, Some("application/x-tar"), body).await;
    let (status, code, message) = refusal(response).await;
    assert_eq!((status, code.as_str()), (400, "invalid_request"));
    assert_eq!(
        message,
        "the upload holds more than 8651520 bytes, the most a canonical tar within this \
         server's bundle limits can hold"
    );
}

/// A job submission's body, in the protocol's JSON, with `data`.
pub(super) fn submission(bundle: &ContentHash, data: serde_json::Value) -> serde_json::Value {
    json!({
        "bundle": bundle.to_string(),
        "data": data,
        "backtest": {"initial_cash": "10000", "fee_bps": "10", "close_out_at_end": true},
    })
}

/// A data selection of BTCUSDT `1m` from `start` to `end`, in `category`
/// unless `None`.
pub(super) fn data(
    category: Option<Category>,
    start: TimestampMs,
    end: TimestampMs,
) -> serde_json::Value {
    let mut data = json!({
        "exchange": VENUE.as_str(),
        "symbol": "BTCUSDT",
        "interval": "1m",
        "start_open_ms": start,
        "end_open_ms": end,
    });
    if let Some(category) = category {
        data["category"] = json!(category.as_str());
    }
    data
}

/// `POST /v1/backtests` with `body`.
pub(super) async fn submit(address: SocketAddr, body: &serde_json::Value) -> reqwest::Response {
    client()
        .post(format!("http://{address}/v1/backtests"))
        .json(body)
        .send()
        .await
        .expect("a response")
}

/// A submission's answer: its status, after checking the `Location` it
/// names.
pub(super) async fn queued(response: reqwest::Response) -> serde_json::Value {
    assert_eq!(response.status(), 201);
    let location = header(&response, LOCATION).map(str::to_string);
    let status: serde_json::Value = response.json().await.expect("a status");
    let id = status["job_id"].as_str().expect("a job id");
    assert_eq!(location, Some(format!("/v1/backtests/{id}")));
    status
}
