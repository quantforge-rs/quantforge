//! The exchange conformance suite: the contract an exchange connector
//! meets, written down once as checks every adapter runs, so two exchanges
//! can be compared side by side, check for check.
//!
//! [`run`] runs one [`Check`] against a [`Target`]: a connector factory,
//! the market to test on, and where the exchange is. The repository runs
//! every check against every registered exchange twice over — replayed
//! from recorded fixtures in CI, and live against each exchange's testnet
//! on a schedule — and [`render_matrix`] lays the results out next to each
//! exchange's [`crate::ExchangeCapabilities`].
//!
//! # The contract
//!
//! What the engines rely on, and what every adapter therefore normalizes
//! its exchange to:
//!
//! - **Candles** carry UTC epoch milliseconds. Open times sit on the
//!   interval's grid, and `close_time_ms` is the bar's last millisecond,
//!   `open_time_ms + step - 1`, even while the bar is still forming. Prices
//!   sit on the symbol's tick grid, and every decimal is the exchange's own
//!   text parsed exactly, never through a float.
//! - **Kline requests** bound the open time inclusively at both ends and
//!   return at most `limit` candles, oldest first, ascending and unique. A
//!   `limit` above [`crate::MarketDataCapabilities::max_klines_per_request`]
//!   is capped, not refused. A window that opens before the exchange's
//!   first bar returns its oldest bars, so a leading gap is skipped, and a
//!   window without bars is empty rather than an error. Paging with
//!   `start = last open + step` therefore walks a window exactly once, and
//!   an empty page means nothing is left.
//! - **Closed bars**: the newest candle may still be forming. The connector
//!   returns it as it stands, and a bar is closed once the exchange clock
//!   passes its `close_time_ms`.
//! - **The exchange clock**, [`crate::MarketDataConnector::server_time`],
//!   reads UTC milliseconds and never runs backwards. Drift between it and
//!   the local clock is not compensated: a request the exchange rejects for
//!   its timestamp fails as [`crate::ExchangeErrorKind::ClockSkew`], so a
//!   caller can resynchronize.
//! - **Symbol rules** report positive limits, an exchange's "no limit" as
//!   absent rather than zero, minimums no larger than maximums, and a
//!   quantity step and a price tick.
//! - **Market orders** come back filled, reporting the executed quantity,
//!   the quote amount, every fill with its trade id, both order ids, and
//!   the transaction time. A filled order reads back by either id, is never
//!   listed as open, and cannot be cancelled: the cancel fails as
//!   [`crate::ExchangeErrorKind::UnknownOrder`]. Its fills appear among the
//!   account's trades, and the base balance moves by exactly the filled
//!   quantity net of base-asset fees. A quantity rounded down to the step
//!   and above the minimums is accepted; an order that breaks the rules
//!   fails as [`crate::ExchangeErrorKind::RuleViolation`].
//! - **Errors** carry the [`crate::ExchangeErrorKind`] every exchange
//!   shares: unknown symbols and orders, a short balance, rejected
//!   credentials, clock skew, rate limits, an exchange refusing work, an
//!   exchange failing mid-request, and unreadable responses each fail as
//!   their own kind.
//!
//! Faults no live exchange can be made to produce on demand — clock skew,
//! rate limits, outages, garbled responses — are checked against scripted
//! responses only ([`Check::runs_live`]).
//!
//! # Safety
//!
//! The order checks place market orders on whatever endpoint the target
//! names, so [`run`] refuses connectors that report a production endpoint,
//! and a replayed target that is not on the local machine, before sending
//! anything. Orders are sized at four times the symbol's minimum notional,
//! and what the suite buys it sells back. A target behind a proxy is only
//! as safe as the proxy's upstream: the caller checks that one.

mod check;
mod clock;
mod errors;
mod klines;
mod matrix;
mod orders;
mod rules;
mod support;

pub use check::Check;
pub use matrix::{ExchangeReport, render_matrix};

use std::fmt;
use std::sync::Arc;
use url::{Host, Url};

use super::{ApiCredentials, ConnectorConfig, ExchangeConnectors};
use crate::ports::ExchangeError;
use crate::{ExchangeId, Interval, Symbol};
use support::{Ctx, Fault};

/// Builds the connectors under test from a base URL and credentials. The
/// suite calls it once per check, and once more with credentials the
/// exchange must reject.
pub type Factory =
    Arc<dyn Fn(ConnectorConfig) -> Result<ExchangeConnectors, ExchangeError> + Send + Sync>;

/// Where a target's responses come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Environment {
    /// The exchange itself, normally its testnet.
    Live,
    /// A local server replaying recorded responses. The local clock means
    /// nothing there, and faults the exchange cannot be made to produce
    /// can be scripted.
    Replay,
}

/// What the checks run against.
#[derive(Clone)]
pub struct Target {
    pub factory: Factory,
    /// The market the checks read and trade.
    pub symbol: Symbol,
    /// The candle interval the checks read, a day or shorter.
    pub interval: Interval,
    pub base_url: Url,
    /// The account's credentials; the signed checks skip without them.
    pub credentials: Option<ApiCredentials>,
    pub environment: Environment,
}

impl Target {
    /// A target for a registered exchange, built by its registry factory.
    pub fn registered(
        exchange: ExchangeId,
        symbol: Symbol,
        interval: Interval,
        base_url: Url,
        credentials: Option<ApiCredentials>,
        environment: Environment,
    ) -> Self {
        Self {
            factory: Arc::new(super::factory(exchange)),
            symbol,
            interval,
            base_url,
            credentials,
            environment,
        }
    }
}

/// Leaves out the factory, and the base URL, which may carry userinfo.
impl fmt::Debug for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Target")
            .field("symbol", &self.symbol)
            .field("interval", &self.interval)
            .field("host", &self.base_url.host_str())
            .field("credentials", &self.credentials)
            .field("environment", &self.environment)
            .finish_non_exhaustive()
    }
}

/// How a check went.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Outcome {
    /// The connector met the contract; `note` tells anything worth knowing
    /// about this run, such as the measured clock drift.
    Passed { note: Option<String> },
    /// The connector broke the contract, as described.
    Failed(String),
    /// The environment failed rather than the connector — an underfunded
    /// account, an exchange outage, a skewed local clock — so the check
    /// proved nothing either way.
    Inconclusive(String),
    /// The check did not run.
    Skipped(Skip),
}

/// Why a check did not run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Skip {
    /// The exchange lacks the capability named.
    Unsupported(&'static str),
    /// The check needs the account's credentials, and the target has none.
    NoCredentials,
    /// The check scripts a fault, which only a replay can.
    ReplayOnly,
}

/// The longest interval whose bars open on the epoch-aligned grid the
/// checks expect; weekly bars open on Mondays instead.
const LONGEST_INTERVAL_MS: i64 = 86_400_000;

/// Runs `check` against `target`. Nothing is sent before the target has
/// passed the safety refusals described in the module docs.
pub async fn run(target: &Target, check: Check) -> Outcome {
    if let Some(refusal) = refusal(target) {
        return Outcome::Failed(refusal);
    }
    let connectors = match (target.factory)(ConnectorConfig {
        base_url: target.base_url.clone(),
        credentials: target.credentials.clone(),
    }) {
        Ok(connectors) => connectors,
        Err(err) => return Outcome::Failed(format!("building the connectors failed: {err}")),
    };
    if connectors.market_data.is_production() {
        return Outcome::Failed(PRODUCTION_REFUSAL.to_string());
    }

    let capabilities = connectors.market_data.capabilities();
    if let Some(missing) = check.missing_capability(&capabilities) {
        return Outcome::Skipped(Skip::Unsupported(missing));
    }
    if !check.runs_live() && target.environment == Environment::Live {
        return Outcome::Skipped(Skip::ReplayOnly);
    }
    let signed = connectors.signed();
    if check.signed() && signed.is_none() {
        return Outcome::Skipped(Skip::NoCredentials);
    }

    let ctx = Ctx {
        target,
        capabilities,
        market_data: connectors.market_data.as_ref(),
        signed,
        step: target.interval.step_ms(),
    };
    match check.verdict(&ctx).await {
        Ok(note) => Outcome::Passed { note },
        Err(Fault::Failed(reason)) => Outcome::Failed(reason),
        Err(Fault::Inconclusive(reason)) => Outcome::Inconclusive(reason),
    }
}

const PRODUCTION_REFUSAL: &str =
    "refusing to run against a production endpoint: the checks place real orders";

/// Why `target` must not be run, if it must not.
fn refusal(target: &Target) -> Option<String> {
    if target.environment == Environment::Replay && !is_local(&target.base_url) {
        return Some(format!(
            "a replayed target must be a local server, not {}: the fault checks place orders \
             that only a replay may answer",
            target.base_url.host_str().unwrap_or("a URL without a host")
        ));
    }
    if target.interval.step_ms() > LONGEST_INTERVAL_MS {
        return Some(format!(
            "interval {} is longer than a day, so its bars do not open on the grid the checks \
             expect",
            target.interval
        ));
    }
    None
}

fn is_local(url: &Url) -> bool {
    match url.host() {
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        Some(Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exchange::{DEFAULT_EXCHANGE, connect, default_base_url, registered};

    fn target(exchange: ExchangeId, base_url: &str, environment: Environment) -> Target {
        Target::registered(
            exchange,
            Symbol::new("BTCUSDT").expect("symbol"),
            Interval::M1,
            Url::parse(base_url).expect("url"),
            None,
            environment,
        )
    }

    // The connectors are built but never used: the refusal comes first, so
    // these tests send nothing anywhere.
    #[tokio::test]
    async fn production_endpoints_are_refused_before_any_request() {
        for &exchange in registered() {
            for environment in [Environment::Live, Environment::Replay] {
                let mut production = target(exchange, default_base_url(exchange), environment);
                if environment == Environment::Replay {
                    // Past the local-server refusal, so the production one
                    // is what answers.
                    production.factory = Arc::new(move |mut config: ConnectorConfig| {
                        config.base_url = Url::parse(default_base_url(exchange)).expect("url");
                        connect(exchange, config)
                    });
                    production.base_url = Url::parse("http://127.0.0.1:9/").expect("url");
                }
                assert_eq!(
                    run(&production, Check::ServerTime).await,
                    Outcome::Failed(PRODUCTION_REFUSAL.to_string()),
                    "{exchange:?} {environment:?}"
                );
            }
        }
    }

    #[tokio::test]
    async fn a_replay_must_be_served_from_the_local_machine() {
        for base_url in ["https://testnet.example/", "http://10.0.0.1:8080/"] {
            let remote = target(DEFAULT_EXCHANGE, base_url, Environment::Replay);
            let outcome = run(&remote, Check::ServerTime).await;
            assert!(
                matches!(&outcome, Outcome::Failed(reason) if reason.contains("local server")),
                "{base_url}: {outcome:?}"
            );
        }
        for local in [
            "http://127.0.0.1:9/",
            "http://localhost:9/",
            "http://[::1]:9/",
        ] {
            assert!(is_local(&Url::parse(local).expect("url")), "{local}");
        }
    }

    #[tokio::test]
    async fn intervals_longer_than_a_day_are_refused() {
        let mut weekly = target(DEFAULT_EXCHANGE, "http://127.0.0.1:9/", Environment::Replay);
        weekly.interval = Interval::W1;
        let outcome = run(&weekly, Check::ServerTime).await;
        assert!(
            matches!(&outcome, Outcome::Failed(reason) if reason.contains("longer than a day")),
            "{outcome:?}"
        );
    }

    // A caller may spawn a check onto a multi-threaded runtime.
    #[test]
    fn a_running_check_can_move_between_threads() {
        fn assert_send<T: Send>(_: &T) {}
        let local = target(DEFAULT_EXCHANGE, "http://127.0.0.1:9/", Environment::Replay);
        let future = run(&local, Check::OrderLifecycle);
        assert_send(&future);
    }

    #[test]
    fn debug_output_names_the_host_but_never_the_credentials() {
        let mut target = target(
            DEFAULT_EXCHANGE,
            "https://user:pass@testnet.example/",
            Environment::Live,
        );
        target.credentials = Some(ApiCredentials {
            api_key: "conformance-key".to_string(),
            secret: "conformance-secret".to_string(),
        });
        let debug = format!("{target:?}");
        assert!(debug.contains("testnet.example"), "{debug}");
        for hidden in ["conformance-key", "conformance-secret", "user", "pass"] {
            assert!(!debug.contains(hidden), "{hidden} in {debug}");
        }
    }
}
