//! What the harness needs to know about each exchange that its connector
//! does not say: where its testnet is, the market to test on, and which
//! parts of its traffic a recording must strip or a proxy must forward.

use quantforge::exchange::{self, ConnectorConfig};
use quantforge::{ExchangeId, Interval, Symbol};
use url::Url;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Profile {
    /// The testnet the live modes run against, and the only upstream the
    /// recording proxy accepts.
    pub(crate) testnet_base_url: &'static str,
    /// The market the checks read and trade: liquid on the testnet.
    pub(crate) symbol: &'static str,
    pub(crate) interval: Interval,
    /// Query parameters that change on every request, such as signing
    /// timestamps and signatures. A recording keeps their place but not
    /// their value.
    pub(crate) volatile_params: &'static [&'static str],
    /// The request headers the proxy forwards: credentials, plus anything
    /// else the exchange signs or needs. Never `Host`.
    pub(crate) forward_headers: &'static [&'static str],
    /// The request headers that carry credentials. A recording keeps only
    /// whether one was sent.
    pub(crate) credential_headers: &'static [&'static str],
    /// Response fields that identify the account, blanked when recording.
    pub(crate) account_fields: &'static [&'static str],
}

impl Profile {
    pub(crate) fn symbol(&self) -> Symbol {
        Symbol::new(self.symbol).expect("profile symbol")
    }

    pub(crate) fn testnet(&self) -> Url {
        Url::parse(self.testnet_base_url).expect("testnet url")
    }
}

pub(crate) fn profile(exchange: ExchangeId) -> Profile {
    // `ExchangeId` is non-exhaustive outside the crate, so a registered
    // exchange without a profile fails here, in every conformance run.
    match exchange {
        ExchangeId::BinanceSpot => Profile {
            testnet_base_url: "https://testnet.binance.vision/",
            symbol: "BTCUSDT",
            interval: Interval::M1,
            volatile_params: &["timestamp", "signature"],
            forward_headers: &["x-mbx-apikey", "content-type"],
            credential_headers: &["x-mbx-apikey"],
            account_fields: &["uid"],
        },
        other => panic!("{other} has no conformance profile"),
    }
}

// The proxy trades on whatever its upstream is; a testnet the connector
// takes for production would be refused anyway, but never get that far.
#[test]
fn every_testnet_is_a_non_production_endpoint() {
    for &exchange in exchange::registered() {
        let connectors = exchange::connect(
            exchange,
            ConnectorConfig {
                base_url: profile(exchange).testnet(),
                credentials: None,
            },
        )
        .expect("connectors");
        assert!(!connectors.market_data.is_production(), "{exchange:?}");
        assert_eq!(connectors.market_data.exchange_id(), exchange);
    }
}
