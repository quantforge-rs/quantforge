//! The connector registry: from an [`ExchangeId`] to the name operators
//! select it by, the endpoint it defaults to, and the factory that builds
//! its connectors. This is the only place an exchange id meets an adapter.

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

/// One exchange's registry entry, declared by its adapter.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Registration {
    /// The name operators select the exchange by.
    pub(crate) name: &'static str,
    /// The production endpoint, used when the caller names no base URL.
    pub(crate) default_base_url: &'static str,
    pub(crate) factory: ConnectorFactory,
}

/// The exchange selected when a caller names none.
pub const DEFAULT_EXCHANGE: ExchangeId = ExchangeId::BinanceSpot;

/// Every registered exchange, in the order operators see them listed.
const EXCHANGES: [ExchangeId; 1] = [ExchangeId::BinanceSpot];

/// The [`Registration`] for `exchange`. The match is exhaustive, so a new
/// [`ExchangeId`] does not compile until its adapter is registered here.
const fn registration(exchange: ExchangeId) -> Registration {
    match exchange {
        ExchangeId::BinanceSpot => binance::REGISTRATION,
    }
}

/// Every exchange the registry can build, in the order operators see them
/// listed.
pub fn registered() -> &'static [ExchangeId] {
    &EXCHANGES
}

/// The name operators select `exchange` by, such as `binance` in
/// `--exchange binance`. It is not the storage name that
/// [`ExchangeId::as_str`] returns.
pub const fn name(exchange: ExchangeId) -> &'static str {
    registration(exchange).name
}

/// The registered exchange called `name`, matched exactly: no other case or
/// spelling, and not the storage name.
pub fn lookup(name: &str) -> Option<ExchangeId> {
    EXCHANGES
        .into_iter()
        .find(|&exchange| registration(exchange).name == name)
}

/// The production endpoint of `exchange`, which its connectors use when
/// the caller names no base URL.
pub const fn default_base_url(exchange: ExchangeId) -> &'static str {
    registration(exchange).default_base_url
}

/// The factory for `exchange`.
pub fn factory(exchange: ExchangeId) -> ConnectorFactory {
    registration(exchange).factory
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
    use std::collections::HashSet;

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
    fn every_exchange_id_is_registered_once() {
        let listed: HashSet<ExchangeId> = registered().iter().copied().collect();
        assert_eq!(
            listed.len(),
            registered().len(),
            "an exchange is listed twice"
        );
        assert_eq!(listed.len(), 1);

        for exchange in listed {
            // Exhaustive on purpose: a new variant stops this module from
            // compiling until it is added to EXCHANGES and the count above.
            match exchange {
                ExchangeId::BinanceSpot => {}
            }
        }
    }

    #[test]
    fn lookup_finds_each_exchange_by_its_exact_name_only() {
        for &exchange in registered() {
            assert_eq!(lookup(name(exchange)), Some(exchange), "{exchange:?}");
        }
        for unknown in [
            "nosuch",
            "",
            "Binance",
            "BINANCE",
            " binance",
            "binance_spot",
        ] {
            assert_eq!(lookup(unknown), None, "{unknown:?}");
        }
    }

    // clap parses `--exchange`'s default only when a command runs, so no
    // definition check would catch a default that names no exchange.
    #[test]
    fn the_default_exchange_is_registered_under_its_name() {
        assert_eq!(lookup(name(DEFAULT_EXCHANGE)), Some(DEFAULT_EXCHANGE));
    }

    // An omitted base URL must reach the real exchange, never a testnet, and
    // be recognized as production by the connectors built for it.
    #[test]
    fn every_default_base_url_is_its_exchanges_production_endpoint() {
        for &exchange in registered() {
            let connectors =
                connect(exchange, config(default_base_url(exchange), false)).expect("connectors");
            assert_eq!(
                connectors.market_data.exchange_id(),
                exchange,
                "{exchange:?}"
            );
            assert!(connectors.market_data.is_production(), "{exchange:?}");
        }
    }

    #[test]
    fn binance_spot_is_registered_as_binance_at_its_production_api() {
        assert_eq!(DEFAULT_EXCHANGE, ExchangeId::BinanceSpot);
        assert_eq!(name(ExchangeId::BinanceSpot), "binance");
        assert_eq!(
            default_base_url(ExchangeId::BinanceSpot),
            "https://api.binance.com/"
        );
    }

    #[test]
    fn without_credentials_only_market_data_is_built() {
        for &exchange in registered() {
            let connectors =
                connect(exchange, config(default_base_url(exchange), false)).expect("connectors");
            assert!(connectors.trading.is_none(), "{exchange:?}");
            assert!(connectors.account.is_none(), "{exchange:?}");
            assert!(connectors.signed().is_none(), "{exchange:?}");
        }
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
