//! The Binance Spot HTTP transport: base-URL handling, the public and signed
//! request paths, and the mapping of HTTP failures onto `ExchangeError`.
//!
//! `send_public` and `send_signed` are the single choke point for every call
//! the adapter makes — rate limiting, retries and timeouts belong here if they
//! are ever added.

use reqwest::{Method, StatusCode};
use serde_json::Value;
use url::Url;

use super::credentials::BinanceCredentials;
use super::signing;
use super::types::BinanceApiError;
use crate::{ExchangeError, now_utc_ms};

#[derive(Clone, Debug)]
pub struct BinanceSpotClient {
    base_url: Url,
    http: reqwest::Client,
    credentials: Option<BinanceCredentials>,
    recv_window_ms: u64,
}

impl BinanceSpotClient {
    pub fn new(mut base_url: Url) -> Self {
        if !base_url.as_str().ends_with('/') {
            base_url.set_path(&format!("{}/", base_url.path()));
        }
        Self {
            base_url,
            http: reqwest::Client::new(),
            credentials: None,
            recv_window_ms: 5_000,
        }
    }

    pub fn with_credentials(mut self, credentials: BinanceCredentials) -> Self {
        self.credentials = Some(credentials);
        self
    }

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
            return Err(ExchangeError::Api {
                code: Some(api_error.code),
                message: api_error.msg,
            });
        }

        return Err(ExchangeError::Api {
            code: status_to_code(status),
            message: format!("http {status}: {body}"),
        });
    }

    serde_json::from_str::<Value>(&body).map_err(|err| ExchangeError::InvalidResponse {
        message: format!("failed to decode JSON body: {err}; body={body}"),
    })
}

fn status_to_code(status: StatusCode) -> Option<i64> {
    Some(i64::from(status.as_u16()))
}
