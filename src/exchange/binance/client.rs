//! The Binance Spot HTTP transport: base-URL handling, the public and signed
//! request paths, and the mapping of HTTP failures onto `ExchangeError`,
//! classified by the sibling `errors` module.
//!
//! `send_public` and `send_signed` are the single choke point for every call
//! the adapter makes — rate limiting, retries and timeouts belong here if they
//! are ever added.

use reqwest::{Method, StatusCode};
use serde_json::Value;
use url::Url;

use super::types::BinanceApiError;
use super::{errors, signing};
use crate::exchange::ApiCredentials;
use crate::{ExchangeError, now_utc_ms};

#[derive(Clone, Debug)]
pub struct BinanceSpotClient {
    base_url: Url,
    http: reqwest::Client,
    credentials: Option<ApiCredentials>,
    recv_window_ms: u64,
}

impl BinanceSpotClient {
    pub fn new(mut base_url: Url) -> Self {
        if !base_url.as_str().ends_with('/') {
            base_url.set_path(&format!("{}/", base_url.path()));
        }
        Self {
            base_url,
            // reqwest honors the HTTP(S)_PROXY/ALL_PROXY/NO_PROXY convention;
            // nothing else in the environment reaches the client.
            http: reqwest::Client::new(),
            credentials: None,
            recv_window_ms: 5_000,
        }
    }

    pub fn with_credentials(mut self, credentials: ApiCredentials) -> Self {
        self.credentials = Some(credentials);
        self
    }

    /// Whether this client talks to Binance's production system rather than
    /// a testnet or a local stand-in.
    pub(super) fn production_endpoint(&self) -> bool {
        is_production_binance_url(&self.base_url)
    }

    /// Swaps in a caller-built HTTP client. One built with reqwest's
    /// `connection_verbose(true)` logs raw request bytes at trace level,
    /// including the `X-MBX-APIKEY` header.
    pub fn with_http(mut self, http: reqwest::Client) -> Self {
        self.http = http;
        self
    }

    pub fn with_recv_window_ms(mut self, recv_window_ms: u64) -> Self {
        self.recv_window_ms = recv_window_ms;
        self
    }

    fn join(&self, path: &str) -> Result<Url, ExchangeError> {
        self.base_url
            .join(path)
            .map_err(|err| ExchangeError::InvalidResponse {
                message: format!("failed to join base URL and path `{path}`: {err}"),
            })
    }

    pub(super) async fn send_public(
        &self,
        method: Method,
        path: &str,
        params: Vec<(&str, String)>,
    ) -> Result<Value, ExchangeError> {
        let mut url = self.join(path)?;
        let query = signing::encode_query(params);
        if !query.is_empty() {
            url.set_query(Some(&query));
        }

        let response = self
            .http
            .request(method, url)
            .send()
            .await
            .map_err(ExchangeError::transport)?;

        decode_json(response).await
    }

    pub(super) async fn send_signed(
        &self,
        method: Method,
        path: &str,
        params: Vec<(&str, String)>,
    ) -> Result<Value, ExchangeError> {
        let credentials = self
            .credentials
            .as_ref()
            .ok_or(ExchangeError::MissingCredentials)?;

        let mut url = self.join(path)?;
        let query = signing::signed_query(
            &credentials.secret,
            params,
            self.recv_window_ms,
            now_utc_ms(),
        )?;
        url.set_query(Some(&query));

        let response = self
            .http
            .request(method, url)
            .header("X-MBX-APIKEY", &credentials.api_key)
            .send()
            .await
            .map_err(ExchangeError::transport)?;

        decode_json(response).await
    }
}

async fn decode_json(response: reqwest::Response) -> Result<Value, ExchangeError> {
    let status = response.status();
    let body = response.text().await.map_err(ExchangeError::transport)?;

    if !status.is_success() {
        if let Ok(api_error) = serde_json::from_str::<BinanceApiError>(&body) {
            return Err(ExchangeError::api(
                errors::classify(status, Some(api_error.code), &api_error.msg),
                Some(api_error.code),
                api_error.msg,
            ));
        }

        return Err(ExchangeError::api(
            errors::classify(status, None, &body),
            status_to_code(status),
            format!("http {status}: {body}"),
        ));
    }

    serde_json::from_str::<Value>(&body).map_err(|err| ExchangeError::InvalidResponse {
        message: format!("failed to decode JSON body: {err}; body={body}"),
    })
}

fn status_to_code(status: StatusCode) -> Option<i64> {
    Some(i64::from(status.as_u16()))
}

const PRODUCTION_BINANCE_DOMAINS: [&str; 2] = ["binance.com", "binance.us"];

fn is_production_binance_url(url: &Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    PRODUCTION_BINANCE_DOMAINS.iter().any(|domain| {
        host == *domain
            || host
                .strip_suffix(domain)
                .is_some_and(|prefix| prefix.ends_with('.'))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_classifier_matches_production_hosts() {
        for raw in [
            "https://api.binance.com/",
            "https://api1.binance.com/",
            "https://api2.binance.com/",
            "https://api3.binance.com/",
            "https://api4.binance.com/",
            "https://api-gcp.binance.com/",
            "https://api5.binance.com/",
            "https://api.binance.us/",
            "https://binance.com/",
            "https://API.BINANCE.COM/",
            "https://api.binance.com:8443/",
            "https://api.binance.com./",
            "http://api1.binance.com/some/path",
        ] {
            let url = Url::parse(raw).expect("url");
            assert!(is_production_binance_url(&url), "for input {raw:?}");
        }
    }

    #[test]
    fn production_classifier_rejects_testnet_local_and_lookalike_hosts() {
        for raw in [
            "https://testnet.binance.vision/",
            "https://data-api.binance.vision/",
            "http://127.0.0.1:9/",
            "https://evil-binance.com/",
            "https://api.binance.com.evil.example/",
            "file:///tmp/no-host",
        ] {
            let url = Url::parse(raw).expect("url");
            assert!(!is_production_binance_url(&url), "for input {raw:?}");
        }
    }

    // The client's own base URL gains a trailing slash in `new`; the host,
    // and so the classification, is unaffected.
    #[test]
    fn the_client_classifies_its_own_base_url() {
        for (raw, production) in [
            ("https://api.binance.com", true),
            ("https://testnet.binance.vision", false),
        ] {
            let client = BinanceSpotClient::new(Url::parse(raw).expect("url"));
            assert_eq!(client.production_endpoint(), production, "for {raw}");
        }
    }
}
