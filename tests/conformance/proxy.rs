//! The recording proxy the live modes run through: it forwards every
//! request to the exchange's testnet and keeps a transcript of the traffic
//! with the credentials, signatures, and account identity stripped out.

use quantforge::ExchangeId;
use quantforge::exchange::{self, ConnectorConfig};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use url::Url;

use crate::fixture::{self, Interaction, Response};
use crate::profiles::Profile;
use crate::server::{Answer, Captured, Handle, Server};

/// How long the proxy waits for the upstream before answering 502.
const UPSTREAM_TIMEOUT: Duration = Duration::from_secs(30);

pub(crate) struct Proxy {
    server: Server,
    recorded: Arc<Mutex<Vec<Interaction>>>,
}

struct Forwarding {
    upstream: Url,
    client: reqwest::Client,
    profile: Profile,
    recorded: Arc<Mutex<Vec<Interaction>>>,
}

impl Handle for Forwarding {
    async fn handle(&self, request: Captured) -> Answer {
        let observed = fixture::observe(&request, &self.profile);
        let (status, body) = self.forward(&request).await;
        // The connector gets the redacted body too, so a live run and its
        // replay read the same bytes.
        let body = fixture::redact(&body, self.profile.account_fields);
        self.recorded
            .lock()
            .expect("proxy transcript")
            .push(Interaction {
                request: observed,
                response: Response::recorded(status, body.clone()),
            });
        Answer { status, body }
    }
}

impl Forwarding {
    async fn forward(&self, request: &Captured) -> (u16, String) {
        // The path and query go upstream as sent: the signature covers
        // their exact bytes.
        let mut target = format!(
            "{}{}",
            self.upstream.as_str().trim_end_matches('/'),
            request.path
        );
        if let Some(query) = &request.query {
            target.push('?');
            target.push_str(query);
        }
        let (Ok(url), Ok(method)) = (
            Url::parse(&target),
            reqwest::Method::from_bytes(request.method.as_bytes()),
        ) else {
            return (
                502,
                "conformance proxy: an unforwardable request".to_string(),
            );
        };
        let mut upstream = self.client.request(method, url);
        for name in self.profile.forward_headers {
            for value in request.headers.get_all(*name) {
                upstream = upstream.header(*name, value);
            }
        }
        if !request.body.is_empty() {
            upstream = upstream.body(request.body.clone());
        }
        match upstream.send().await {
            Ok(response) => {
                let status = response.status().as_u16();
                match response.text().await {
                    Ok(body) => (status, body),
                    Err(err) => (502, upstream_failed(err)),
                }
            }
            Err(err) => (502, upstream_failed(err)),
        }
    }
}

/// The failure without its URL, which carries the request's signature.
fn upstream_failed(err: reqwest::Error) -> String {
    format!(
        "conformance proxy: the upstream request failed: {}",
        err.without_url()
    )
}

impl Proxy {
    /// A proxy in front of `upstream`, which must not be a production
    /// endpoint of `exchange`: the checks trade on whatever it is.
    pub(crate) fn start(exchange: ExchangeId, profile: Profile, upstream: Url) -> Self {
        let connectors = exchange::connect(
            exchange,
            ConnectorConfig {
                base_url: upstream.clone(),
                credentials: None,
            },
        )
        .expect("connectors for the upstream");
        assert!(
            !connectors.market_data.is_production(),
            "refusing to proxy to {}, a production endpoint",
            upstream.host_str().unwrap_or("a host-less URL")
        );

        let recorded = Arc::new(Mutex::new(Vec::new()));
        let server = Server::start(Forwarding {
            upstream,
            client: reqwest::Client::builder()
                .timeout(UPSTREAM_TIMEOUT)
                .build()
                .expect("upstream client"),
            profile,
            recorded: Arc::clone(&recorded),
        });
        Self { server, recorded }
    }

    pub(crate) fn base_url(&self) -> Url {
        self.server.base_url()
    }

    /// The traffic since the last call, oldest first.
    pub(crate) fn take(&self) -> Vec<Interaction> {
        std::mem::take(&mut *self.recorded.lock().expect("proxy transcript"))
    }
}
