//! What every check shares: the context it runs in, how it fails, and the
//! requests and comparisons several checks make.

use std::time::Duration;

use rust_decimal::Decimal;

use super::{Environment, Target};
use crate::ports::{ExchangeError, MarketDataConnector, SignedConnectors};
use crate::{
    AssetBalance, Candle, ExchangeCapabilities, ExchangeErrorKind, KlineRequest, SymbolRules,
    TimestampMs,
};

/// What a check runs with: its target, the connectors built for it, and
/// the interval's step in milliseconds.
pub(super) struct Ctx<'a> {
    pub(super) target: &'a Target,
    pub(super) capabilities: ExchangeCapabilities,
    pub(super) market_data: &'a dyn MarketDataConnector,
    pub(super) signed: Option<SignedConnectors<'a>>,
    pub(super) step: i64,
}

/// Why a check did not pass.
pub(super) enum Fault {
    /// The connector broke the contract.
    Failed(String),
    /// The environment failed, not the connector.
    Inconclusive(String),
}

impl Fault {
    pub(super) fn failed(reason: impl Into<String>) -> Self {
        Self::Failed(reason.into())
    }
}

/// A passing check's note, or why it did not pass.
pub(super) type Verdict = Result<Option<String>, Fault>;

/// Fails the check with the formatted reason unless `$condition` holds.
macro_rules! ensure {
    ($condition:expr, $($reason:tt)+) => {
        if !$condition {
            return Err($crate::exchange::conformance::support::Fault::Failed(format!($($reason)+)));
        }
    };
}
pub(super) use ensure;

/// The exchange clock as one reading: `server_ms` itself, and `aligned`,
/// the open time of the bar it falls in. Every window and client order id
/// a check uses hangs from one, so a recorded run replays exactly.
pub(super) struct Anchor {
    pub(super) server_ms: TimestampMs,
    pub(super) aligned: TimestampMs,
}

/// How often a live check asks again for state the exchange may not have
/// settled yet, such as trades and balances after a fill.
pub(super) const POLL_ATTEMPTS: usize = 5;
const POLL_PAUSE: Duration = Duration::from_millis(500);

impl<'a> Ctx<'a> {
    /// An error from `what` that the check did not expect. Live, an error
    /// the exchange's side caused — no connection, a rate limit, an outage —
    /// says nothing about the connector, so it is inconclusive.
    pub(super) fn unexpected(&self, what: &str, err: ExchangeError) -> Fault {
        let kind = err.kind();
        let reason = format!("{what} failed as {kind}: {err}");
        if self.environment_failed(kind) {
            Fault::Inconclusive(reason)
        } else {
            Fault::Failed(reason)
        }
    }

    fn environment_failed(&self, kind: ExchangeErrorKind) -> bool {
        self.target.environment == Environment::Live
            && matches!(
                kind,
                ExchangeErrorKind::Transport
                    | ExchangeErrorKind::RateLimited
                    | ExchangeErrorKind::Unavailable
                    | ExchangeErrorKind::OutcomeUnknown
            )
    }

    /// Passes when `result` failed as `kind`.
    pub(super) fn expect_kind<T>(
        &self,
        what: &str,
        result: Result<T, ExchangeError>,
        kind: ExchangeErrorKind,
    ) -> Result<(), Fault> {
        match result {
            Ok(_) => Err(Fault::Failed(format!(
                "{what} succeeded; it must fail as {kind}"
            ))),
            Err(err) if err.kind() == kind => Ok(()),
            Err(err) if self.environment_failed(err.kind()) => Err(Fault::Inconclusive(format!(
                "{what} failed as {} before the exchange could answer as {kind}: {err}",
                err.kind()
            ))),
            Err(err) => Err(Fault::Failed(format!(
                "{what} failed as {}, not {kind}: {err}",
                err.kind()
            ))),
        }
    }

    /// The signed connectors; `run` skips a signed check without them.
    pub(super) fn signed(&self) -> Result<SignedConnectors<'a>, Fault> {
        self.signed.ok_or_else(|| {
            Fault::Failed("the check is signed but the target has no credentials".to_string())
        })
    }

    pub(super) async fn server_time(&self) -> Result<TimestampMs, Fault> {
        self.market_data
            .server_time()
            .await
            .map_err(|err| self.unexpected("reading the exchange clock", err))
    }

    pub(super) async fn anchor(&self) -> Result<Anchor, Fault> {
        let server_ms = self.server_time().await?;
        Ok(Anchor {
            server_ms,
            aligned: server_ms.saturating_sub(server_ms.rem_euclid(self.step)),
        })
    }

    pub(super) async fn klines(
        &self,
        start_time_ms: Option<TimestampMs>,
        end_time_ms: Option<TimestampMs>,
        limit: Option<u16>,
    ) -> Result<Vec<Candle>, Fault> {
        self.market_data
            .fetch_klines(&KlineRequest {
                symbol: self.target.symbol.clone(),
                interval: self.target.interval,
                start_time_ms,
                end_time_ms,
                limit,
            })
            .await
            .map_err(|err| self.unexpected("fetching klines", err))
    }

    pub(super) async fn rules(&self) -> Result<SymbolRules, Fault> {
        self.market_data
            .fetch_symbol_rules(&self.target.symbol)
            .await
            .map_err(|err| self.unexpected("fetching the symbol rules", err))
    }

    pub(super) async fn balances(&self) -> Result<Vec<AssetBalance>, Fault> {
        self.signed()?
            .account
            .account_balances()
            .await
            .map_err(|err| self.unexpected("reading the balances", err))
    }

    /// Waits before polling again. Only a live exchange can be behind; a
    /// replay answers at once, from the responses recorded.
    pub(super) async fn pause(&self) {
        if self.target.environment == Environment::Live {
            tokio::time::sleep(POLL_PAUSE).await;
        }
    }

    /// Checks the grid, close time, order, and prices of `candles`, the
    /// shape every kline response has.
    pub(super) fn well_formed(&self, candles: &[Candle]) -> Result<(), Fault> {
        let mut previous: Option<TimestampMs> = None;
        for candle in candles {
            let open = candle.open_time_ms;
            ensure!(
                open.rem_euclid(self.step) == 0,
                "candle {open} does not open on the {} grid",
                self.target.interval
            );
            ensure!(
                candle.close_time_ms == open.saturating_add(self.step - 1),
                "candle {open} closes at {}; its close time must be its last millisecond, {}",
                candle.close_time_ms,
                open.saturating_add(self.step - 1)
            );
            if let Some(previous) = previous {
                ensure!(
                    open > previous,
                    "candle {open} follows {previous}: candles must be ascending and unique"
                );
            }
            previous = Some(open);
            let Candle {
                open: first,
                high,
                low,
                close,
                volume,
                ..
            } = candle;
            ensure!(
                *low > Decimal::ZERO,
                "candle {open} has a non-positive low {low}"
            );
            ensure!(
                low <= first && low <= close && first <= high && close <= high,
                "candle {open} has inconsistent prices: open {first} high {high} low {low} \
                 close {close}"
            );
            ensure!(
                *volume >= Decimal::ZERO,
                "candle {open} has a negative volume {volume}"
            );
        }
        Ok(())
    }
}

/// Whether two candle lists are the same, decimal text included: `1.10`
/// and `1.1` differ here, though they compare equal as numbers.
pub(super) fn same_candles(left: &[Candle], right: &[Candle]) -> bool {
    left.len() == right.len() && left.iter().zip(right).all(|(a, b)| exact(a) == exact(b))
}

fn exact(candle: &Candle) -> (TimestampMs, TimestampMs, [String; 5], Option<u64>) {
    (
        candle.open_time_ms,
        candle.close_time_ms,
        [
            candle.open.to_string(),
            candle.high.to_string(),
            candle.low.to_string(),
            candle.close.to_string(),
            candle.volume.to_string(),
        ],
        candle.trades,
    )
}

/// The client order id a check sends under `tag`, unique to the exchange
/// clock's reading so that two runs never share one.
pub(super) fn client_order_id(anchor: &Anchor, tag: &str) -> String {
    format!("qfc-{}-{tag}", anchor.server_ms)
}

/// The free balance of `asset`, zero when the exchange lists none.
pub(super) fn free(balances: &[AssetBalance], asset: &str) -> Decimal {
    balances
        .iter()
        .find(|balance| balance.asset.eq_ignore_ascii_case(asset))
        .map(|balance| balance.free)
        .unwrap_or(Decimal::ZERO)
}
