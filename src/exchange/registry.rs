//! The connector registry: from an [`ExchangeId`] to the factory that builds
//! that exchange's connectors. This is the only place an exchange id meets
//! an adapter.

use super::ApiCredentials;
use super::binance;
use crate::ExchangeId;
use crate::ports::{
    AccountConnector, ExchangeError, MarketDataConnector, SignedConnectors, TradingConnector,
};
use std::fmt;
use std::sync::Arc;
use url::Url;

/// What a connector factory is given: where the exchange is, and the
/// credentials its signed connectors use, if any.
#[derive(Clone, Debug)]
pub struct ConnectorConfig {
    pub base_url: Url,
    pub credentials: Option<ApiCredentials>,
}

/// One exchange's connectors. Market data needs no credentials and is
/// always present; the signed connectors exist only when credentials were
/// supplied.
#[derive(Clone)]
pub struct ExchangeConnectors {
    pub market_data: Arc<dyn MarketDataConnector>,
    pub trading: Option<Arc<dyn TradingConnector>>,
    pub account: Option<Arc<dyn AccountConnector>>,
}

impl ExchangeConnectors {
    /// The signed half, or `None` without credentials.
    pub fn signed(&self) -> Option<SignedConnectors<'_>> {
        Some(SignedConnectors {
            trading: self.trading.as_deref()?,
            account: self.account.as_deref()?,
        })
    }
}

/// Names the exchange and whether it is signed; the connectors themselves
/// may hold credentials.
impl fmt::Debug for ExchangeConnectors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExchangeConnectors")
            .field("exchange", &self.market_data.exchange_id())
            .field("signed", &self.signed().is_some())
            .finish_non_exhaustive()
    }
}

/// Builds one exchange's connectors.
pub type ConnectorFactory = fn(ConnectorConfig) -> Result<ExchangeConnectors, ExchangeError>;

/// The factory for `exchange`. The match is exhaustive, so a new
/// [`ExchangeId`] does not compile until it has a factory here.
pub fn factory(exchange: ExchangeId) -> ConnectorFactory {
    match exchange {
        ExchangeId::BinanceSpot => binance::connect,
    }
}

/// Builds `exchange`'s connectors from `config`.
pub fn connect(
    exchange: ExchangeId,
    config: ConnectorConfig,
) -> Result<ExchangeConnectors, ExchangeError> {
    factory(exchange)(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(base_url: &str, with_credentials: bool) -> ConnectorConfig {
        ConnectorConfig {
            base_url: Url::parse(base_url).expect("url"),
            credentials: with_credentials.then(|| ApiCredentials {
                api_key: "key".to_string(),
                secret: "secret".to_string(),
            }),
        }
    }

    #[test]
    fn without_credentials_only_market_data_is_built() {
        let connectors = connect(
            ExchangeId::BinanceSpot,
            config("https://api.binance.com/", false),
        )
        .expect("connectors");
        assert!(connectors.trading.is_none());
        assert!(connectors.account.is_none());
        assert!(connectors.signed().is_none());
        assert_eq!(
            connectors.market_data.exchange_id(),
            ExchangeId::BinanceSpot
        );
    }

    // Every connector reports the same exchange and endpoint, so a caller
    // can ask whichever one it holds.
    #[test]
    fn with_credentials_every_connector_is_built_for_the_same_endpoint() {
        for (base_url, production) in [
            ("https://api.binance.com/", true),
            ("https://testnet.binance.vision/", false),
        ] {
            let connectors =
                connect(ExchangeId::BinanceSpot, config(base_url, true)).expect("connectors");
            let signed = connectors.signed().expect("signed connectors");
            for (name, exchange, is_production) in [
                (
                    "market data",
                    connectors.market_data.exchange_id(),
                    connectors.market_data.is_production(),
                ),
                (
                    "trading",
                    signed.trading.exchange_id(),
                    signed.trading.is_production(),
                ),
                (
                    "account",
                    signed.account.exchange_id(),
                    signed.account.is_production(),
                ),
            ] {
                assert_eq!(exchange, ExchangeId::BinanceSpot, "{name}");
                assert_eq!(is_production, production, "{name} at {base_url}");
            }
        }
    }
}
