//! The client of a QuantForge server's routes: what `backtest --remote`
//! speaks. It agrees on a protocol version first, through `GET /version`,
//! and then asks under that version's paths, as [`protocol`](crate::protocol)
//! specifies them: the runtime the server's workers run bundles on, the
//! markets it stores, a bundle uploaded under its code hash, a job
//! submitted, where it stands, its log followed to its end, and its result,
//! every downloaded file checked against the size and hash the server
//! listed before it is decoded.
//!
//! A refusal is the server's [`ErrorBody`], its message word for word. A
//! request that changes nothing is sent again while the server answers
//! `503` with `Retry-After`, a few times; a submission never is, since one
//! the server took but could not answer for would run twice.
//!
//! A token is sent with every request, as `Authorization: Bearer`, and
//! never shown: no server checks one yet.

mod error;
mod logs;
mod result;
#[cfg(test)]
mod tests;

use std::fmt;
use std::time::Duration;

use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderValue, RETRY_AFTER};
use reqwest::{RequestBuilder, Response, StatusCode};
use serde::de::DeserializeOwned;
use url::Url;

use crate::protocol::bundle::{BUNDLE_MEDIA_TYPE, Bundle, Runtime};
use crate::protocol::jobs::{BundleReceipt, JobId, JobRuntime, JobStatus, JobSubmission, LogPage};
use crate::protocol::segments::MarketIndex;
use crate::protocol::stream::AuthToken;
use crate::protocol::{ErrorEnvelope, ProtoVersion, ServerVersion, VersionRange};

pub use error::ClientError;
pub use logs::LogEvent;
pub use result::RemoteResult;

/// How long opening a connection to the server may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// How long the server may stay silent while it answers.
const READ_TIMEOUT: Duration = Duration::from_secs(60);

/// How many times a request that changes nothing is sent while the server
/// answers `503`.
const ATTEMPTS: u32 = 5;

/// The longest a `Retry-After` is waited for.
const MOST_RETRY_WAIT: Duration = Duration::from_secs(5);

/// A server's URL, checked: `http` or `https`, a host, no query or
/// fragment, and a path that ends with `/`, so that the routes join below
/// it, as behind a proxy that serves the server under a path.
///
/// ```
/// use quantforge::net::client::server_url;
///
/// let url = server_url("http://127.0.0.1:9600")?;
/// assert_eq!(url.as_str(), "http://127.0.0.1:9600/");
/// assert_eq!(server_url("https://qf.example/lab")?.as_str(), "https://qf.example/lab/");
/// assert!(server_url("ftp://qf.example").is_err());
/// # Ok::<(), quantforge::net::client::ClientError>(())
/// ```
pub fn server_url(text: &str) -> Result<Url, ClientError> {
    let invalid = |reason: &str| ClientError::InvalidUrl {
        reason: reason.to_string(),
    };
    let mut url = Url::parse(text.trim()).map_err(|error| invalid(&error.to_string()))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(invalid("a server's URL starts with http:// or https://"));
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err(invalid("a server's URL names a host"));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(invalid("a server's URL has no query or fragment"));
    }
    if !url.path().ends_with('/') {
        let path = format!("{}/", url.path());
        url.set_path(&path);
    }
    Ok(url)
}

/// The URL as it may be shown: without the user name and password it can
/// carry.
fn shown(url: &Url) -> String {
    let mut shown = url.clone();
    let _ = shown.set_username("");
    let _ = shown.set_password(None);
    shown.to_string()
}

/// A connection to one QuantForge server, at the newest protocol version
/// both sides speak.
pub struct RemoteClient {
    http: reqwest::Client,
    base: Url,
    shown: String,
    release: String,
    version: ProtoVersion,
    bearer: Option<HeaderValue>,
}

impl fmt::Debug for RemoteClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RemoteClient")
            .field("url", &self.shown)
            .field("release", &self.release)
            .field("version", &self.version)
            .field("token", &self.bearer.as_ref().map(|_| "<redacted>"))
            .finish_non_exhaustive()
    }
}

/// What an upload came to: the server's receipt, and whether it stored the
/// bundle now or held it already.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Uploaded {
    /// The bundle the server holds, under the client's code hash.
    pub receipt: BundleReceipt,
    /// `true` when the server stored it now, `false` when it held it.
    pub stored: bool,
}

impl RemoteClient {
    /// Connects to the server at `url` (see [`server_url`]): asks it which
    /// protocol versions it speaks, and agrees on the newest both sides do.
    /// `token`, when given, goes with every request.
    pub async fn connect(url: Url, token: Option<AuthToken>) -> Result<Self, ClientError> {
        let bearer = token.map(|token| bearer(&token)).transpose()?;
        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(READ_TIMEOUT)
            .user_agent(concat!("quantforge/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|error| ClientError::Unreachable {
                url: shown(&url),
                cause: causes(&error.without_url()),
            })?;
        let mut client = Self {
            http,
            shown: shown(&url),
            base: url,
            release: String::new(),
            version: ProtoVersion(0),
            bearer,
        };
        let address = client.at("version");
        let server: ServerVersion = client
            .patiently(|| client.http.get(address.clone()))
            .await
            .map_err(|error| match error {
                ClientError::Refused { status, .. } => ClientError::NotQuantForge {
                    url: client.shown.clone(),
                    what: format!("answered GET /version with {status}"),
                },
                other => other,
            })?;
        client.version = VersionRange::SUPPORTED
            .negotiate(server.versions())
            .map_err(|_| ClientError::Incompatible {
                url: client.shown.clone(),
                release: server.version.clone(),
                server: server.versions(),
                ours: VersionRange::SUPPORTED,
            })?;
        client.release = server.version;
        Ok(client)
    }

    /// The server's URL, without credentials.
    pub fn url(&self) -> &str {
        &self.shown
    }

    /// The server's release, as it names it.
    pub fn release(&self) -> &str {
        &self.release
    }

    /// The protocol version the client speaks to it.
    pub fn version(&self) -> ProtoVersion {
        self.version
    }

    /// What the server's workers run bundles on, or `None` when they run
    /// none.
    pub async fn runtime(&self) -> Result<Option<Runtime>, ClientError> {
        let address = self.route("runtime");
        let answer: JobRuntime = self.patiently(|| self.http.get(address.clone())).await?;
        Ok(answer.runtime)
    }

    /// The markets the server stores bars of, and the open times each
    /// spans.
    pub async fn markets(&self) -> Result<MarketIndex, ClientError> {
        let address = self.route("markets");
        self.patiently(|| self.http.get(address.clone())).await
    }

    /// Uploads `bundle`'s canonical tar under its code hash, which the
    /// server takes the SHA-256 of the bytes to check before it reads them.
    pub async fn upload(&self, bundle: &Bundle) -> Result<Uploaded, ClientError> {
        let hash = bundle.code_hash();
        let address = self.route(&format!("bundles/{hash}"));
        let tar = bundle.canonical_tar();
        let response = self
            .send_patiently(|| {
                self.http
                    .put(address.clone())
                    .header(CONTENT_TYPE, BUNDLE_MEDIA_TYPE)
                    .body(tar.clone())
            })
            .await?;
        let stored = response.status() == StatusCode::CREATED;
        let receipt: BundleReceipt = self.read(response).await?;
        if receipt.code_hash != hash {
            return Err(ClientError::NotQuantForge {
                url: self.shown.clone(),
                what: format!(
                    "answered the upload of {hash} with a receipt for {}",
                    receipt.code_hash
                ),
            });
        }
        Ok(Uploaded { receipt, stored })
    }

    /// Submits a job, once: a submission is never sent again, since one the
    /// server took but could not answer for would run twice.
    pub async fn submit(&self, submission: &JobSubmission) -> Result<JobStatus, ClientError> {
        let address = self.route("backtests");
        let response = self.send(self.http.post(address).json(submission)).await?;
        self.read(response).await
    }

    /// Where `job` stands.
    pub async fn status(&self, job: &JobId) -> Result<JobStatus, ClientError> {
        let address = self.route(&format!("backtests/{job}"));
        self.patiently(|| self.http.get(address.clone())).await
    }

    /// The lines of `job`'s log after line `after`, and its progress.
    pub async fn log_page(&self, job: &JobId, after: u64) -> Result<LogPage, ClientError> {
        let address = self.route(&format!("backtests/{job}/logs?after={after}"));
        self.patiently(|| self.http.get(address.clone())).await
    }

    /// `path` below the server's URL.
    fn at(&self, path: &str) -> Url {
        // Every path here is relative and well formed.
        self.base.join(path).unwrap_or_else(|_| self.base.clone())
    }

    /// The route `rest` under the agreed version.
    fn route(&self, rest: &str) -> Url {
        self.at(&format!("{}/{rest}", self.version.path_segment()))
    }

    fn damaged(&self, what: String) -> ClientError {
        ClientError::Damaged {
            url: self.shown.clone(),
            what,
        }
    }

    /// Sends a request once, with the token.
    async fn send(&self, request: RequestBuilder) -> Result<Response, ClientError> {
        let request = match &self.bearer {
            Some(bearer) => request.header(AUTHORIZATION, bearer.clone()),
            None => request,
        };
        request
            .send()
            .await
            .map_err(|error| self.unreachable(error))
    }

    /// Sends the request `build` makes, again while the server answers
    /// `503` with `Retry-After`, a few times: only for a request that
    /// changes nothing, or nothing more the second time.
    async fn send_patiently(
        &self,
        build: impl Fn() -> RequestBuilder,
    ) -> Result<Response, ClientError> {
        let mut attempt = 1;
        loop {
            let response = self.send(build()).await?;
            if response.status() != StatusCode::SERVICE_UNAVAILABLE || attempt >= ATTEMPTS {
                return Ok(response);
            }
            let Some(wait) = retry_after(&response) else {
                return Ok(response);
            };
            tokio::time::sleep(wait).await;
            attempt += 1;
        }
    }

    /// [`Self::send_patiently`], then the answer read as `T`.
    async fn patiently<T: DeserializeOwned>(
        &self,
        build: impl Fn() -> RequestBuilder,
    ) -> Result<T, ClientError> {
        let response = self.send_patiently(build).await?;
        self.read(response).await
    }

    /// A successful answer's body, or the refusal it is.
    async fn bytes(&self, response: Response) -> Result<Vec<u8>, ClientError> {
        let status = response.status();
        let body = response
            .bytes()
            .await
            .map_err(|error| self.unreachable(error))?;
        if status.is_success() {
            return Ok(body.to_vec());
        }
        match serde_json::from_slice::<ErrorEnvelope>(&body) {
            Ok(envelope) => Err(ClientError::Refused {
                status: status.as_u16(),
                body: envelope.error,
            }),
            Err(_) => Err(ClientError::NotQuantForge {
                url: self.shown.clone(),
                what: format!("answered {status} without QuantForge's error envelope"),
            }),
        }
    }

    /// A successful answer read as `T`, or the refusal it is.
    async fn read<T: DeserializeOwned>(&self, response: Response) -> Result<T, ClientError> {
        let path = response.url().path().to_string();
        let body = self.bytes(response).await?;
        serde_json::from_slice(&body).map_err(|error| ClientError::NotQuantForge {
            url: self.shown.clone(),
            what: format!("answered {path} with a body the protocol does not read: {error}"),
        })
    }

    /// A transport's failure, without the URL it names, which can carry
    /// credentials.
    fn unreachable(&self, error: reqwest::Error) -> ClientError {
        ClientError::Unreachable {
            url: self.shown.clone(),
            cause: causes(&error.without_url()),
        }
    }
}

/// The header a token is sent in. A token that cannot be a header's value
/// is refused, without being shown.
fn bearer(token: &AuthToken) -> Result<HeaderValue, ClientError> {
    let text = token.expose();
    let printable = !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_graphic());
    let mut value = printable
        .then(|| HeaderValue::from_str(&format!("Bearer {text}")).ok())
        .flatten()
        .ok_or(ClientError::InvalidToken)?;
    value.set_sensitive(true);
    Ok(value)
}

/// How long a `503`'s `Retry-After` asks to wait, at most
/// [`MOST_RETRY_WAIT`]; `None` without one, which says not to ask again.
fn retry_after(response: &Response) -> Option<Duration> {
    let seconds: u64 = response
        .headers()
        .get(RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()?;
    Some(Duration::from_secs(seconds).min(MOST_RETRY_WAIT))
}

/// An error and every cause under it, as one line: a transport's own
/// message stops at "error sending request".
fn causes(error: &(dyn std::error::Error + 'static)) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        let next = cause.to_string();
        if !text.contains(&next) {
            text = format!("{text}: {next}");
        }
        source = cause.source();
    }
    text
}
