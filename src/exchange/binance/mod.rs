//! Binance Spot REST adapter. The work is split across private sibling
//! modules — `client` owns the HTTP transport, `venue` implements the
//! [`crate::ports`] connector traits on top of it, `signing` computes the
//! HMAC-SHA256 query signature, `errors` classifies Binance's error codes,
//! and `types` and `convert` hold the wire DTOs and their mapping onto
//! domain types. Only [`BinanceSpotClient`] leaves this namespace; the
//! registry reaches the rest through its `REGISTRATION` entry.

mod client;
mod convert;
mod errors;
mod signing;
mod types;
mod venue;

pub use client::BinanceSpotClient;

use super::registry::Registration;
use super::{ConnectorConfig, ExchangeConnectors};
use crate::ports::ExchangeError;
use std::sync::Arc;

/// Binance Spot's registry entry: served by the production REST API unless
/// the caller names another base URL, such as the Spot testnet at
/// `https://testnet.binance.vision/`. Operators select it by its venue's
/// name, `binance`.
pub(crate) const REGISTRATION: Registration = Registration {
    default_base_url: "https://api.binance.com/",
    factory: connect,
};

/// The registry's factory for Binance Spot. One client backs every
/// connector: public requests never carry the credentials (see `client`), so
/// serving market data from the client that also signs sends exactly what a
/// separate unsigned client would.
fn connect(config: ConnectorConfig) -> Result<ExchangeConnectors, ExchangeError> {
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
