//! The connector registry: from an [`ExchangeId`] to the name operators
//! select it by, the endpoint it defaults to, and the factory that builds
//! its connectors. This is the only place an exchange id meets an adapter.
//! A product line whose adapter is still to come is named here too, and
//! refused.

use super::ApiCredentials;
use super::binance;
use crate::ports::{
    AccountConnector, ExchangeError, MarketDataConnector, SignedConnectors, TradingConnector,
};
use crate::{Category, ExchangeId};
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

/// One product line's registry entry, declared by its adapter. Its name is
/// its venue's.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Registration {
    /// The production endpoint, used when the caller names no base URL.
    pub(crate) default_base_url: &'static str,
    pub(crate) factory: ConnectorFactory,
}

/// The exchange selected when a caller names none.
pub const DEFAULT_EXCHANGE: ExchangeId = ExchangeId::BinanceSpot;

/// Every registered exchange, in the order operators see them listed.
const EXCHANGES: [ExchangeId; 1] = [ExchangeId::BinanceSpot];

/// The [`Registration`] for `exchange`, or `None` while its adapter is
/// still to come. The match is exhaustive, so a new [`ExchangeId`] does not
/// compile until it is registered here or listed as waiting.
const fn registration(exchange: ExchangeId) -> Option<Registration> {
    match exchange {
        ExchangeId::BinanceSpot => Some(binance::REGISTRATION),
        // The futures lines of the first venue register with ALG-759, and
        // the second venue's lines with ALG-693.
        ExchangeId::BinanceLinear | ExchangeId::BinanceInverse => None,
        ExchangeId::BybitSpot | ExchangeId::BybitLinear | ExchangeId::BybitInverse => None,
    }
}

/// Every exchange the registry can build, in the order operators see them
/// listed.
pub fn registered() -> &'static [ExchangeId] {
    &EXCHANGES
}

/// The name operators select `exchange` by: its venue's, such as `binance`
/// in `--exchange binance`. It is not the storage name that
/// [`ExchangeId::as_str`] returns.
pub const fn name(exchange: ExchangeId) -> &'static str {
    exchange.venue().as_str()
}

/// The registered Spot line of the venue called `name`, matched exactly: no
/// other case or spelling, and not the storage name. Selecting a futures
/// line comes with its adapter.
pub fn lookup(name: &str) -> Option<ExchangeId> {
    EXCHANGES.into_iter().find(|&exchange| {
        exchange.category() == Category::Spot && exchange.venue().as_str() == name
    })
}

/// The production endpoint of `exchange`, which its connectors use when
/// the caller names no base URL; `None` for a line without an adapter.
pub fn default_base_url(exchange: ExchangeId) -> Option<&'static str> {
    registration(exchange).map(|registration| registration.default_base_url)
}

/// The factory for `exchange`, or `None` for a line without an adapter.
pub fn factory(exchange: ExchangeId) -> Option<ConnectorFactory> {
    registration(exchange).map(|registration| registration.factory)
}

/// Builds `exchange`'s connectors from `config`, or refuses a line whose
/// adapter is still to come.
pub fn connect(
    exchange: ExchangeId,
    config: ConnectorConfig,
) -> Result<ExchangeConnectors, ExchangeError> {
    let factory = factory(exchange).ok_or_else(|| ExchangeError::InvalidRequest {
        message: format!(
            "{} {} has no adapter yet",
            exchange.venue(),
            exchange.category()
        ),
    })?;
    factory(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ExchangeErrorKind, Venue};
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

    /// Every product line, built from its venue and category.
    fn every_line() -> Vec<ExchangeId> {
        Venue::ALL
            .into_iter()
            .flat_map(|venue| Category::ALL.map(|category| ExchangeId::from_parts(venue, category)))
            .collect()
    }

    // `registration()`'s exhaustive match decides each line; this checks
    // that EXCHANGES lists exactly the lines it registers.
    #[test]
    fn exactly_the_listed_lines_have_an_adapter() {
        let listed: HashSet<ExchangeId> = registered().iter().copied().collect();
        assert_eq!(
            listed.len(),
            registered().len(),
            "an exchange is listed twice"
        );
        for exchange in every_line() {
            let adapted = listed.contains(&exchange);
            assert_eq!(registration(exchange).is_some(), adapted, "{exchange:?}");
            assert_eq!(
                default_base_url(exchange).is_some(),
                adapted,
                "{exchange:?}"
            );
            assert_eq!(factory(exchange).is_some(), adapted, "{exchange:?}");
            assert_eq!(name(exchange), exchange.venue().as_str(), "{exchange:?}");
        }
        assert_eq!(listed, HashSet::from([ExchangeId::BinanceSpot]));
    }

    // `--exchange` lists the names, so two registered lines of one venue
    // would list it twice.
    #[test]
    fn registered_lines_are_spot_lines_of_distinct_venues() {
        let names: HashSet<&str> = registered()
            .iter()
            .map(|&exchange| name(exchange))
            .collect();
        assert_eq!(names.len(), registered().len());
        for exchange in registered() {
            assert_eq!(exchange.category(), Category::Spot, "{exchange:?}");
        }
    }

    #[test]
    fn a_line_without_an_adapter_is_refused_before_anything_is_built() {
        for exchange in every_line() {
            if registered().contains(&exchange) {
                continue;
            }
            let error = connect(exchange, config("https://example.invalid/", false))
                .expect_err("no adapter");
            assert_eq!(
                error.kind(),
                ExchangeErrorKind::InvalidRequest,
                "{exchange:?}"
            );
            let expected = format!(
                "invalid request: {} {} has no adapter yet",
                exchange.venue(),
                exchange.category()
            );
            assert_eq!(error.to_string(), expected);
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
            "bybit",
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
            let base_url = default_base_url(exchange).expect("default base url");
            let connectors = connect(exchange, config(base_url, false)).expect("connectors");
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
            Some("https://api.binance.com/")
        );
    }

    #[test]
    fn without_credentials_only_market_data_is_built() {
        for &exchange in registered() {
            let base_url = default_base_url(exchange).expect("default base url");
            let connectors = connect(exchange, config(base_url, false)).expect("connectors");
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
