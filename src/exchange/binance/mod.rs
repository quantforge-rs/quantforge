//! Binance Spot REST adapter. The work is split across private sibling
//! modules — `client` owns the HTTP transport, `venue` implements the
//! [`crate::ports`] connector traits on top of it, `signing` computes the
//! HMAC-SHA256 query signature, and `types` and `convert` hold the wire DTOs
//! and their mapping onto domain types. Only [`BinanceSpotClient`] leaves
//! this namespace; the registry builds it through `connect`.

mod client;
mod convert;
mod signing;
mod types;
mod venue;

pub use client::BinanceSpotClient;

use super::{ConnectorConfig, ExchangeConnectors};
use crate::ports::ExchangeError;
use std::sync::Arc;

/// The registry's factory for Binance Spot. One client backs every
/// connector: public requests never carry the credentials (see `client`), so
/// serving market data from the client that also signs sends exactly what a
/// separate unsigned client would.
pub(crate) fn connect(config: ConnectorConfig) -> Result<ExchangeConnectors, ExchangeError> {
    let signed = config.credentials.is_some();
    let mut client = BinanceSpotClient::new(config.base_url);
    if let Some(credentials) = config.credentials {
        client = client.with_credentials(credentials);
    }
    let client = Arc::new(client);
    let mut connectors = ExchangeConnectors {
        market_data: client.clone(),
        trading: None,
        account: None,
    };
    if signed {
        connectors.trading = Some(client.clone());
        connectors.account = Some(client);
    }
    Ok(connectors)
}
